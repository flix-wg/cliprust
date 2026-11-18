//! Clipboard history core: capture, dedup, compressed storage and FTS5 search.

pub mod capture;
pub mod contacts;
#[cfg(feature = "ffi")]
pub mod ffi;
#[cfg(feature = "pastenow-import")]
pub mod import_pastenow;
pub mod model;
pub mod monitor;
pub mod platform;
pub mod query;
pub mod rules;
mod schema;
pub mod secrets;
pub mod store;

pub use contacts::Contact;
pub use model::{
    AppInfo, Capture, Item, ItemSummary, Kind, Rep, Secret, SecretKind, Settings, format,
};
pub use rules::{Rule, SortType};
pub use store::{
    AppCount, Facet, Facets, Find, IDS_LIST, Ingest, KEYS_LIST, PINNED_LIST, PasteList,
    SecretFilter, Stats, Store,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("clipboard: {0}")]
    Clipboard(String),
    #[error("{0}")]
    Refused(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The database shared by the app and the CLI.
pub fn default_db_path() -> Option<std::path::PathBuf> {
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    let base = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map_or_else(|| home.join(".local/share"), std::path::PathBuf::from)
    };
    Some(base.join("cliprust/cliprust.db"))
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}
