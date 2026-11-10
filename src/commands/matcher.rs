//! Works out the AniDB id of a disc's series. Folder names are never guessed from: the id must be
//! given explicitly, in order by --anidb; an anidisc.json in the disc's folder or a parent (up to
//! the folder that was scanned); or an [anidb-1234] / {anidb-1234} tag in the disc's or a folder's
//! name (the Plex HAMA convention). Anything else is unmatched.

use std::fs;
use std::path::Path;

use regex_lite::Regex;
use serde_json::Value;

use crate::data::json::{self, Kind};

pub const SIDECAR_NAME: &str = "anidisc.json";

/// The AniDB anime a disc belongs to, and where that came from.
pub struct SeriesMatch {
    pub anidb: i32,
    pub source: String,
}

pub fn find(disc: &Path, scan_root: &Path, anidb_flag: Option<i32>) -> Option<SeriesMatch> {
    if let Some(flag) = anidb_flag {
        return Some(SeriesMatch {
            anidb: flag,
            source: format!("--anidb {flag}"),
        });
    }

    let stop = if scan_root.is_file() {
        scan_root.parent().unwrap_or(scan_root)
    } else {
        scan_root
    };
    let stop = std::path::absolute(stop).unwrap_or_else(|_| stop.to_path_buf());

    // The disc's own name can carry a tag too ("Show [anidb-123] Vol 1.iso", or a VIDEO_TS rip folder).
    let own = file_name(disc);
    if let Some(id) = tag(&own) {
        return Some(SeriesMatch {
            anidb: id,
            source: format!("[anidb-{id}] in \"{own}\""),
        });
    }

    let stop_text = stop.to_string_lossy().to_lowercase();
    for dir in disc.ancestors().skip(1) {
        let sidecar = dir.join(SIDECAR_NAME);
        if sidecar.is_file()
            && let Some(id) = read_sidecar(&sidecar)
        {
            return Some(SeriesMatch {
                anidb: id,
                source: sidecar.display().to_string(),
            });
        }

        let name = file_name(dir);
        if let Some(id) = tag(&name) {
            return Some(SeriesMatch {
                anidb: id,
                source: format!("[anidb-{id}] in folder \"{name}\""),
            });
        }

        let here = dir.to_string_lossy().to_lowercase();
        if here.trim_end_matches(['\\', '/']) == stop_text.trim_end_matches(['\\', '/'])
            || here.len() < stop_text.len()
        {
            break;
        }
    }

    None
}

pub fn print_help(unmatched: &[String]) {
    eprintln!();
    eprintln!(
        "{} disc(s) were skipped because they have no AniDB id:",
        unmatched.len()
    );
    for path in unmatched {
        eprintln!("  {path}");
    }
    eprintln!();
    eprintln!(
        "Give each disc its series' AniDB id (the number in anidb.net/anime/<id>) in one of these ways:"
    );
    eprintln!("  - pass --anidb <id> to apply one series to every disc in the run");
    eprintln!(
        "  - put a file named {SIDECAR_NAME} in the disc's folder containing:  {{ \"anidb\": 12345 }}"
    );
    eprintln!("  - put the id in the disc's or folder's name:  \"Some Title [anidb-12345]\"");
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn tag(name: &str) -> Option<i32> {
    let re = Regex::new(r"(?i)[\[{]anidb[-=](\d+)[\]}]").unwrap();
    re.captures(name).and_then(|c| c[1].parse().ok())
}

fn read_sidecar(file: &Path) -> Option<i32> {
    let parsed = fs::read_to_string(file)
        .map_err(|e| e.to_string())
        .and_then(|t| json::parse_lenient::<Value>(&t, Kind::Any));
    match parsed {
        Ok(v) => match v.get("anidb").and_then(Value::as_i64) {
            Some(id) if id > 0 && id <= i32::MAX as i64 => Some(id as i32),
            _ => {
                eprintln!("warning: {} has no \"anidb\" id; ignored", file.display());
                None
            }
        },
        Err(e) => {
            eprintln!(
                "warning: {} could not be read ({e}); ignored",
                file.display()
            );
            None
        }
    }
}
