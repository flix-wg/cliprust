//! Detected keys and IDs: masking and filing them, marking them by hand,
//! the Keys window's queries and the one-time scan of older history.

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params, params_from_iter};
use serde::{Deserialize, Serialize};

use super::items::{INDEXED_TEXT_LIMIT, truncate};
use super::keys::{hide_key, mark_scrub, merge_into, seal_item};
use super::lists::{IDS_LIST, KEYS_LIST, add_member};
use super::reps::{insert_reps, load_reps, wipe};
use super::{OPTIMIZE_FTS, SUMMARY_SQL, Store};
use crate::model::{
    AppInfo, Capture, ItemSummary, Kind, Secret, SecretKind, plain_checksum, text_of,
};
use crate::rules::SortType;
use crate::{Error, Result, contacts, secrets};

/// Raised when detection learns a new kind of key; 2 added passwords.
const KEY_SCAN: u32 = 2;

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

impl Store {
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

    /// Files text item `id` as a key whatever detection says: masked, sealed
    /// like a detected key and kept a key by later copies and rescans.
    /// [`Error::Locked`] while the key that seals keys is unavailable.
    /// Returns the id that remains: a copy of a key already stored merges into it.
    pub fn mark_key(&mut self, id: i64, now: i64) -> Result<i64> {
        let keys = &self.keys;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        type Row = (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        );
        let (kind, old_kind, title, domain, app): Row = tx.query_row(
            "SELECT p.kind, p.secret_kind, p.title, p.source_domain, a.name FROM paste_items p \
             LEFT JOIN applications a ON a.id = p.app_id WHERE p.id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
        if !matches!(Kind::parse(&kind), Some(Kind::Text | Kind::Url)) {
            return Err(Error::Refused("only text can be marked as a key".into()));
        }
        let old_kind = old_kind.as_deref().and_then(SecretKind::parse);
        let sealer = keys.sealer(&tx)?;
        let mut reps = load_reps(&tx, keys, id)?;
        let filed = (|| -> Result<bool> {
            if old_kind == Some(SecretKind::Key) {
                return hide_key(&tx, id, &reps);
            }
            let masked = text_of(&reps)
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(secrets::mask_marked)
                .ok_or_else(|| Error::Refused("the item has no text".into()))?;
            let site = domain.as_deref().map(secrets::site_name);
            let secret = Secret {
                kind: SecretKind::Key,
                name: site
                    .as_ref()
                    .map_or("Key".to_owned(), |s| format!("{s} key")),
                provider: site.or(app).unwrap_or_else(|| "Marked by hand".to_owned()),
                masked,
            };
            apply_secret(&tx, id, old_kind, &secret, None, now)?;
            // The row shows the masked value; only an ID's made-up title is replaced.
            if old_kind.is_none() {
                tx.execute(
                    "UPDATE paste_items SET title = ?2 WHERE id = ?1",
                    params![id, title],
                )?;
            }
            tx.execute(OPTIMIZE_FTS, [])?;
            hide_key(&tx, id, &reps)?;
            Ok(true)
        })();
        wipe(&mut reps);
        // A key the rescan filed still waits in plaintext for `encrypt_pending`.
        let mut changed = filed?;
        let mut kept = id;
        if let Some(sealer) = &sealer
            && tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM paste_item_data WHERE item_id = ?1 AND key_gen IS NOT ?2)",
                params![id, sealer.0],
                |r| r.get::<_, bool>(0),
            )?
        {
            kept = seal_item(&tx, keys, id, sealer)?;
            changed = true;
        }
        if changed {
            mark_scrub(&tx)?;
        }
        tx.execute(
            "UPDATE paste_items SET secret_override = 1 WHERE id = ?1",
            [kept],
        )?;
        tx.commit()?;
        if changed {
            self.scrub()?;
        }
        Ok(kept)
    }

    /// Files key item `id` as plain text again, stored unsealed and
    /// searchable, and detection leaves it alone from now on. Returns the id
    /// that remains: the same text stored before merges with it.
    pub fn unmark_key(&mut self, id: i64) -> Result<i64> {
        let keys = &self.keys;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (kind, secret_kind): (String, Option<String>) = tx.query_row(
            "SELECT kind, secret_kind FROM paste_items WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut kept = id;
        if let (Some(kind), Some("key")) = (Kind::parse(&kind), secret_kind.as_deref()) {
            let mut reps = load_reps(&tx, keys, id)?;
            let checksum = plain_checksum(kind, reps.first().map_or(&[][..], |r| &r.data)) as i64;
            let twin: Option<i64> = tx
                .query_row(
                    "SELECT id FROM paste_items WHERE checksum = ?1 AND id != ?2",
                    params![checksum, id],
                    |r| r.get(0),
                )
                .optional()?;
            let result = match twin {
                Some(into) => merge_into(&tx, id, into).map(|()| into),
                None => (|| {
                    let text = text_of(&reps).map(|t| truncate(t, INDEXED_TEXT_LIMIT));
                    let contact = text.and_then(|t| contacts::join(&contacts::detect(t)));
                    // A title that detection made up from the provider goes too.
                    tx.execute(
                        "UPDATE paste_items SET text = ?2, checksum = ?3, contact = ?4, \
                         title = CASE WHEN title = secret_provider OR substr(title, 1, \
                         length(secret_provider) + 1) = secret_provider || ' ' THEN NULL ELSE title END, \
                         secret_kind = NULL, secret_provider = NULL WHERE id = ?1",
                        params![id, text, checksum, contact],
                    )?;
                    tx.execute("DELETE FROM paste_item_data WHERE item_id = ?1", [id])?;
                    insert_reps(&tx, id, &reps, None)?;
                    Ok(id)
                })(),
            };
            wipe(&mut reps);
            kept = result?;
            tx.execute(
                "DELETE FROM paste_list_items WHERE item_id = ?1 \
                 AND list_id = (SELECT id FROM paste_lists WHERE name = ?2)",
                params![kept, KEYS_LIST],
            )?;
        }
        tx.execute(
            "UPDATE paste_items SET secret_override = 0 WHERE id = ?1",
            [kept],
        )?;
        tx.commit()?;
        Ok(kept)
    }

    /// History stored before key detection was on by default is scanned
    /// once, and once more whenever detection learns a new kind of key.
    pub(super) fn scan_keys_once(&mut self) -> Result<()> {
        let done: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'keys_scanned'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if done.and_then(|v| v.parse().ok()).unwrap_or(0) < KEY_SCAN {
            if self.settings()?.detect_keys {
                self.rescan_keys()?;
            }
            self.conn.execute(
                "INSERT INTO settings (key, value) VALUES ('keys_scanned', ?1) \
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                [KEY_SCAN.to_string()],
            )?;
        }
        Ok(())
    }

    /// Runs key detection over text and URL clips not yet filed as a key or
    /// ID, masking and filing what it finds as `ingest` does. Items keep their
    /// place in history. Returns the number of items upgraded.
    pub fn rescan_keys(&mut self) -> Result<usize> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        type Row = (
            i64,
            String,
            i64,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        );
        let rows: Vec<Row> = tx
            .prepare(
                "SELECT p.id, p.kind, p.last_used_at, p.source_url, p.source_title, a.bundle_id, a.name \
                 FROM paste_items p LEFT JOIN applications a ON a.id = p.app_id \
                 WHERE p.kind IN ('text', 'url') AND p.secret_kind IS NULL \
                 AND p.secret_override IS NULL",
            )?
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut found = 0;
        for (id, kind, last_used_at, source_url, source_title, bundle_id, name) in rows {
            let Some(kind) = Kind::parse(&kind) else {
                continue;
            };
            let mut cap = Capture {
                kind,
                base_uniform_type: String::new(),
                reps: load_reps(&tx, &self.keys, id)?,
                app: bundle_id.map(|bundle_id| AppInfo {
                    bundle_id,
                    name,
                    path: None,
                }),
                source_url,
                source_title,
                secret: None,
            };
            secrets::annotate(&mut cap, || None);
            if let Some(secret) = &cap.secret {
                let domain = cap.source_url.as_deref().and_then(secrets::domain);
                if apply_secret(&tx, id, None, secret, domain.as_deref(), last_used_at)? {
                    found += 1;
                }
                // Detection cut a key out of the page fields.
                if secret.kind == SecretKind::Key {
                    tx.execute(
                        "UPDATE paste_items SET source_url = ?2, source_title = ?3 WHERE id = ?1",
                        params![id, cap.source_url, cap.source_title],
                    )?;
                }
            }
        }
        if found > 0 {
            tx.execute(OPTIMIZE_FTS, [])?;
        }
        tx.commit()?;
        Ok(found)
    }
}

/// Masks a newly detected secret into item `id` and files it; a key found in
/// an item filed as an ID replaces the ID. Returns whether the item changed,
/// after which the caller runs [`OPTIMIZE_FTS`].
pub(super) fn apply_secret(
    conn: &Connection,
    id: i64,
    old_kind: Option<SecretKind>,
    secret: &Secret,
    domain: Option<&str>,
    now: i64,
) -> rusqlite::Result<bool> {
    let upgrade = match old_kind {
        None => true,
        Some(old) => old == SecretKind::Id && secret.kind == SecretKind::Key,
    };
    if !upgrade {
        return Ok(false);
    }
    conn.execute(
        "UPDATE paste_items SET text = ?1, \
         title = CASE WHEN secret_kind = 'id' THEN ?2 ELSE coalesce(title, ?2) END, \
         secret_provider = ?3, secret_kind = ?4, source_domain = coalesce(?5, source_domain), \
         contact = NULL \
         WHERE id = ?6",
        params![
            secret.masked,
            secret.name,
            secret.provider,
            secret.kind.as_str(),
            domain,
            id
        ],
    )?;
    if old_kind.is_some() {
        conn.execute(
            "DELETE FROM paste_list_items WHERE item_id = ?1 \
             AND list_id = (SELECT id FROM paste_lists WHERE name = ?2)",
            params![id, IDS_LIST],
        )?;
    }
    file_secret(conn, id, secret.kind, now)?;
    Ok(true)
}

/// Adds the item to the Keys or IDs list.
pub(super) fn file_secret(
    conn: &Connection,
    item_id: i64,
    kind: SecretKind,
    now: i64,
) -> rusqlite::Result<()> {
    let list = match kind {
        SecretKind::Key => KEYS_LIST,
        SecretKind::Id => IDS_LIST,
    };
    add_member(conn, list, item_id, now)
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

#[cfg(test)]
mod tests {
    use super::super::Ingest;
    use super::super::test_support::{add, ids, store};
    use super::*;
    use crate::model::{Rep, format};

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
        let reps = s.get(id).unwrap().unwrap().reps.clone();
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

        // Stored while detection was off; a rescan masks it in place.
        let older = add(&mut s, "ghp_Aa1Bb2Cc3Dd4Ee5Ff6Gg7Hh8Ii9Jj0Kk1Ll2", None, 35);
        assert_eq!(ids(s.search("Hh8Ii9", 10).unwrap()), [older]);
        assert_eq!(s.rescan_keys().unwrap(), 1);
        let item = s.summary(older).unwrap().unwrap();
        assert_eq!(
            (item.secret_kind, item.last_used_at),
            (Some(SecretKind::Key), 35)
        );
        assert!(!item.preview.unwrap().contains("Hh8Ii9"));
        assert!(s.search("Hh8Ii9", 10).unwrap().is_empty());
        assert_eq!(s.rescan_keys().unwrap(), 0);
    }

    #[test]
    fn marking_by_hand_seals_or_unseals_and_outlasts_detection() {
        use super::super::keys::tests::{VALUE, on_disk};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let keys = std::sync::Arc::new(crate::crypto::MemoryKeys::default());
        let mut s = Store::open_with_keys(&path, Some(keys)).unwrap();
        let password = "Zq8wX3vLp-staging";
        let id = add(&mut s, password, None, 1);
        s.set_note(id, Some("Zq8wX3vLp-staging for the replica"))
            .unwrap();
        assert_eq!(s.mark_key(id, 2).unwrap(), id);
        assert!(!on_disk(&path, "Zq8wX3vLp"));
        assert!(s.search("Zq8wX3", 10).unwrap().is_empty());
        assert_eq!(s.get(id).unwrap().unwrap().text(), Some(password));
        // Detection sees plain text here, but the marked value stays the key.
        assert_eq!(
            s.ingest(&Capture::from_text(password.into()), 3).unwrap(),
            Ingest::Duplicate(id)
        );

        // An HTML-only clip quoted in its note, a credential in its own page
        // URL, and a key the rescan filed but left in plaintext.
        let mut html = Capture::from_text(String::new());
        html.reps = vec![Rep::new(
            format::HTML,
            b"<b>Hq7xZ2mPw9</b> for prod".to_vec(),
        )];
        let html = s.ingest(&html, 6).unwrap().id();
        s.set_note(html, Some("see <b>Hq7xZ2mPw9</b> for prod"))
            .unwrap();
        let url = "https://admin:Tk9xQ2mLp7Vz@db.example.com/x";
        let mut link = Capture::from_text(url.into());
        link.source_url = Some(url.into());
        let link = s.ingest(&link, 7).unwrap().id();
        let filed = add(
            &mut s,
            "TOKEN=ghp_Aa1Bb2Cc3Dd4Ee5Ff6Gg7Hh8Ii9Jj0Kk1Ll2",
            None,
            8,
        );
        let quote = "rotate ghp_Aa1Bb2Cc3Dd4Ee5Ff6Gg7Hh8Ii9Jj0Kk1Ll2 soon";
        s.set_note(filed, Some(quote)).unwrap();
        assert_eq!(s.rescan_keys().unwrap(), 1);
        assert_eq!(s.encryption_status().unwrap().pending, 1);
        for id in [html, link, filed] {
            s.mark_key(id, 9).unwrap();
        }
        assert_eq!(s.encryption_status().unwrap().pending, 0);
        let mut again = Capture::from_text(url.into());
        again.source_url = Some(url.into());
        assert_eq!(s.ingest(&again, 10).unwrap(), Ingest::Duplicate(link));
        for secret in ["Hq7xZ2mPw9", "Tk9xQ2mLp7Vz", "Cc3Dd4Ee5Ff6"] {
            assert!(!on_disk(&path, secret), "{secret}");
            assert!(s.search(secret, 10).unwrap().is_empty(), "{secret}");
        }
        let link = s.summary(link).unwrap().unwrap();
        assert_eq!(
            (link.source_url.as_deref(), link.source_domain.as_deref()),
            (Some("https://db.example.com"), Some("example.com"))
        );

        let mut key = Capture::from_text(VALUE.into());
        crate::secrets::annotate(&mut key, || None);
        let key_id = s.ingest(&key, 4).unwrap().id();
        assert_eq!(s.unmark_key(key_id).unwrap(), key_id);
        assert_eq!(ids(s.search(&VALUE[8..20], 10).unwrap()), [key_id]);
        assert_eq!(s.ingest(&key, 5).unwrap(), Ingest::Duplicate(key_id));
        assert_eq!(s.rescan_keys().unwrap(), 0);
        let item = s.summary(key_id).unwrap().unwrap();
        assert_eq!((item.secret_kind, item.last_used_at), (None, 5));
        assert_eq!(s.encryption_status().unwrap().pending, 0);
    }
}
