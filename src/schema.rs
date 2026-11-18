use rusqlite::{Connection, Transaction, TransactionBehavior};

pub(crate) const V1: &str = r"
CREATE TABLE applications (
    id INTEGER PRIMARY KEY,
    bundle_id TEXT NOT NULL UNIQUE,
    name TEXT,
    path TEXT,
    copy_count INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE paste_items (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    text TEXT,
    title TEXT,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL,
    app_id INTEGER REFERENCES applications(id) ON DELETE SET NULL,
    base_uniform_type TEXT NOT NULL,
    checksum INTEGER NOT NULL UNIQUE,
    byte_size INTEGER NOT NULL,
    source_url TEXT,
    source_title TEXT,
    secret_provider TEXT,
    color_hex TEXT,
    sort_order REAL NOT NULL DEFAULT 0
);
CREATE INDEX paste_items_last_used ON paste_items (last_used_at DESC, id DESC);
CREATE INDEX paste_items_app ON paste_items (app_id);
CREATE TABLE paste_item_data (
    id INTEGER PRIMARY KEY,
    item_id INTEGER NOT NULL REFERENCES paste_items(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    format TEXT NOT NULL,
    payload BLOB NOT NULL,
    raw_size INTEGER NOT NULL,
    UNIQUE (item_id, position)
);
CREATE TABLE paste_lists (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    filter_rule_data TEXT,
    sort_weight REAL NOT NULL DEFAULT 0,
    sort_type TEXT NOT NULL DEFAULT 'recent'
);
CREATE TABLE paste_list_items (
    list_id INTEGER NOT NULL REFERENCES paste_lists(id) ON DELETE CASCADE,
    item_id INTEGER NOT NULL REFERENCES paste_items(id) ON DELETE CASCADE,
    added_at INTEGER NOT NULL,
    PRIMARY KEY (list_id, item_id)
);
CREATE INDEX paste_list_items_item ON paste_list_items (item_id);
CREATE TABLE exclude_rules (
    id INTEGER PRIMARY KEY,
    enabled INTEGER NOT NULL DEFAULT 1,
    app_id INTEGER NOT NULL UNIQUE REFERENCES applications(id) ON DELETE CASCADE
);
CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE VIRTUAL TABLE paste_items_fts USING fts5(
    text, title, content='paste_items', content_rowid='id', tokenize='trigram'
);
CREATE TRIGGER paste_items_ai AFTER INSERT ON paste_items BEGIN
    INSERT INTO paste_items_fts (rowid, text, title) VALUES (new.id, new.text, new.title);
END;
CREATE TRIGGER paste_items_ad AFTER DELETE ON paste_items BEGIN
    INSERT INTO paste_items_fts (paste_items_fts, rowid, text, title)
    VALUES ('delete', old.id, old.text, old.title);
END;
CREATE TRIGGER paste_items_au AFTER UPDATE OF text, title ON paste_items BEGIN
    INSERT INTO paste_items_fts (paste_items_fts, rowid, text, title)
    VALUES ('delete', old.id, old.text, old.title);
    INSERT INTO paste_items_fts (rowid, text, title) VALUES (new.id, new.text, new.title);
END;
PRAGMA user_version = 1;
";

/// Adds key/ID kinds, source domains and notes, and indexes notes.
pub(crate) const V2: &str = r"
ALTER TABLE paste_items ADD COLUMN secret_kind TEXT;
ALTER TABLE paste_items ADD COLUMN source_domain TEXT;
ALTER TABLE paste_items ADD COLUMN note TEXT;
UPDATE paste_items SET secret_kind = 'key' WHERE secret_provider IS NOT NULL;
CREATE INDEX paste_items_secret ON paste_items (secret_kind) WHERE secret_kind IS NOT NULL;
DROP TRIGGER paste_items_ai;
DROP TRIGGER paste_items_ad;
DROP TRIGGER paste_items_au;
DROP TABLE paste_items_fts;
CREATE VIRTUAL TABLE paste_items_fts USING fts5(
    text, title, note, content='paste_items', content_rowid='id', tokenize='trigram'
);
CREATE TRIGGER paste_items_ai AFTER INSERT ON paste_items BEGIN
    INSERT INTO paste_items_fts (rowid, text, title, note)
    VALUES (new.id, new.text, new.title, new.note);
END;
CREATE TRIGGER paste_items_ad AFTER DELETE ON paste_items BEGIN
    INSERT INTO paste_items_fts (paste_items_fts, rowid, text, title, note)
    VALUES ('delete', old.id, old.text, old.title, old.note);
END;
CREATE TRIGGER paste_items_au AFTER UPDATE OF text, title, note ON paste_items BEGIN
    INSERT INTO paste_items_fts (paste_items_fts, rowid, text, title, note)
    VALUES ('delete', old.id, old.text, old.title, old.note);
    INSERT INTO paste_items_fts (rowid, text, title, note)
    VALUES (new.id, new.text, new.title, new.note);
END;
INSERT INTO paste_items_fts (paste_items_fts) VALUES ('rebuild');
PRAGMA user_version = 2;
";

/// Tags phone numbers, emails and addresses; `migrate` backfills them.
pub(crate) const V3: &str = r"
ALTER TABLE paste_items ADD COLUMN contact TEXT;
CREATE INDEX paste_items_contact ON paste_items (contact) WHERE contact IS NOT NULL;
PRAGMA user_version = 3;
";

/// Encrypted key payloads (`key_gen`, NULL while plaintext) and an index that
/// never holds a key's text, not even masked: FTS reads a view instead.
const V4: &str = r"
ALTER TABLE paste_item_data ADD COLUMN key_gen INTEGER;
DROP TRIGGER paste_items_ai;
DROP TRIGGER paste_items_ad;
DROP TRIGGER paste_items_au;
DROP TABLE paste_items_fts;
CREATE VIEW paste_items_search AS
    SELECT id, CASE WHEN secret_kind = 'key' THEN NULL ELSE text END AS text, title, note
    FROM paste_items;
CREATE VIRTUAL TABLE paste_items_fts USING fts5(
    text, title, note, content='paste_items_search', content_rowid='id', tokenize='trigram'
);
CREATE TRIGGER paste_items_ai AFTER INSERT ON paste_items BEGIN
    INSERT INTO paste_items_fts (rowid, text, title, note) VALUES (new.id,
        CASE WHEN new.secret_kind = 'key' THEN NULL ELSE new.text END, new.title, new.note);
END;
CREATE TRIGGER paste_items_ad AFTER DELETE ON paste_items BEGIN
    INSERT INTO paste_items_fts (paste_items_fts, rowid, text, title, note) VALUES ('delete', old.id,
        CASE WHEN old.secret_kind = 'key' THEN NULL ELSE old.text END, old.title, old.note);
END;
CREATE TRIGGER paste_items_au AFTER UPDATE OF text, title, note, secret_kind ON paste_items BEGIN
    INSERT INTO paste_items_fts (paste_items_fts, rowid, text, title, note) VALUES ('delete', old.id,
        CASE WHEN old.secret_kind = 'key' THEN NULL ELSE old.text END, old.title, old.note);
    INSERT INTO paste_items_fts (rowid, text, title, note) VALUES (new.id,
        CASE WHEN new.secret_kind = 'key' THEN NULL ELSE new.text END, new.title, new.note);
END;
INSERT INTO paste_items_fts (paste_items_fts) VALUES ('rebuild');
PRAGMA user_version = 4;
";

pub(crate) const VERSION: i64 = 4;

pub(crate) fn configure(conn: &Connection) -> rusqlite::Result<()> {
    // Both persist in the file, so only a new one is written; setting them
    // again on every open rewrites the header and syncs.
    let new: i64 = conn.pragma_query_value(None, "page_count", |r| r.get(0))?;
    if new == 0 {
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
    }
    let mode: String = conn.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
    }
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    // Checkpoint small and often; a checkpoint that resets the WAL also truncates it to this size.
    conn.pragma_update_and_check(None, "wal_autocheckpoint", 200, |_| Ok(()))?;
    conn.pragma_update_and_check(None, "journal_size_limit", 1 << 20, |_| Ok(()))?;
    conn.pragma_update(None, "foreign_keys", true)?;
    // Deleted clips and replaced secrets are overwritten, not left in free pages.
    conn.pragma_update(None, "secure_delete", true)?;
    // Small page cache keeps the resident set low; the hot data is tiny.
    conn.pragma_update(None, "cache_size", -1024)?;
    conn.busy_timeout(BUSY_TIMEOUT)
}

const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Folds the WAL into the main file and truncates it. Best effort: with the
/// busy timeout off, another process's reader or writer just makes it skip.
/// Returns whether it completed, so everything committed is synced to the file.
pub(crate) fn checkpoint(conn: &Connection) -> bool {
    let _ = conn.busy_timeout(std::time::Duration::ZERO);
    let done = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
        r.get::<_, i64>(0)
    });
    let _ = conn.busy_timeout(BUSY_TIMEOUT);
    matches!(done, Ok(0))
}

/// Copies what it can of the WAL back without waiting for or blocking anyone.
pub(crate) fn checkpoint_passive(conn: &Connection) {
    let _ = conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()));
}

pub(crate) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version >= VERSION {
        return Ok(());
    }
    // Another process may be migrating too; re-check under the write lock.
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version < 1 {
        tx.execute_batch(V1)?;
    }
    if version < 2 {
        tx.execute_batch(V2)?;
        let urls: Vec<(i64, String)> = tx
            .prepare("SELECT id, source_url FROM paste_items WHERE source_url IS NOT NULL")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut set = tx.prepare("UPDATE paste_items SET source_domain = ?1 WHERE id = ?2")?;
        for (id, url) in urls {
            if let Some(domain) = crate::secrets::domain(&url) {
                set.execute(rusqlite::params![domain, id])?;
            }
        }
        drop(set);
    }
    if version < 3 {
        tx.execute_batch(V3)?;
        // `text` holds the clip up to 64 KiB, far above what detection reads.
        let texts: Vec<(i64, String)> = tx
            .prepare(
                "SELECT id, text FROM paste_items WHERE kind IN ('text', 'url') \
                 AND secret_kind IS NULL AND text IS NOT NULL AND length(text) <= ?1",
            )?
            .query_map([crate::contacts::MAX_CHARS as i64 * 2], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut set = tx.prepare("UPDATE paste_items SET contact = ?1 WHERE id = ?2")?;
        for (id, text) in texts {
            if let Some(found) = crate::contacts::join(&crate::contacts::detect(&text)) {
                set.execute(rusqlite::params![found, id])?;
            }
        }
        drop(set);
    }
    if version < 4 {
        tx.execute_batch(V4)?;
    }
    tx.commit()
}
