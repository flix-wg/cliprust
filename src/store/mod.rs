//! SQLite storage: dedup on ingest, zstd payloads, FTS5 trigram search,
//! manual and smart lists, exclusions, settings and retention.
//!
//! - `items`: ingest, single items, edits, deletion and retention
//! - `reps`: the stored formats of an item (compression lives here)
//! - `search`: filtered listing, apps and contact counts
//! - `lists`: manual and smart lists
//! - `secrets`: detected keys and IDs
//! - `settings`: settings and excluded apps
//! - `keys`: sealing key items, the locked state, rotation and recovery
//! - `file`: plaintext or SQLCipher file, switching, replaced-file checks

mod file;
mod items;
mod keys;
mod lists;
mod reps;
mod search;
mod secrets;
mod settings;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{Connection, Row, params_from_iter, types::Value};

use crate::crypto::{self, KeySource};
use crate::model::{ItemSummary, Kind, SecretKind};
use crate::{Error, Result, contacts, schema};

pub use file::is_encrypted;
pub use items::Ingest;
pub use keys::{EncryptionStatus, KeyState};
pub use lists::{IDS_LIST, KEYS_LIST, PINNED_LIST, PasteList};
pub use search::{AppCount, Find};
pub use secrets::{Facet, Facets, SecretFilter};

const SUMMARY_SQL: &str = "SELECT p.id, p.kind, p.title, substr(p.text, 1, 200), a.bundle_id, \
     p.source_url, p.source_domain, p.secret_kind, p.secret_provider, p.note, \
     p.created_at, p.last_used_at, p.byte_size, p.contact \
     FROM paste_items p LEFT JOIN applications a ON a.id = p.app_id";
/// Merges the index so a replaced text's tokens are rewritten too.
const OPTIMIZE_FTS: &str = "INSERT INTO paste_items_fts (paste_items_fts) VALUES ('optimize')";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub items: i64,
    pub raw_bytes: i64,
    pub stored_bytes: i64,
    pub file_bytes: i64,
}

pub struct Store {
    conn: Connection,
    keys: keys::Keyring,
    path: Option<PathBuf>,
    /// `(dev, ino)` of the file when it was opened.
    file_id: Option<(u64, u64)>,
    /// The generation whose `db` key opened an encrypted file.
    db_gen: Option<u32>,
}

impl Drop for Store {
    fn drop(&mut self) {
        // A reader leaves the WAL to the writers.
        if self.conn.total_changes() > 0 {
            schema::checkpoint(&self.conn);
        }
    }
}

impl Store {
    /// Opens (creating if needed) the database, with the platform's key store.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with_keys(path, crypto::default_keys())
    }

    /// Opens with `keys` holding the master keys; `None` stores keys unsealed.
    pub fn open_with_keys(path: &Path, keys: Option<Arc<dyn KeySource>>) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let keys = keys::Keyring::new(keys);
        let (conn, db_gen, file_id) = {
            let _lock = file::OpenLock::shared(path)?;
            let (conn, db_gen) = file::connect(path, &keys)?;
            schema::configure(&conn)?;
            (conn, db_gen, file::id(path))
        };
        Self::init(conn, keys, Some(path.to_owned()), file_id, db_gen)
    }

    pub fn open_in_memory() -> Result<Self> {
        let keys = keys::Keyring::new(crypto::default_keys());
        let conn = Connection::open_in_memory()?;
        schema::configure(&conn)?;
        Self::init(conn, keys, None, None, None)
    }

    fn init(
        conn: Connection,
        keys: keys::Keyring,
        path: Option<PathBuf>,
        file_id: Option<(u64, u64)>,
        db_gen: Option<u32>,
    ) -> Result<Self> {
        schema::migrate(&conn)?;
        let mut store = Self {
            conn,
            keys,
            path,
            file_id,
            db_gen,
        };
        store.scan_keys_once()?;
        Ok(store)
    }

    /// Restores a key from a recovery code into `keys`, also for an encrypted
    /// file that no stored key opens. Returns the key's generation.
    pub fn restore_key_at(
        path: &Path,
        keys: Option<Arc<dyn KeySource>>,
        code: &str,
    ) -> Result<u32> {
        match Self::open_with_keys(path, keys.clone()) {
            Ok(mut store) => store.restore_key(code),
            Err(Error::Locked(_)) if is_encrypted(path)? => {
                let source = keys.ok_or_else(|| Error::Locked(keys::NO_KEY_STORE.into()))?;
                let (generation, master) = crypto::parse_recovery_code(code)
                    .ok_or_else(|| Error::Refused("that is not a ClipRust recovery code".into()))?;
                if !file::opens_with(path, &master)? {
                    return Err(Error::Refused(
                        "this recovery code does not open this database".into(),
                    ));
                }
                keys::add_or_confirm(source.as_ref(), generation, &master)?;
                Ok(generation)
            }
            Err(e) => Err(e),
        }
    }

    /// Changes whenever this or any other connection commits a change.
    pub fn data_version(&self) -> Result<(i64, u64)> {
        let others = self
            .conn
            .pragma_query_value(None, "data_version", |r| r.get(0))?;
        Ok((others, self.conn.total_changes()))
    }

    /// Folds the WAL back into the file as far as no reader or writer prevents it.
    pub fn checkpoint(&self) {
        schema::checkpoint_passive(&self.conn);
    }

    pub fn stats(&self) -> Result<Stats> {
        let (items, raw_bytes) = self.conn.query_row(
            "SELECT count(*), coalesce(sum(byte_size), 0) FROM paste_items",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let stored_bytes = self.conn.query_row(
            "SELECT coalesce(sum(length(payload)), 0) FROM paste_item_data",
            [],
            |r| r.get(0),
        )?;
        let pages: i64 = self
            .conn
            .pragma_query_value(None, "page_count", |r| r.get(0))?;
        let page_size: i64 = self
            .conn
            .pragma_query_value(None, "page_size", |r| r.get(0))?;
        Ok(Stats {
            items,
            raw_bytes,
            stored_bytes,
            file_bytes: pages * page_size,
        })
    }

    fn summaries(&self, sql: &str, params: Vec<Value>) -> Result<Vec<ItemSummary>> {
        let mut stmt = self.conn.prepare_cached(sql)?;
        let rows = stmt.query_map(params_from_iter(params), summary_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

fn summary_from_row(r: &Row<'_>) -> rusqlite::Result<ItemSummary> {
    let kind: String = r.get(1)?;
    Ok(ItemSummary {
        id: r.get(0)?,
        kind: Kind::parse(&kind).ok_or_else(|| {
            rusqlite::Error::InvalidColumnType(1, "kind".into(), rusqlite::types::Type::Text)
        })?,
        title: r.get(2)?,
        preview: r.get(3)?,
        app_bundle_id: r.get(4)?,
        source_url: r.get(5)?,
        source_domain: r.get(6)?,
        secret_kind: r
            .get::<_, Option<String>>(7)?
            .as_deref()
            .and_then(SecretKind::parse),
        secret_provider: r.get(8)?,
        note: r.get(9)?,
        created_at: r.get(10)?,
        last_used_at: r.get(11)?,
        byte_size: r.get(12)?,
        contact: contacts::split(r.get::<_, Option<String>>(13)?.as_deref()),
    })
}

#[cfg(test)]
mod test_support {
    use super::Store;
    use crate::model::{AppInfo, Capture, ItemSummary};

    pub fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    pub fn add(store: &mut Store, text: &str, bundle_id: Option<&str>, now: i64) -> i64 {
        let mut cap = Capture::from_text(text.to_owned());
        cap.app = bundle_id.map(|b| AppInfo {
            bundle_id: b.into(),
            name: None,
            path: None,
        });
        store.ingest(&cap, now).unwrap().id()
    }

    pub fn ids(items: Vec<ItemSummary>) -> Vec<i64> {
        items.into_iter().map(|i| i.id).collect()
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::test_support::ids;
    use super::*;
    use crate::{Contact, Rule, SortType};

    #[test]
    fn v1_databases_migrate_with_domains_kinds_and_searchable_notes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v1.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(schema::V1).unwrap();
        conn.execute_batch(
            "INSERT INTO paste_items (kind, text, title, created_at, last_used_at, base_uniform_type, \
             checksum, byte_size, source_url, secret_provider) VALUES \
             ('text', 'sk-pro…Ab7c', 'OPENAI_API_KEY', 1, 1, 't', 1, 1, \
              'https://platform.openai.com/api-keys', 'OpenAI API key'), \
             ('text', 'meeting notes', NULL, 2, 2, 't', 2, 1, NULL, NULL), \
             ('text', 'Bahnhofstrasse 12, 8001 Zürich', NULL, 3, 3, 't', 3, 1, NULL, NULL);",
        )
        .unwrap();
        drop(conn);

        let s = Store::open(&path).unwrap();
        let rule: Rule = serde_json::from_str(r#"{"type":"contact","contact":"address"}"#).unwrap();
        let list = PasteList {
            id: 0,
            name: "Addresses".into(),
            rule: Some(rule),
            sort: SortType::Recent,
            sort_weight: 0.0,
        };
        let found = s.list_items(&list, 10, 4).unwrap();
        assert_eq!(
            (ids(found.clone()), found[0].contact.clone()),
            (vec![3], vec![Contact::Address])
        );
        let any: Rule = serde_json::from_str(r#"{"type":"contact"}"#).unwrap();
        assert_eq!(any, Rule::Contact { contact: None });
        let keys = s.secrets(&SecretFilter::default(), 10).unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].secret_kind, Some(SecretKind::Key));
        assert_eq!(keys[0].source_domain.as_deref(), Some("openai.com"));
        let site = SecretFilter {
            domain: Some("platform.openai.com".into()),
            provider: Some("openai api KEY".into()),
            ..SecretFilter::default()
        };
        assert_eq!(ids(s.secrets(&site, 10).unwrap()), [1]);
        let facets = s.secret_facets(None).unwrap();
        assert_eq!(
            facets.domains,
            [Facet {
                name: "openai.com".into(),
                count: 1
            }]
        );
        assert_eq!(ids(s.search("OPENAI", 10).unwrap()), [1]);

        s.set_note(2, Some("staging billing")).unwrap();
        assert_eq!(ids(s.search("billing", 10).unwrap()), [2]);
        assert_eq!(ids(s.search("bi", 10).unwrap()), [2]);
        s.set_note(2, None).unwrap();
        assert!(s.search("billing", 10).unwrap().is_empty());
    }
}
