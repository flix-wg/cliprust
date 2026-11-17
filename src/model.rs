use std::fmt;

use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::Xxh3;

use crate::capture::TEXT_TYPE;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Text,
    Url,
    Image,
    File,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Url => "url",
            Kind::Image => "image",
            Kind::File => "file",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "text" => Kind::Text,
            "url" => Kind::Url,
            "image" => Kind::Image,
            "file" => Kind::File,
            _ => return None,
        })
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// Portable names for the representations clipboard-rs reads and writes with
/// typed APIs. Any other `Rep::format` is a raw platform type (UTI or MIME).
pub mod format {
    pub const TEXT: &str = "text";
    pub const RTF: &str = "rtf";
    pub const HTML: &str = "html";
    pub const FILES: &str = "files";
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rep {
    pub format: String,
    pub data: Vec<u8>,
}

impl Rep {
    pub fn new(format: &str, data: Vec<u8>) -> Self {
        Self {
            format: format.to_owned(),
            data,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub bundle_id: String,
    pub name: Option<String>,
    pub path: Option<String>,
}

/// One clipboard change, ready to store. `reps[0]` is the primary
/// representation and the only one that feeds the dedup checksum.
#[derive(Debug, Clone)]
pub struct Capture {
    pub kind: Kind,
    pub base_uniform_type: String,
    pub reps: Vec<Rep>,
    pub app: Option<AppInfo>,
    /// Page the content was copied from, when the browser or a lookup says.
    pub source_url: Option<String>,
    pub source_title: Option<String>,
    pub secret: Option<Secret>,
}

/// A detected key, or an identifier copied from a web page. Only `masked`
/// reaches the search index; for an ID it is the plain text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Secret {
    pub kind: SecretKind,
    pub provider: String,
    pub name: String,
    pub masked: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretKind {
    Key,
    Id,
}

impl SecretKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SecretKind::Key => "key",
            SecretKind::Id => "id",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "key" => Some(SecretKind::Key),
            "id" => Some(SecretKind::Id),
            _ => None,
        }
    }
}

impl Capture {
    pub fn from_text(text: String) -> Self {
        let kind = if looks_like_url(&text) {
            Kind::Url
        } else {
            Kind::Text
        };
        Self {
            kind,
            reps: vec![Rep::new(format::TEXT, text.into_bytes())],
            base_uniform_type: TEXT_TYPE.to_owned(),
            app: None,
            source_url: None,
            source_title: None,
            secret: None,
        }
    }

    /// The searchable text, read from the representations so it is held once.
    pub fn text(&self) -> Option<&str> {
        text_of(&self.reps)
    }

    pub fn checksum(&self) -> u64 {
        let mut hasher = Xxh3::new();
        hasher.update(self.kind.as_str().as_bytes());
        hasher.update(&[0]);
        if let Some(rep) = self.reps.first() {
            hasher.update(&rep.data);
        }
        hasher.digest()
    }
}

pub(crate) fn text_of(reps: &[Rep]) -> Option<&str> {
    reps.iter()
        .find(|r| [format::TEXT, format::FILES, format::HTML].contains(&r.format.as_str()))
        .and_then(|r| std::str::from_utf8(&r.data).ok())
}

fn looks_like_url(text: &str) -> bool {
    let t = text.trim();
    t.len() <= 2048
        && !t.contains(char::is_whitespace)
        && t.split_once("://").is_some_and(|(scheme, rest)| {
            !rest.is_empty()
                && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemSummary {
    pub id: i64,
    pub kind: Kind,
    pub title: Option<String>,
    /// First 200 characters of the indexed text.
    pub preview: Option<String>,
    pub app_bundle_id: Option<String>,
    pub source_url: Option<String>,
    /// Registrable domain of `source_url`, e.g. `cloudflare.com`.
    pub source_domain: Option<String>,
    pub secret_kind: Option<SecretKind>,
    pub secret_provider: Option<String>,
    pub note: Option<String>,
    pub created_at: i64,
    pub last_used_at: i64,
    pub byte_size: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub summary: ItemSummary,
    pub reps: Vec<Rep>,
}

impl Item {
    pub fn text(&self) -> Option<&str> {
        text_of(&self.reps)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub paused: bool,
    /// Recognise API keys and tokens and file them in the Keys list.
    pub detect_keys: bool,
    /// Retention limits. Items in manual lists (including Keys) are kept.
    pub max_items: Option<u32>,
    pub max_age_days: Option<u32>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            paused: false,
            detect_keys: false,
            max_items: Some(5000),
            max_age_days: None,
        }
    }
}
