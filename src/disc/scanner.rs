//! Finding discs under the given paths, and scanning one: hash, file list, titles.

use std::fs;
use std::path::{Path, PathBuf};

use super::{DiscSource, bluray, dvd, is_disc_folder, is_image, listing};
use crate::data::models::{DiscFile, DiscTitle};

pub struct ScannedDisc {
    pub is_folder: bool,
    pub content_hash: Option<String>,
    pub format: Option<String>,
    pub volume_label: Option<String>,
    pub titles: Option<Vec<DiscTitle>>,
    pub error: Option<String>,
    /// Every file in the image or folder, '/'-separated, sorted.
    pub files: Vec<DiscFile>,
}

/// Disc images (.iso/.img/.udf) and disc folders (a folder holding VIDEO_TS or BDMV) under the
/// inputs. A disc folder is never descended into.
pub fn find(inputs: &[String], recursive: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for input in inputs {
        let path = std::path::absolute(input).unwrap_or_else(|_| PathBuf::from(input));
        if path.is_file() {
            if is_image(&path) {
                out.push(path);
            }
            continue;
        }

        if !path.is_dir() {
            continue;
        }

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.eq_ignore_ascii_case("VIDEO_TS") || name.eq_ignore_ascii_case("BDMV") {
            if let Some(parent) = path.parent() {
                out.push(parent.to_path_buf());
            }
            continue;
        }

        if is_disc_folder(&path) {
            out.push(path);
            continue;
        }

        walk(&path, recursive, &mut out);
    }

    out
}

fn walk(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => dirs.push(path),
            Ok(_) if is_image(&path) => files.push(path),
            _ => {}
        }
    }

    let key = |p: &PathBuf| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_uppercase())
            .unwrap_or_default()
    };
    files.sort_by_key(key);
    dirs.sort_by_key(key);
    out.extend(files);
    for child in dirs {
        if is_disc_folder(&child) {
            out.push(child);
        } else if recursive {
            walk(&child, recursive, out);
        }
    }
}

pub fn scan(path: &Path) -> ScannedDisc {
    let is_folder = path.is_dir();
    let source = match DiscSource::open(path) {
        Ok(s) => s,
        Err(error) => {
            return ScannedDisc {
                is_folder,
                content_hash: None,
                format: None,
                volume_label: None,
                titles: None,
                error: Some(error),
                files: Vec::new(),
            };
        }
    };

    let files: Vec<DiscFile> = source
        .files
        .iter()
        .map(|f| DiscFile {
            path: f.path.clone(),
            size: f.size,
        })
        .collect();
    let hash = listing::content_hash(&files, source.format);
    let titles = if source.format == "dvd" {
        dvd::read(&source)
    } else {
        bluray::read(&source)
    };
    let (titles, error) = match titles {
        Ok(t) => (Some(t), None),
        Err(e) => (None, Some(format!("titles could not be read: {e}"))),
    };

    ScannedDisc {
        is_folder,
        content_hash: Some(hash),
        format: Some(source.format.to_string()),
        volume_label: Some(source.label.clone()),
        titles,
        error,
        files,
    }
}
