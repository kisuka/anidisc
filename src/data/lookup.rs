//! Looks discs up by content hash in one of three places: a published site
//! (https://.../api/hash/HASH.json), a built site directory (api/hash/HASH.json), or a checkout
//! of the data repository (data/**.json, indexed in memory).

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use super::repo::DataRepository;
use super::site::{self, HashRecord};
use crate::http::{Fetched, Http};

pub const ENVIRONMENT_VARIABLE: &str = "ANIDISC_DB";

/// The published database, used when neither --db nor ANIDISC_DB is given.
pub const DEFAULT_DB: &str = "https://anidisc.com/";

enum Source {
    Url(String, Http),
    Api(PathBuf),
    Local(HashMap<String, HashRecord>),
}

pub struct HashLookup {
    pub description: String,
    source: Source,
    cache: HashMap<String, Option<HashRecord>>,
}

impl HashLookup {
    pub fn create(location: Option<&str>) -> Result<HashLookup, String> {
        let location = location
            .map(str::to_string)
            .or_else(|| std::env::var(ENVIRONMENT_VARIABLE).ok())
            .filter(|l| !l.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_DB.to_string());

        let lower = location.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            let base = if location.ends_with('/') {
                location.clone()
            } else {
                format!("{location}/")
            };
            return Ok(HashLookup::new(location, Source::Url(base, Http::new())));
        }

        let path = std::path::absolute(&location).unwrap_or_else(|_| PathBuf::from(&location));
        let api = path.join("api").join("hash");
        if api.is_dir() {
            return Ok(HashLookup::new(
                path.display().to_string(),
                Source::Api(api),
            ));
        }

        let data = if path.join("data").is_dir() {
            path.join("data")
        } else {
            path.clone()
        };
        if data.is_dir() {
            let repo = DataRepository::load(&data);
            if let Some(first) = repo.errors.first() {
                return Err(format!(
                    "The data in {} has {} error(s); run `anidisc validate` first. First: {first}",
                    data.display(),
                    repo.errors.len()
                ));
            }
            let records = site::hash_records(&repo)
                .into_iter()
                .map(|r| (r.content_hash.clone(), r))
                .collect();
            return Ok(HashLookup::new(
                data.display().to_string(),
                Source::Local(records),
            ));
        }

        Err(format!("Database not found: {location}"))
    }

    fn new(description: String, source: Source) -> HashLookup {
        HashLookup {
            description,
            source,
            cache: HashMap::new(),
        }
    }

    pub fn find(&mut self, hash: &str) -> Result<Option<HashRecord>, String> {
        if let Some(cached) = self.cache.get(hash) {
            return Ok(cached.clone());
        }

        let record = match &self.source {
            Source::Local(records) => records.get(hash).cloned(),
            Source::Api(dir) => match fs::read_to_string(dir.join(format!("{hash}.json"))) {
                Ok(text) => Some(parse(&text, hash)?),
                Err(_) => None,
            },
            Source::Url(base, http) => match http.get(&format!("{base}api/hash/{hash}.json"))? {
                Fetched::Ok(text) => Some(parse(&text, hash)?),
                Fetched::NotFound => None,
                Fetched::Status(code) => {
                    return Err(format!("{base} answered {code} when looking up {hash}"));
                }
            },
        };

        self.cache.insert(hash.to_string(), record.clone());
        Ok(record)
    }
}

fn parse(text: &str, hash: &str) -> Result<HashRecord, String> {
    serde_json::from_str(text).map_err(|e| format!("the record for {hash} could not be read: {e}"))
}
