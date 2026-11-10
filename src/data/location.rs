//! Finds the data folder of a repository checkout, so commands never create a stray "data"
//! folder wherever they happen to be run.

use std::path::{Path, PathBuf};

pub const ENVIRONMENT_VARIABLE: &str = "ANIDISC_DATA";

/// A checkout's root holds Cargo.toml (the anidisc crate) next to data/.
fn is_checkout(dir: &Path) -> bool {
    dir.join("data").is_dir()
        && std::fs::read_to_string(dir.join("Cargo.toml"))
            .is_ok_and(|t| t.contains("name = \"anidisc\""))
}

/// --data when given; otherwise the checkout containing the current folder; otherwise $ANIDISC_DATA.
pub fn resolve(given: Option<&str>) -> Result<PathBuf, String> {
    if let Some(given) = given.filter(|g| !g.trim().is_empty()) {
        return Ok(absolute(given));
    }

    if let Ok(cwd) = std::env::current_dir()
        && let Some(root) = cwd.ancestors().find(|d| is_checkout(d))
    {
        return Ok(root.join("data"));
    }

    if let Some(env) = std::env::var(ENVIRONMENT_VARIABLE)
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        return Ok(absolute(&env));
    }

    Err(format!(
        "Can't tell which data folder to use: this folder is not inside an anidiscdb checkout. \
         Run from inside one, pass --data <folder>, or set {ENVIRONMENT_VARIABLE} to the checkout's data folder."
    ))
}

/// The checkout root that holds the given data folder, if it is a checkout's data folder.
pub fn root_of(data: &Path) -> Option<PathBuf> {
    data.parent()
        .filter(|p| is_checkout(p))
        .map(Path::to_path_buf)
}

fn absolute(path: &str) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path))
}
