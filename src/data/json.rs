//! Reading and writing the data files.
//!
//! Data files are read strictly (unknown properties are errors, so typos in contributed files are
//! caught) but leniently in syntax: comments and trailing commas are allowed. Files are written
//! indented by two spaces, with non-ASCII text kept as is.

use std::fs;
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::models::keys;

/// Which kind of data file, for the unknown-property check.
#[derive(Clone, Copy)]
pub enum Kind {
    Series,
    Release,
    Disc,
    /// Any shape (organize logs, sidecars): no property check.
    Any,
}

/// Parses JSON that may contain // and /* */ comments and trailing commas.
pub fn parse_lenient<T: DeserializeOwned>(text: &str, kind: Kind) -> Result<T, String> {
    let value: Value = serde_json::from_str(&strip(text)).map_err(|e| e.to_string())?;
    check_keys(&value, kind)?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}

pub fn write_file<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    text.push('\n');
    fs::write(path, text)
}

pub fn to_compact<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("serializable")
}

fn check_keys(value: &Value, kind: Kind) -> Result<(), String> {
    fn object(v: &Value, allowed: &[&str], what: &str) -> Result<(), String> {
        if let Value::Object(map) = v
            && let Some(key) = map.keys().find(|k| !allowed.contains(&k.as_str()))
        {
            return Err(format!(
                "unknown property \"{key}\" in {what} (allowed: {})",
                allowed.join(", ")
            ));
        }
        Ok(())
    }

    fn each(v: &Value, field: &str, allowed: &[&str], what: &str) -> Result<(), String> {
        if let Some(Value::Array(items)) = v.get(field) {
            for item in items {
                object(item, allowed, what)?;
            }
        }
        Ok(())
    }

    match kind {
        Kind::Series => {
            object(value, keys::SERIES, "the series")?;
            each(value, "episodes", keys::EPISODE, "an episode")
        }
        Kind::Release => object(value, keys::RELEASE, "the release"),
        Kind::Disc => {
            object(value, keys::DISC, "the disc")?;
            each(value, "titles", keys::TITLE, "a title")?;
            each(value, "files", keys::FILE, "a file")
        }
        Kind::Any => Ok(()),
    }
}

/// Removes comments and trailing commas, leaving strings untouched.
fn strip(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => {
                out.push(c);
                i += 1;
                while i < chars.len() {
                    out.push(chars[i]);
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        out.push(chars[i + 1]);
                        i += 2;
                        continue;
                    }
                    i += 1;
                    if chars[i - 1] == '"' {
                        break;
                    }
                }
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
                out.push(' ');
            }
            ',' => {
                // Drop a comma followed (after whitespace/comments) by a closing bracket.
                let mut j = i + 1;
                loop {
                    while j < chars.len() && chars[j].is_whitespace() {
                        j += 1;
                    }
                    if chars.get(j) == Some(&'/') && chars.get(j + 1) == Some(&'/') {
                        while j < chars.len() && chars[j] != '\n' {
                            j += 1;
                        }
                    } else if chars.get(j) == Some(&'/') && chars.get(j + 1) == Some(&'*') {
                        j += 2;
                        while j < chars.len()
                            && !(chars[j] == '*' && chars.get(j + 1) == Some(&'/'))
                        {
                            j += 1;
                        }
                        j += 2;
                    } else {
                        break;
                    }
                }
                if !matches!(chars.get(j), Some('}') | Some(']')) {
                    out.push(',');
                }
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn strips_comments_and_trailing_commas() {
        let text = "{ // c\n \"a\": \"x // y, ]\", /* b */ \"b\": [1, 2,], }";
        let v: serde_json::Value = serde_json::from_str(&super::strip(text)).unwrap();
        assert_eq!(v["a"], "x // y, ]");
        assert_eq!(v["b"], serde_json::json!([1, 2]));
    }
}
