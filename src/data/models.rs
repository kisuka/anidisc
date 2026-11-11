//! The on-disk format of the data repository:
//!
//!   data/<series>/series.json
//!   data/<series>/<release>/release.json
//!   data/<series>/<release>/<disc>.json
//!
//! Folder and file names are slugs (lowercase letters, digits, hyphens). A disc is identified by
//! its content hash; everything else is descriptive. Field order here is the order in the files.

use serde::{Deserialize, Serialize};

use super::episode::EpisodeNumber;

pub const MEDIA: [&str; 3] = ["DVD", "BD", "UHD"];
pub const TITLE_TYPES: [&str; 6] = ["episode", "extra", "trailer", "menu", "notice", "other"];
/// Types that never hold episodes: menus, and notices (copyright warnings, rating certificates).
pub const NO_EPISODE_TYPES: [&str; 2] = ["menu", "notice"];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Series {
    pub title: String,
    #[serde(
        default,
        deserialize_with = "blank",
        skip_serializing_if = "Option::is_none"
    )]
    pub original_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anidb: Option<Vec<i32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episodes: Option<Vec<Episode>>,
    #[serde(
        default,
        deserialize_with = "blank",
        skip_serializing_if = "Option::is_none"
    )]
    pub notes: Option<String>,
}

impl Series {
    /// The main AniDB id (the first listed).
    pub fn main_anidb(&self) -> Option<i32> {
        self.anidb.as_ref().and_then(|a| a.first().copied())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Episode {
    /// 1, 2... for regular episodes; "S1", "C1", "T1", "P1", "O1" for AniDB's other kinds.
    pub number: EpisodeNumber,
    /// English title (AniDB's), or the romaji / original one when there is no English title.
    #[serde(
        default,
        deserialize_with = "blank",
        skip_serializing_if = "Option::is_none"
    )]
    pub title: Option<String>,
    /// Romanized Japanese title, e.g. "Hokenshitsu e Youkoso".
    #[serde(
        default,
        deserialize_with = "blank",
        skip_serializing_if = "Option::is_none"
    )]
    pub romaji_title: Option<String>,
    /// Title in the original language and script, e.g. "保健室へようこそ".
    #[serde(
        default,
        deserialize_with = "blank",
        skip_serializing_if = "Option::is_none"
    )]
    pub original_title: Option<String>,
    /// AniDB anime id this episode belongs to, when the series spans several AniDB entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anidb: Option<i32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    /// Region tag as used in file names, e.g. R2-JP, R1-US, A-JP.
    pub region: String,
    /// DVD, BD or UHD.
    pub media: String,
    #[serde(
        default,
        deserialize_with = "blank",
        skip_serializing_if = "Option::is_none"
    )]
    pub name: Option<String>,
    #[serde(
        default,
        deserialize_with = "blank",
        skip_serializing_if = "Option::is_none"
    )]
    pub publisher: Option<String>,
    #[serde(
        default,
        deserialize_with = "blank",
        skip_serializing_if = "Option::is_none"
    )]
    pub release_date: Option<String>,
    #[serde(
        default,
        deserialize_with = "blank",
        skip_serializing_if = "Option::is_none"
    )]
    pub edition: Option<String>,
    #[serde(
        default,
        deserialize_with = "blank",
        skip_serializing_if = "Option::is_none"
    )]
    pub notes: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Disc {
    pub content_hash: String,
    /// dvd or blu-ray.
    pub format: String,
    /// Volume 1, Disc 2, Complete Edition, Specials... Absent for a single-disc release.
    #[serde(
        default,
        deserialize_with = "blank",
        skip_serializing_if = "Option::is_none"
    )]
    pub label: Option<String>,
    #[serde(
        default,
        deserialize_with = "blank",
        skip_serializing_if = "Option::is_none"
    )]
    pub catalog_number: Option<String>,
    /// The barcode on the case the disc is sold in: EAN-13 / JAN (13 digits) or UPC-A (12).
    /// Discs sold together in one box all carry the box's barcode.
    #[serde(
        default,
        deserialize_with = "blank",
        skip_serializing_if = "Option::is_none"
    )]
    pub barcode: Option<String>,
    /// Tells re-pressings apart when they share a catalog number, e.g. "2nd pressing, 2009".
    #[serde(
        default,
        deserialize_with = "blank",
        skip_serializing_if = "Option::is_none"
    )]
    pub pressing: Option<String>,
    /// The file system volume label, for reference only.
    #[serde(
        default,
        deserialize_with = "blank",
        skip_serializing_if = "Option::is_none"
    )]
    pub volume_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub titles: Option<Vec<DiscTitle>>,
    /// Every file on the reference disc. A copy whose files differ (junk added, files removed or
    /// resized) is reported as altered even though its contentHash matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<DiscFile>>,
    #[serde(
        default,
        deserialize_with = "blank",
        skip_serializing_if = "Option::is_none"
    )]
    pub notes: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscFile {
    pub path: String,
    pub size: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscTitle {
    /// DVD logical title number (as MakeMKV and players number them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<i32>,
    /// Blu-ray playlist file, e.g. 00800.mpls.
    #[serde(
        default,
        deserialize_with = "blank",
        skip_serializing_if = "Option::is_none"
    )]
    pub playlist: Option<String>,
    /// h:mm:ss, as read from the disc.
    #[serde(
        default,
        deserialize_with = "blank",
        skip_serializing_if = "Option::is_none"
    )]
    pub duration: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapters: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// What the title is: episode, extra, trailer, menu, notice, other. One ("episode") or several
    /// (["episode", "trailer"]) for a title holding an episode followed by trailers. Absent means
    /// not yet identified; Some(empty) is written as a "" blank for the contributor.
    #[serde(
        default,
        deserialize_with = "types_in",
        serialize_with = "types_out",
        rename = "type",
        skip_serializing_if = "Option::is_none"
    )]
    pub kind: Option<Vec<String>>,
    /// Episodes (from the series' episode list) this title contains: 1, 2, "S1", "T1"...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episodes: Option<Vec<EpisodeNumber>>,
    /// AniDB anime id the episode numbers refer to, when the series spans several, or when the
    /// title is an episode of a different show included as a bonus (its title then goes in name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anidb: Option<i32>,
    /// Name for extras ("Creditless opening", "Trailer"), or the other show's title.
    #[serde(
        default,
        deserialize_with = "blank",
        skip_serializing_if = "Option::is_none"
    )]
    pub name: Option<String>,
}

/// Reads an optional text field, treating "" (or only spaces) as not filled in. `contribute`
/// writes empty fields as blanks for contributors to fill.
fn blank<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let value: Option<String> = Option::deserialize(d)?;
    Ok(value.filter(|v| !v.trim().is_empty()))
}

/// A title's type: "episode", or ["episode", "trailer"]; "" and [] read as not filled in.
fn types_in<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Vec<String>>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    let list = match Option::<OneOrMany>::deserialize(d)? {
        None => Vec::new(),
        Some(OneOrMany::One(t)) => vec![t],
        Some(OneOrMany::Many(ts)) => ts,
    };
    let mut out: Vec<String> = Vec::new();
    for t in list.into_iter().map(|t| t.trim().to_string()) {
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    }
    Ok((!out.is_empty()).then_some(out))
}

/// One type is written as a plain string, several as a list, none as a "" blank.
fn types_out<S: serde::Serializer>(v: &Option<Vec<String>>, s: S) -> Result<S::Ok, S::Error> {
    match v.as_deref() {
        None => s.serialize_none(),
        Some([]) => s.serialize_str(""),
        Some([one]) => s.serialize_str(one),
        Some(many) => many.serialize(s),
    }
}

/// The JSON keys each kind of data file may contain; anything else is an error, so typos in
/// contributed files are caught.
pub mod keys {
    pub const SERIES: &[&str] = &[
        "title",
        "originalTitle",
        "year",
        "anidb",
        "aliases",
        "episodes",
        "notes",
    ];
    pub const EPISODE: &[&str] = &["number", "title", "romajiTitle", "originalTitle", "anidb"];
    pub const RELEASE: &[&str] = &[
        "region",
        "media",
        "name",
        "publisher",
        "releaseDate",
        "edition",
        "notes",
    ];
    pub const DISC: &[&str] = &[
        "contentHash",
        "format",
        "label",
        "catalogNumber",
        "barcode",
        "pressing",
        "volumeLabel",
        "titles",
        "files",
        "notes",
    ];
    pub const TITLE: &[&str] = &[
        "title",
        "playlist",
        "duration",
        "chapters",
        "sizeBytes",
        "type",
        "episodes",
        "anidb",
        "name",
    ];
    pub const FILE: &[&str] = &["path", "size"];
}
