//! The content hash, and comparing a copy's files against the reference disc's.

use std::collections::{HashMap, HashSet};

use md5::{Digest, Md5};

use crate::data::models::DiscFile;

/// A disc's content hash: MD5 over the sizes (int64 little-endian) of its video files, ordered by
/// file name. Used both when scanning and to check a stored listing matches its contentHash.
///   DVD: VIDEO_TS/*.VOB, *.IFO, *.BUP. Blu-ray: BDMV/STREAM/*.m2ts (not in subfolders).
pub fn content_hash(files: &[DiscFile], format: &str) -> String {
    let mut inputs: Vec<&DiscFile> = files
        .iter()
        .filter(|f| is_hash_input(&f.path, format))
        .collect();
    inputs.sort_by(|a, b| file_name(&a.path).cmp(file_name(&b.path))); // stable, like the original
    let mut md5 = Md5::new();
    for f in inputs {
        md5.update((f.size as i64).to_le_bytes());
    }
    md5.finalize().iter().map(|b| format!("{b:02X}")).collect()
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len()
        && s.is_char_boundary(prefix.len())
        && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn ends_with_ci(s: &str, suffix: &str) -> bool {
    s.len() >= suffix.len()
        && s.is_char_boundary(s.len() - suffix.len())
        && s[s.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

fn is_hash_input(path: &str, format: &str) -> bool {
    if starts_with_ci(path, "BDMV/BACKUP/") || ends_with_ci(path, ".ssif") {
        return false;
    }

    match format {
        "blu-ray" | "uhd-blu-ray" => {
            starts_with_ci(path, "BDMV/STREAM/")
                && ends_with_ci(path, ".m2ts")
                && !path["BDMV/STREAM/".len()..].contains('/')
        }
        "dvd" => {
            starts_with_ci(path, "VIDEO_TS/")
                && (ends_with_ci(path, ".vob")
                    || ends_with_ci(path, ".ifo")
                    || ends_with_ci(path, ".bup"))
        }
        _ => false,
    }
}

pub struct ListingDiff {
    pub extra: Vec<DiscFile>,
    pub missing: Vec<DiscFile>,
    pub changed: Vec<(DiscFile, u64)>,
}

impl ListingDiff {
    pub fn is_clean(&self) -> bool {
        self.extra.is_empty() && self.missing.is_empty() && self.changed.is_empty()
    }

    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.extra.is_empty() {
            parts.push(format!("{} extra file(s)", self.extra.len()));
        }
        if !self.missing.is_empty() {
            parts.push(format!("{} missing file(s)", self.missing.len()));
        }
        if !self.changed.is_empty() {
            parts.push(format!(
                "{} file(s) with a different size",
                self.changed.len()
            ));
        }
        if parts.is_empty() {
            "clean".to_string()
        } else {
            parts.join(", ")
        }
    }

    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.extend(
            self.extra
                .iter()
                .map(|f| format!("+ {} ({} bytes)", f.path, f.size)),
        );
        out.extend(self.missing.iter().map(|f| format!("- {}", f.path)));
        out.extend(
            self.changed
                .iter()
                .map(|(e, a)| format!("~ {} ({} -> {} bytes)", e.path, e.size, a)),
        );
        out
    }
}

/// Disc file systems are case-insensitive in practice (ISO 9660 is upper case, UDF readers vary).
pub fn compare(reference: &[DiscFile], actual: &[DiscFile]) -> ListingDiff {
    let expected: HashMap<String, &DiscFile> = reference
        .iter()
        .map(|f| (f.path.to_lowercase(), f))
        .collect();
    let mut seen = HashSet::new();
    let mut extra = Vec::new();
    let mut changed = Vec::new();
    for f in actual {
        let key = f.path.to_lowercase();
        match expected.get(&key) {
            None => extra.push(f.clone()),
            Some(e) if e.size != f.size => changed.push(((*e).clone(), f.size)),
            _ => {}
        }
        seen.insert(key);
    }

    let missing = reference
        .iter()
        .filter(|f| !seen.contains(&f.path.to_lowercase()))
        .cloned()
        .collect();
    ListingDiff {
        extra,
        missing,
        changed,
    }
}
