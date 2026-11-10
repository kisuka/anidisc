use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::data::json::{self, Kind};
use crate::data::lookup::HashLookup;
use crate::disc::{listing, scanner};
use crate::{ExitCode, naming};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OrganizeLog {
    created: String,
    mode: String,
    entries: Vec<LogEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LogEntry {
    from: String,
    to: String,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Status {
    Move,
    InPlace,
    Altered,
    Unknown,
    Conflict,
    Error,
}

struct Plan {
    source: PathBuf,
    target: Option<PathBuf>,
    status: Status,
    detail: String,
}

pub struct Options<'a> {
    pub inputs: &'a [String],
    pub destination: &'a str,
    pub recursive: bool,
    pub db: Option<&'a str>,
    pub template: &'a str,
    pub apply: bool,
    pub copy: bool,
    pub include_altered: bool,
    pub log: Option<&'a str>,
}

/// Paths compare case-insensitively on Windows and macOS.
fn key(path: &Path) -> String {
    let text = path.to_string_lossy().into_owned();
    if cfg!(any(windows, target_os = "macos")) {
        text.to_lowercase()
    } else {
        text
    }
}

pub fn run(o: Options) -> ExitCode {
    let mut lookup = match HashLookup::create(o.db) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::BadArguments;
        }
    };

    let dest = std::path::absolute(o.destination).unwrap_or_else(|_| PathBuf::from(o.destination));
    let discs = scanner::find(o.inputs, o.recursive);
    if discs.is_empty() {
        eprintln!("No disc images or disc folders found.");
        return ExitCode::NothingFound;
    }

    let mut plans = Vec::new();
    let mut claimed = HashSet::new();
    for (i, path) in discs.iter().enumerate() {
        eprint!("\rScanning {}/{}...", i + 1, discs.len());
        let scan = scanner::scan(path);
        let plan = |target: Option<PathBuf>, status, detail: String| Plan {
            source: path.clone(),
            target,
            status,
            detail,
        };
        let Some(hash) = &scan.content_hash else {
            plans.push(plan(
                None,
                Status::Error,
                scan.error
                    .clone()
                    .unwrap_or_else(|| "could not be read".into()),
            ));
            continue;
        };

        let found = match lookup.find(hash) {
            Ok(r) => r,
            Err(e) => {
                eprintln!();
                eprintln!("{e}");
                return ExitCode::Failure;
            }
        };
        let Some(m) = found else {
            plans.push(plan(None, Status::Unknown, hash.clone()));
            continue;
        };

        if let Some(reference) = m
            .disc
            .disc
            .files
            .as_ref()
            .filter(|f| !f.is_empty() && !o.include_altered)
        {
            let diff = listing::compare(reference, &scan.files);
            if !diff.is_clean() {
                let lines = diff
                    .lines()
                    .into_iter()
                    .take(10)
                    .collect::<Vec<_>>()
                    .join("\n      ");
                let label = m
                    .disc
                    .disc
                    .label
                    .clone()
                    .unwrap_or_else(|| m.disc.slug.clone());
                plans.push(plan(
                    None,
                    Status::Altered,
                    format!(
                        "{} / {label}: {}\n      {lines}",
                        m.series.title,
                        diff.summary()
                    ),
                ));
                continue;
            }
        }

        let mut target = dest.clone();
        for part in naming::render(o.template, &m).split('/') {
            target.push(part);
        }
        if !scan.is_folder
            && let Some(ext) = path.extension()
        {
            let name = format!(
                "{}.{}",
                target.file_name().unwrap_or_default().to_string_lossy(),
                ext.to_string_lossy().to_lowercase()
            );
            target.set_file_name(name);
        }

        let same = key(path) == key(&target);
        if same && path.file_name() == target.file_name() {
            claimed.insert(key(&target));
            plans.push(plan(Some(target), Status::InPlace, String::new()));
        } else if !claimed.insert(key(&target)) {
            plans.push(plan(
                Some(target),
                Status::Conflict,
                "another disc in this run has the same target (duplicate copy?)".into(),
            ));
        } else if target.exists() && !same {
            plans.push(plan(
                Some(target),
                Status::Conflict,
                "target already exists".into(),
            ));
        } else {
            plans.push(plan(Some(target), Status::Move, String::new()));
        }
    }
    eprintln!();

    print_plan(&plans, o.copy);
    let moves: Vec<&Plan> = plans.iter().filter(|p| p.status == Status::Move).collect();
    let verb = if o.copy { "copy" } else { "move" };
    if !o.apply {
        if moves.is_empty() {
            eprintln!("Dry run: nothing to do.");
        } else {
            eprintln!(
                "Dry run: nothing was changed. Re-run with --apply to {verb} {} disc(s).",
                moves.len()
            );
        }
        return ExitCode::Success;
    }

    if moves.is_empty() {
        eprintln!("Nothing to do.");
        return ExitCode::Success;
    }

    let log_path = o.log.map(PathBuf::from).unwrap_or_else(|| {
        crate::local_data_dir()
            .join("logs")
            .join(format!("organize-{}.json", crate::time::utc_now_stamp()))
    });
    let mut log = OrganizeLog {
        created: crate::time::utc_now_iso(),
        mode: verb.to_string(),
        entries: Vec::new(),
    };
    let done = if o.copy { "copied" } else { "moved" };
    let mut failed = 0;
    for plan in moves {
        let target = plan.target.as_ref().expect("move has a target");
        let result = target
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|_| transfer(&plan.source, target, o.copy))
            .and_then(|_| {
                log.entries.push(LogEntry {
                    from: plan.source.display().to_string(),
                    to: target.display().to_string(),
                });
                json::write_file(&log_path, &log) // after every step, so an interrupted run can still be undone
            });
        match result {
            Ok(()) => eprintln!("{done}  {}", target.display()),
            Err(e) => {
                failed += 1;
                eprintln!("FAILED  {}: {e}", plan.source.display());
            }
        }
    }

    eprintln!(
        "{} {done}, {failed} failed. Log: {}",
        log.entries.len(),
        log_path.display()
    );
    if !o.copy && !log.entries.is_empty() {
        eprintln!("Undo with: anidisc undo \"{}\"", log_path.display());
    }

    if failed > 0 {
        ExitCode::Failure
    } else {
        ExitCode::Success
    }
}

pub fn undo(log_path: &str) -> ExitCode {
    let log = fs::read_to_string(log_path)
        .map_err(|e| e.to_string())
        .and_then(|t| json::parse_lenient::<OrganizeLog>(&t, Kind::Any));
    let log = match log {
        Ok(l) if l.mode == "move" => l,
        Ok(_) => {
            eprintln!(
                "Only move logs can be undone. For a copy, delete the copies listed in the log yourself."
            );
            return ExitCode::BadArguments;
        }
        Err(e) => {
            eprintln!("{log_path}: {e}");
            return ExitCode::BadArguments;
        }
    };

    let (mut restored, mut skipped) = (0, 0);
    for entry in log.entries.iter().rev() {
        let (from, to) = (Path::new(&entry.from), Path::new(&entry.to));
        let present = to.exists();
        if !present || from.exists() {
            skipped += 1;
            eprintln!(
                "skipped  {}: {}",
                entry.to,
                if present {
                    "original location is taken"
                } else {
                    "no longer there"
                }
            );
            continue;
        }

        let result = from
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|_| transfer(to, from, false));
        match result {
            Ok(()) => {
                restored += 1;
                eprintln!("restored {}", entry.from);
            }
            Err(e) => {
                skipped += 1;
                eprintln!("skipped  {}: {e}", entry.to);
            }
        }
    }

    eprintln!(
        "{restored} restored, {skipped} skipped. Folders created by organize are left in place."
    );
    if skipped > 0 {
        ExitCode::Failure
    } else {
        ExitCode::Success
    }
}

fn print_plan(plans: &[Plan], copy: bool) {
    let mut statuses: Vec<Status> = plans.iter().map(|p| p.status).collect();
    statuses.sort();
    statuses.dedup();
    for status in statuses {
        let group: Vec<&Plan> = plans.iter().filter(|p| p.status == status).collect();
        let heading = match status {
            Status::Move => {
                if copy {
                    "Copy"
                } else {
                    "Move"
                }
            }
            Status::InPlace => "Already in place",
            Status::Altered => {
                "Altered copies: files differ from the reference disc (left alone; --include-altered to move them)"
            }
            Status::Unknown => "Not in the database (left alone)",
            Status::Conflict => "Conflicts (left alone)",
            Status::Error => "Could not be read (left alone)",
        };
        println!("{heading}: {}", group.len());
        for p in group {
            println!("  {}", p.source.display());
            if matches!(p.status, Status::Move | Status::Conflict)
                && let Some(t) = &p.target
            {
                println!("    -> {}", t.display());
            }
            if !p.detail.is_empty() {
                println!("    {}", p.detail);
            }
        }
    }
}

/// Moves (or copies) a file or folder; a move across volumes copies and then deletes.
fn transfer(source: &Path, target: &Path, copy: bool) -> io::Result<()> {
    // A rename that only changes letter case targets the source itself on case-insensitive systems.
    if target.exists() && key(source) != key(target) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", target.display()),
        ));
    }

    if source.is_file() {
        if copy {
            fs::copy(source, target)?;
            return Ok(());
        }
        return match fs::rename(source, target) {
            Err(e) if is_cross_device(&e) => {
                fs::copy(source, target)?;
                fs::remove_file(source)
            }
            other => other,
        };
    }

    if !copy {
        match fs::rename(source, target) {
            Err(e) if is_cross_device(&e) => {}
            other => return other,
        }
    }

    copy_dir(source, target)?;
    if copy {
        Ok(())
    } else {
        fs::remove_dir_all(source)
    }
}

fn is_cross_device(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::CrossesDevices || (cfg!(windows) && e.raw_os_error() == Some(17))
}

fn copy_dir(source: &Path, target: &Path) -> io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let to = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            if to.exists() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{} already exists", to.display()),
                ));
            }
            fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}
