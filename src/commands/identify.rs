use serde::Serialize;

use crate::data::lookup::HashLookup;
use crate::data::models::{DiscFile, DiscTitle};
use crate::data::site::HashRecord;
use crate::disc::{listing, scanner};
use crate::{ExitCode, naming};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonResult {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    volume_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    copy: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extra_files: Option<Vec<DiscFile>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    missing_files: Option<Vec<DiscFile>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    changed_files: Option<Vec<ChangedFile>>,
    #[serde(rename = "match", skip_serializing_if = "Option::is_none")]
    matched: Option<HashRecord>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChangedFile {
    path: String,
    expected_size: u64,
    actual_size: u64,
}

pub fn run(
    inputs: &[String],
    recursive: bool,
    db: Option<&str>,
    template: &str,
    json: bool,
) -> ExitCode {
    let mut lookup = match HashLookup::create(db) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::BadArguments;
        }
    };

    let discs = scanner::find(inputs, recursive);
    if discs.is_empty() {
        eprintln!("No disc images or disc folders found.");
        return ExitCode::NothingFound;
    }

    let mut results = Vec::new();
    let mut matched = 0;
    for path in &discs {
        let scan = scanner::scan(path);
        let found = match &scan.content_hash {
            Some(hash) => match lookup.find(hash) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("{e}");
                    return ExitCode::Failure;
                }
            },
            None => None,
        };
        let target = found.as_ref().map(|m| naming::render(template, m));
        if found.is_some() {
            matched += 1;
        }

        let diff = found
            .as_ref()
            .and_then(|m| m.disc.disc.files.as_ref())
            .filter(|f| !f.is_empty())
            .map(|r| listing::compare(r, &scan.files));
        if json {
            results.push(JsonResult {
                path: path.display().to_string(),
                content_hash: scan.content_hash.clone(),
                format: scan.format.clone(),
                volume_label: scan.volume_label.clone(),
                error: scan.error.clone(),
                target,
                copy: found.as_ref().map(|_| match &diff {
                    None => "unverified",
                    Some(d) if d.is_clean() => "clean",
                    Some(_) => "altered",
                }),
                extra_files: diff.as_ref().map(|d| d.extra.clone()),
                missing_files: diff.as_ref().map(|d| d.missing.clone()),
                changed_files: diff.as_ref().map(|d| {
                    d.changed
                        .iter()
                        .map(|(e, a)| ChangedFile {
                            path: e.path.clone(),
                            expected_size: e.size,
                            actual_size: *a,
                        })
                        .collect()
                }),
                matched: found,
            });
            continue;
        }

        println!("{}", path.display());
        let Some(hash) = &scan.content_hash else {
            println!("  error  {}", scan.error.as_deref().unwrap_or(""));
            continue;
        };

        println!(
            "  hash   {hash}  ({}, label {})",
            scan.format.as_deref().unwrap_or(""),
            scan.volume_label.as_deref().unwrap_or("")
        );
        let Some(m) = found else {
            println!("  match  none (use `anidisc contribute` to add it)");
            continue;
        };

        let year = m.series.year.map(|y| format!(" ({y})")).unwrap_or_default();
        println!(
            "  match  {}{year} / {} / {}",
            m.series.title, m.release.slug, m.disc.slug
        );
        println!("  name   {}", target.unwrap_or_default());
        match diff {
            None => {
                println!("  copy   unverified (the database has no file listing for this disc)")
            }
            Some(d) => {
                if d.is_clean() {
                    println!("  copy   clean: files match the reference disc");
                } else {
                    println!("  copy   ALTERED: {}", d.summary());
                }
                for line in d.lines() {
                    println!("         {line}");
                }
            }
        }

        for t in m.disc.disc.titles.iter().flatten() {
            if t.kind.is_none() {
                continue;
            }
            let id = match t.title {
                Some(n) => format!("title {n}"),
                None => t.playlist.clone().unwrap_or_default(),
            };
            println!(
                "  {id:<12} {:>8}  {}",
                t.duration.as_deref().unwrap_or(""),
                describe(t, &m)
            );
        }
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&results).expect("serializable")
        );
    } else {
        eprintln!(
            "{} disc(s), {matched} matched in {}",
            discs.len(),
            lookup.description
        );
    }

    ExitCode::Success
}

/// "episode 1 "Title"", or the title's type and name.
pub fn describe(t: &DiscTitle, m: &HashRecord) -> String {
    if let Some(episodes) = t.episodes.as_ref().filter(|e| !e.is_empty()) {
        let main = m.series.anidb.as_ref().and_then(|a| a.first().copied());
        let work = t.anidb.or(main);
        return episodes
            .iter()
            .map(|n| {
                let title = m
                    .episodes
                    .iter()
                    .flatten()
                    .find(|e| e.number == *n && e.anidb.or(main) == work)
                    .and_then(|e| e.title.clone());
                match title {
                    Some(title) => format!("{} \"{title}\"", n.describe()),
                    None => n.describe(),
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
    }

    let kind = t.kind.as_ref().map(|k| k.join(" + ")).unwrap_or_default();
    match &t.name {
        Some(name) => format!("{kind}: {name}"),
        None => kind,
    }
}
