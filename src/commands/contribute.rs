//! Writes pre-filled data files for discs that are not in the database yet. Everything read from
//! the disc (hash, titles, durations, files) is exact. The series is identified by an AniDB id given
//! explicitly (see the matcher), and its titles and episodes come from AniDB; discs without an id
//! are skipped, never filed under a guess. Release details come from the file name; fields that
//! could not be filled are "TODO", which `validate` rejects.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::matcher::{self, SeriesMatch};
use crate::data::anidb::{self, AnidbRecords, Anime};
use crate::data::json::{self, Kind};
use crate::data::lookup::{self, HashLookup};
use crate::data::models::{Disc, Release, Series};
use crate::disc::scanner::{self, ScannedDisc};
use crate::{ExitCode, naming};

const TODO: &str = "TODO";

pub struct Options<'a> {
    pub inputs: &'a [String],
    pub recursive: bool,
    pub data: PathBuf,
    pub db: Option<&'a str>,
    pub anidb: Option<i32>,
    pub all: bool,
    pub offline: bool,
    pub anidb_client: Option<anidb::Client>,
}

pub fn run(o: Options) -> ExitCode {
    let mut lookup = None;
    if !o.all && (o.db.is_some() || std::env::var(lookup::ENVIRONMENT_VARIABLE).is_ok()) {
        match HashLookup::create(o.db) {
            Ok(l) => lookup = Some(l),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::BadArguments;
            }
        }
    }

    // Discs already present in this checkout are always skipped, whatever --db says.
    let mut existing = HashSet::new();
    collect_hashes(&o.data, &mut existing);

    let mut discs: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut seen = HashSet::new();
    for input in o.inputs {
        let root = std::path::absolute(input).unwrap_or_else(|_| PathBuf::from(input));
        for path in scanner::find(std::slice::from_ref(input), o.recursive) {
            if seen.insert(path.to_string_lossy().to_lowercase()) {
                discs.push((path, root.clone()));
            }
        }
    }
    if discs.is_empty() {
        eprintln!("No disc images or disc folders found.");
        return ExitCode::NothingFound;
    }

    let mut series = SeriesIndex {
        data: o.data.clone(),
        anidb: AnidbRecords::new(o.offline, o.anidb_client),
        by_anidb: None,
        refreshed: HashSet::new(),
    };
    let mut unmatched = Vec::new();
    let (mut written, mut skipped, mut failed) = (0, 0, 0);
    for (path, root) in &discs {
        let scan = scanner::scan(path);
        let (Some(hash), Some(_)) = (&scan.content_hash, &scan.titles) else {
            failed += 1;
            eprintln!(
                "{}: {}",
                path.display(),
                scan.error.as_deref().unwrap_or("")
            );
            continue;
        };

        let known = match lookup.as_mut().map(|l| l.find(hash)) {
            Some(Err(e)) => {
                eprintln!("{e}");
                return ExitCode::Failure;
            }
            Some(Ok(r)) => r.is_some(),
            None => false,
        };
        if existing.contains(hash) || known {
            skipped += 1;
            eprintln!("{}: already in the database, skipped", path.display());
            continue;
        }

        let Some(m) = matcher::find(path, root, o.anidb) else {
            unmatched.push(path.display().to_string());
            continue;
        };

        let series_dir = series.resolve(&m);
        match write_disc(&series_dir, path, &scan) {
            Ok(file) => {
                existing.insert(hash.clone());
                written += 1;
                println!("{}", file.display());
            }
            Err(e) => {
                failed += 1;
                eprintln!("{}: {e}", path.display());
            }
        }
    }

    eprintln!(
        "{written} written to {}, {skipped} already known, {} skipped (series unknown), {failed} failed.",
        o.data.display(),
        unmatched.len()
    );
    if !unmatched.is_empty() {
        matcher::print_help(&unmatched);
    }
    if written > 0 {
        eprintln!();
        eprintln!(
            "Check what was guessed, fill in any TODO fields and title types, then run `anidisc validate`."
        );
    }

    if failed > 0 || !unmatched.is_empty() {
        ExitCode::Failure
    } else {
        ExitCode::Success
    }
}

fn write_disc(series_dir: &Path, disc_path: &Path, scan: &ScannedDisc) -> std::io::Result<PathBuf> {
    let hints = naming::guess(disc_path);
    let format = scan.format.clone().unwrap_or_default();
    let media = match hints.media.as_deref() {
        Some(m @ ("DVD" | "BD" | "UHD")) => m.to_string(),
        _ if format == "dvd" => "DVD".to_string(),
        _ => "BD".to_string(),
    };

    // Releases are told apart by media, region and edition tags, so a first press and a regular
    // edition of the same region get separate folders.
    let release_slug = non_empty_slug(
        &format!(
            "{media} {} {}",
            hints.region.as_deref().unwrap_or(""),
            hints.edition.as_deref().unwrap_or("")
        ),
        "release",
    );
    let release_dir = series_dir.join(release_slug);
    let release_file = release_dir.join("release.json");
    if !release_file.exists() {
        json::write_file(
            &release_file,
            &Release {
                region: hints.region.clone().unwrap_or_else(|| TODO.to_string()),
                media,
                name: None,
                publisher: None,
                release_date: None,
                edition: hints.edition.clone(),
                upc: None,
                notes: None,
            },
        )?;
    }

    // Disc files are named <catalog>-<hash8>, or <hash8> without a catalog number. The hash part
    // keeps re-pressings that share a catalog number apart, and is unique because known hashes
    // were skipped above.
    let hash = scan.content_hash.clone().unwrap_or_default();
    let short = hash[..8].to_lowercase();
    let slug = match &hints.catalog_number {
        Some(c) => format!("{}-{short}", non_empty_slug(c, "disc")),
        None => short,
    };
    let file = release_dir.join(format!("{slug}.json"));
    json::write_file(
        &file,
        &Disc {
            content_hash: hash,
            format,
            label: hints.label,
            catalog_number: hints.catalog_number,
            pressing: None,
            volume_label: scan.volume_label.clone(),
            titles: scan.titles.clone(),
            files: Some(scan.files.clone()),
            notes: None,
        },
    )?;
    Ok(file)
}

fn non_empty_slug(text: &str, fallback: &str) -> String {
    let slug = naming::slugify(text);
    if slug.is_empty() {
        fallback.to_string()
    } else {
        slug
    }
}

fn collect_hashes(dir: &Path, out: &mut HashSet<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            collect_hashes(&path, out);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        {
            let hash = fs::read_to_string(&path)
                .ok()
                .and_then(|t| json::parse_lenient::<Value>(&t, Kind::Any).ok())
                .and_then(|v| {
                    v.get("contentHash")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
            if let Some(h) = hash {
                out.insert(h);
            }
        }
    }
}

/// The series folders of the data checkout, created or completed as discs are filed.
struct SeriesIndex {
    data: PathBuf,
    anidb: AnidbRecords,
    by_anidb: Option<HashMap<i32, String>>,
    /// AniDB ids whose existing series already had missing episode titles filled in this run.
    refreshed: HashSet<i32>,
}

impl SeriesIndex {
    fn resolve(&mut self, m: &SeriesMatch) -> PathBuf {
        if self.by_anidb.is_none() {
            self.by_anidb = Some(self.load_index());
        }
        if let Some(known) = self.by_anidb.as_ref().and_then(|i| i.get(&m.anidb)) {
            let dir = self.data.join(known);
            if self.refreshed.insert(m.anidb) {
                self.fill_episode_titles(&dir.join("series.json"), m.anidb);
            }
            return dir;
        }

        let anime = match self.anidb.get(m.anidb) {
            Ok(a) => Some(a),
            Err(problem) => {
                eprintln!(
                    "warning: {problem}; the series' title and episodes need filling in by hand"
                );
                None
            }
        };

        let fallback = format!("anidb-{}", m.anidb);
        let slug = match &anime {
            Some(a) => non_empty_slug(
                &format!(
                    "{} {}",
                    a.title,
                    a.year.map(|y| y.to_string()).unwrap_or_default()
                ),
                &fallback,
            ),
            None => fallback,
        };
        let dir = self.data.join(&slug);
        let file = dir.join("series.json");
        if file.exists() {
            // A series with this name but no AniDB id yet (e.g. entered by hand) gets it added.
            self.complete(&file, m.anidb, anime.as_ref());
        } else {
            let series = Series {
                title: anime
                    .as_ref()
                    .map_or_else(|| TODO.to_string(), |a| a.title.clone()),
                original_title: anime.as_ref().and_then(|a| a.original_title.clone()),
                year: anime.as_ref().and_then(|a| a.year),
                anidb: Some(vec![m.anidb]),
                aliases: None,
                episodes: anime
                    .as_ref()
                    .map(|a| a.episodes.clone())
                    .filter(|e| !e.is_empty()),
                notes: None,
            };
            if let Err(e) = json::write_file(&file, &series) {
                eprintln!("warning: could not write {}: {e}", file.display());
            }
            self.by_anidb
                .as_mut()
                .expect("loaded")
                .insert(m.anidb, slug.clone());
            eprintln!("new series {slug} (AniDB {}, from {})", m.anidb, m.source);
        }

        dir
    }

    /// Adds AniDB's romaji and original episode titles where an existing series lacks them (series
    /// entered before those fields existed); never overwrites, and quietly does nothing when the
    /// AniDB record can't be had.
    fn fill_episode_titles(&mut self, file: &Path, aid: i32) {
        let Ok(text) = fs::read_to_string(file) else {
            return;
        };
        let Ok(mut s) = json::parse_lenient::<Series>(&text, Kind::Series) else {
            return;
        };
        let main = s.main_anidb();
        let Some(episodes) = s.episodes.as_mut() else {
            return;
        };
        if episodes
            .iter()
            .filter(|e| e.anidb.or(main) == Some(aid))
            .all(|e| e.romaji_title.is_some() && e.original_title.is_some())
        {
            return;
        }
        let Ok(anime) = self.anidb.get(aid) else {
            return;
        };

        let mut changed = 0;
        for e in episodes
            .iter_mut()
            .filter(|e| e.anidb.or(main) == Some(aid))
        {
            let Some(a) = anime.episodes.iter().find(|a| a.number == e.number) else {
                continue;
            };
            // Only titles that differ from what this file already calls the episode.
            let new = |t: &Option<String>| t.clone().filter(|t| Some(t) != e.title.as_ref());
            let (romaji, original) = (new(&a.romaji_title), new(&a.original_title));
            if e.romaji_title.is_none() && romaji.is_some() {
                e.romaji_title = romaji;
                changed += 1;
            }
            if e.original_title.is_none() && original.is_some() {
                e.original_title = original;
                changed += 1;
            }
        }
        if changed > 0 && json::write_file(file, &s).is_ok() {
            eprintln!(
                "added {changed} romaji/original episode title(s) from AniDB {aid} to {}",
                file.display()
            );
        }
    }

    /// Adds what AniDB knows to an existing series that lacks it; never overwrites.
    fn complete(&mut self, file: &Path, aid: i32, anime: Option<&Anime>) {
        let Ok(text) = fs::read_to_string(file) else {
            return;
        };
        let Ok(mut s) = json::parse_lenient::<Series>(&text, Kind::Series) else {
            return;
        }; // `validate` reports it
        if s.anidb.as_ref().is_some_and(|a| !a.is_empty()) {
            return;
        }

        s.anidb = Some(vec![aid]);
        if s.original_title.is_none() {
            s.original_title = anime.and_then(|a| a.original_title.clone());
        }
        if s.year.is_none() {
            s.year = anime.and_then(|a| a.year);
        }
        if s.episodes.is_none() {
            s.episodes = anime.map(|a| a.episodes.clone()).filter(|e| !e.is_empty());
        }
        if json::write_file(file, &s).is_ok() {
            let slug = file
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.by_anidb.as_mut().expect("loaded").insert(aid, slug);
            eprintln!("added AniDB {aid} to {}", file.display());
        }
    }

    fn load_index(&self) -> HashMap<i32, String> {
        let mut index = HashMap::new();
        let Ok(entries) = fs::read_dir(&self.data) else {
            return index;
        };
        for entry in entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        {
            let ids = fs::read_to_string(entry.path().join("series.json"))
                .ok()
                .and_then(|t| json::parse_lenient::<Value>(&t, Kind::Any).ok())
                .and_then(|v| v.get("anidb").and_then(Value::as_array).cloned());
            for id in ids.into_iter().flatten().filter_map(|v| v.as_i64()) {
                index
                    .entry(id as i32)
                    .or_insert_with(|| entry.file_name().to_string_lossy().into_owned());
            }
        }
        index
    }
}
