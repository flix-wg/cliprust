use rusqlite::Connection;

const V1: &str = r"
BEGIN;
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
COMMIT;
";

pub(crate) fn configure(conn: &Connection) -> rusqlite::Result<()> {
    // Only takes effect before the first table exists, i.e. on a new file.
    conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
    conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", true)?;
    // Deleted clips and replaced secrets are overwritten, not left in free pages.
    conn.pragma_update(None, "secure_delete", true)?;
    // Small page cache keeps the resident set low; the hot data is tiny.
    conn.pragma_update(None, "cache_size", -1024)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
}

pub(crate) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version < 1 {
        conn.execute_batch(V1)?;
    }
    Ok(())
}
