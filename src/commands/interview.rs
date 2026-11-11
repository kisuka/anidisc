//! `contribute --interactive`: after a disc's files are written, asks for what can't be read from
//! the disc (region, label, catalog number, barcode, and what each title is), with a checklist of
//! the series' episodes. Answers are saved after each question, so stopping keeps them; anything
//! skipped stays blank for later.

use std::io::IsTerminal;
use std::path::Path;

use dialoguer::theme::ColorfulTheme;
use dialoguer::{Input, MultiSelect, Select};

use crate::data::episode::EpisodeNumber;
use crate::data::json::{self, Kind};
use crate::data::models::{Disc, DiscTitle, Episode, NO_EPISODE_TYPES, Release, Series};
use crate::data::repo::barcode_problem;

/// Whether questions can be asked: both reading answers and showing menus need a terminal.
pub fn available() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

pub struct Files<'a> {
    pub disc_path: &'a Path,
    pub disc_file: &'a Path,
    pub release_file: &'a Path,
    pub new_release: bool,
    pub series_file: &'a Path,
}

/// Whether to keep asking about the discs after this one.
pub enum Next {
    Continue,
    StopAll,
}

enum Stop {
    /// Esc, or "stop asking about this disc".
    Disc,
    /// "stop asking about all discs".
    All,
}

pub fn ask(f: &Files) -> Result<Next, String> {
    match run(f) {
        Ok(()) | Err(Failure::Stop(Stop::Disc)) => Ok(Next::Continue),
        Err(Failure::Stop(Stop::All)) => Ok(Next::StopAll),
        Err(Failure::Error(e)) => Err(e),
    }
}

enum Failure {
    Stop(Stop),
    Error(String),
}

impl From<Stop> for Failure {
    fn from(s: Stop) -> Self {
        Failure::Stop(s)
    }
}

impl From<dialoguer::Error> for Failure {
    fn from(e: dialoguer::Error) -> Self {
        Failure::Error(e.to_string())
    }
}

fn read<T: serde::de::DeserializeOwned>(path: &Path, kind: Kind) -> Result<T, Failure> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| Failure::Error(format!("{}: {e}", path.display())))?;
    json::parse_lenient(&text, kind).map_err(|e| Failure::Error(format!("{}: {e}", path.display())))
}

fn save<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), Failure> {
    json::write_file(path, value).map_err(|e| Failure::Error(format!("{}: {e}", path.display())))
}

/// Other releases of the series, as (folder, "Volume 1 (QNB-M033), Volume 2 (QNB-M034)").
fn other_releases(own: &Path) -> Vec<(std::path::PathBuf, String)> {
    let Some(series_dir) = own.parent() else {
        return Vec::new();
    };
    // A box holds discs of one kind: a DVD can only join a DVD release.
    let media = |dir: &Path| -> Option<String> {
        let text = std::fs::read_to_string(dir.join("release.json")).ok()?;
        let r: Release = json::parse_lenient(&text, Kind::Release).ok()?;
        Some(r.media.to_uppercase())
    };
    let own_media = media(own);
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(series_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if dir == own || !dir.join("release.json").is_file() || media(&dir) != own_media {
            continue;
        }
        let mut discs: Vec<String> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|x| x == "json")
                    && p.file_name().is_some_and(|n| n != "release.json")
            })
            .filter_map(|p| {
                let d: Disc =
                    json::parse_lenient(&std::fs::read_to_string(&p).ok()?, Kind::Disc).ok()?;
                let label = d.label.unwrap_or_else(|| "Disc".to_string());
                Some(match d.catalog_number {
                    Some(c) => format!("{label} ({c})"),
                    None => label,
                })
            })
            .collect();
        discs.sort();
        let slug = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push((dir, format!("{slug}: {}", discs.join(", "))));
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// For a disc that came in a box with others: moves its file into that box's release folder and
/// removes the release folder `contribute` had just made for it. Returns the disc file's new path.
fn join_release(
    disc_file: &Path,
    release_file: &Path,
    into: &Path,
) -> Result<std::path::PathBuf, Failure> {
    let name = disc_file
        .file_name()
        .ok_or_else(|| Failure::Error("bad disc file name".into()))?;
    let target = into.join(name);
    if target.exists() {
        return Err(Failure::Error(format!(
            "{} already exists",
            target.display()
        )));
    }
    std::fs::rename(disc_file, &target)
        .map_err(|e| Failure::Error(format!("could not move {}: {e}", disc_file.display())))?;
    let _ = std::fs::remove_file(release_file);
    if let Some(dir) = release_file.parent() {
        let _ = std::fs::remove_dir(dir); // only if now empty
    }
    Ok(target)
}

fn run(f: &Files) -> Result<(), Failure> {
    let theme = ColorfulTheme::default();
    let mut disc_file = f.disc_file.to_path_buf();
    let mut new_release = f.new_release;
    let mut disc: Disc = read(&disc_file, Kind::Disc)?;
    keep_disc_blanks(&mut disc);
    let series: Series = read(f.series_file, Kind::Series)?;
    let titles = disc.titles.as_ref().map_or(0, Vec::len);

    eprintln!();
    eprintln!(
        "── {}  ({})",
        disc.label
            .as_deref()
            .filter(|l| !l.is_empty())
            .unwrap_or("Disc"),
        f.disc_path.display()
    );
    eprintln!(
        "   {} · {titles} title(s). Enter keeps the value shown or skips; Esc stops asking about this disc.",
        series.title
    );

    // A release is one product as sold. A disc sold on its own keeps the release made for it;
    // a disc from a box set joins the release of the box, if another of its discs is already here.
    let own_dir = f.release_file.parent().unwrap_or(Path::new("."));
    let others = if new_release {
        other_releases(own_dir)
    } else {
        Vec::new()
    };
    if !others.is_empty() {
        let mut items = vec!["sold on its own (its own release)".to_string()];
        items.extend(
            others
                .iter()
                .map(|(_, d)| format!("in the same box as {d}")),
        );
        let choice = Select::with_theme(&theme)
            .with_prompt("Was this disc sold on its own, or in one box with discs already added?")
            .items(&items)
            .default(0)
            .interact_opt()?
            .ok_or(Stop::Disc)?;
        if choice > 0 {
            disc_file = join_release(&disc_file, f.release_file, &others[choice - 1].0)?;
            new_release = false;
            eprintln!("   Moved to {}", disc_file.display());
        }
    }

    if new_release {
        let mut release: Release = read(f.release_file, Kind::Release)?;
        for field in [
            &mut release.name,
            &mut release.publisher,
            &mut release.release_date,
            &mut release.edition,
        ] {
            field.get_or_insert_with(String::new);
        }
        eprintln!("   Release (the product as sold: this volume, or the box it came in):");
        release.region = text(
            &theme,
            "Region (e.g. R2-JP, R1-US, A-JP)",
            &release.region,
            |v| {
                (!v.trim().is_empty())
                    .then_some(())
                    .ok_or("the region is required")
            },
        )?;
        release.publisher = optional(
            &theme,
            "Publisher",
            release.publisher.as_deref(),
            |_| Ok(()),
        )?;
        release.release_date = optional(
            &theme,
            "Release date (YYYY-MM-DD, YYYY-MM or YYYY)",
            release.release_date.as_deref(),
            |v| {
                let ok = regex_lite::Regex::new(r"^\d{4}(-\d{2}(-\d{2})?)?$").unwrap();
                ok.is_match(v)
                    .then_some(())
                    .ok_or("use YYYY-MM-DD, YYYY-MM or YYYY")
            },
        )?;
        save(f.release_file, &release)?;
    }

    eprintln!("   This disc:");
    disc.label = optional(
        &theme,
        "Label (Volume 1, Disc 2; empty if it's the only disc)",
        disc.label.as_deref(),
        |_| Ok(()),
    )?;
    disc.catalog_number = optional(
        &theme,
        "Catalog number (on the disc or spine)",
        disc.catalog_number.as_deref(),
        |_| Ok(()),
    )?;
    disc.barcode = barcode(
        &theme,
        "Barcode on the case (a box set's, if it came in one)",
        disc.barcode.as_deref(),
    )?;
    save(&disc_file, &disc)?;

    let episodes = series.episodes.clone().unwrap_or_default();
    let main = series.main_anidb();
    if episodes.is_empty() {
        eprintln!("   (series.json has no episode list, so titles can only be given a type)");
    }
    for i in 0..titles {
        let title = disc.titles.as_ref().expect("counted above")[i].clone();
        let answered = ask_title(&theme, &title, &episodes, main)?;
        disc.titles.as_mut().expect("counted above")[i] = answered;
        save(&disc_file, &disc)?;
    }
    eprintln!("   Saved {}", disc_file.display());
    Ok(())
}

const KINDS: [&str; 6] = ["episode", "extra", "trailer", "menu", "notice", "other"];
const STOP_DISC: &str = "skip the rest of this disc";
const STOP_ALL: &str = "stop asking about all discs";
const GO_ON: &str = "no, go back to this title";

fn ask_title(
    theme: &ColorfulTheme,
    t: &DiscTitle,
    episodes: &[Episode],
    main: Option<i32>,
) -> Result<DiscTitle, Failure> {
    let mut t = t.clone();
    let id = match (&t.title, &t.playlist) {
        (Some(n), _) => format!("Title {n}"),
        (None, Some(p)) => format!("Playlist {p}"),
        _ => "Title".to_string(),
    };
    let seconds = t.duration.as_deref().map_or(0, seconds);
    let prompt = format!(
        "{id} · {} · {} chapter(s): what's in it? (Space ticks each kind it holds, Enter confirms; none if unsure, Esc to stop)",
        t.duration.as_deref().unwrap_or("?"),
        t.chapters.unwrap_or(0)
    );

    let current: Vec<String> = t.kind.clone().unwrap_or_default();
    // Without an answer yet, a guess from the length: menus loop for seconds, episodes run for
    // many minutes.
    let ticked: Vec<bool> = KINDS
        .iter()
        .map(|k| {
            if current.is_empty() {
                match seconds {
                    0..=30 => *k == "menu",
                    600.. => *k == "episode",
                    _ => false,
                }
            } else {
                current.iter().any(|c| c == k)
            }
        })
        .collect();
    let Some(picked) = MultiSelect::with_theme(theme)
        .with_prompt(prompt)
        .items(KINDS)
        .defaults(&ticked)
        .interact_opt()?
    else {
        let stop = Select::with_theme(theme)
            .with_prompt("Stop asking?")
            .items([STOP_DISC, STOP_ALL, GO_ON])
            .default(0)
            .interact_opt()?;
        return match stop {
            Some(1) => Err(Stop::All.into()),
            Some(2) => ask_title(theme, &t, episodes, main),
            _ => Err(Stop::Disc.into()),
        };
    };
    let kinds: Vec<String> = picked.iter().map(|&i| KINDS[i].to_string()).collect();
    t.kind = Some(kinds.clone());
    let has = |k: &str| kinds.iter().any(|x| x == k);
    // Menus and notices (copyright warnings, rating certificates) hold no episodes.
    let only_menu =
        !kinds.is_empty() && kinds.iter().all(|k| NO_EPISODE_TYPES.contains(&k.as_str()));
    if only_menu {
        t.episodes = Some(Vec::new());
    }
    if kinds.is_empty() || only_menu || episodes.is_empty() {
        return Ok(t);
    }

    // Regular episodes first for an episode, AniDB's other kinds (S, C, T...) first otherwise.
    let mut order: Vec<&Episode> = episodes.iter().collect();
    if !has("episode") {
        order.sort_by_key(|e| (e.number.kind.is_none(), e.number));
    }
    let labels: Vec<String> = order
        .iter()
        .map(|e| {
            let work = e
                .anidb
                .filter(|a| Some(*a) != main)
                .map(|a| format!("  (AniDB {a})"))
                .unwrap_or_default();
            format!(
                "{:>4}  {}{work}",
                e.number.to_string(),
                e.title.as_deref().unwrap_or("")
            )
        })
        .collect();
    let picked: Vec<EpisodeNumber> = t.episodes.clone().unwrap_or_default();
    let ticked: Vec<bool> = order
        .iter()
        .map(|e| picked.contains(&e.number) && e.anidb.or(main) == t.anidb.or(main))
        .collect();
    let chosen = MultiSelect::with_theme(theme)
        .with_prompt(format!(
            "{id}: tick everything it contains, e.g. an episode plus trailers (Space ticks, Enter confirms)"
        ))
        .items(&labels)
        .defaults(&ticked)
        .interact_opt()?
        .ok_or(Stop::Disc)?;

    let chosen: Vec<&Episode> = chosen.into_iter().map(|i| order[i]).collect();
    let works: Vec<Option<i32>> = {
        let mut w: Vec<Option<i32>> = chosen.iter().map(|e| e.anidb.or(main)).collect();
        w.dedup();
        w
    };
    if works.len() > 1 {
        eprintln!(
            "   A title can only hold episodes of one AniDB entry; kept those of AniDB {}.",
            works[0].unwrap_or(0)
        );
    }
    let work = works.first().copied().flatten();
    t.episodes = Some(
        chosen
            .iter()
            .filter(|e| e.anidb.or(main) == work)
            .map(|e| e.number)
            .collect(),
    );
    t.anidb = work.filter(|w| Some(*w) != main);

    if kinds
        .iter()
        .any(|k| !matches!(k.as_str(), "episode" | "menu" | "notice"))
    {
        t.name = optional(
            theme,
            &format!("{id}: name of the extras/trailers (e.g. PV, Creditless opening)"),
            t.name.as_deref(),
            |_| Ok(()),
        )?;
        if t.name.as_deref() == Some("") {
            t.name = None;
        }
    }
    Ok(t)
}

/// A required text answer.
fn text(
    theme: &ColorfulTheme,
    prompt: &str,
    current: &str,
    check: impl Fn(&str) -> Result<(), &'static str>,
) -> Result<String, Failure> {
    let mut input = Input::<String>::with_theme(theme)
        .with_prompt(prompt)
        .allow_empty(true)
        .validate_with(|v: &String| check(v));
    if !current.trim().is_empty() {
        input = input.default(current.to_string());
    }
    Ok(input.interact_text()?.trim().to_string())
}

/// An optional answer; empty stays blank ("") in the file, for filling in later.
fn optional(
    theme: &ColorfulTheme,
    prompt: &str,
    current: Option<&str>,
    check: impl Fn(&str) -> Result<(), &'static str>,
) -> Result<Option<String>, Failure> {
    let mut input = Input::<String>::with_theme(theme)
        .with_prompt(prompt)
        .allow_empty(true)
        .validate_with(|v: &String| {
            let v = v.trim();
            if v.is_empty() { Ok(()) } else { check(v) }
        });
    if let Some(c) = current.filter(|c| !c.trim().is_empty()) {
        input = input.default(c.to_string());
    }
    Ok(Some(input.interact_text()?.trim().to_string()))
}

fn barcode(
    theme: &ColorfulTheme,
    prompt: &str,
    current: Option<&str>,
) -> Result<Option<String>, Failure> {
    let mut input = Input::<String>::with_theme(theme)
        .with_prompt(format!("{prompt} (13-digit JAN/EAN or 12-digit UPC)"))
        .allow_empty(true)
        .validate_with(|v: &String| -> Result<(), String> {
            let v: String = v
                .chars()
                .filter(|c| !c.is_whitespace() && *c != '-')
                .collect();
            match barcode_problem(&v) {
                Some(problem) if !v.is_empty() => Err(problem),
                _ => Ok(()),
            }
        });
    if let Some(c) = current.filter(|c| !c.trim().is_empty()) {
        input = input.default(c.to_string());
    }
    let v: String = input
        .interact_text()?
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    Ok(Some(v))
}

/// Reading a file turns blanks ("") into "not set"; put them back so that whatever is skipped
/// still shows as a blank to fill in later.
fn keep_disc_blanks(disc: &mut Disc) {
    for field in [&mut disc.label, &mut disc.catalog_number, &mut disc.barcode] {
        field.get_or_insert_with(String::new);
    }
    for t in disc.titles.iter_mut().flatten() {
        t.kind.get_or_insert_with(Vec::new);
        t.episodes.get_or_insert_with(Vec::new);
    }
}

/// "0:47:46" → 2866.
fn seconds(duration: &str) -> u64 {
    duration.split(':').fold(0, |acc, part| {
        acc * 60 + part.trim().parse::<u64>().unwrap_or(0)
    })
}
