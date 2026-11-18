//! SQLite storage: dedup on ingest, zstd payloads, FTS5 trigram search,
//! manual and smart lists, exclusions, settings and retention.

use std::path::Path;

use rusqlite::{
    Connection, OptionalExtension, Row, TransactionBehavior, params, params_from_iter, types::Value,
};
use serde::{Deserialize, Serialize};

use crate::model::{AppInfo, Capture, Item, ItemSummary, Kind, Rep, SecretKind, Settings};
use crate::rules::{CONTAINS_SQL, Rule, SortType};
use crate::{Error, Result, schema, secrets};

pub const KEYS_LIST: &str = "Keys";
/// IDs get their own list so that Keys holds only masked secrets.
pub const IDS_LIST: &str = "IDs";
/// The manual list item pinning adds to; no smart list may take its name.
pub const PINNED_LIST: &str = "Pinned";

const ZSTD_LEVEL: i32 = 3;
/// FTS indexes at most this much of a clip; the full text stays in the payload.
const INDEXED_TEXT_LIMIT: usize = 64 * 1024;
const SUMMARY_SQL: &str = "SELECT p.id, p.kind, p.title, substr(p.text, 1, 200), a.bundle_id, \
     p.source_url, p.source_domain, p.secret_kind, p.secret_provider, p.note, \
     p.created_at, p.last_used_at, p.byte_size \
     FROM paste_items p LEFT JOIN applications a ON a.id = p.app_id";
/// Items in a manual list are kept by retention and `prune`.
const UNLISTED: &str = "p.id NOT IN (SELECT item_id FROM paste_list_items)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ingest {
    Inserted(i64),
    Duplicate(i64),
}

impl Ingest {
    pub fn id(self) -> i64 {
        match self {
            Ingest::Inserted(id) | Ingest::Duplicate(id) => id,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PasteList {
    pub id: i64,
    pub name: String,
    /// `None` for a manual list whose members are added by hand.
    pub rule: Option<Rule>,
    pub sort: SortType,
    pub sort_weight: f64,
}

/// Narrows [`Store::secrets`]; `None` fields match everything.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SecretFilter {
    pub kind: Option<SecretKind>,
    /// Case-insensitive provider name, e.g. "Cloudflare API token".
    pub provider: Option<String>,
    /// A registrable domain; any host under it matches.
    pub domain: Option<String>,
}

/// Narrows [`Store::find`]; `None` fields match everything.
#[derive(Debug, Clone, Copy, Default)]
pub struct Find<'a> {
    /// Whitespace-separated terms, all of which must occur as substrings.
    pub text: Option<&'a str>,
    pub kind: Option<Kind>,
    /// Bundle id of the app the item was copied from.
    pub app: Option<&'a str>,
    pub list: Option<&'a PasteList>,
}

/// An app with the number of stored items copied from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppCount {
    pub bundle_id: String,
    pub name: Option<String>,
    pub count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Facet {
    pub name: String,
    pub count: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Facets {
    pub providers: Vec<Facet>,
    pub domains: Vec<Facet>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub items: i64,
    pub raw_bytes: i64,
    pub stored_bytes: i64,
    pub file_bytes: i64,
}

pub struct Store {
    conn: Connection,
}

impl Drop for Store {
    fn drop(&mut self) {
        schema::checkpoint(&self.conn);
    }
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        schema::configure(&conn)?;
        schema::migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Stores a capture. A copy whose checksum is already known updates that
    /// item instead: a newer copy brings its formats and moves it to the top,
    /// and a newly detected key is masked and filed in the Keys list.
    pub fn ingest(&mut self, cap: &Capture, now: i64) -> Result<Ingest> {
        let checksum = cap.checksum() as i64;
        // Take the write lock up front so a concurrent writer cannot void
        // this transaction's snapshot between the lookup and the write.
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let app_id = cap
            .app
            .as_ref()
            .map(|app| upsert_app(&tx, app, 1))
            .transpose()?;
        let byte_size: i64 = cap.reps.iter().map(|r| r.data.len() as i64).sum();
        let domain = cap.source_url.as_deref().and_then(secrets::domain);
        let existing: Option<(i64, i64, Option<String>)> = tx
            .query_row(
                "SELECT id, last_used_at, secret_kind FROM paste_items WHERE checksum = ?1",
                [checksum],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let outcome = if let Some((id, last_used_at, old_kind)) = existing {
            if now >= last_used_at {
                tx.execute(
                    "UPDATE paste_items SET last_used_at = ?1, app_id = coalesce(?2, app_id), \
                     source_url = coalesce(?3, source_url), source_title = coalesce(?4, source_title), \
                     source_domain = coalesce(?5, source_domain), byte_size = ?6 WHERE id = ?7",
                    params![now, app_id, cap.source_url, cap.source_title, domain, byte_size, id],
                )?;
                tx.execute("DELETE FROM paste_item_data WHERE item_id = ?1", [id])?;
                insert_reps(&tx, id, &cap.reps)?;
            }
            // A key found in a clip filed as an ID replaces the ID.
            let old_kind = old_kind.as_deref().and_then(SecretKind::parse);
            let upgrade = |s: &&crate::Secret| match old_kind {
                None => true,
                Some(old) => old == SecretKind::Id && s.kind == SecretKind::Key,
            };
            if let Some(secret) = cap.secret.as_ref().filter(upgrade) {
                tx.execute(
                    "UPDATE paste_items SET text = ?1, \
                     title = CASE WHEN secret_kind = 'id' THEN ?2 ELSE coalesce(title, ?2) END, \
                     secret_provider = ?3, secret_kind = ?4, source_domain = coalesce(?5, source_domain) \
                     WHERE id = ?6",
                    params![secret.masked, secret.name, secret.provider, secret.kind.as_str(), domain, id],
                )?;
                if old_kind.is_some() {
                    tx.execute(
                        "DELETE FROM paste_list_items WHERE item_id = ?1 \
                         AND list_id = (SELECT id FROM paste_lists WHERE name = ?2)",
                        params![id, IDS_LIST],
                    )?;
                }
                file_secret(&tx, id, secret.kind, now)?;
                // Merge the index so the unmasked text's tokens are rewritten too.
                tx.execute(
                    "INSERT INTO paste_items_fts (paste_items_fts) VALUES ('optimize')",
                    [],
                )?;
            }
            Ingest::Duplicate(id)
        } else {
            let (text, title, provider, secret_kind) = match &cap.secret {
                Some(s) => (
                    Some(s.masked.as_str()),
                    Some(s.name.as_str()),
                    Some(s.provider.as_str()),
                    Some(s.kind.as_str()),
                ),
                None => (
                    cap.text().map(|t| truncate(t, INDEXED_TEXT_LIMIT)),
                    None,
                    None,
                    None,
                ),
            };
            tx.execute(
                "INSERT INTO paste_items (kind, text, title, created_at, last_used_at, app_id, \
                 base_uniform_type, checksum, byte_size, source_url, source_title, secret_provider, \
                 secret_kind, source_domain) \
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    cap.kind.as_str(),
                    text,
                    title,
                    now,
                    app_id,
                    cap.base_uniform_type,
                    checksum,
                    byte_size,
                    cap.source_url,
                    cap.source_title,
                    provider,
                    secret_kind,
                    domain
                ],
            )?;
            let id = tx.last_insert_rowid();
            insert_reps(&tx, id, &cap.reps)?;
            if let Some(secret) = &cap.secret {
                file_secret(&tx, id, secret.kind, now)?;
            }
            Ingest::Inserted(id)
        };
        tx.commit()?;
        Ok(outcome)
    }

    pub fn recent(&self, limit: usize) -> Result<Vec<ItemSummary>> {
        self.find(&Find::default(), limit, 0)
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<ItemSummary>> {
        let find = Find {
            text: Some(query),
            ..Find::default()
        };
        self.find(&find, limit, 0)
    }

    /// Items matching every given filter, in the list's order or most recent first.
    pub fn find(&self, f: &Find<'_>, limit: usize, now: i64) -> Result<Vec<ItemSummary>> {
        let mut clauses = Vec::new();
        let mut params = Vec::new();
        if let Some(list) = f.list {
            clauses.push(match &list.rule {
                Some(rule) => rule.to_sql(now, &mut params),
                None => {
                    params.push(Value::from(list.id));
                    "p.id IN (SELECT item_id FROM paste_list_items WHERE list_id = ?)".to_owned()
                }
            });
        }
        let (long, short): (Vec<&str>, Vec<&str>) = f
            .text
            .unwrap_or_default()
            .split_whitespace()
            .partition(|t| t.chars().count() >= 3);
        if !long.is_empty() {
            clauses.push(
                "p.id IN (SELECT rowid FROM paste_items_fts WHERE paste_items_fts MATCH ?)".into(),
            );
            let phrases: Vec<String> = long
                .iter()
                .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
                .collect();
            params.push(Value::from(phrases.join(" AND ")));
        }
        // The trigram index cannot answer terms shorter than three characters.
        for term in short {
            clauses.push(CONTAINS_SQL.into());
            params.extend(std::iter::repeat_n(Value::from(term.to_owned()), 3));
        }
        if let Some(kind) = f.kind {
            clauses.push("p.kind = ?".into());
            params.push(Value::from(kind.as_str().to_owned()));
        }
        if let Some(app) = f.app {
            clauses.push("p.app_id = (SELECT id FROM applications WHERE bundle_id = ?)".into());
            params.push(Value::from(app.to_owned()));
        }
        let filter = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE ({})", clauses.join(") AND ("))
        };
        params.push(Value::from(limit as i64));
        let sort = f.list.map_or(SortType::Recent, |l| l.sort);
        let sql = format!("{SUMMARY_SQL} {filter} {} LIMIT ?", sort.order_sql());
        self.summaries(&sql, params)
    }

    /// Apps that stored items were copied from, most copied first. A query
    /// keeps apps whose name, a word of it, or a bundle-id component starts
    /// with it, best match first.
    pub fn apps(&self, query: Option<&str>) -> Result<Vec<AppCount>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT a.bundle_id, a.name, count(*) FROM applications a \
             JOIN paste_items p ON p.app_id = a.id \
             GROUP BY a.id ORDER BY a.copy_count DESC, 3 DESC, a.bundle_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(AppCount {
                bundle_id: r.get(0)?,
                name: r.get(1)?,
                count: r.get(2)?,
            })
        })?;
        let apps: Vec<AppCount> = rows.collect::<rusqlite::Result<_>>()?;
        let Some(q) = query
            .map(|q| q.trim().to_lowercase())
            .filter(|q| !q.is_empty())
        else {
            return Ok(apps);
        };
        let mut ranked: Vec<(u8, AppCount)> = apps
            .into_iter()
            .filter_map(|a| Some((app_rank(&q, &a)?, a)))
            .collect();
        ranked.sort_by_key(|(rank, _)| *rank);
        Ok(ranked.into_iter().map(|(_, a)| a).collect())
    }

    pub fn summary(&self, id: i64) -> Result<Option<ItemSummary>> {
        let sql = format!("{SUMMARY_SQL} WHERE p.id = ?");
        Ok(self.summaries(&sql, vec![Value::from(id)])?.pop())
    }

    /// The stored formats of an item, without reading their payloads.
    pub fn formats(&self, id: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT format FROM paste_item_data WHERE item_id = ?1 ORDER BY position",
        )?;
        let rows = stmt.query_map([id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// One representation, decompressed. `"image"` picks the first image format.
    pub fn rep(&self, id: i64, format: &str) -> Result<Option<Vec<u8>>> {
        let row: Option<(Vec<u8>, i64)> = self
            .conn
            .prepare_cached(
                "SELECT payload, raw_size FROM paste_item_data WHERE item_id = ?1 AND (format = ?2 \
                 OR (?2 = 'image' AND format IN ('public.png', 'public.tiff', 'public.jpeg', 'public.heic'))) \
                 ORDER BY position LIMIT 1",
            )?
            .query_row(params![id, format], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        Ok(row
            .map(|(payload, raw_size)| zstd::bulk::decompress(&payload, raw_size as usize))
            .transpose()?)
    }

    pub fn get(&self, id: i64) -> Result<Option<Item>> {
        let Some(summary) = self.summary(id)? else {
            return Ok(None);
        };
        let mut stmt = self
            .conn
            .prepare_cached("SELECT format, payload, raw_size FROM paste_item_data WHERE item_id = ?1 ORDER BY position")?;
        let rows = stmt.query_map([id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        let mut reps = Vec::new();
        for row in rows {
            let (format, payload, raw_size) = row?;
            reps.push(Rep {
                format,
                data: zstd::bulk::decompress(&payload, raw_size as usize)?,
            });
        }
        Ok(Some(Item { summary, reps }))
    }

    pub fn touch(&self, id: i64, now: i64) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE paste_items SET last_used_at = ?1 WHERE id = ?2",
            params![now, id],
        )? > 0)
    }

    pub fn set_title(&self, id: i64, title: Option<&str>) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE paste_items SET title = ?1 WHERE id = ?2",
            params![title, id],
        )? > 0)
    }

    /// Attaches a searchable comment to an item; `None` or blank removes it.
    pub fn set_note(&self, id: i64, note: Option<&str>) -> Result<bool> {
        let note = note.map(str::trim).filter(|n| !n.is_empty());
        Ok(self.conn.execute(
            "UPDATE paste_items SET note = ?1 WHERE id = ?2",
            params![note, id],
        )? > 0)
    }

    /// Detected keys and IDs, most recently used first.
    pub fn secrets(&self, filter: &SecretFilter, limit: usize) -> Result<Vec<ItemSummary>> {
        let mut params = Vec::new();
        let filter = secret_filter_sql(filter, &mut params);
        params.push(Value::from(limit as i64));
        let sql = format!(
            "{SUMMARY_SQL} WHERE {filter} {} LIMIT ?",
            SortType::Recent.order_sql()
        );
        self.summaries(&sql, params)
    }

    /// Providers and domains among detected keys and IDs, with counts.
    pub fn secret_facets(&self, kind: Option<SecretKind>) -> Result<Facets> {
        let filter = SecretFilter {
            kind,
            ..SecretFilter::default()
        };
        let mut facets = Facets::default();
        for (column, out) in [
            ("p.secret_provider", &mut facets.providers),
            ("p.source_domain", &mut facets.domains),
        ] {
            let mut params = Vec::new();
            let sql = format!(
                "SELECT {column}, count(*) FROM paste_items p WHERE {} AND {column} IS NOT NULL \
                 GROUP BY 1 ORDER BY 2 DESC, 1",
                secret_filter_sql(&filter, &mut params)
            );
            let mut stmt = self.conn.prepare_cached(&sql)?;
            let rows = stmt.query_map(params_from_iter(params), |r| {
                Ok(Facet {
                    name: r.get(0)?,
                    count: r.get(1)?,
                })
            })?;
            *out = rows.collect::<rusqlite::Result<_>>()?;
        }
        Ok(facets)
    }

    pub fn delete(&self, id: i64) -> Result<bool> {
        let deleted = self
            .conn
            .execute("DELETE FROM paste_items WHERE id = ?1", [id])?
            > 0;
        self.reclaim()?;
        Ok(deleted)
    }

    /// Deletes the history except items kept in manual lists.
    pub fn clear(&self) -> Result<usize> {
        let n = self.conn.execute(
            &format!("DELETE FROM paste_items AS p WHERE {UNLISTED}"),
            [],
        )?;
        self.reclaim()?;
        Ok(n)
    }

    /// Deletes unlisted items beyond the newest `max_items`, or last used
    /// before `older_than`. Returns how many were deleted.
    pub fn prune(&self, max_items: Option<u32>, older_than: Option<i64>) -> Result<usize> {
        let mut deleted = 0;
        if let Some(cutoff) = older_than {
            deleted += self.conn.execute(
                &format!("DELETE FROM paste_items AS p WHERE {UNLISTED} AND p.last_used_at < ?1"),
                [cutoff],
            )?;
        }
        if let Some(max) = max_items {
            deleted += self.conn.execute(
                &format!(
                    "DELETE FROM paste_items AS p WHERE {UNLISTED} AND p.id NOT IN \
                     (SELECT p.id FROM paste_items AS p WHERE {UNLISTED} {} LIMIT ?1)",
                    SortType::Recent.order_sql()
                ),
                [max],
            )?;
        }
        if deleted > 0 {
            self.reclaim()?;
        }
        Ok(deleted)
    }

    /// Applies the retention limits from settings.
    pub fn apply_retention(&self, settings: &Settings, now: i64) -> Result<usize> {
        let older_than = settings
            .max_age_days
            .map(|d| now - i64::from(d) * 86_400_000);
        self.prune(settings.max_items, older_than)
    }

    fn reclaim(&self) -> Result<()> {
        self.conn
            .query_row("PRAGMA incremental_vacuum", [], |_| Ok(()))
            .optional()?;
        // Deleted rows also linger in the WAL until it is checkpointed.
        schema::checkpoint(&self.conn);
        Ok(())
    }

    pub fn create_list(&self, name: &str, rule: Option<&Rule>, sort: SortType) -> Result<i64> {
        if name == PINNED_LIST && rule.is_some() {
            return Err(reserved());
        }
        let rule = rule.map(serde_json::to_string).transpose()?;
        self.conn.execute(
            "INSERT INTO paste_lists (name, filter_rule_data, sort_weight, sort_type) \
             VALUES (?1, ?2, (SELECT coalesce(max(sort_weight), 0) + 1 FROM paste_lists), ?3)",
            params![name, rule, sort.as_str()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn lists(&self) -> Result<Vec<PasteList>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, filter_rule_data, sort_type, sort_weight FROM paste_lists ORDER BY sort_weight, id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get(4)?,
            ))
        })?;
        let mut lists = Vec::new();
        for row in rows {
            let (id, name, rule, sort, sort_weight) = row?;
            lists.push(PasteList {
                id,
                name,
                rule: rule.as_deref().map(serde_json::from_str).transpose()?,
                sort: SortType::parse(&sort).unwrap_or_default(),
                sort_weight,
            });
        }
        Ok(lists)
    }

    pub fn list_by_name(&self, name: &str) -> Result<Option<PasteList>> {
        Ok(self.lists()?.into_iter().find(|l| l.name == name))
    }

    pub fn delete_list(&self, name: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM paste_lists WHERE name = ?1", [name])?
            > 0)
    }

    pub fn rename_list(&self, name: &str, new_name: &str) -> Result<bool> {
        if new_name == PINNED_LIST && self.list_by_name(name)?.is_some_and(|l| l.rule.is_some()) {
            return Err(reserved());
        }
        Ok(self.conn.execute(
            "UPDATE paste_lists SET name = ?2 WHERE name = ?1",
            [name, new_name],
        )? > 0)
    }

    /// Adds an item to a manual list, creating the list if needed.
    pub fn add_to_list(&self, name: &str, item_id: i64, now: i64) -> Result<()> {
        if self.list_by_name(name)?.is_some_and(|l| l.rule.is_some()) {
            return Err(Error::Refused(format!(
                "{name:?} is a smart list; its items come from its rule"
            )));
        }
        let list_id = ensure_list(&self.conn, name)?;
        self.conn.execute(
            "INSERT OR IGNORE INTO paste_list_items (list_id, item_id, added_at) VALUES (?1, ?2, ?3)",
            params![list_id, item_id, now],
        )?;
        Ok(())
    }

    pub fn remove_from_list(&self, name: &str, item_id: i64) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM paste_list_items WHERE item_id = ?2 AND list_id = (SELECT id FROM paste_lists WHERE name = ?1)",
            params![name, item_id],
        )? > 0)
    }

    pub fn list_items(&self, list: &PasteList, limit: usize, now: i64) -> Result<Vec<ItemSummary>> {
        let find = Find {
            list: Some(list),
            ..Find::default()
        };
        self.find(&find, limit, now)
    }

    /// Ids of the items in a manual list.
    pub fn list_ids(&self, name: &str) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT i.item_id FROM paste_list_items i JOIN paste_lists l ON l.id = i.list_id WHERE l.name = ?1",
        )?;
        let rows = stmt.query_map([name], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn add_exclude(&mut self, bundle_id: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        let app = AppInfo {
            bundle_id: bundle_id.to_owned(),
            name: None,
            path: None,
        };
        let app_id = upsert_app(&tx, &app, 0)?;
        tx.execute(
            "INSERT INTO exclude_rules (app_id, enabled) VALUES (?1, 1) ON CONFLICT (app_id) DO UPDATE SET enabled = 1",
            [app_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn remove_exclude(&self, bundle_id: &str) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM exclude_rules WHERE app_id = (SELECT id FROM applications WHERE bundle_id = ?1)",
            [bundle_id],
        )? > 0)
    }

    pub fn excludes(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT a.bundle_id FROM exclude_rules e JOIN applications a ON a.id = e.app_id \
             WHERE e.enabled ORDER BY a.bundle_id",
        )?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn is_excluded(&self, bundle_id: &str) -> Result<bool> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT 1 FROM exclude_rules e JOIN applications a ON a.id = e.app_id \
                 WHERE e.enabled AND a.bundle_id = ?1",
            )?
            .exists([bundle_id])?)
    }

    pub fn settings(&self) -> Result<Settings> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'settings'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?
            .unwrap_or_default())
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES ('settings', ?1) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [serde_json::to_string(settings)?],
        )?;
        Ok(())
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
    })
}

fn app_rank(q: &str, app: &AppCount) -> Option<u8> {
    let name = app.name.as_deref().unwrap_or_default().to_lowercase();
    let bundle = app.bundle_id.to_lowercase();
    let parts: Vec<&str> = bundle.split('.').collect();
    if name.starts_with(q) {
        Some(0)
    } else if name
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| w.starts_with(q))
    {
        Some(1)
    } else if parts.last().is_some_and(|p| p.starts_with(q)) {
        Some(2)
    } else if q.len() >= 3 && parts.iter().skip(1).any(|p| p.starts_with(q)) {
        Some(3)
    } else {
        None
    }
}

fn secret_filter_sql(filter: &SecretFilter, params: &mut Vec<Value>) -> String {
    let mut sql = match filter.kind {
        Some(kind) => {
            params.push(Value::from(kind.as_str().to_owned()));
            "p.secret_kind = ?".to_owned()
        }
        None => "p.secret_kind IS NOT NULL".to_owned(),
    };
    if let Some(provider) = &filter.provider {
        sql.push_str(" AND p.secret_provider = ? COLLATE NOCASE");
        params.push(Value::from(provider.clone()));
    }
    if let Some(domain) = &filter.domain {
        // Accept a host or URL as well as a bare domain.
        let domain = domain.trim();
        let url = if domain.contains("://") {
            domain.to_owned()
        } else {
            format!("https://{domain}")
        };
        let domain = secrets::domain(&url).unwrap_or_else(|| domain.to_ascii_lowercase());
        sql.push_str(" AND p.source_domain = ?");
        params.push(Value::from(domain));
    }
    sql
}

fn insert_reps(conn: &Connection, item_id: i64, reps: &[Rep]) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        "INSERT INTO paste_item_data (item_id, position, format, payload, raw_size) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for (pos, rep) in reps.iter().enumerate() {
        let payload = zstd::bulk::compress(&rep.data, ZSTD_LEVEL)?;
        stmt.execute(params![
            item_id,
            pos as i64,
            rep.format,
            payload,
            rep.data.len() as i64
        ])?;
    }
    Ok(())
}

fn file_secret(
    conn: &Connection,
    item_id: i64,
    kind: SecretKind,
    now: i64,
) -> rusqlite::Result<()> {
    let list = match kind {
        SecretKind::Key => KEYS_LIST,
        SecretKind::Id => IDS_LIST,
    };
    let list_id = ensure_list(conn, list)?;
    conn.execute(
        "INSERT OR IGNORE INTO paste_list_items (list_id, item_id, added_at) VALUES (?1, ?2, ?3)",
        params![list_id, item_id, now],
    )?;
    Ok(())
}

fn upsert_app(conn: &Connection, app: &AppInfo, copies: i64) -> rusqlite::Result<i64> {
    conn.query_row(
        "INSERT INTO applications (bundle_id, name, path, copy_count) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT (bundle_id) DO UPDATE SET name = coalesce(excluded.name, name), \
         path = coalesce(excluded.path, path), copy_count = copy_count + excluded.copy_count \
         RETURNING id",
        params![app.bundle_id, app.name, app.path, copies],
        |r| r.get(0),
    )
}

fn reserved() -> Error {
    Error::Refused(format!("{PINNED_LIST:?} is reserved for pinned items"))
}

fn ensure_list(conn: &Connection, name: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "INSERT INTO paste_lists (name, sort_weight) \
         VALUES (?1, (SELECT coalesce(max(sort_weight), 0) + 1 FROM paste_lists)) \
         ON CONFLICT (name) DO UPDATE SET name = name RETURNING id",
        [name],
        |r| r.get(0),
    )
}

fn truncate(s: &str, max_bytes: usize) -> &str {
    let mut end = max_bytes.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Secret, format};

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    fn add(store: &mut Store, text: &str, bundle_id: Option<&str>, now: i64) -> i64 {
        let mut cap = Capture::from_text(text.to_owned());
        cap.app = bundle_id.map(|b| AppInfo {
            bundle_id: b.into(),
            name: None,
            path: None,
        });
        store.ingest(&cap, now).unwrap().id()
    }

    fn ids(items: Vec<ItemSummary>) -> Vec<i64> {
        items.into_iter().map(|i| i.id).collect()
    }

    #[test]
    fn duplicate_copies_move_to_top_and_retention_keeps_listed_items() {
        let mut s = store();
        let hello = add(&mut s, "hello world", Some("com.a"), 1);
        let other = add(&mut s, "other", Some("com.a"), 2);
        let mut again = Capture::from_text("hello world".into());
        again.app = Some(AppInfo {
            bundle_id: "com.b".into(),
            name: None,
            path: None,
        });
        assert_eq!(s.ingest(&again, 3).unwrap(), Ingest::Duplicate(hello));
        let recent = s.recent(10).unwrap();
        assert_eq!(ids(recent.clone()), [hello, other]);
        assert_eq!(recent[0].app_bundle_id.as_deref(), Some("com.b"));

        let pinned = add(&mut s, "pinned", None, 0);
        s.add_to_list("Snippets", pinned, 5).unwrap();
        let newest = add(&mut s, "newest", None, 6);
        assert_eq!(s.prune(Some(1), None).unwrap(), 2);
        assert_eq!(ids(s.recent(10).unwrap()), [newest, pinned]);
        assert_eq!(s.prune(None, Some(7)).unwrap(), 1);
        assert_eq!(ids(s.recent(10).unwrap()), [pinned]);
    }

    #[test]
    fn search_finds_substrings_and_forgets_deleted_items() {
        let mut s = store();
        let code = add(&mut s, "let cliprusty = 1; // 東京", None, 1);
        let url = add(&mut s, "https://example.com/path", None, 2);
        add(&mut s, "unrelated note", None, 3);

        assert_eq!(ids(s.search("rust", 10).unwrap()), [code]);
        assert_eq!(ids(s.search("EXAMPLE path", 10).unwrap()), [url]);
        assert_eq!(ids(s.search("ex", 10).unwrap()), [url]);
        assert_eq!(ids(s.search("東京", 10).unwrap()), [code]);
        assert!(s.search("\"unbalanced AND (", 10).unwrap().is_empty());

        s.set_title(url, Some("Work docs")).unwrap();
        assert_eq!(ids(s.search("docs", 10).unwrap()), [url]);
        s.delete(code).unwrap();
        assert!(s.search("rust", 10).unwrap().is_empty());
        s.add_to_list("Snippets", url, 4).unwrap();
        let note = add(&mut s, "another note", None, 5);
        assert_eq!(s.clear().unwrap(), 2);
        assert_eq!(ids(s.recent(10).unwrap()), [url]);
        assert!(s.search("note", 10).unwrap().is_empty() && note > url);
    }

    #[test]
    fn payloads_round_trip_and_secrets_stay_out_of_the_index() {
        let mut s = store();
        let mut cap = Capture::from_text("x".repeat(100_000));
        cap.reps.push(Rep::new(format::HTML, b"<b>x</b>".to_vec()));
        cap.reps.push(Rep::new("public.png", b"\x89PNG".to_vec()));
        let id = s.ingest(&cap, 1).unwrap().id();
        assert_eq!(s.get(id).unwrap().unwrap().reps, cap.reps);
        assert_eq!(s.formats(id).unwrap(), ["text", "html", "public.png"]);
        assert_eq!(s.rep(id, "image").unwrap().unwrap(), b"\x89PNG");
        assert_eq!(s.rep(id, "rtf").unwrap(), None);
        let stats = s.stats().unwrap();
        assert!(stats.stored_bytes * 10 < stats.raw_bytes);

        let mut key = Capture::from_text("sk-live-secret-value".into());
        key.secret = Some(Secret {
            kind: crate::SecretKind::Key,
            provider: "Test key".into(),
            name: "TEST_KEY".into(),
            masked: "sk-liv…alue".into(),
        });
        let key_id = s.ingest(&key, 2).unwrap().id();
        assert!(s.search("secret", 10).unwrap().is_empty());
        assert_eq!(ids(s.search("TEST_KEY", 10).unwrap()), [key_id]);
        let keys = s.list_by_name(KEYS_LIST).unwrap().unwrap();
        assert_eq!(ids(s.list_items(&keys, 10, 3).unwrap()), [key_id]);
        assert_eq!(
            s.get(key_id).unwrap().unwrap().text(),
            Some("sk-live-secret-value")
        );
    }

    #[test]
    fn newer_duplicates_refresh_formats_and_late_detection_masks_keys() {
        let mut s = store();
        let styled = |html: &str| {
            let mut cap = Capture::from_text("same words".into());
            cap.reps
                .push(Rep::new(format::HTML, html.as_bytes().to_vec()));
            cap
        };
        let id = s.ingest(&styled("<b>same words</b>"), 10).unwrap().id();
        s.ingest(&styled("<i>same words</i>"), 20).unwrap();
        s.ingest(&styled("<u>same words</u>"), 5).unwrap();
        let reps = s.get(id).unwrap().unwrap().reps;
        assert_eq!(reps[1].data, b"<i>same words</i>");
        assert_eq!(s.recent(1).unwrap()[0].last_used_at, 20);

        let key = "TOKEN=ghp_Zx9Yw8Vu7Ts6Rq5Po4Nm3Lk2Ji1Hg0FeDcBa99";
        let plain = add(&mut s, key, None, 30);
        assert_eq!(ids(s.search("Rq5Po4", 10).unwrap()), [plain]);
        let mut detected = Capture::from_text(key.into());
        crate::secrets::annotate(&mut detected, || None);
        assert_eq!(s.ingest(&detected, 40).unwrap(), Ingest::Duplicate(plain));
        assert!(s.search("Rq5Po4", 10).unwrap().is_empty());
        let keys = s.list_by_name(KEYS_LIST).unwrap().unwrap();
        assert_eq!(ids(s.list_items(&keys, 10, 50).unwrap()), [plain]);
    }

    #[test]
    fn smart_lists_filter_with_rules() {
        let mut s = store();
        let home = add(&mut s, "https://home.example", Some("com.home"), 1_000);
        add(&mut s, "https://work.example", Some("com.work"), 2_000);
        add(&mut s, "plain text", Some("com.home"), 3_000);
        let no_app = add(&mut s, "https://none.example", None, 4_000);

        let rule: Rule = serde_json::from_str(
            r#"{"type":"all","rules":[{"type":"kind","kind":"url"},{"type":"not","rule":{"type":"app","bundle_id":"com.work"}}]}"#,
        )
        .unwrap();
        s.create_list("Links", Some(&rule), SortType::Oldest)
            .unwrap();
        let links = s.list_by_name("Links").unwrap().unwrap();
        assert_eq!(links.rule.as_ref(), Some(&rule));
        assert_eq!(
            ids(s.list_items(&links, 10, 5_000).unwrap()),
            [home, no_app]
        );

        let home_links = Find {
            text: Some("exa"),
            kind: Some(Kind::Url),
            app: Some("com.home"),
            list: Some(&links),
        };
        assert_eq!(ids(s.find(&home_links, 10, 5_000).unwrap()), [home]);
        let names = |q| -> Vec<String> {
            s.apps(q)
                .unwrap()
                .into_iter()
                .map(|a| a.bundle_id)
                .collect()
        };
        assert_eq!(names(None), ["com.home", "com.work"]);
        assert_eq!(names(Some("WOR")), ["com.work"]);
        assert!(names(Some("com")).is_empty());

        let recent = PasteList {
            rule: Some(Rule::CopiedWithin { seconds: 2 }),
            sort: SortType::Recent,
            ..links
        };
        assert_eq!(s.list_items(&recent, 10, 5_000).unwrap().len(), 2);
        // A saved search is one `contains` per term, like the search itself.
        let saved = PasteList {
            rule: Some(Rule::All {
                rules: ["EXAMPLE", "home"]
                    .map(|t| Rule::Contains { text: t.into() })
                    .into(),
            }),
            ..recent
        };
        let found = ids(s.search("EXAMPLE home", 10).unwrap());
        assert_eq!(
            (ids(s.list_items(&saved, 10, 5_000).unwrap()), found),
            (vec![home], vec![home])
        );
    }

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
             ('text', 'meeting notes', NULL, 2, 2, 't', 2, 1, NULL, NULL);",
        )
        .unwrap();
        drop(conn);

        let s = Store::open(&path).unwrap();
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

    #[test]
    fn exclusions_and_settings_persist() {
        let mut s = store();
        s.add_exclude("com.agilebits.onepassword7").unwrap();
        assert!(s.is_excluded("com.agilebits.onepassword7").unwrap());
        assert!(!s.is_excluded("com.apple.Safari").unwrap());
        assert!(s.remove_exclude("com.agilebits.onepassword7").unwrap());
        assert!(s.excludes().unwrap().is_empty());

        let settings = Settings {
            detect_keys: true,
            max_items: None,
            ..Settings::default()
        };
        s.save_settings(&settings).unwrap();
        assert_eq!(s.settings().unwrap(), settings);
    }
}
