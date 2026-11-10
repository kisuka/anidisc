//! What the static API serves. Everything is plain JSON files, so it can be hosted anywhere that
//! serves files (GitHub Pages) or read straight from a local directory:
//!
//!   api/index.json           every series, for search
//!   api/series/<slug>.json   one series with all releases and discs
//!   api/hash/<HASH>.json     one disc, with its series and release, for the CLI lookup

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::json;
use super::models::{Disc, Episode, Release, Series};
use super::repo::{DataRepository, SeriesEntry};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesSummary {
    pub slug: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anidb: Option<Vec<i32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
    #[serde(default)]
    pub releases: usize,
    #[serde(default)]
    pub discs: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseRecord {
    pub slug: String,
    pub release: Release,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discs: Option<Vec<DiscRecord>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscRecord {
    pub slug: String,
    pub disc: Disc,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SeriesRecord<'a> {
    slug: &'a str,
    series: &'a Series,
    releases: Vec<ReleaseRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HashRecord {
    pub content_hash: String,
    pub series: SeriesSummary,
    pub release: ReleaseRecord,
    pub disc: DiscRecord,
    /// The series' episodes this disc refers to, so a lookup needs one request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episodes: Option<Vec<Episode>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexRecord {
    generated: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    repository_url: Option<String>,
    disc_count: usize,
    series: Vec<SeriesSummary>,
}

pub fn build(
    data: &DataRepository,
    site_source: &Path,
    output: &Path,
    repository_url: Option<String>,
) -> io::Result<usize> {
    let api = output.join("api");
    if api.is_dir() {
        fs::remove_dir_all(&api)?;
    }
    fs::create_dir_all(api.join("hash"))?;
    fs::create_dir_all(api.join("series"))?;

    if site_source.is_dir() {
        copy_tree(site_source, output)?;
    }

    let mut summaries = Vec::new();
    for entry in &data.series {
        summaries.push(summarize(entry));
        let releases = entry
            .releases
            .iter()
            .map(|r| ReleaseRecord {
                slug: r.slug.clone(),
                release: r.release.clone(),
                discs: Some(
                    r.discs
                        .iter()
                        .map(|d| DiscRecord {
                            slug: d.slug.clone(),
                            disc: d.disc.clone(),
                        })
                        .collect(),
                ),
            })
            .collect();
        let record = SeriesRecord {
            slug: &entry.slug,
            series: &entry.series,
            releases,
        };
        fs::write(
            api.join("series").join(format!("{}.json", entry.slug)),
            json::to_compact(&record),
        )?;
    }

    let mut count = 0;
    for record in hash_records(data) {
        count += 1;
        fs::write(
            api.join("hash")
                .join(format!("{}.json", record.content_hash)),
            json::to_compact(&record),
        )?;
    }

    summaries.sort_by_key(|s| s.title.to_uppercase());
    let index = IndexRecord {
        generated: crate::time::utc_now_iso(),
        repository_url,
        disc_count: count,
        series: summaries,
    };
    fs::write(api.join("index.json"), json::to_compact(&index))?;

    // GitHub Pages would otherwise run Jekyll, which is slower and adds nothing.
    fs::write(output.join(".nojekyll"), "")?;
    Ok(count)
}

pub fn hash_records(data: &DataRepository) -> Vec<HashRecord> {
    let mut out = Vec::new();
    for entry in &data.series {
        let summary = summarize(entry);
        for release in &entry.releases {
            let release_record = ReleaseRecord {
                slug: release.slug.clone(),
                release: release.release.clone(),
                discs: None,
            };
            for disc in &release.discs {
                out.push(HashRecord {
                    content_hash: disc.disc.content_hash.clone(),
                    series: summary.clone(),
                    release: release_record.clone(),
                    disc: DiscRecord {
                        slug: disc.slug.clone(),
                        disc: disc.disc.clone(),
                    },
                    episodes: referenced_episodes(&entry.series, &disc.disc),
                });
            }
        }
    }
    out
}

fn summarize(entry: &SeriesEntry) -> SeriesSummary {
    SeriesSummary {
        slug: entry.slug.clone(),
        title: entry.series.title.clone(),
        original_title: entry.series.original_title.clone(),
        year: entry.series.year,
        anidb: entry.series.anidb.clone(),
        aliases: entry.series.aliases.clone(),
        releases: entry.releases.len(),
        discs: entry.releases.iter().map(|r| r.discs.len()).sum(),
    }
}

fn referenced_episodes(series: &Series, disc: &Disc) -> Option<Vec<Episode>> {
    let (Some(episodes), Some(titles)) = (&series.episodes, &disc.titles) else {
        return None;
    };
    let main = series.main_anidb();
    let wanted: HashSet<_> = titles
        .iter()
        .filter_map(|t| t.episodes.as_ref().map(|eps| (t.anidb.or(main), eps)))
        .flat_map(|(work, eps)| eps.iter().map(move |n| (work, *n)))
        .collect();
    let found: Vec<Episode> = episodes
        .iter()
        .filter(|e| wanted.contains(&(e.anidb.or(main), e.number)))
        .cloned()
        .collect();
    (!found.is_empty()).then_some(found)
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
