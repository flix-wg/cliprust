//! Manual lists (members added by hand) and smart lists (a [`Rule`]).

use rusqlite::{Connection, params};

use super::{Find, Store};
use crate::model::ItemSummary;
use crate::rules::{Rule, SortType};
use crate::{Error, Result};

pub const KEYS_LIST: &str = "Keys";
/// IDs get their own list so that Keys holds only masked secrets.
pub const IDS_LIST: &str = "IDs";
/// The manual list item pinning adds to; no smart list may take its name.
pub const PINNED_LIST: &str = "Pinned";

#[derive(Debug, Clone, PartialEq)]
pub struct PasteList {
    pub id: i64,
    pub name: String,
    /// `None` for a manual list whose members are added by hand.
    pub rule: Option<Rule>,
    pub sort: SortType,
    pub sort_weight: f64,
}

impl Store {
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
        add_member(&self.conn, name, item_id, now)?;
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
}

/// Adds an item to the manual list `name`, creating the list if needed.
pub(super) fn add_member(
    conn: &Connection,
    name: &str,
    item_id: i64,
    now: i64,
) -> rusqlite::Result<()> {
    let list_id: i64 = conn.query_row(
        "INSERT INTO paste_lists (name, sort_weight) \
         VALUES (?1, (SELECT coalesce(max(sort_weight), 0) + 1 FROM paste_lists)) \
         ON CONFLICT (name) DO UPDATE SET name = name RETURNING id",
        [name],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO paste_list_items (list_id, item_id, added_at) VALUES (?1, ?2, ?3)",
        params![list_id, item_id, now],
    )?;
    Ok(())
}

fn reserved() -> Error {
    Error::Refused(format!("{PINNED_LIST:?} is reserved for pinned items"))
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{add, ids, store};
    use super::*;
    use crate::model::Kind;

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
            contact: None,
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
}
