//! anidisc: identify anime DVD/Blu-ray discs by content hash and organize them.

mod commands;
mod data;
mod disc;
mod http;
mod naming;
mod time;

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use data::{location, repo::DataRepository, site};

/// The release version (set by CI from the tag), else the crate version.
pub const VERSION: &str = match option_env!("ANIDISC_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Clone, Copy)]
pub enum ExitCode {
    Success = 0,
    Failure = 1,
    BadArguments = 2,
    NothingFound = 3,
}

#[derive(Parser)]
#[command(
    name = "anidisc",
    version = VERSION,
    about = "anidisc: identify anime DVD/Blu-ray discs by content hash and organize them."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

const PATHS_HELP: &str =
    "Disc images (.iso), disc folders (containing VIDEO_TS or BDMV), or folders to search.";
const DB_HELP: &str = "Database: a site URL, a built site folder, or a data repository checkout. Defaults to $ANIDISC_DB, else https://anidisc.com/.";
const DATA_HELP: &str = "The data folder of a repository checkout. Defaults to the checkout containing the current folder, then $ANIDISC_DATA.";

#[derive(Subcommand)]
enum Command {
    /// Hash discs and look them up.
    Identify {
        #[arg(required = true, help = PATHS_HELP)]
        paths: Vec<String>,
        /// Search subfolders too.
        #[arg(short, long)]
        recursive: bool,
        #[arg(long, value_name = "url-or-folder", help = DB_HELP)]
        db: Option<String>,
        #[arg(short, long, default_value = naming::DEFAULT_TEMPLATE, help = naming::TEMPLATE_HELP)]
        template: String,
        /// Print results as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Move recognized discs into a folder structure. Dry run unless --apply.
    Organize {
        #[arg(required = true, help = PATHS_HELP)]
        paths: Vec<String>,
        /// Folder to organize into. May be the same folder the discs are in.
        #[arg(short, long, value_name = "folder")]
        dest: String,
        /// Search subfolders too.
        #[arg(short, long)]
        recursive: bool,
        #[arg(long, value_name = "url-or-folder", help = DB_HELP)]
        db: Option<String>,
        #[arg(short, long, default_value = naming::DEFAULT_TEMPLATE, help = naming::TEMPLATE_HELP)]
        template: String,
        /// Actually move the files. Without it, only the plan is printed.
        #[arg(long)]
        apply: bool,
        /// Copy instead of move.
        #[arg(long)]
        copy: bool,
        /// Also move discs whose files differ from the reference disc (junk added, files missing).
        #[arg(long)]
        include_altered: bool,
        /// Where to write the undo log.
        #[arg(long, value_name = "file")]
        log: Option<String>,
    },
    /// Reverse an `organize --apply` run from its log.
    Undo {
        /// An organize log file.
        log: String,
    },
    /// Write pre-filled data files for discs that are not in the database yet.
    Contribute {
        #[arg(required = true, help = PATHS_HELP)]
        paths: Vec<String>,
        /// Search subfolders too.
        #[arg(short, long)]
        recursive: bool,
        #[arg(long, value_name = "folder", help = DATA_HELP)]
        data: Option<String>,
        #[arg(long, value_name = "url-or-folder", help = DB_HELP)]
        db: Option<String>,
        /// AniDB anime id of every disc in this run. Fills in the series' titles and episodes.
        #[arg(long, value_name = "id")]
        anidb: Option<i32>,
        /// Do not check --db; write stubs for every disc not already in --data.
        #[arg(long)]
        all: bool,
        /// Don't fetch AniDB records; use only local ones (the cache and $ANIDISC_ANIDB_XML).
        #[arg(long)]
        offline: bool,
        /// Fetch AniDB records from AniDB itself with your own registered AniDB client, given as
        /// name/version (e.g. myclient/1), instead of from Kometa's cache. Defaults to $ANIDISC_ANIDB_CLIENT.
        #[arg(long, value_name = "name/version")]
        anidb_client: Option<String>,
        /// After scanning each new disc, ask for its region, label, catalog number and barcode, and
        /// what each title is, picking its episodes from a checklist.
        #[arg(short, long)]
        interactive: bool,
    },
    /// Check the data files.
    Validate {
        #[arg(long, value_name = "folder", help = DATA_HELP)]
        data: Option<String>,
    },
    /// Validate the data and build the static site and lookup API.
    Build {
        #[arg(long, value_name = "folder", help = DATA_HELP)]
        data: Option<String>,
        /// Static site source folder. Defaults to the checkout's site folder.
        #[arg(long, value_name = "folder")]
        site: Option<String>,
        /// Output folder. Defaults to the checkout's _site folder.
        #[arg(short, long, value_name = "folder")]
        out: Option<String>,
        /// Repository URL, linked from the site.
        #[arg(long, value_name = "url")]
        repo_url: Option<String>,
    },
}

fn main() {
    let code = match Cli::parse().command {
        Command::Identify {
            paths,
            recursive,
            db,
            template,
            json,
        } => commands::identify::run(&paths, recursive, db.as_deref(), &template, json),
        Command::Organize {
            paths,
            dest,
            recursive,
            db,
            template,
            apply,
            copy,
            include_altered,
            log,
        } => commands::organize::run(commands::organize::Options {
            inputs: &paths,
            destination: &dest,
            recursive,
            db: db.as_deref(),
            template: &template,
            apply,
            copy,
            include_altered,
            log: log.as_deref(),
        }),
        Command::Undo { log } => commands::organize::undo(&log),
        Command::Contribute {
            paths,
            recursive,
            data,
            db,
            anidb,
            all,
            offline,
            anidb_client,
            interactive,
        } => match (
            location::resolve(data.as_deref()),
            data::anidb::Client::from_option(anidb_client.as_deref()),
        ) {
            (Err(e), _) | (_, Err(e)) => fail(&e),
            (Ok(data), Ok(client)) => commands::contribute::run(commands::contribute::Options {
                inputs: &paths,
                recursive,
                data,
                db: db.as_deref(),
                anidb,
                all,
                offline,
                anidb_client: client,
                interactive,
            }),
        },
        Command::Validate { data } => match location::resolve(data.as_deref()) {
            Err(e) => fail(&e),
            Ok(data) => validate(&data).0,
        },
        Command::Build {
            data,
            site,
            out,
            repo_url,
        } => match location::resolve(data.as_deref()) {
            Err(e) => fail(&e),
            Ok(data_path) => build(&data_path, site, out, repo_url),
        },
    };
    std::process::exit(code as i32);
}

fn fail(message: &str) -> ExitCode {
    eprintln!("{message}");
    ExitCode::BadArguments
}

fn validate(data: &Path) -> (ExitCode, DataRepository) {
    eprintln!("Data: {}", data.display());
    let repo = DataRepository::load(data);
    for w in &repo.warnings {
        eprintln!("warning: {w}");
    }
    for e in &repo.errors {
        eprintln!("error: {e}");
    }
    eprintln!(
        "{} series, {} discs: {} error(s), {} warning(s).",
        repo.series.len(),
        repo.disc_count(),
        repo.errors.len(),
        repo.warnings.len()
    );
    let code = if repo.errors.is_empty() {
        ExitCode::Success
    } else {
        ExitCode::Failure
    };
    (code, repo)
}

fn build(
    data: &Path,
    site_dir: Option<String>,
    out: Option<String>,
    repo_url: Option<String>,
) -> ExitCode {
    let (code, repo) = validate(data);
    if !matches!(code, ExitCode::Success) {
        return code;
    }

    // Site source and output sit next to the data folder unless given.
    let root =
        location::root_of(data).unwrap_or_else(|| data.parent().unwrap_or(data).to_path_buf());
    let absolute = |p: String| std::path::absolute(&p).unwrap_or_else(|_| PathBuf::from(p));
    let site_dir = site_dir.map(absolute).unwrap_or_else(|| root.join("site"));
    let output = out.map(absolute).unwrap_or_else(|| root.join("_site"));
    match site::build(&repo, &site_dir, &output, repo_url) {
        Ok(discs) => {
            eprintln!(
                "Built {}: {} series, {discs} discs.",
                output.display(),
                repo.series.len()
            );
            ExitCode::Success
        }
        Err(e) => {
            eprintln!("could not build {}: {e}", output.display());
            ExitCode::Failure
        }
    }
}

/// Per-user application data: %LOCALAPPDATA%\anidisc on Windows, ~/Library/Application Support/anidisc
/// on macOS, $XDG_DATA_HOME/anidisc (or ~/.local/share/anidisc) elsewhere.
pub fn local_data_dir() -> PathBuf {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let home = || env("HOME").unwrap_or_else(|| PathBuf::from("."));
    let base = if cfg!(windows) {
        env("LOCALAPPDATA").unwrap_or_else(home)
    } else if cfg!(target_os = "macos") {
        home().join("Library").join("Application Support")
    } else {
        env("XDG_DATA_HOME").unwrap_or_else(|| home().join(".local").join("share"))
    };
    base.join("anidisc")
}
