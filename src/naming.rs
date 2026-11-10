//! Target paths from a template, and best-effort guesses from existing file names.

use std::path::Path;

use regex_lite::Regex;

use crate::data::site::HashRecord;

pub const DEFAULT_TEMPLATE: &str = "{series} ({year})/{label} [{media}] [{region}] [{catalog}]";

pub const TEMPLATE_HELP: &str = "Target path template. Placeholders: {series} {original} {year} {label} {media} {region} \
{catalog} {edition} {pressing} {publisher} {release} {volumelabel} {anidb} {hash}. '/' starts a subfolder. \
Empty [] and () left by missing values are removed.";

/// The target path (relative, '/'-separated, no extension) for a disc.
pub fn render(template: &str, record: &HashRecord) -> String {
    let release = &record.release.release;
    let disc = &record.disc.disc;
    let value = |key: &str| -> Option<Option<String>> {
        Some(match key.to_ascii_lowercase().as_str() {
            "series" => Some(record.series.title.clone()),
            "original" => record.series.original_title.clone(),
            "year" => record.series.year.map(|y| y.to_string()),
            "label" => disc.label.clone(),
            "media" => Some(release.media.clone()),
            "region" => Some(release.region.clone()),
            "catalog" => disc.catalog_number.clone(),
            "edition" => release.edition.clone(),
            "pressing" => disc.pressing.clone(),
            "publisher" => release.publisher.clone(),
            "release" => release.name.clone(),
            "volumelabel" => disc.volume_label.clone(),
            "anidb" => record
                .series
                .anidb
                .as_ref()
                .map(|a| a.first().copied().unwrap_or(0).to_string()),
            "hash" => Some(record.content_hash.clone()),
            _ => return None,
        })
    };

    let placeholder = Regex::new(r"\{([a-zA-Z]+)\}").unwrap();
    template
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|segment| {
            placeholder
                .replace_all(segment, |c: &regex_lite::Captures| match value(&c[1]) {
                    Some(v) => sanitize(v.as_deref().unwrap_or("")),
                    None => c[0].to_string(),
                })
                .into_owned()
        })
        .map(|s| clean_segment(&s))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/// A folder or file name safe on Windows, macOS and Linux.
fn sanitize(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut out = String::with_capacity(value.len());
    for (i, &c) in chars.iter().enumerate() {
        match c {
            // "Title: Subtitle" reads best as "Title - Subtitle".
            ':' => out.push_str(if chars.get(i + 1) == Some(&' ') {
                " -"
            } else {
                "-"
            }),
            '/' | '\\' | '|' => out.push('-'),
            '?' | '*' | '"' | '<' | '>' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn clean_segment(segment: &str) -> String {
    let empty = Regex::new(r"\s*(\[\s*\]|\(\s*\))\s*").unwrap();
    let spaces = Regex::new(r"\s{2,}").unwrap();
    let s = empty.replace_all(segment, " ");
    let s = spaces.replace_all(&s, " ");
    s.trim().trim_end_matches('.').trim().to_string()
}

// ---------------------------------------------------------------- hints from existing names

#[derive(Default, Debug)]
pub struct FileHints {
    pub series: Option<String>,
    pub year: Option<i32>,
    pub label: Option<String>,
    pub media: Option<String>,
    pub region: Option<String>,
    pub catalog_number: Option<String>,
    pub edition: Option<String>,
}

/// Best-effort guesses from a "Title (Year)/Label [Media] [Region] [Catalog].iso" style path,
/// used only to pre-fill contribution stubs.
pub fn guess(disc: &Path) -> FileHints {
    let name = if disc.is_dir() {
        disc.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        disc.file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };

    let mut hints = FileHints::default();
    if let Some(parent) = disc
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        && let Some(m) = Regex::new(r"^#?\s*(.+?)\s*\((\d{4})\)\s*$")
            .unwrap()
            .captures(&parent)
    {
        hints.series = Some(m[1].trim().to_string());
        let y: i32 = m[2].parse().unwrap_or(0);
        hints.year = (y != 1905).then_some(y); // 1905 is a common "year unknown" placeholder
    }

    let tag = Regex::new(r"\[([^\]]*)\]").unwrap();
    let anidb = Regex::new(r"(?i)^anidb[-=]\d+$").unwrap();
    let media = Regex::new(r"(?i)^(DVD|BD|UHD|Blu-?ray)$").unwrap();
    let region = Regex::new(r"^(R[0-9](-[A-Z]{2,3})?|[ABC](-[A-Z]{2,3})?|R?ALL(-[A-Z]{2,3})?|NTSC-[A-Z]{2}|PAL-[A-Z]{2})$").unwrap();
    let mut extras = Vec::new();
    for c in tag.captures_iter(&name) {
        let value = c[1].trim().to_string();
        if anidb.is_match(&value) {
            continue; // [anidb-1234] names the series, not the disc
        }
        if hints.media.is_none() && media.is_match(&value) {
            let upper = value.to_uppercase();
            hints.media = Some(if upper == "BLU-RAY" || upper == "BLURAY" {
                "BD".to_string()
            } else {
                value
            });
        } else if hints.region.is_none() && region.is_match(&value) {
            hints.region = Some(value);
        } else if hints.catalog_number.is_none() && is_catalog(&value) {
            hints.catalog_number = Some(value);
        } else if !value.is_empty() {
            extras.push(value); // [First Press], [Deluxe Complete Edition], ...
        }
    }

    let without_tags = tag.replace_all(&name, "");
    let label = Regex::new(r"(?i)\{anidb[-=]\d+\}")
        .unwrap()
        .replace_all(&without_tags, "")
        .trim()
        .to_string();
    hints.label = (!label.is_empty()).then_some(label);
    hints.edition = (!extras.is_empty()).then(|| extras.join(", "));
    hints
}

/// Letters and digits (both present), in groups joined by - _ space or dot: "KVDVD-0603".
fn is_catalog(value: &str) -> bool {
    value.bytes().any(|b| b.is_ascii_alphabetic())
        && value.bytes().any(|b| b.is_ascii_digit())
        && Regex::new(r"^[A-Za-z0-9]+([-_ .][A-Za-z0-9]+)*$")
            .unwrap()
            .is_match(value)
}

/// Lowercase ASCII letters and digits joined by single hyphens. Accented Latin letters lose their
/// accents; anything else separates words.
pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if let Some(base) = fold_accent(c) {
            out.push(base);
        } else if is_combining_mark(c) {
            // dropped, like the accent of a decomposed letter
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn is_combining_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

/// The base letter of an accented Latin letter (those with a canonical decomposition), lowercased.
fn fold_accent(c: char) -> Option<char> {
    const TABLE: &[(&str, char)] = &[
        ("ÀÁÂÃÄÅàáâãäåĀāĂăĄąǍǎǺǻ", 'a'),
        ("ÇçĆćĈĉĊċČč", 'c'),
        ("ĎďḌḍ", 'd'),
        ("ÈÉÊËèéêëĒēĔĕĖėĘęĚěẼẽ", 'e'),
        ("ĜĝĞğĠġĢģǦǧ", 'g'),
        ("ĤĥḤḥ", 'h'),
        ("ÌÍÎÏìíîïĨĩĪīĬĭĮįİǏǐ", 'i'),
        ("Ĵĵ", 'j'),
        ("ĶķǨǩ", 'k'),
        ("ĹĺĻļĽľ", 'l'),
        ("ÑñŃńŅņŇň", 'n'),
        ("ÒÓÔÕÖòóôõöŌōŎŏŐőǑǒƠơ", 'o'),
        ("ŔŕŖŗŘř", 'r'),
        ("ŚśŜŝŞşŠšȘș", 's'),
        ("ŢţŤťȚț", 't'),
        ("ÙÚÛÜùúûüŨũŪūŬŭŮůŰűŲųǓǔǕǖǗǘǙǚǛǜƯư", 'u'),
        ("Ŵŵ", 'w'),
        ("ÝýÿŶŷŸ", 'y'),
        ("ŹźŻżŽž", 'z'),
    ];
    TABLE
        .iter()
        .find(|(set, _)| set.contains(c))
        .map(|&(_, base)| base)
}
