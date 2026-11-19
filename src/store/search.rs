//! Filtered listing of items, the apps they came from and contact counts.

use rusqlite::types::Value;
use serde::Serialize;

use super::{PasteList, SUMMARY_SQL, Store};
use crate::model::{ItemSummary, Kind};
use crate::rules::{CONTAINS_SQL, SortType, contact_sql};
use crate::{Contact, Result, contacts};

/// Narrows [`Store::find`]; `None` fields match everything.
#[derive(Debug, Clone, Copy, Default)]
pub struct Find<'a> {
    /// Whitespace-separated terms, all of which must occur as substrings.
    pub text: Option<&'a str>,
    pub kind: Option<Kind>,
    /// Bundle id of the app the item was copied from.
    pub app: Option<&'a str>,
    pub list: Option<&'a PasteList>,
    /// `Some(None)` matches items with any contact.
    pub contact: Option<Option<Contact>>,
    /// Lowercase domain of the page the item was copied from; see [`crate::query`].
    pub site: Option<&'a str>,
}

/// An app with the number of stored items copied from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppCount {
    pub bundle_id: String,
    pub name: Option<String>,
    pub count: i64,
}

impl Store {
    pub fn recent(&self, limit: usize) -> Result<Vec<ItemSummary>> {
        self.find(&Find::default(), limit, 0)
    }

    /// Plain-text search; see [`crate::query`] for `from:` and `is:` terms.
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
        if let Some(contact) = f.contact {
            clauses.push(contact_sql(contact, &mut params));
        }
        if let Some(site) = f.site {
            clauses.push(
                "p.source_domain LIKE ? ESCAPE '\\' OR ? LIKE '%.' || p.source_domain".into(),
            );
            let escaped = site
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            params.push(Value::from(format!("{escaped}%")));
            params.push(Value::from(site.to_owned()));
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

    /// Items per contact type, and with any contact under `"any"`.
    pub fn contact_counts(&self) -> Result<Vec<(&'static str, i64)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT contact, count(*) FROM paste_items WHERE contact IS NOT NULL GROUP BY 1",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        let mut out = vec![("phone", 0), ("email", 0), ("address", 0), ("any", 0)];
        for row in rows {
            let (contact, n) = row?;
            for c in contacts::split(Some(&contact)) {
                if let Some(slot) = out.iter_mut().find(|(name, _)| *name == c.as_str()) {
                    slot.1 += n;
                }
            }
            out[3].1 += n;
        }
        Ok(out)
    }
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

#[cfg(test)]
mod tests {
    use super::super::test_support::{add, ids, store};

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
}
