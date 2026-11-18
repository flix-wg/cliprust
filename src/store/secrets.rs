//! Detected keys and IDs: masking and filing them, the Keys window's
//! queries and the one-time scan of older history.

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params, params_from_iter};
use serde::{Deserialize, Serialize};

use super::lists::{IDS_LIST, KEYS_LIST, add_member};
use super::reps::load_reps;
use super::{OPTIMIZE_FTS, SUMMARY_SQL, Store};
use crate::model::{AppInfo, Capture, ItemSummary, Kind, Secret, SecretKind};
use crate::rules::SortType;
use crate::{Result, secrets};

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

    /// History stored before key detection was on by default is scanned once.
    pub(super) fn scan_keys_once(&mut self) -> Result<()> {
        let done = self
            .conn
            .query_row(
                "SELECT 1 FROM settings WHERE key = 'keys_scanned'",
                [],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !done {
            if self.settings()?.detect_keys {
                self.rescan_keys()?;
            }
            self.conn.execute(
                "INSERT OR IGNORE INTO settings (key, value) VALUES ('keys_scanned', '1')",
                [],
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
                 WHERE p.kind IN ('text', 'url') AND p.secret_kind IS NULL",
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
}
