//! Keys at rest for key items: which generation seals them, minting the
//! first key, the locked state, sealing plaintext keys, rotation, recovery.
//!
//! Settings rows: `key_gen` (the generation that seals new keys),
//! `key_check:<gen>` (tells a wrong key from corrupt data; kept for older
//! generations until `prune_keys`), `key_old` (a generation a rotation still
//! has to move off) and `scrub_pending` (plaintext may linger in free pages
//! or the WAL until a VACUUM and a full checkpoint succeed; a counter, so a
//! scrub only clears what it covered).
//!
//! Rotation never deletes a key from the key store: copies and backups of
//! the database made before still need it. `prune_keys` does that on request.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use zeroize::Zeroizing;

use super::reps::{insert_reps, load_reps, wipe};
use super::{OPTIMIZE_FTS, Store, file};
use crate::crypto::{self, KeyError, KeySource, Subkeys};
use crate::model::{Rep, format};
use crate::{Error, Result, schema};

pub(super) const MISSING: &str = "ClipRust's key is missing from the Keychain";
const WRONG_KEY: &str = "the key in the Keychain does not belong to this database";
pub(super) const NO_KEY_STORE: &str = "this platform has no keychain for ClipRust's key";

/// The generation and subkeys that seal new payloads.
pub(super) type Sealer = (u32, Arc<Subkeys>);

/// How a generation's key reads.
pub(super) enum Probe {
    Ready(Arc<Subkeys>),
    Missing,
    Wrong,
    Unavailable(String),
}

pub(super) struct Keyring {
    pub(super) source: Option<Arc<dyn KeySource>>,
    cache: RefCell<HashMap<u32, Arc<Subkeys>>>,
}

impl Keyring {
    pub(super) fn new(source: Option<Arc<dyn KeySource>>) -> Self {
        Self {
            source,
            cache: RefCell::default(),
        }
    }

    pub(super) fn remember(&self, generation: u32, keys: Arc<Subkeys>) {
        self.cache.borrow_mut().insert(generation, keys);
    }

    fn forget(&self, generation: u32) {
        self.cache.borrow_mut().remove(&generation);
    }

    pub(super) fn probe(&self, conn: &Connection, generation: u32) -> Result<Probe> {
        if let Some(keys) = self.cache.borrow().get(&generation) {
            return Ok(Probe::Ready(keys.clone()));
        }
        let Some(source) = &self.source else {
            return Ok(Probe::Unavailable(NO_KEY_STORE.into()));
        };
        let master = match source.load(generation) {
            Ok(master) => master,
            Err(KeyError::Missing) => return Ok(Probe::Missing),
            Err(e) => return Ok(Probe::Unavailable(e.to_string())),
        };
        let keys = Arc::new(Subkeys::derive(&master));
        if let Some(tag) = setting(conn, &check_name(generation))?
            && tag != keys.check_tag()
        {
            return Ok(Probe::Wrong);
        }
        self.remember(generation, keys.clone());
        Ok(Probe::Ready(keys))
    }

    /// The subkeys of `generation`, or [`Error::Locked`].
    pub(super) fn get(&self, conn: &Connection, generation: u32) -> Result<Arc<Subkeys>> {
        match self.probe(conn, generation)? {
            Probe::Ready(keys) => Ok(keys),
            Probe::Missing => Err(Error::Locked(MISSING.into())),
            Probe::Wrong => Err(Error::Locked(WRONG_KEY.into())),
            Probe::Unavailable(why) => Err(Error::Locked(why)),
        }
    }

    /// The key that seals new key items; `None` where there is no key store
    /// (keys then stay plaintext, as before). The first key is minted here,
    /// and a new one only while no stored row needs the unreadable one.
    pub(super) fn sealer(&self, conn: &Connection) -> Result<Option<Sealer>> {
        if self.source.is_none() {
            return Ok(None);
        }
        let current = current_gen(conn)?;
        if let Some(generation) = current {
            match self.probe(conn, generation)? {
                Probe::Ready(keys) => return Ok(Some((generation, keys))),
                Probe::Unavailable(why) => return Err(Error::Locked(why)),
                Probe::Missing | Probe::Wrong => {}
            }
        }
        if any_sealed(conn)? {
            return Err(Error::Locked(MISSING.into()));
        }
        self.mint(conn).map(Some)
    }

    /// The current key if it reads; unlike [`Keyring::sealer`] it never mints one.
    pub(super) fn current(&self, conn: &Connection) -> Result<Option<Sealer>> {
        let Some(generation) = self.source.as_ref().and(current_gen(conn)?) else {
            return Ok(None);
        };
        Ok(match self.probe(conn, generation)? {
            Probe::Ready(keys) => Some((generation, keys)),
            _ => None,
        })
    }

    /// Adds a new generation to the key store and makes it current. Holds
    /// the write lock, so two processes never both mint.
    fn mint(&self, conn: &Connection) -> Result<Sealer> {
        let source = self
            .source
            .as_ref()
            .ok_or_else(|| Error::Locked(NO_KEY_STORE.into()))?;
        let own = conn.is_autocommit();
        if own {
            conn.execute_batch("BEGIN IMMEDIATE")?;
        }
        let minted = (|| {
            // Generations are never reused, also not across databases that
            // share the keychain, so adding one never replaces a key in use.
            let used: Option<i64> = conn.query_row(
                "SELECT max(g) FROM (SELECT max(key_gen) AS g FROM paste_item_data \
                 UNION ALL SELECT CAST(value AS INTEGER) FROM settings \
                 WHERE key IN ('key_gen', 'key_old') \
                 UNION ALL SELECT CAST(substr(key, 11) AS INTEGER) FROM settings \
                 WHERE key LIKE 'key_check:%')",
                [],
                |r| r.get(0),
            )?;
            let stored = source.generations()?.into_iter().max().unwrap_or(0);
            let mut generation = stored.max(used.unwrap_or(0).max(0) as u32) + 1;
            let master = crypto::new_master();
            loop {
                match source.add(generation, &master) {
                    Ok(()) => break,
                    Err(KeyError::Exists) if generation < u32::MAX => generation += 1,
                    Err(e) => return Err(Error::from(e)),
                }
            }
            let keys = Arc::new(Subkeys::derive(&master));
            set_setting(conn, &check_name(generation), &keys.check_tag())?;
            set_setting(conn, "key_gen", &generation.to_string())?;
            self.remember(generation, keys.clone());
            Ok((generation, keys))
        })();
        if own {
            conn.execute_batch(if minted.is_ok() { "COMMIT" } else { "ROLLBACK" })?;
        }
        minted
    }
}

/// Whether the master key is usable, as `encryption_status` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyState {
    /// The current key reads.
    Ready,
    /// No key yet; the first stored key creates one.
    NoKey,
    /// The key is missing or wrong while stored keys need it.
    Locked,
    /// The keychain refused or was cancelled; nothing is assumed missing.
    Unavailable,
    /// No key store on this platform: keys are stored as before.
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EncryptionStatus {
    pub keys: KeyState,
    pub generation: Option<u32>,
    pub message: Option<String>,
    /// Key items not yet sealed with the current key.
    pub pending: i64,
    /// Key items whose key is missing or wrong.
    pub unreadable: i64,
    pub db_encrypted: bool,
    pub db_encryption_available: bool,
}

impl Store {
    pub fn encryption_status(&self) -> Result<EncryptionStatus> {
        let generation = current_gen(&self.conn)?;
        let (keys, message) = match (&self.keys.source, generation) {
            (None, _) => (KeyState::Unsupported, None),
            (Some(_), None) if any_sealed(&self.conn)? => {
                (KeyState::Locked, Some(MISSING.to_owned()))
            }
            (Some(_), None) => (KeyState::NoKey, None),
            (Some(_), Some(g)) => match self.keys.probe(&self.conn, g)? {
                Probe::Ready(_) => (KeyState::Ready, None),
                Probe::Missing => (KeyState::Locked, Some(MISSING.to_owned())),
                Probe::Wrong => (KeyState::Locked, Some(WRONG_KEY.to_owned())),
                Probe::Unavailable(why) => (KeyState::Unavailable, Some(why)),
            },
        };
        let mut unreadable = 0;
        for g in self.sealed_generations()? {
            if matches!(
                self.keys.probe(&self.conn, g)?,
                Probe::Missing | Probe::Wrong
            ) {
                unreadable += self.conn.query_row(
                    "SELECT count(DISTINCT item_id) FROM paste_item_data WHERE key_gen = ?1",
                    [g],
                    |r| r.get::<_, i64>(0),
                )?;
            }
        }
        let pending = if self.keys.source.is_some() {
            self.pending_ids(generation)?.len() as i64
        } else {
            0
        };
        Ok(EncryptionStatus {
            keys,
            generation,
            message,
            pending,
            unreadable,
            db_encrypted: self.db_gen.is_some(),
            db_encryption_available: cfg!(target_os = "macos") && self.keys.source.is_some(),
        })
    }

    /// Seals key items still stored in plaintext or under an older key, then
    /// rewrites the file so the plaintext is gone from free pages and the
    /// WAL. Also finishes an interrupted rotation. Returns how many it sealed.
    pub fn encrypt_pending(&mut self) -> Result<usize> {
        if self.keys.source.is_none() {
            return Ok(0);
        }
        let mut sealed = 0;
        if !self.pending_ids(current_gen(&self.conn)?)?.is_empty() {
            let keys = &self.keys;
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(sealer) = keys.sealer(&tx)? {
                let ids = pending_ids(&tx, Some(sealer.0))?;
                for id in ids {
                    match seal_item(&tx, keys, id, &sealer) {
                        Ok(_) => sealed += 1,
                        // Under an unreadable older key: left for "forget".
                        Err(Error::Locked(_)) => {}
                        Err(e) => return Err(e),
                    }
                }
            }
            if sealed > 0 {
                mark_scrub(&tx)?;
            }
            tx.commit()?;
        }
        if sealed > 0 {
            self.scrub()?;
        }
        self.finish_rotation()?;
        Ok(sealed)
    }

    /// Replaces the master key: adds a new generation, re-seals every key
    /// item in one transaction (rows carry their generation, so a crash
    /// leaves a readable mix) and re-keys an encrypted file. The old key
    /// stays in the key store for older copies; see [`Store::prune_keys`].
    /// Returns the new generation.
    pub fn rotate_key(&mut self) -> Result<u32> {
        // The re-sealed rows must reach the disk before anything relies on them.
        self.conn.pragma_update(None, "synchronous", "FULL")?;
        let rotated = self.rotate();
        self.conn.pragma_update(None, "synchronous", "NORMAL")?;
        rotated
    }

    fn rotate(&mut self) -> Result<u32> {
        if self.keys.source.is_none() {
            return Err(Error::Locked(NO_KEY_STORE.into()));
        }
        self.finish_rotation()?;
        if setting(&self.conn, "key_old")?.is_some() {
            return Err(Error::Refused(UNFINISHED.into()));
        }
        let keys = &self.keys;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (old, _) = keys
            .sealer(&tx)?
            .ok_or_else(|| Error::Locked(NO_KEY_STORE.into()))?;
        let had_plain: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM paste_item_data d JOIN paste_items p ON p.id = d.item_id \
             WHERE p.secret_kind = 'key' AND d.key_gen IS NULL)",
            [],
            |r| r.get(0),
        )?;
        let sealer = keys.mint(&tx)?;
        set_setting(&tx, "key_old", &old.to_string())?;
        if had_plain {
            mark_scrub(&tx)?;
        }
        let ids: Vec<i64> = tx
            .prepare(
                "SELECT DISTINCT d.item_id FROM paste_item_data d \
                 JOIN paste_items p ON p.id = d.item_id WHERE p.secret_kind = 'key'",
            )?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for id in ids {
            match seal_item(&tx, keys, id, &sealer) {
                Ok(_) | Err(Error::Locked(_)) => {}
                Err(e) => return Err(e),
            }
        }
        tx.commit()?;
        if had_plain {
            self.scrub()?;
        } else {
            let _ = schema::checkpoint(&self.conn);
        }
        self.finish_rotation()?;
        Ok(sealer.0)
    }

    /// Deletes from the key store the older generations this database no
    /// longer uses, once that is durable in the file. Copies and backups
    /// made before then open only with their own recovery code. Returns the
    /// generations deleted.
    pub fn prune_keys(&mut self) -> Result<Vec<u32>> {
        let source = self
            .keys
            .source
            .clone()
            .ok_or_else(|| Error::Locked(NO_KEY_STORE.into()))?;
        self.finish_rotation()?;
        if setting(&self.conn, "key_old")?.is_some() {
            return Err(Error::Refused(UNFINISHED.into()));
        }
        if !schema::checkpoint(&self.conn) {
            return Err(Error::Refused(
                "the database is busy; quit other ClipRust processes and try again".into(),
            ));
        }
        // A switch's rename that is not on disk could come back after a
        // crash as the file before it, which needs its older key.
        if let Some(path) = &self.path {
            file::sync_dir(path).map_err(|e| {
                Error::Refused(format!(
                    "could not flush the database folder to disk ({e}); no key was deleted"
                ))
            })?;
        }
        let current = current_gen(&self.conn)?;
        let mut used = self.sealed_generations()?;
        used.extend(file::unsynced_swaps(&self.conn)?);
        let known: Vec<u32> = self
            .conn
            .prepare("SELECT CAST(substr(key, 11) AS INTEGER) FROM settings WHERE key LIKE 'key_check:%'")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut pruned = Vec::new();
        for g in known {
            if Some(g) == current || Some(g) == self.db_gen || used.contains(&g) {
                continue;
            }
            source.delete(g)?;
            self.keys.forget(g);
            self.conn
                .execute("DELETE FROM settings WHERE key = ?1", [check_name(g)])?;
            pruned.push(g);
        }
        Ok(pruned)
    }

    /// The current master key as a recovery code (the app gates this behind
    /// Touch ID; the CLI asks first).
    pub fn recovery_code(&self) -> Result<Zeroizing<String>> {
        let source = self
            .keys
            .source
            .as_ref()
            .ok_or_else(|| Error::Locked(NO_KEY_STORE.into()))?;
        let generation = current_gen(&self.conn)?
            .ok_or_else(|| Error::Refused("ClipRust has not created a key yet".into()))?;
        // Checks the key against the database before handing it out.
        self.keys.get(&self.conn, generation)?;
        let master = source.load(generation)?;
        Ok(crypto::recovery_code(generation, &master))
    }

    /// Puts a key from a recovery code back into the key store, after
    /// checking that it belongs to this database. Returns its generation.
    pub fn restore_key(&mut self, code: &str) -> Result<u32> {
        let source = self
            .keys
            .source
            .clone()
            .ok_or_else(|| Error::Locked(NO_KEY_STORE.into()))?;
        let (generation, master) = crypto::parse_recovery_code(code)
            .ok_or_else(|| Error::Refused("that is not a ClipRust recovery code".into()))?;
        match setting(&self.conn, &check_name(generation))? {
            Some(tag) if tag == Subkeys::derive(&master).check_tag() => {}
            Some(_) => {
                return Err(Error::Refused(
                    "this recovery code belongs to a different database".into(),
                ));
            }
            None => {
                return Err(Error::Refused(format!(
                    "this database has no key generation {generation}"
                )));
            }
        }
        add_or_confirm(source.as_ref(), generation, &master)?;
        self.keys.forget(generation);
        Ok(generation)
    }

    /// Deletes the key items whose key is missing or wrong (never ones the
    /// keychain merely refused), so a new key can be created. Returns how
    /// many items it deleted.
    pub fn forget_unreadable_keys(&mut self) -> Result<usize> {
        let generations = self.sealed_generations()?;
        let keys = &self.keys;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut deleted = 0;
        for g in generations {
            match keys.probe(&tx, g)? {
                Probe::Ready(_) => {}
                Probe::Unavailable(why) => return Err(Error::Locked(why)),
                Probe::Missing | Probe::Wrong => {
                    deleted += tx.execute(
                        "DELETE FROM paste_items WHERE id IN \
                         (SELECT item_id FROM paste_item_data WHERE key_gen = ?1)",
                        [g],
                    )?;
                }
            }
        }
        if let Some(g) = current_gen(&tx)?
            && matches!(keys.probe(&tx, g)?, Probe::Missing | Probe::Wrong)
        {
            tx.execute("DELETE FROM settings WHERE key = 'key_gen'", [])?;
        }
        tx.commit()?;
        if deleted > 0 {
            self.scrub()?;
        }
        Ok(deleted)
    }

    /// Finishes moving off the generation a rotation replaced; the key itself
    /// stays in the key store.
    fn finish_rotation(&mut self) -> Result<()> {
        let Some(old) = setting(&self.conn, "key_old")?.and_then(|v| v.parse::<u32>().ok()) else {
            return Ok(());
        };
        let current = current_gen(&self.conn)?;
        // Busy while other processes have the file open; retried next start.
        if self.db_gen.is_some()
            && self.db_gen != current
            && let Some(sealer) = self.keys.sealer(&self.conn)?
            && self.swap(Some(&sealer)).is_err()
        {
            return Ok(());
        }
        let in_use: bool = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM paste_item_data WHERE key_gen = ?1)",
            [old],
            |r| r.get(0),
        )?;
        if in_use || self.db_gen == Some(old) {
            return Ok(());
        }
        self.conn
            .execute("DELETE FROM settings WHERE key = 'key_old'", [])?;
        Ok(())
    }

    /// Rewrites the file so replaced plaintext leaves no trace in free pages
    /// or the WAL (`secure_delete` covers single deletes). Stays pending,
    /// and is retried on the next open, until the checkpoint completes.
    pub(super) fn scrub(&self) -> Result<()> {
        let seen = setting(&self.conn, SCRUB_PENDING)?;
        self.conn.execute_batch("VACUUM")?;
        if schema::checkpoint(&self.conn) {
            clear_scrub(&self.conn, seen.as_deref())?;
        }
        Ok(())
    }

    /// Finishes a scrub that a crash or a busy checkpoint interrupted.
    pub(super) fn resume_scrub(&self) -> Result<()> {
        if setting(&self.conn, SCRUB_PENDING)?.is_some() {
            self.scrub()?;
        }
        Ok(())
    }

    /// `value` with a key item's own value replaced by its masked form, so
    /// a title or note never holds the key in plaintext (or in the index).
    /// [`Error::Locked`] while the key cannot be read, since the value could
    /// not be checked.
    pub(super) fn masked_label(&self, id: i64, value: Option<&str>) -> Result<Option<String>> {
        let Some(value) = value else { return Ok(None) };
        let masked: Option<String> = self
            .conn
            .query_row(
                "SELECT text FROM paste_items WHERE id = ?1 AND secret_kind = 'key'",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let Some(masked) = masked else {
            return Ok(Some(value.to_owned()));
        };
        let mut reps = load_reps(&self.conn, &self.keys, id)?;
        let out = match key_text(&reps) {
            Some(secret) => value.replace(secret, &masked),
            None => value.to_owned(),
        };
        wipe(&mut reps);
        Ok(Some(out))
    }

    fn sealed_generations(&self) -> Result<Vec<u32>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT key_gen FROM paste_item_data WHERE key_gen IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn pending_ids(&self, current: Option<u32>) -> Result<Vec<i64>> {
        pending_ids(&self.conn, current)
    }
}

/// Key items with a payload in plaintext or under another generation.
fn pending_ids(conn: &Connection, current: Option<u32>) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT d.item_id FROM paste_item_data d JOIN paste_items p ON p.id = d.item_id \
         WHERE p.secret_kind = 'key' AND (d.key_gen IS NULL OR d.key_gen IS NOT ?1)",
    )?;
    let rows = stmt.query_map([current], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Whether item `id` still needs [`seal_item`] to carry `checksum` and `generation`.
pub(super) fn needs_seal(
    conn: &Connection,
    id: i64,
    checksum: i64,
    generation: u32,
) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT checksum != ?2 OR EXISTS (SELECT 1 FROM paste_item_data \
         WHERE item_id = ?1 AND key_gen IS NOT ?3) FROM paste_items WHERE id = ?1",
        params![id, checksum, generation],
        |r| r.get(0),
    )?)
}

/// Re-writes item `id`'s payloads sealed by `sealer`, with its keyed
/// checksum. If another item already has that checksum, it is the same key:
/// this one is merged into it. Returns the id that remains.
pub(super) fn seal_item(
    conn: &Connection,
    keys: &Keyring,
    id: i64,
    sealer: &Sealer,
) -> Result<i64> {
    let kind: String = conn.query_row("SELECT kind FROM paste_items WHERE id = ?1", [id], |r| {
        r.get(0)
    })?;
    let mut reps = load_reps(conn, keys, id)?;
    if let Some(secret) = key_text(&reps)
        && conn.execute(
            "UPDATE paste_items SET title = replace(title, ?2, text), note = replace(note, ?2, text) \
             WHERE id = ?1 AND (instr(title, ?2) > 0 OR instr(note, ?2) > 0)",
            params![id, secret],
        )? > 0
    {
        conn.execute(OPTIMIZE_FTS, [])?;
    }
    let checksum = sealer
        .1
        .checksum(&kind, reps.first().map_or(&[][..], |r| &r.data));
    let twin: Option<i64> = conn
        .query_row(
            "SELECT id FROM paste_items WHERE checksum = ?1 AND id != ?2",
            params![checksum, id],
            |r| r.get(0),
        )
        .optional()?;
    let result = match twin {
        Some(into) => merge_into(conn, id, into).map(|()| into),
        None => (|| {
            conn.execute(
                "UPDATE paste_items SET checksum = ?1 WHERE id = ?2",
                params![checksum, id],
            )?;
            conn.execute("DELETE FROM paste_item_data WHERE item_id = ?1", [id])?;
            insert_reps(conn, id, &reps, Some(sealer))?;
            Ok(id)
        })(),
    };
    wipe(&mut reps);
    result
}

/// Folds a duplicate item into `into`: lists, note and recency are kept.
pub(super) fn merge_into(conn: &Connection, from: i64, into: i64) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO paste_list_items (list_id, item_id, added_at) \
         SELECT list_id, ?2, added_at FROM paste_list_items WHERE item_id = ?1",
        params![from, into],
    )?;
    conn.execute(
        "UPDATE paste_items SET \
         note = coalesce(note, (SELECT note FROM paste_items WHERE id = ?1)), \
         last_used_at = max(last_used_at, (SELECT last_used_at FROM paste_items WHERE id = ?1)) \
         WHERE id = ?2",
        params![from, into],
    )?;
    conn.execute("DELETE FROM paste_items WHERE id = ?1", [from])?;
    Ok(())
}

/// Adds a restored key, or accepts it if the same key is already there.
pub(super) fn add_or_confirm(
    source: &dyn KeySource,
    generation: u32,
    master: &[u8; 32],
) -> Result<()> {
    match source.add(generation, master) {
        Ok(()) => Ok(()),
        Err(KeyError::Exists) if *source.load(generation)? == *master => Ok(()),
        Err(KeyError::Exists) => Err(Error::Refused(format!(
            "a different key for generation {generation} is already in the Keychain"
        ))),
        Err(e) => Err(e.into()),
    }
}

/// The plain-text value of a key item.
fn key_text(reps: &[Rep]) -> Option<&str> {
    reps.iter()
        .find(|r| r.format == format::TEXT)
        .and_then(|r| std::str::from_utf8(&r.data).ok())
        .filter(|s| !s.is_empty())
}

const SCRUB_PENDING: &str = "scrub_pending";

/// Flags a scrub, counting up so a scrub already running does not clear it.
pub(super) fn mark_scrub(conn: &Connection) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, '1') ON CONFLICT (key) \
         DO UPDATE SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT)",
        [SCRUB_PENDING],
    )?;
    Ok(())
}

/// Clears the scrub flag only if it is still the value `seen` before the scrub.
fn clear_scrub(conn: &Connection, seen: Option<&str>) -> Result<()> {
    conn.execute(
        "DELETE FROM settings WHERE key = ?1 AND value IS ?2",
        params![SCRUB_PENDING, seen],
    )?;
    Ok(())
}
const UNFINISHED: &str =
    "the previous key rotation is not finished; quit other ClipRust processes and try again";

fn current_gen(conn: &Connection) -> Result<Option<u32>> {
    Ok(setting(conn, "key_gen")?.and_then(|v| v.parse().ok()))
}

fn any_sealed(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM paste_item_data WHERE key_gen IS NOT NULL)",
        [],
        |r| r.get(0),
    )?)
}

fn check_name(generation: u32) -> String {
    format!("key_check:{generation}")
}

fn setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .optional()?)
}

fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) \
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use std::path::Path;

    use rusqlite::Connection;

    use super::*;
    use crate::crypto::MemoryKeys;
    use crate::model::{Capture, Secret, SecretKind};

    pub const VALUE: &str = "sk-proj-Q7wErTy9UiOp3AsDfGh5JkLzXcVbNm1Q2w";

    pub fn key_capture(value: &str) -> Capture {
        let mut cap = Capture::from_text(value.into());
        cap.secret = Some(Secret {
            kind: SecretKind::Key,
            provider: "OpenAI API key".into(),
            name: "OPENAI_API_KEY".into(),
            masked: "sk-pro…2w".into(),
        });
        cap
    }

    /// Whether `needle` occurs anywhere in the database file or its WAL.
    pub fn on_disk(db: &Path, needle: &str) -> bool {
        ["", "-wal"].iter().any(|suffix| {
            let mut p = db.as_os_str().to_owned();
            p.push(suffix);
            std::fs::read(p)
                .is_ok_and(|bytes| bytes.windows(needle.len()).any(|w| w == needle.as_bytes()))
        })
    }

    fn source() -> Arc<dyn KeySource> {
        Arc::new(MemoryKeys::default())
    }

    #[test]
    fn keys_are_sealed_on_disk_unsearchable_and_bound_to_their_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let keys = source();
        let mut s = Store::open_with_keys(&path, Some(keys)).unwrap();
        let id = s.ingest(&key_capture(VALUE), 1).unwrap().id();
        let other = s
            .ingest(&key_capture(&VALUE.replace('Q', "R")), 2)
            .unwrap()
            .id();
        assert!(!on_disk(&path, &VALUE[8..]));
        assert_eq!(s.get(id).unwrap().unwrap().text(), Some(VALUE));
        for term in [&VALUE[8..20], "pro", "sk"] {
            assert!(s.search(term, 10).unwrap().is_empty(), "{term}");
        }
        assert_eq!(s.encryption_status().unwrap().keys, KeyState::Ready);
        // The same key again is found by its keyed checksum.
        assert_eq!(
            s.ingest(&key_capture(VALUE), 5).unwrap(),
            crate::Ingest::Duplicate(id)
        );

        s.conn
            .execute(
                "UPDATE paste_item_data SET payload = \
                 (SELECT payload FROM paste_item_data WHERE item_id = ?1) WHERE item_id = ?2",
                [id, other],
            )
            .unwrap();
        assert!(matches!(s.get(other), Err(Error::Corrupt(_))));
    }

    #[test]
    fn plaintext_keys_from_v3_are_sealed_and_scrubbed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v3.db");
        let conn = Connection::open(&path).unwrap();
        for v in [schema::V1, schema::V2, schema::V3] {
            conn.execute_batch(v).unwrap();
        }
        let packed = zstd::bulk::compress(VALUE.as_bytes(), 3).unwrap();
        conn.execute(
            "INSERT INTO paste_items (kind, text, title, created_at, last_used_at, base_uniform_type, \
             checksum, byte_size, secret_provider, secret_kind, note) VALUES \
             ('text', 'sk-pro…2w', 'OPENAI_API_KEY', 1, 1, 't', ?1, ?2, 'OpenAI API key', 'key', ?3)",
            rusqlite::params![
                key_capture(VALUE).checksum() as i64,
                VALUE.len() as i64,
                format!("staging {VALUE}")
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO paste_item_data (item_id, position, format, payload, raw_size) \
             VALUES (1, 0, 'text', ?1, ?2)",
            rusqlite::params![packed, VALUE.len() as i64],
        )
        .unwrap();
        drop(conn);
        assert!(on_disk(&path, VALUE));

        let keys = source();
        let mut s = Store::open_with_keys(&path, Some(keys.clone())).unwrap();
        assert_eq!(s.encryption_status().unwrap().pending, 1);
        assert_eq!(s.encrypt_pending().unwrap(), 1);
        assert_eq!(s.encryption_status().unwrap().pending, 0);
        let note = s.summary(1).unwrap().unwrap().note;
        assert_eq!(note.as_deref(), Some("staging sk-pro…2w"));
        drop(s);
        assert!(!on_disk(&path, VALUE));

        let mut s = Store::open_with_keys(&path, Some(keys)).unwrap();
        assert_eq!(s.get(1).unwrap().unwrap().text(), Some(VALUE));
        // A copy made before detection (plain checksum) and after both land on it.
        assert_eq!(
            s.ingest(&key_capture(VALUE), 2).unwrap(),
            crate::Ingest::Duplicate(1)
        );
        assert!(s.search("OPENAI", 10).unwrap().len() == 1);
    }

    #[test]
    fn a_scrub_keeps_the_flag_a_later_seal_raised() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();
        mark_scrub(&conn).unwrap();
        let seen = setting(&conn, SCRUB_PENDING).unwrap();
        mark_scrub(&conn).unwrap();
        clear_scrub(&conn, seen.as_deref()).unwrap();
        assert!(setting(&conn, SCRUB_PENDING).unwrap().is_some());
    }

    #[test]
    fn a_missing_key_locks_without_deleting_or_replacing_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let keys = source();
        let open = || Store::open_with_keys(&path, Some(keys.clone())).unwrap();
        let mut s = open();
        let id = s.ingest(&key_capture(VALUE), 1).unwrap().id();
        assert_eq!(s.rotate_key().unwrap(), 2);
        assert_eq!(keys.generations().unwrap().len(), 2);
        assert_eq!(s.prune_keys().unwrap(), [1]);
        assert_eq!(keys.generations().unwrap(), [2]);
        assert_eq!(s.get(id).unwrap().unwrap().text(), Some(VALUE));
        let code = s.recovery_code().unwrap();
        drop(s);

        keys.delete(2).unwrap();
        let mut s = open();
        let status = s.encryption_status().unwrap();
        assert_eq!((status.keys, status.unreadable), (KeyState::Locked, 1));
        assert!(matches!(s.get(id), Err(Error::Locked(_))));
        assert!(matches!(s.set_note(id, Some(VALUE)), Err(Error::Locked(_))));
        assert!(matches!(
            s.ingest(
                &key_capture("ghp_Zx9Yw8Vu7Ts6Rq5Po4Nm3Lk2Ji1Hg0FeDcBa99"),
                2
            ),
            Err(Error::Locked(_))
        ));
        assert!(keys.generations().unwrap().is_empty());
        assert_eq!(s.recent(10).unwrap().len(), 1);
        assert!(s.restore_key("cliprust-key-2-00").is_err());

        assert_eq!(s.restore_key(&code).unwrap(), 2);
        assert_eq!(s.get(id).unwrap().unwrap().text(), Some(VALUE));
        drop(s);

        keys.delete(2).unwrap();
        let mut s = open();
        assert_eq!(s.forget_unreadable_keys().unwrap(), 1);
        assert!(s.recent(10).unwrap().is_empty());
        s.ingest(&key_capture(VALUE), 3).unwrap();
        let status = s.encryption_status().unwrap();
        assert_eq!((status.keys, status.generation), (KeyState::Ready, Some(3)));
    }
}
