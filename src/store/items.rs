//! Ingest, single items, edits, deletion and retention.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params, types::Value};

use super::keys::{needs_seal, seal_item};
use super::reps::{insert_reps, load_reps, wipe};
use super::secrets::{apply_secret, file_secret};
use super::{OPTIMIZE_FTS, SUMMARY_SQL, Store};
use crate::Result;
use crate::model::{AppInfo, Capture, Item, ItemSummary, Kind, SecretKind, Settings};
use crate::rules::SortType;
use crate::{contacts, schema, secrets};

/// FTS indexes at most this much of a clip; the full text stays in the payload.
const INDEXED_TEXT_LIMIT: usize = 64 * 1024;
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

impl Store {
    /// Stores a capture. A copy whose checksum is already known updates that
    /// item instead: a newer copy brings its formats and moves it to the top,
    /// and a newly detected key is masked and filed in the Keys list.
    ///
    /// A key is stored sealed with the master key and a keyed checksum. When
    /// that key is unavailable the key is not stored at all and this returns
    /// [`crate::Error::Locked`]; only without any key store (Linux) is it
    /// stored unsealed, as before.
    pub fn ingest(&mut self, cap: &Capture, now: i64) -> Result<Ingest> {
        let plain_checksum = cap.checksum() as i64;
        let is_key = cap
            .secret
            .as_ref()
            .is_some_and(|s| s.kind == SecretKind::Key);
        let keys = &self.keys;
        // Take the write lock up front so a concurrent writer cannot void
        // this transaction's snapshot between the lookup and the write.
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let sealer = if is_key { keys.sealer(&tx)? } else { None };
        let checksum = match &sealer {
            Some((_, k)) => k.checksum(
                cap.kind.as_str(),
                cap.reps.first().map_or(&[][..], |r| &r.data),
            ),
            None => plain_checksum,
        };
        let app_id = cap
            .app
            .as_ref()
            .map(|app| upsert_app(&tx, app, 1))
            .transpose()?;
        let byte_size: i64 = cap.reps.iter().map(|r| r.data.len() as i64).sum();
        let domain = cap.source_url.as_deref().and_then(secrets::domain);
        // A copy stored before it was recognised as a key has the plain checksum.
        let existing: Option<(i64, i64, Option<String>)> = tx
            .query_row(
                "SELECT id, last_used_at, secret_kind FROM paste_items WHERE checksum IN (?1, ?2) \
                 ORDER BY checksum = ?1 DESC LIMIT 1",
                [checksum, plain_checksum],
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
                // Most re-copies bring the same formats; rewriting them costs pages of WAL.
                let mut old = load_reps(&tx, keys, id)?;
                if old != cap.reps {
                    tx.execute("DELETE FROM paste_item_data WHERE item_id = ?1", [id])?;
                    insert_reps(&tx, id, &cap.reps, sealer.as_ref())?;
                }
                wipe(&mut old);
            }
            let old_kind = old_kind.as_deref().and_then(SecretKind::parse);
            if let Some(secret) = &cap.secret
                && apply_secret(&tx, id, old_kind, secret, domain.as_deref(), now)?
            {
                tx.execute(OPTIMIZE_FTS, [])?;
            }
            let id = match &sealer {
                Some(s) if needs_seal(&tx, id, checksum, s.0)? => seal_item(&tx, keys, id, s)?,
                _ => id,
            };
            Ingest::Duplicate(id)
        } else {
            let contact = match (&cap.secret, cap.kind) {
                (None, Kind::Text | Kind::Url) => cap
                    .text()
                    .and_then(|t| contacts::join(&contacts::detect(t))),
                _ => None,
            };
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
                 secret_kind, source_domain, contact) \
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
                    domain,
                    contact
                ],
            )?;
            let id = tx.last_insert_rowid();
            insert_reps(&tx, id, &cap.reps, sealer.as_ref())?;
            if let Some(secret) = &cap.secret {
                file_secret(&tx, id, secret.kind, now)?;
            }
            Ingest::Inserted(id)
        };
        tx.commit()?;
        Ok(outcome)
    }

    pub fn summary(&self, id: i64) -> Result<Option<ItemSummary>> {
        let sql = format!("{SUMMARY_SQL} WHERE p.id = ?");
        Ok(self.summaries(&sql, vec![Value::from(id)])?.pop())
    }

    pub fn get(&self, id: i64) -> Result<Option<Item>> {
        // One snapshot, so the summary (and whether it is a key) matches the payloads.
        let _snapshot = if self.conn.is_autocommit() {
            Some(self.conn.unchecked_transaction()?)
        } else {
            None
        };
        let Some(summary) = self.summary(id)? else {
            return Ok(None);
        };
        let reps = load_reps(&self.conn, &self.keys, id)?;
        Ok(Some(Item { summary, reps }))
    }

    pub fn touch(&self, id: i64, now: i64) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE paste_items SET last_used_at = ?1 WHERE id = ?2",
            params![now, id],
        )? > 0)
    }

    pub fn set_title(&self, id: i64, title: Option<&str>) -> Result<bool> {
        let title = self.masked_label(id, title)?;
        Ok(self.conn.execute(
            "UPDATE paste_items SET title = ?1 WHERE id = ?2",
            params![title, id],
        )? > 0)
    }

    /// Attaches a searchable comment to an item; `None` or blank removes it.
    pub fn set_note(&self, id: i64, note: Option<&str>) -> Result<bool> {
        let note = note.map(str::trim).filter(|n| !n.is_empty());
        let note = self.masked_label(id, note)?;
        Ok(self.conn.execute(
            "UPDATE paste_items SET note = ?1 WHERE id = ?2",
            params![note, id],
        )? > 0)
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
        let _ = schema::checkpoint(&self.conn);
        Ok(())
    }
}

pub(super) fn upsert_app(conn: &Connection, app: &AppInfo, copies: i64) -> rusqlite::Result<i64> {
    conn.query_row(
        "INSERT INTO applications (bundle_id, name, path, copy_count) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT (bundle_id) DO UPDATE SET name = coalesce(excluded.name, name), \
         path = coalesce(excluded.path, path), copy_count = copy_count + excluded.copy_count \
         RETURNING id",
        params![app.bundle_id, app.name, app.path, copies],
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
    use super::super::test_support::{add, ids, store};
    use super::*;
    use crate::model::{Rep, Secret, format};
    use crate::store::KEYS_LIST;

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
        let data_rows = |s: &Store| -> String {
            s.conn
                .query_row("SELECT group_concat(id) FROM paste_item_data", [], |r| {
                    r.get(0)
                })
                .unwrap()
        };
        let rows = data_rows(&s);
        assert_eq!(s.ingest(&again, 3).unwrap(), Ingest::Duplicate(hello));
        assert_eq!(data_rows(&s), rows);
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
            kind: SecretKind::Key,
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
}
