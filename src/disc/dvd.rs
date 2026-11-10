//! Reads a DVD's title list from its IFO files: VIDEO_TS.IFO names the titles (and which title
//! set holds each), and each VTS_nn_0.IFO maps a title's chapters to program chains and cells.
//! A title's length is the sum of the cells it plays, its size the sectors those cells span.
//! Titles that play nothing (every cell under a second) are left out, as players and MakeMKV do.

use std::collections::HashMap;

use super::DiscSource;
use crate::data::models::DiscTitle;

const SECTOR: usize = 2048;

pub fn read(source: &DiscSource) -> Result<Vec<DiscTitle>, String> {
    let vmg = read_ifo(source, "VIDEO_TS")
        .filter(|d| d.starts_with(b"DVDVIDEO-VMG"))
        .ok_or("VIDEO_TS/VIDEO_TS.IFO is missing or not a DVD video manager")?;
    parse(&vmg, |n| read_ifo(source, &format!("VTS_{n:02}_0")))
        .ok_or_else(|| "VIDEO_TS/VIDEO_TS.IFO is damaged".to_string())
}

fn parse(vmg: &[u8], mut title_set: impl FnMut(u8) -> Option<Vec<u8>>) -> Option<Vec<DiscTitle>> {
    let table = u32be(vmg, 0xC4)? as usize * SECTOR;
    let count = u16be(vmg, table)? as usize;
    let mut sets: HashMap<u8, Option<Vec<u8>>> = HashMap::new();
    let mut titles = Vec::new();
    for i in 0..count {
        let entry = table + 8 + i * 12;
        let angles = *vmg.get(entry + 1)?;
        let vts = *vmg.get(entry + 6)?;
        let vts_title = *vmg.get(entry + 7)? as usize;
        let ifo = sets
            .entry(vts)
            .or_insert_with(|| title_set(vts).filter(|d| d.starts_with(b"DVDVIDEO-VTS")))
            .as_deref();
        let Some(info) = ifo.and_then(|ifo| title_info(ifo, vts_title)) else {
            continue;
        };
        if angles <= 1 && info.whole_seconds == 0.0 {
            continue;
        }

        titles.push(DiscTitle {
            title: Some(i as i32 + 1),
            duration: Some(duration(info.seconds)),
            chapters: Some(info.chapters as i32),
            size_bytes: Some(info.bytes),
            ..Default::default()
        });
    }

    Some(titles)
}

/// h:mm:ss from seconds, rounded down.
pub fn duration(seconds: f64) -> String {
    let s = seconds.floor() as u64;
    format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

fn read_ifo(source: &DiscSource, name: &str) -> Option<Vec<u8>> {
    // The .BUP is a byte-for-byte backup of the .IFO; use it if the .IFO is unreadable.
    for ext in ["IFO", "BUP"] {
        let Some(file) = source.find(&format!("VIDEO_TS/{name}.{ext}")) else {
            continue;
        };
        if !(0x100..=16 * 1024 * 1024).contains(&file.size) {
            continue;
        }
        if let Ok(data) = source.read(file) {
            return Some(data);
        }
    }

    None
}

struct TitleInfo {
    seconds: f64,
    whole_seconds: f64,
    chapters: usize,
    bytes: u64,
}

/// The title's chapters (part-of-title entries) resolved to the cells they play.
fn title_info(ifo: &[u8], vts_title: usize) -> Option<TitleInfo> {
    let ptt = u32be(ifo, 0xC8)? as usize * SECTOR;
    let pgci = u32be(ifo, 0xCC)? as usize * SECTOR;
    let titles = u16be(ifo, ptt)? as usize;
    if vts_title < 1 || vts_title > titles {
        return None;
    }

    let start = ptt + u32be(ifo, ptt + 8 + (vts_title - 1) * 4)? as usize;
    let end = if vts_title < titles {
        ptt + u32be(ifo, ptt + 8 + vts_title * 4)? as usize
    } else {
        ptt + u32be(ifo, ptt + 4)? as usize + 1
    };

    let mut parts = Vec::new();
    let mut at = start;
    while at + 4 <= end {
        parts.push((u16be(ifo, at)? as usize, u16be(ifo, at + 2)? as usize));
        at += 4;
    }
    if parts.is_empty() {
        return None;
    }

    let mut info = TitleInfo {
        seconds: 0.0,
        whole_seconds: 0.0,
        chapters: parts.len(),
        bytes: 0,
    };
    for (pgc, program) in parts {
        for (seconds, bytes) in program_cells(ifo, pgci, pgc, program)? {
            info.seconds += seconds;
            info.whole_seconds += seconds.floor();
            info.bytes += bytes;
        }
    }

    Some(info)
}

/// The cells of one program (length in seconds, size in bytes), skipping the extra angles of an
/// angle block. None when the IFO points outside itself.
fn program_cells(
    ifo: &[u8],
    pgci: usize,
    pgc_number: usize,
    program: usize,
) -> Option<Vec<(f64, u64)>> {
    let count = u16be(ifo, pgci)? as usize;
    if pgc_number < 1 || pgc_number > count {
        return Some(Vec::new());
    }

    let pgc = pgci + u32be(ifo, pgci + 8 + (pgc_number - 1) * 8 + 4)? as usize;
    let programs = *ifo.get(pgc + 2)? as usize;
    let cells = *ifo.get(pgc + 3)? as usize;
    if program < 1 || program > programs {
        return Some(Vec::new());
    }

    let program_map = pgc + u16be(ifo, pgc + 0xE6)? as usize;
    let cell_table = pgc + u16be(ifo, pgc + 0xE8)? as usize;
    let first = *ifo.get(program_map + program - 1)? as usize;
    let last = if program < programs {
        (*ifo.get(program_map + program)? as usize).saturating_sub(1)
    } else {
        cells
    };
    let mut out = Vec::new();
    for c in first..=last.min(cells) {
        if c == 0 {
            continue;
        }
        let at = cell_table + (c - 1) * 24;
        let flags = *ifo.get(at)?;
        let block_mode = (flags >> 6) & 3; // 0 none, 1 first, 2 middle, 3 last
        let block_type = (flags >> 4) & 3; // 1 angle block
        if block_type == 1 && (block_mode == 2 || block_mode == 3) {
            continue; // other angles of the same scene
        }

        let seconds = time(ifo.get(at + 4..at + 8)?);
        let first_sector = u32be(ifo, at + 8)?;
        let last_sector = u32be(ifo, at + 0x14)?;
        let bytes = if last_sector >= first_sector {
            (last_sector - first_sector + 1) as u64 * SECTOR as u64
        } else {
            0
        };
        out.push((seconds, bytes));
    }

    Some(out)
}

/// A BCD playback time: hours, minutes, seconds, then frames with the frame rate in the top bits.
fn time(t: &[u8]) -> f64 {
    let bcd = |b: u8| ((b >> 4) * 10 + (b & 0x0F)) as f64;
    let fps = match t[3] >> 6 {
        1 => 25.0,
        3 => 30000.0 / 1001.0,
        _ => 0.0,
    };
    let frames = if fps > 0.0 {
        bcd(t[3] & 0x3F) / fps
    } else {
        0.0
    };
    bcd(t[0]) * 3600.0 + bcd(t[1]) * 60.0 + bcd(t[2]) + frames
}

fn u16be(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

fn u32be(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?))
}
