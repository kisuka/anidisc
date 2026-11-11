//! Loading and validating the data folder.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use regex_lite::Regex;
use serde::de::DeserializeOwned;

use super::episode::EpisodeNumber;
use super::json::{self, Kind};
use super::models::{
    Disc, DiscTitle, Episode, MEDIA, NO_EPISODE_TYPES, Release, Series, TITLE_TYPES,
};
use crate::disc::listing;

pub struct SeriesEntry {
    pub slug: String,
    pub series: Series,
    pub releases: Vec<ReleaseEntry>,
}

pub struct ReleaseEntry {
    pub slug: String,
    pub release: Release,
    pub discs: Vec<DiscEntry>,
}

pub struct DiscEntry {
    pub slug: String,
    pub disc: Disc,
}

pub struct DataRepository {
    pub series: Vec<SeriesEntry>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl DataRepository {
    pub fn disc_count(&self) -> usize {
        self.series
            .iter()
            .map(|s| s.releases.iter().map(|r| r.discs.len()).sum::<usize>())
            .sum()
    }

    pub fn load(data: &Path) -> DataRepository {
        let mut v = Validator::default();
        let mut series = Vec::new();
        if !data.is_dir() {
            v.errors
                .push(format!("Data directory not found: {}", data.display()));
            return v.finish(series);
        }

        for series_dir in subdirectories(data) {
            let slug = name(&series_dir);
            v.check_slug(&slug, &slug);
            let Some(s) = v.read::<Series>(&series_dir.join("series.json"), Kind::Series) else {
                continue;
            };
            v.series(&s, &slug);
            let mut releases = Vec::new();
            for release_dir in subdirectories(&series_dir) {
                let release_slug = name(&release_dir);
                let where_ = format!("{slug}/{release_slug}");
                v.check_slug(&release_slug, &where_);
                let Some(r) = v.read::<Release>(&release_dir.join("release.json"), Kind::Release)
                else {
                    continue;
                };
                v.release(&r, &where_);
                let mut discs = Vec::new();
                for file in json_files(&release_dir) {
                    let disc_slug = file
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let disc_where = format!("{where_}/{disc_slug}.json");
                    v.check_slug(&disc_slug, &disc_where);
                    let Some(d) = v.read::<Disc>(&file, Kind::Disc) else {
                        continue;
                    };
                    v.disc(&d, &s, &r, &disc_where);
                    if is_hash(&d.content_hash) {
                        match v.hashes.get(&d.content_hash) {
                            Some(other) => v.errors.push(format!(
                                "{disc_where}: contentHash {} is already used by {other}",
                                d.content_hash
                            )),
                            None => {
                                v.hashes.insert(d.content_hash.clone(), disc_where.clone());
                            }
                        }
                    }
                    discs.push(DiscEntry {
                        slug: disc_slug,
                        disc: d,
                    });
                }

                if discs.is_empty() {
                    v.warnings.push(format!("{where_}: release has no discs"));
                }
                releases.push(ReleaseEntry {
                    slug: release_slug,
                    release: r,
                    discs,
                });
            }

            series.push(SeriesEntry {
                slug,
                series: s,
                releases,
            });
        }

        v.finish(series)
    }
}

fn subdirectories(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    dirs.sort();
    dirs
}

fn json_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .is_some_and(|x| x.eq_ignore_ascii_case("json"))
                })
                .filter(|p| p.file_name().is_some_and(|n| n != "release.json"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

fn name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn is_hash(text: &str) -> bool {
    text.len() == 32
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
}

fn is_slug(text: &str) -> bool {
    !text.is_empty()
        && text.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

#[derive(Default)]
struct Validator {
    errors: Vec<String>,
    warnings: Vec<String>,
    hashes: HashMap<String, String>,
}

impl Validator {
    fn finish(self, series: Vec<SeriesEntry>) -> DataRepository {
        DataRepository {
            series,
            errors: self.errors,
            warnings: self.warnings,
        }
    }

    fn read<T: DeserializeOwned>(&mut self, path: &Path, kind: Kind) -> Option<T> {
        let Ok(text) = fs::read_to_string(path) else {
            self.errors.push(format!("{}: missing", path.display()));
            return None;
        };
        match json::parse_lenient::<T>(&text, kind) {
            Ok(v) => Some(v),
            Err(e) => {
                self.errors.push(format!("{}: {e}", path.display()));
                None
            }
        }
    }

    fn check_slug(&mut self, slug: &str, where_: &str) {
        if !is_slug(slug) {
            self.errors.push(format!(
                "{where_}: name must be lowercase letters, digits and single hyphens"
            ));
        }
    }

    fn series(&mut self, s: &Series, where_: &str) {
        if s.title.trim().is_empty() || s.title == "TODO" {
            self.errors
                .push(format!("{where_}/series.json: title is required"));
        }
        if let Some(y) = s.year
            && !(1900..=2100).contains(&y)
        {
            self.errors
                .push(format!("{where_}/series.json: year {y} is out of range"));
        }
        if s.anidb.as_ref().is_none_or(|a| a.is_empty()) {
            self.errors
                .push(format!("{where_}/series.json: an anidb id is required"));
        }

        if let Some(episodes) = &s.episodes {
            let main = s.main_anidb();
            let mut counts: Vec<((Option<i32>, EpisodeNumber), usize)> = Vec::new();
            for e in episodes {
                let key = (e.anidb.or(main), e.number);
                match counts.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((key, 1)),
                }
            }
            for ((_, number), n) in counts.into_iter().filter(|(_, n)| *n > 1) {
                self.errors.push(format!(
                    "{where_}/series.json: episode {number} is listed {n} times"
                ));
            }
            let ids = s.anidb.clone().unwrap_or_default();
            for e in episodes
                .iter()
                .filter(|e| e.anidb.is_some_and(|a| !ids.contains(&a)))
            {
                self.errors.push(format!(
                    "{where_}/series.json: episode {} refers to anidb {}, which is not in the series' anidb list",
                    e.number,
                    e.anidb.unwrap_or_default()
                ));
            }
        }
    }

    fn release(&mut self, r: &Release, where_: &str) {
        if r.region.trim().is_empty() || r.region == "TODO" {
            self.errors
                .push(format!("{where_}/release.json: region is required"));
        }
        if !MEDIA.contains(&r.media.as_str()) {
            self.errors.push(format!(
                "{where_}/release.json: media must be one of {}",
                MEDIA.join(", ")
            ));
        }
        if let Some(date) = &r.release_date
            && !Regex::new(r"^\d{4}(-\d{2}(-\d{2})?)?$")
                .unwrap()
                .is_match(date)
        {
            self.errors.push(format!(
                "{where_}/release.json: releaseDate must be YYYY, YYYY-MM or YYYY-MM-DD"
            ));
        }
    }

    fn disc(&mut self, d: &Disc, s: &Series, r: &Release, where_: &str) {
        if let Some(problem) = d.barcode.as_deref().and_then(barcode_problem) {
            self.errors.push(format!("{where_}: barcode {problem}"));
        }
        if !is_hash(&d.content_hash) {
            self.errors.push(format!(
                "{where_}: contentHash must be 32 uppercase hex characters"
            ));
        }

        let expected = if r.media == "DVD" { "dvd" } else { "blu-ray" };
        if d.format != expected {
            self.errors.push(format!(
                "{where_}: format \"{}\" does not match the release media {} (expected \"{expected}\")",
                d.format, r.media
            ));
        }

        match &d.files {
            None => self.no_files(where_),
            Some(files) if files.is_empty() => self.no_files(where_),
            Some(files) => {
                let mut seen = HashSet::new();
                let mut reported = HashSet::new();
                for f in files {
                    let key = f.path.to_lowercase();
                    if !seen.insert(key.clone()) && reported.insert(key) {
                        self.errors
                            .push(format!("{where_}: file {} is listed twice", f.path));
                    }
                }

                if files.iter().any(|f| {
                    f.path.trim().is_empty() || f.path.contains('\\') || f.path.starts_with('/')
                }) {
                    self.errors.push(format!(
                        "{where_}: file paths must be relative, '/'-separated, with sizes >= 0"
                    ));
                } else if is_hash(&d.content_hash)
                    && listing::content_hash(files, &d.format) != d.content_hash
                {
                    self.errors.push(format!(
                        "{where_}: the files listing does not produce the contentHash {}; it belongs to a different disc or was edited",
                        d.content_hash
                    ));
                }
            }
        }

        let Some(titles) = &d.titles else { return };
        let mut seen = HashSet::new();
        for t in titles {
            self.title(t, s, where_, &mut seen);
        }
    }

    fn no_files(&mut self, where_: &str) {
        self.warnings.push(format!("{where_}: no files listing, so copies of this disc cannot be checked for added or missing files"));
    }

    fn title(&mut self, t: &DiscTitle, s: &Series, where_: &str, seen: &mut HashSet<String>) {
        let id = match (&t.title, &t.playlist) {
            (Some(n), _) => format!("title {n}"),
            (None, p) => format!("playlist {}", p.as_deref().unwrap_or("")),
        };
        if t.title.is_none() == t.playlist.is_none() {
            self.errors.push(format!("{where_}: each title needs exactly one of \"title\" (DVD) or \"playlist\" (Blu-ray)"));
            return;
        }
        if !seen.insert(id.to_lowercase()) {
            self.errors.push(format!("{where_}: {id} is listed twice"));
        }
        if let Some(d) = &t.duration
            && !Regex::new(r"^\d+:[0-5]\d:[0-5]\d$").unwrap().is_match(d)
        {
            self.errors
                .push(format!("{where_}: {id} duration must be h:mm:ss"));
        }
        match &t.kind {
            None => self
                .warnings
                .push(format!("{where_}: {id} has no type yet")),
            Some(kinds) if kinds.iter().any(|k| !TITLE_TYPES.contains(&k.as_str())) => {
                self.errors.push(format!(
                    "{where_}: {id} type must be one or more of {}",
                    TITLE_TYPES.join(", ")
                ));
            }
            _ => {}
        }

        let Some(episodes) = t.episodes.as_ref().filter(|e| !e.is_empty()) else {
            return;
        };
        if let Some(kinds) = t.kind.as_deref()
            && !kinds.is_empty()
            && kinds.iter().all(|k| NO_EPISODE_TYPES.contains(&k.as_str()))
        {
            self.errors.push(format!(
                "{where_}: {id} is a {} but lists episodes",
                kinds.join(" + ")
            ));
        }
        let main = s.main_anidb();
        let work = t.anidb.or(main);
        match &s.episodes {
            None => self.warnings.push(format!(
                "{where_}: {id} lists episodes but the series has no episode list"
            )),
            Some(list) => {
                for n in episodes {
                    if !list
                        .iter()
                        .any(|e: &Episode| e.number == *n && e.anidb.or(main) == work)
                    {
                        self.errors.push(format!(
                            "{where_}: {id} refers to {}, which is not in the series' episode list",
                            n.describe()
                        ));
                    }
                }
            }
        }
    }
}

/// What is wrong with a barcode, if anything: it must be the digits printed under the bars of an
/// EAN-13 / JAN (13), UPC-A (12) or EAN-8 (8) code, with a correct check digit.
pub fn barcode_problem(code: &str) -> Option<String> {
    if !code.bytes().all(|b| b.is_ascii_digit()) {
        return Some(format!(
            "\"{code}\" must be digits only (no spaces or dashes)"
        ));
    }
    if ![8, 12, 13].contains(&code.len()) {
        return Some(format!(
            "\"{code}\" must be 13 digits (EAN / JAN), 12 (UPC) or 8 (EAN-8)"
        ));
    }
    // Weights 3 and 1 alternate from the digit before the check digit, leftwards.
    let digits: Vec<u32> = code.bytes().map(|b| u32::from(b - b'0')).collect();
    let (body, check) = digits.split_at(digits.len() - 1);
    let sum: u32 = body
        .iter()
        .rev()
        .enumerate()
        .map(|(i, d)| if i % 2 == 0 { d * 3 } else { *d })
        .sum();
    ((10 - sum % 10) % 10 != check[0])
        .then(|| format!("\"{code}\" has a wrong check digit; check it for typos"))
}

#[cfg(test)]
mod tests {
    use super::barcode_problem;

    #[test]
    fn barcodes() {
        assert!(barcode_problem("4006381333931").is_none()); // EAN-13
        assert!(barcode_problem("036000291452").is_none()); // UPC-A
        assert!(barcode_problem("96385074").is_none()); // EAN-8
        assert!(barcode_problem("4006381333932").is_some()); // wrong check digit
        assert!(barcode_problem("4006-381333931").is_some());
        assert!(barcode_problem("12345").is_some());
    }
}
