//! Writes pre-filled data files for discs that are not in the database yet. Everything read from
//! the disc (hash, titles, durations, files) is exact. The series is identified by an AniDB id given
//! explicitly (see the matcher), and its titles and episodes come from AniDB; discs without an id
//! are skipped, never filed under a guess. Release details come from the file name; fields left
//! for the contributor are written as blanks ("" and []), which read as not filled in.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::interview;
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
    /// Ask about each new disc (region, label, what each title is) instead of leaving blanks.
    pub interactive: bool,
}

pub fn run(o: Options) -> ExitCode {
    if o.interactive && !interview::available() {
        eprintln!("--interactive needs a terminal to ask questions in.");
        return ExitCode::BadArguments;
    }
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
    let mut stubs: Vec<Stub> = Vec::new();
    let mut interactive = o.interactive;
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
            Ok(stub) => {
                existing.insert(hash.clone());
                written += 1;
                println!("{}", stub.disc.display());
                if interactive {
                    let files = interview::Files {
                        disc_path: path,
                        disc_file: &stub.disc,
                        release_file: &stub.release,
                        new_release: stub.new_release,
                        series_file: &series_dir.join("series.json"),
                    };
                    match interview::ask(&files) {
                        Ok(interview::Next::Continue) => {}
                        Ok(interview::Next::StopAll) => interactive = false,
                        Err(e) => {
                            eprintln!("Stopped asking: {e}");
                            interactive = false;
                        }
                    }
                }
                stubs.push(stub);
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
    if !stubs.is_empty() && o.interactive {
        eprintln!();
        eprintln!(
            "Anything you skipped is still blank (\"\" or []) in the files listed above; fill it in any time."
        );
        eprintln!("Then run `anidisc validate`.");
    } else if !stubs.is_empty() {
        print_next_steps(&o.data, &stubs);
    }

    if failed > 0 || !unmatched.is_empty() {
        ExitCode::Failure
    } else {
        ExitCode::Success
    }
}

/// The files written for one disc.
struct Stub {
    disc: PathBuf,
    release: PathBuf,
    new_release: bool,
}

/// What to do with the written files, in plain words.
fn print_next_steps(data: &Path, stubs: &[Stub]) {
    let show = |p: &Path| {
        p.strip_prefix(data)
            .map(|r| Path::new("data").join(r).display().to_string())
            .unwrap_or_else(|_| p.display().to_string())
    };
    let mut releases: Vec<&Path> = Vec::new();
    for s in stubs.iter().filter(|s| s.new_release) {
        if !releases.contains(&s.release.as_path()) {
            releases.push(&s.release);
        }
    }

    eprintln!();
    eprintln!(
        "Next: fill in what only someone with the disc knows. Blank fields (\"\" and []) are"
    );
    eprintln!("yours to fill; leave any you don't know blank.");
    if !releases.is_empty() {
        eprintln!();
        eprintln!("In each new release.json:");
        eprintln!("  region       required: region code and country, e.g. R2-JP, R1-US, A-JP");
        eprintln!("  name, publisher, releaseDate (YYYY-MM-DD), edition    if you know them");
        for r in &releases {
            eprintln!("    {}", show(r));
        }
    }
    eprintln!();
    eprintln!("In each disc file:");
    eprintln!(
        "  label        how the release names this disc: \"Volume 1\", \"Disc 2\"; blank if it's the only one"
    );
    eprintln!("  catalogNumber  printed on the disc or the case's spine, e.g. JDXA-57678");
    eprintln!(
        "  barcode      the digits under the barcode on the case (13 for JAN/EAN, 12 for UPC); a box set's for each disc in it"
    );
    eprintln!(
        "  titles       for each: \"type\" (episode, extra, trailer, menu, notice, other) and, for episodes,"
    );
    eprintln!(
        "               \"episodes\": [1], [1, 2], [\"S1\"] as numbered in series.json. Check by playing"
    );
    eprintln!("               the title; the long ones are usually the episodes.");
    eprintln!("  everything else (hash, durations, files) was read from the disc: don't change it");
    for s in stubs {
        eprintln!("    {}", show(&s.disc));
    }
    eprintln!();
    eprintln!("Then run `anidisc validate`. CONTRIBUTING.md explains every field.");
}

fn blank() -> Option<String> {
    Some(String::new())
}

fn write_disc(series_dir: &Path, disc_path: &Path, scan: &ScannedDisc) -> std::io::Result<Stub> {
    let hints = naming::guess(disc_path);
    let format = scan.format.clone().unwrap_or_default();
    // The disc itself decides DVD or Blu-ray; the name can only tell a UHD from a BD.
    let media = match (format.as_str(), hints.media.as_deref()) {
        ("dvd", _) => "DVD".to_string(),
        (_, Some(m @ ("BD" | "UHD"))) => m.to_string(),
        (_, Some("DVD")) if format.is_empty() => "DVD".to_string(),
        _ => "BD".to_string(),
    };

    // A release is one product as sold: a single volume, or a box set. Each disc starts as its own
    // release, named by media, region, edition and its catalog number (or label, or hash); a
    // re-pressing with the same catalog number lands in the same release. Discs of a box set are
    // put together with `-i`, or by moving their files into one release folder.
    let hash = scan.content_hash.clone().unwrap_or_default();
    let short = hash[..8].to_lowercase();
    let product = hints
        .catalog_number
        .clone()
        .or_else(|| hints.label.clone())
        .unwrap_or_else(|| short.clone());
    let release_slug = non_empty_slug(
        &format!(
            "{media} {} {} {product}",
            hints.region.as_deref().unwrap_or(""),
            hints.edition.as_deref().unwrap_or("")
        ),
        "release",
    );
    let release_dir = series_dir.join(release_slug);
    let release_file = release_dir.join("release.json");
    let new_release = !release_file.exists();
    if new_release {
        json::write_file(
            &release_file,
            &Release {
                region: hints.region.clone().unwrap_or_default(),
                media,
                name: blank(),
                publisher: blank(),
                release_date: blank(),
                edition: hints.edition.clone().or_else(blank),
                notes: None,
            },
        )?;
    }

    // Disc files are named <catalog>-<hash8>, or <hash8> without a catalog number. The hash part
    // keeps re-pressings that share a catalog number apart, and is unique because known hashes
    // were skipped above.
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
            label: hints.label.or_else(blank),
            catalog_number: hints.catalog_number.or_else(blank),
            barcode: blank(),
            pressing: None,
            volume_label: scan.volume_label.clone(),
            titles: scan.titles.clone().map(|titles| {
                titles
                    .into_iter()
                    .map(|mut t| {
                        t.kind = t.kind.or_else(|| Some(Vec::new()));
                        t.episodes = t.episodes.or_else(|| Some(Vec::new()));
                        t
                    })
                    .collect()
            }),
            files: Some(scan.files.clone()),
            notes: None,
        },
    )?;
    Ok(Stub {
        disc: file,
        release: release_file,
        new_release,
    })
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
