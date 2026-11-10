//! AniDB anime records (titles, start date, episodes), in AniDB's HTTP API XML format. Looked up,
//! in order:
//!   1. the local cache (<local app data>/anidisc/anidb/<aid>.xml), which anidisc fills itself;
//!   2. folders listed in ANIDISC_ANIDB_XML (';'-separated), holding <aid>.xml files;
//!   3. unless offline, AniDB's own HTTP API when the user gives their registered AniDB client
//!      (--anidb-client / ANIDISC_ANIDB_CLIENT), otherwise Kometa's AniDB cache service.
//!
//! Each record fetched is cached and never fetched again.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::episode::EpisodeNumber;
use super::models::Episode;
use crate::http::{Fetched, Http};

pub const ENVIRONMENT_VARIABLE: &str = "ANIDISC_ANIDB_XML";
pub const CLIENT_VARIABLE: &str = "ANIDISC_ANIDB_CLIENT";
const KOMETA_URL: &str = "https://utilities.kometa.wiki/anidb-service/anime/{}?mature=true";
const ANIDB_URL: &str = "http://api.anidb.net:9001/httpapi?request=anime&protover=1";
/// AniDB allows one request every 2 seconds; anidisc waits twice that, across runs too.
const ANIDB_GAP: Duration = Duration::from_secs(4);
/// After AniDB reports a ban, it isn't asked again for this long, even by later runs.
const BAN_PAUSE_HOURS: u128 = 24;

/// A client registered on AniDB (anidb.net → Settings → API clients), given as "name/version".
pub struct Client {
    name: String,
    version: u32,
}

impl Client {
    /// From --anidb-client, else ANIDISC_ANIDB_CLIENT; None when neither is set.
    pub fn from_option(value: Option<&str>) -> Result<Option<Client>, String> {
        let Some(value) = value
            .map(str::to_string)
            .or_else(|| std::env::var(CLIENT_VARIABLE).ok())
            .filter(|v| !v.trim().is_empty())
        else {
            return Ok(None);
        };
        let bad = || {
            format!(
                "AniDB client \"{value}\" should be <name>/<version>, as registered on AniDB, e.g. myclient/1"
            )
        };
        let (name, version) = value.trim().split_once('/').ok_or_else(bad)?;
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(bad());
        }
        let version = version.trim().parse().map_err(|_| bad())?;
        Ok(Some(Client { name, version }))
    }
}

pub struct Anime {
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub episodes: Vec<Episode>,
}

pub struct AnidbRecords {
    offline: bool,
    cache: PathBuf,
    extra: Vec<PathBuf>,
    http: Option<Http>,
    client: Option<Client>,
    /// Set when AniDB refused a request; no more requests are made this run.
    stopped: Option<String>,
    last_request: Option<Instant>,
}

impl AnidbRecords {
    pub fn new(offline: bool, client: Option<Client>) -> AnidbRecords {
        let extra = std::env::var(ENVIRONMENT_VARIABLE)
            .unwrap_or_default()
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect();
        AnidbRecords {
            offline,
            cache: crate::local_data_dir().join("anidb"),
            extra,
            http: None,
            client,
            stopped: None,
            last_request: None,
        }
    }

    /// The record, or the reason it can't be had.
    pub fn get(&mut self, aid: i32) -> Result<Anime, String> {
        let xml = match self.read_local(aid) {
            Some(xml) => xml,
            None => {
                if self.offline {
                    return Err(format!(
                        "AniDB {aid} is not cached locally and --offline is set"
                    ));
                }
                let xml = if self.client.is_some() {
                    self.fetch_anidb(aid)?
                } else {
                    self.fetch_kometa(aid)?
                };
                let _ = fs::create_dir_all(&self.cache);
                let _ = fs::write(self.cache.join(format!("{aid}.xml")), &xml);
                xml
            }
        };

        parse(&xml).ok_or_else(|| format!("the cached record for AniDB {aid} could not be read"))
    }

    fn fetch_kometa(&mut self, aid: i32) -> Result<String, String> {
        eprintln!("Fetching AniDB {aid} from Kometa's AniDB cache...");
        let http = self.http.get_or_insert_with(Http::new);
        let xml = match http.get(&KOMETA_URL.replace("{}", &aid.to_string())) {
            Ok(Fetched::Ok(text)) => text,
            Ok(Fetched::NotFound) => return Err(format!("AniDB {aid} was not found")),
            Ok(Fetched::Status(code)) => {
                return Err(format!(
                    "Kometa's AniDB cache answered {code} for {aid} (it limits requests per day; try again later or use --offline)"
                ));
            }
            Err(e) => return Err(format!("could not reach Kometa's AniDB cache: {e}")),
        };
        if parse(&xml).is_none() {
            return Err(format!(
                "Kometa's AniDB cache returned something that is not an anime record for {aid}"
            ));
        }
        Ok(xml)
    }

    /// One request to AniDB's HTTP API, keeping to its flood rules: never two requests within
    /// ANIDB_GAP (also across runs, via a time stamp in the cache folder), and no further
    /// requests after AniDB answers with an error, which is how it reports bans.
    fn fetch_anidb(&mut self, aid: i32) -> Result<String, String> {
        if let Some(reason) = &self.stopped {
            return Err(reason.clone());
        }
        if let Some(hours) = self.banned_hours_ago()
            && hours < BAN_PAUSE_HOURS
        {
            let reason = format!(
                "AniDB banned this client {hours} hour(s) ago; anidisc won't ask AniDB again until {BAN_PAUSE_HOURS} hours have passed (delete {} to override)",
                self.ban_file().display()
            );
            self.stopped = Some(reason.clone());
            return Err(reason);
        }
        let client = self.client.as_ref().expect("checked by the caller");
        // Debug builds can point at a mock server, so tests never touch AniDB.
        let base = if cfg!(debug_assertions) {
            std::env::var("ANIDISC_TEST_ANIDB_URL").unwrap_or_else(|_| ANIDB_URL.to_string())
        } else {
            ANIDB_URL.to_string()
        };
        let url = format!(
            "{base}&client={}&clientver={}&aid={aid}",
            client.name, client.version
        );

        self.wait_for_gap();
        eprintln!("Fetching AniDB {aid} from AniDB...");
        let http = self.http.get_or_insert_with(Http::new);
        let result = http.get(&url);
        self.last_request = Some(Instant::now());
        let _ = fs::create_dir_all(&self.cache);
        let _ = fs::write(self.stamp_file(), unix_millis().to_string());

        let xml = match result {
            Ok(Fetched::Ok(text)) => text,
            Ok(Fetched::NotFound) => return Err(format!("AniDB {aid} was not found")),
            Ok(Fetched::Status(code)) => {
                return Err(self.stop(format!("AniDB answered HTTP {code}")));
            }
            Err(e) => return Err(format!("could not reach AniDB: {e}")),
        };
        let body = xml.trim_start();
        if body.starts_with("<error") {
            let message = roxmltree::Document::parse(body)
                .ok()
                .and_then(|d| d.root_element().text().map(|t| t.trim().to_string()))
                .unwrap_or_else(|| body.chars().take(200).collect());
            if message.eq_ignore_ascii_case("anime not found") {
                return Err(format!("AniDB {aid} was not found"));
            }
            return Err(self.stop(format!("AniDB refused the request: {message}")));
        }
        if parse(&xml).is_none() {
            return Err(self.stop(format!(
                "AniDB returned something that is not an anime record for {aid}"
            )));
        }
        Ok(xml)
    }

    /// Stops all further AniDB requests this run; a ban warning when AniDB said so.
    fn stop(&mut self, reason: String) -> String {
        let reason = if reason.to_ascii_lowercase().contains("banned") {
            let _ = fs::write(self.ban_file(), unix_millis().to_string());
            format!(
                "{reason}. AniDB has temporarily banned this address or client; wait at least a day before trying again"
            )
        } else {
            format!("{reason}; no more AniDB requests this run")
        };
        self.stopped = Some(reason.clone());
        reason
    }

    fn stamp_file(&self) -> PathBuf {
        self.cache.join(".last-anidb-request")
    }

    fn ban_file(&self) -> PathBuf {
        self.cache.join(".anidb-banned")
    }

    fn banned_hours_ago(&self) -> Option<u128> {
        fs::read_to_string(self.ban_file())
            .ok()
            .and_then(|s| s.trim().parse::<u128>().ok())
            .map(|then| unix_millis().saturating_sub(then) / 3_600_000)
    }

    fn wait_for_gap(&self) {
        let since_this_run = self.last_request.map(|t| t.elapsed());
        let since_last_run = fs::read_to_string(self.stamp_file())
            .ok()
            .and_then(|s| s.trim().parse::<u128>().ok())
            .map(|then| Duration::from_millis(unix_millis().saturating_sub(then) as u64));
        let since = match (since_this_run, since_last_run) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if let Some(wait) = since.and_then(|s| ANIDB_GAP.checked_sub(s))
            && !wait.is_zero()
        {
            std::thread::sleep(wait);
        }
    }

    fn read_local(&self, aid: i32) -> Option<String> {
        std::iter::once(&self.cache)
            .chain(self.extra.iter())
            .map(|dir| dir.join(format!("{aid}.xml")))
            .find_map(|file| fs::read_to_string(file).ok())
    }
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

pub fn parse(xml: &str) -> Option<Anime> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    if root.tag_name().name() != "anime" {
        return None;
    }

    fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
        node.children().find(|c| c.has_tag_name(name))
    }
    let lang =
        |n: &roxmltree::Node<'_, '_>| n.attribute((XML_NS, "lang")).unwrap_or("").to_string();
    let text = |n: &roxmltree::Node<'_, '_>| n.text().unwrap_or("").trim().to_string();

    let titles: Vec<_> = child(root, "titles")
        .map(|t| t.children().filter(|c| c.has_tag_name("title")).collect())
        .unwrap_or_default();
    let main = titles
        .iter()
        .find(|t| t.attribute("type") == Some("main"))
        .map(text)
        .filter(|t| !t.is_empty())?;
    let original = titles
        .iter()
        .filter(|t| lang(t) == "ja" && t.attribute("type") == Some("official"))
        .map(text)
        .find(|t| !t.is_empty());

    let start = child(root, "startdate")
        .map(|n| text(&n))
        .unwrap_or_default();
    let year = start
        .get(..4)
        .filter(|y| y.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|y| y.parse().ok());

    // Every kind AniDB lists: regular (1), specials (S1), credits (C1), trailers (T1), parodies (P1), other (O1).
    let mut episodes: Vec<Episode> = Vec::new();
    if let Some(list) = child(root, "episodes") {
        for e in list.children().filter(|c| c.has_tag_name("episode")) {
            let Some(number) =
                child(e, "epno").and_then(|n| EpisodeNumber::parse(n.text().unwrap_or("")))
            else {
                continue;
            };
            if episodes.iter().any(|x| x.number == number) {
                continue;
            }
            let ep_titles: Vec<_> = e.children().filter(|c| c.has_tag_name("title")).collect();
            let in_lang = |l: &str| {
                ep_titles
                    .iter()
                    .filter(|t| lang(t) == l)
                    .map(text)
                    .find(|t| !t.is_empty())
            };
            let (english, romaji, original) = (in_lang("en"), in_lang("x-jat"), in_lang("ja"));
            let title = english
                .or_else(|| romaji.clone())
                .or_else(|| original.clone());
            // Not repeated when one is the title itself.
            let differs = |t: Option<String>| t.filter(|t| Some(t) != title.as_ref());
            episodes.push(Episode {
                number,
                romaji_title: differs(romaji),
                original_title: differs(original),
                title,
                anidb: None,
            });
        }
    }
    episodes.sort_by_key(|a| a.number);

    Some(Anime {
        title: main,
        original_title: original,
        year,
        episodes,
    })
}
