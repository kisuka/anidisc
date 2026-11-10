//! An AniDB episode number: 1 for a regular episode, or a prefixed one for the other kinds AniDB
//! lists: S1 special, C1 credits (opening/ending), T1 trailer/promo, P1 parody, O1 other.
//! In JSON a regular episode is a plain number (1) and the others are strings ("S1").

use std::cmp::Ordering;
use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// AniDB's own listing order: regular, specials, credits, trailers, parodies, other.
const KIND_ORDER: &str = "SCTPO";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EpisodeNumber {
    /// None for a regular episode, otherwise S, C, T, P or O.
    pub kind: Option<char>,
    pub number: u32,
}

impl EpisodeNumber {
    pub fn regular(number: u32) -> Self {
        EpisodeNumber { kind: None, number }
    }

    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (kind, digits) = match text.chars().next()? {
            c if c.is_ascii_digit() => (None, text),
            c if "SCTPOscpto".contains(c) => (Some(c.to_ascii_uppercase()), &text[1..]),
            _ => return None,
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let number: u32 = digits.parse().ok()?;
        (number > 0 && number <= i32::MAX as u32).then_some(EpisodeNumber { kind, number })
    }

    /// "episode 2", "special S1", "trailer T3"...
    pub fn describe(&self) -> String {
        match self.kind {
            None => format!("episode {}", self.number),
            Some('S') => format!("special {self}"),
            Some('C') => format!("credits {self}"),
            Some('T') => format!("trailer {self}"),
            Some('P') => format!("parody {self}"),
            Some(_) => format!("other {self}"),
        }
    }

    fn rank(&self) -> i32 {
        self.kind
            .map_or(-1, |k| KIND_ORDER.find(k).map_or(99, |i| i as i32))
    }
}

impl fmt::Display for EpisodeNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            Some(k) => write!(f, "{k}{}", self.number),
            None => write!(f, "{}", self.number),
        }
    }
}

impl Ord for EpisodeNumber {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank()
            .cmp(&other.rank())
            .then(self.number.cmp(&other.number))
    }
}

impl PartialOrd for EpisodeNumber {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Serialize for EpisodeNumber {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.kind {
            None => s.serialize_u32(self.number),
            Some(_) => s.serialize_str(&self.to_string()),
        }
    }
}

const EXPECTED: &str = "an episode number is a positive number (1) or an AniDB number such as \"S1\", \"C1\", \"T1\", \"P1\", \"O1\"";

impl<'de> Deserialize<'de> for EpisodeNumber {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = EpisodeNumber;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(EXPECTED)
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<EpisodeNumber, E> {
                if v > 0 && v <= i32::MAX as u64 {
                    Ok(EpisodeNumber::regular(v as u32))
                } else {
                    Err(E::custom(EXPECTED))
                }
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<EpisodeNumber, E> {
                if v > 0 {
                    self.visit_u64(v as u64)
                } else {
                    Err(E::custom(EXPECTED))
                }
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<EpisodeNumber, E> {
                EpisodeNumber::parse(v).ok_or_else(|| E::custom(EXPECTED))
            }
        }
        d.deserialize_any(V)
    }
}
