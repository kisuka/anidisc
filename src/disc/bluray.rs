//! Reads a Blu-ray's playlists (BDMV/PLAYLIST/*.mpls). A playlist's length is the sum of its play
//! items' in/out times (45 kHz clock), its chapters are its entry marks, and its size is the size of
//! the distinct stream files it plays. Playlists that repeat an earlier one exactly are left out.

use std::collections::{HashMap, HashSet};

use super::DiscSource;
use super::dvd::duration;
use crate::data::models::DiscTitle;

const CLOCK: f64 = 45000.0;

pub fn read(source: &DiscSource) -> Result<Vec<DiscTitle>, String> {
    let mut streams: HashMap<String, u64> = HashMap::new();
    for f in source
        .files
        .iter()
        .filter(|f| starts_with_ci(&f.path, "BDMV/STREAM/"))
    {
        let name = f.path.rsplit('/').next().unwrap_or("");
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
        streams.entry(stem.to_uppercase()).or_insert(f.size);
    }

    let mut playlists: Vec<_> = source
        .files
        .iter()
        .filter(|f| {
            starts_with_ci(&f.path, "BDMV/PLAYLIST/") && f.path.to_lowercase().ends_with(".mpls")
        })
        .filter(|f| f.size > 0 && f.size < 4 * 1024 * 1024)
        .collect();
    playlists.sort_by_key(|f| f.path.to_lowercase());

    let mut seen = HashSet::new();
    let mut titles = Vec::new();
    for file in playlists {
        let Some(playlist) = source.read(file).ok().and_then(|d| parse(&d)) else {
            continue;
        };
        let key: Vec<String> = playlist
            .items
            .iter()
            .map(|i| format!("{}:{}-{}", i.clip, i.in_time, i.out_time))
            .collect();
        if playlist.items.is_empty() || !seen.insert(key.join(";")) {
            continue;
        }

        let mut clips: Vec<String> = Vec::new();
        for item in &playlist.items {
            if !clips.iter().any(|c| c.eq_ignore_ascii_case(&item.clip)) {
                clips.push(item.clip.clone());
            }
        }
        let sizes: Option<Vec<u64>> = clips
            .iter()
            .map(|c| streams.get(&c.to_uppercase()).copied())
            .collect();
        let seconds: f64 = playlist
            .items
            .iter()
            .map(|i| (i.out_time - i.in_time) as f64 / CLOCK)
            .sum();
        titles.push(DiscTitle {
            playlist: Some(file.path.rsplit('/').next().unwrap_or("").to_string()),
            duration: Some(duration(seconds)),
            chapters: Some(playlist.chapters as i32),
            size_bytes: sizes.map(|s| s.iter().sum()),
            ..Default::default()
        });
    }

    Ok(titles)
}

struct Item {
    clip: String,
    in_time: u32,
    out_time: u32,
}

struct Playlist {
    items: Vec<Item>,
    chapters: usize,
}

fn parse(d: &[u8]) -> Option<Playlist> {
    if d.len() < 20 || &d[0..4] != b"MPLS" {
        return None;
    }

    let list = u32be(d, 8)? as usize;
    let marks = u32be(d, 12)? as usize;
    let count = u16be(d, list + 6)? as usize;
    let mut items = Vec::with_capacity(count);
    let mut at = list + 10;
    for _ in 0..count {
        let len = u16be(d, at)? as usize;
        let clip = String::from_utf8_lossy(d.get(at + 2..at + 7)?).into_owned();
        let in_time = u32be(d, at + 14)?;
        let out_time = u32be(d, at + 18)?;
        if out_time > in_time {
            items.push(Item {
                clip,
                in_time,
                out_time,
            });
        }
        at += 2 + len;
    }

    let mark_count = u16be(d, marks + 4)? as usize;
    let mut chapters = 0;
    for i in 0..mark_count {
        if *d.get(marks + 6 + i * 14 + 1)? == 1 {
            chapters += 1; // entry mark
        }
    }

    Some(Playlist { items, chapters })
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len()
        && s.is_char_boundary(prefix.len())
        && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn u16be(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

fn u32be(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?))
}
