//! Settings (one JSON row) and apps excluded from recording.

use rusqlite::OptionalExtension;

use super::Store;
use super::items::upsert_app;
use crate::Result;
use crate::model::{AppInfo, Settings};

impl Store {
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

    /// Saves settings; turning key detection on scans the stored history.
    pub fn save_settings(&mut self, settings: &Settings) -> Result<()> {
        let was_on = self.settings()?.detect_keys;
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES ('settings', ?1) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [serde_json::to_string(settings)?],
        )?;
        if settings.detect_keys && !was_on {
            self.rescan_keys()?;
        }
        Ok(())
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
}

#[cfg(test)]
mod tests {
    use super::super::test_support::store;
    use super::*;

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
