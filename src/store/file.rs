//! The database file: plaintext or SQLCipher, switching between the two,
//! and noticing when another process replaced it.
//!
//! - The mode is read from the header: `SQLite format 3\0` is plaintext,
//!   anything else is encrypted. An encrypted file is opened with the `db`
//!   subkey of whichever stored generation opens it, and never re-created.
//! - Switching exports to `.<name>.cliprust-swap` beside the database
//!   (a name no user database has), checks the copy, and renames it over
//!   the database. It needs the only connection to the file (otherwise a
//!   stale connection could later delete the new file's WAL by name), and
//!   holds `<db>.lock` exclusively so nobody opens the old file meanwhile;
//!   every open holds that lock shared for the moment it takes.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{Connection, OpenFlags};

use super::Store;
use super::keys::{Keyring, NO_KEY_STORE, Sealer};
use crate::crypto::{KeyError, Subkeys};
use crate::{Error, Result, schema};

const PLAIN_HEADER: &[u8; 16] = b"SQLite format 3\0";
const ENCRYPTED_NO_KEY: &str = "the database is encrypted and its key is missing from the Keychain";
const OTHERS_OPEN: &str = "another ClipRust process has the database open; quit `cliprust watch` (or the app) and try again";

/// Holds `<db>.lock`: shared while opening, exclusive while switching.
pub(super) struct OpenLock(#[allow(dead_code)] File);

impl OpenLock {
    pub(super) fn shared(db: &Path) -> Result<Self> {
        let file = lock_file(db)?;
        file.lock_shared()?;
        Ok(Self(file))
    }

    fn exclusive(db: &Path) -> Result<Self> {
        let file = lock_file(db)?;
        file.lock()?;
        Ok(Self(file))
    }
}

fn lock_file(db: &Path) -> Result<File> {
    Ok(File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(with_suffix(db, ".lock"))?)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// The copy a switch writes before renaming it over `db`.
fn swap_path(db: &Path) -> PathBuf {
    let mut name = std::ffi::OsString::from(".");
    name.push(db.file_name().unwrap_or_default());
    name.push(".cliprust-swap");
    db.with_file_name(name)
}

/// Deletes the copy an interrupted switch left behind (in plaintext, when it
/// was decrypting). Takes `<db>.lock`, so no switch is running.
pub(super) fn remove_stale_copy(db: &Path, _held: &OpenLock) -> Result<()> {
    let tmp = swap_path(db);
    for stale in [with_suffix(&tmp, "-journal"), tmp] {
        match std::fs::remove_file(stale) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    Ok(())
}

/// Flushes the directory holding `db`, so a rename in it is on disk.
pub(super) fn sync_dir(db: &Path) -> std::io::Result<()> {
    let dir = match db.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    File::open(dir)?.sync_all()
}

/// Settings rows `swap_unsynced:<gen>`: generations of files a switch
/// replaced without a directory sync, which a crash could bring back.
const UNSYNCED_SWAP: &str = "swap_unsynced:";

/// The generations [`UNSYNCED_SWAP`] rows keep.
pub(super) fn unsynced_swaps(conn: &Connection) -> Result<Vec<u32>> {
    Ok(conn
        .prepare("SELECT CAST(substr(key, 15) AS INTEGER) FROM settings WHERE key LIKE 'swap_unsynced:%'")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// `(dev, ino)` of the file at `path`.
pub(super) fn id(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.dev(), meta.ino()))
}

/// Whether the file holds an encrypted database. Missing or empty is plaintext.
pub fn is_encrypted(path: &Path) -> Result<bool> {
    let mut header = [0u8; 16];
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    let mut read = 0;
    while read < header.len() {
        match file.read(&mut header[read..])? {
            0 => break,
            n => read += n,
        }
    }
    // A short file is not a database either way; SQLite reports it.
    Ok(read == header.len() && &header != PLAIN_HEADER)
}

/// Opens the file at `path` as it is: plaintext, or encrypted with a stored key.
pub(super) fn connect(path: &Path, keys: &Keyring) -> Result<(Connection, Option<u32>)> {
    if !is_encrypted(path)? {
        return Ok((Connection::open(path)?, None));
    }
    if !cfg!(target_os = "macos") {
        return Err(Error::Locked(
            "the database is encrypted; only the macOS build can open it".into(),
        ));
    }
    let source = keys
        .source
        .as_ref()
        .ok_or_else(|| Error::Locked(NO_KEY_STORE.into()))?;
    let mut generations = source.generations()?;
    generations.sort_unstable_by(|a, b| b.cmp(a));
    let mut refused = None;
    for generation in generations {
        let master = match source.load(generation) {
            Ok(master) => master,
            Err(KeyError::Unavailable(why)) => {
                refused = Some(why);
                continue;
            }
            Err(_) => continue,
        };
        let sub = Arc::new(Subkeys::derive(&master));
        if let Some(conn) = try_key(path, &sub)? {
            keys.remember(generation, sub);
            return Ok((conn, Some(generation)));
        }
    }
    Err(Error::Locked(
        refused.unwrap_or_else(|| ENCRYPTED_NO_KEY.into()),
    ))
}

/// Opens an existing encrypted file with `keys`, or `None` if they do not fit.
fn try_key(path: &Path, keys: &Subkeys) -> Result<Option<Connection>> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::default().difference(OpenFlags::SQLITE_OPEN_CREATE),
    )?;
    apply_key(&conn, keys)?;
    match conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(())) {
        Ok(()) => Ok(Some(conn)),
        Err(e) if e.sqlite_error_code() == Some(rusqlite::ErrorCode::NotADatabase) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn apply_key(conn: &Connection, keys: &Subkeys) -> Result<()> {
    let sql = zeroize::Zeroizing::new(format!("PRAGMA key = \"{}\";", keys.db_key().as_str()));
    conn.execute_batch(&sql)?;
    Ok(())
}

/// Checks a key from a recovery code against an encrypted file that no
/// stored key opens.
pub(super) fn opens_with(path: &Path, master: &[u8; 32]) -> Result<bool> {
    Ok(try_key(path, &Subkeys::derive(master))?.is_some())
}

impl Store {
    /// Reopens the file if another process replaced it (one `stat`).
    pub fn revalidate(&mut self) -> Result<()> {
        let Some(path) = self.path.clone() else {
            return Ok(());
        };
        let now = id(&path);
        if now.is_none() || now == self.file_id {
            return Ok(());
        }
        keep_wal_on_close(&self.conn);
        self.reconnect(&path, true)
    }

    /// Replaces the connection with a fresh one to whatever is at `path`.
    fn reconnect(&mut self, path: &Path, lock: bool) -> Result<()> {
        let _lock = if lock {
            Some(OpenLock::shared(path)?)
        } else {
            None
        };
        let (conn, db_gen) = connect(path, &self.keys)?;
        schema::configure(&conn)?;
        schema::migrate(&conn)?;
        self.conn = conn;
        self.db_gen = db_gen;
        self.file_id = id(path);
        Ok(())
    }

    /// Encrypts the whole database file with the `db` subkey, or decrypts it.
    /// Fails while another process has the file open.
    pub fn set_db_encryption(&mut self, on: bool) -> Result<()> {
        if on == self.db_gen.is_some() {
            return Ok(());
        }
        if !cfg!(target_os = "macos") {
            return Err(Error::Refused(
                "database encryption needs the macOS build".into(),
            ));
        }
        let sealer = if on {
            Some(
                self.keys
                    .sealer(&self.conn)?
                    .ok_or_else(|| Error::Locked(NO_KEY_STORE.into()))?,
            )
        } else {
            None
        };
        self.swap(sealer.as_ref())
    }

    /// Rewrites the file encrypted with `sealer`'s `db` key, or in plaintext.
    pub(super) fn swap(&mut self, sealer: Option<&Sealer>) -> Result<()> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| Error::Refused("an in-memory database has no file".into()))?;
        let lock = OpenLock::exclusive(&path)?;
        let tmp = swap_path(&path);
        let replaced = self.db_gen;
        let synced = match self.export(&path, &tmp, sealer, &lock) {
            Ok(synced) => synced,
            Err(e) => {
                let _ = remove_stale_copy(&path, &lock);
                // Back to shared locking and WAL on the unchanged file.
                self.conn.execute_batch("PRAGMA locking_mode = NORMAL")?;
                self.conn
                    .query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))?;
                schema::configure(&self.conn)?;
                return Err(e);
            }
        };
        // The old connection now points at the replaced file; open the new one.
        // Should that fail, the old one must not delete the new WAL by name,
        // and the next call retries through `revalidate`.
        if let Err(e) = self.reconnect(&path, false) {
            keep_wal_on_close(&self.conn);
            self.file_id = None;
            return Err(e);
        }
        // Without the directory sync a crash could still bring back the file
        // just replaced, so `prune_keys` keeps its key until a later switch
        // syncs.
        if synced {
            self.conn
                .execute("DELETE FROM settings WHERE key LIKE 'swap_unsynced:%'", [])?;
        } else if let Some(g) = replaced {
            self.conn.execute(
                "INSERT OR IGNORE INTO settings (key, value) VALUES (?1, '')",
                [format!("{UNSYNCED_SWAP}{g}")],
            )?;
        }
        Ok(())
    }

    /// Returns whether the directory sync after the rename succeeded.
    fn export(
        &self,
        path: &Path,
        tmp: &Path,
        sealer: Option<&Sealer>,
        lock: &OpenLock,
    ) -> Result<bool> {
        remove_stale_copy(path, lock)?;
        let conn = &self.conn;
        // Leaving WAL mode needs the only connection; exclusive locking then
        // keeps everyone else out until this connection closes.
        conn.busy_timeout(std::time::Duration::from_millis(200))?;
        conn.execute_batch("PRAGMA locking_mode = EXCLUSIVE")?;
        let mode: String = conn
            .query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))
            .map_err(|e| match e.sqlite_error_code() {
                Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                    Error::Refused(OTHERS_OPEN.into())
                }
                _ => e.into(),
            })?;
        if !mode.eq_ignore_ascii_case("delete") {
            return Err(Error::Refused(OTHERS_OPEN.into()));
        }
        let user_version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let auto_vacuum: i64 = conn.pragma_query_value(None, "auto_vacuum", |r| r.get(0))?;
        let tables: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND rootpage > 0 \
                 AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let counts = row_counts(conn, &tables)?;

        let key = sealer.map(|(_, k)| k.db_key()).unwrap_or_default();
        let tmp_str = tmp
            .to_str()
            .ok_or_else(|| Error::Refused("the database path is not UTF-8".into()))?;
        // An encrypted main file was opened without SQLITE_OPEN_CREATE, which
        // ATTACH inherits; an empty file counts as a new database.
        File::create(tmp)?;
        conn.execute("ATTACH DATABASE ?1 AS swap KEY ?2", [tmp_str, key.as_str()])?;
        let copied = (|| -> Result<()> {
            // sqlcipher_export copies schema and data but not these two.
            conn.execute_batch(&format!("PRAGMA swap.auto_vacuum = {auto_vacuum}"))?;
            conn.pragma_update(None, "foreign_keys", false)?;
            conn.query_row("SELECT sqlcipher_export('swap')", [], |_| Ok(()))?;
            conn.execute_batch(&format!("PRAGMA swap.user_version = {user_version}"))?;
            Ok(())
        })();
        let _ = conn.pragma_update(None, "foreign_keys", true);
        let detached = conn.execute_batch("DETACH DATABASE swap");
        copied?;
        detached?;

        let check = Connection::open_with_flags(
            tmp,
            OpenFlags::default().difference(OpenFlags::SQLITE_OPEN_CREATE),
        )?;
        if let Some((_, k)) = sealer {
            apply_key(&check, k)?;
            let problems: Vec<String> = check
                .prepare("PRAGMA cipher_integrity_check")?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            if !problems.is_empty() {
                return Err(Error::Corrupt(format!(
                    "the encrypted copy failed its check: {}",
                    problems.join("; ")
                )));
            }
        }
        let quick: String = check.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        let same = quick == "ok"
            && check.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))?
                == user_version
            && check.pragma_query_value(None, "auto_vacuum", |r| r.get::<_, i64>(0))?
                == auto_vacuum
            && row_counts(&check, &tables)? == counts;
        drop(check);
        if !same {
            return Err(Error::Corrupt(
                "the converted copy does not match the database".into(),
            ));
        }
        File::open(tmp)?.sync_all()?;
        std::fs::rename(tmp, path)?;
        // The switch has happened; a failed directory sync only makes the
        // rename less durable, so it must not send the caller down the
        // path that keeps the old connection.
        Ok(sync_dir(path).is_ok())
    }
}

fn row_counts(conn: &Connection, tables: &[String]) -> Result<Vec<i64>> {
    tables
        .iter()
        .map(|t| {
            let sql = format!("SELECT count(*) FROM \"{}\"", t.replace('"', "\"\""));
            Ok(conn.query_row(&sql, [], |r| r.get(0))?)
        })
        .collect()
}

/// Stops a connection to a replaced file from deleting, by name, the WAL and
/// shared-memory files that now belong to the new one when it closes.
fn keep_wal_on_close(conn: &Connection) {
    let mut on: std::ffi::c_int = 1;
    // SAFETY: the handle is live for the duration of the call, the schema
    // name is a C string, and PERSIST_WAL takes a pointer to an int.
    unsafe {
        rusqlite::ffi::sqlite3_file_control(
            conn.handle(),
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_PERSIST_WAL,
            (&raw mut on).cast(),
        );
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::super::keys::tests::{VALUE, key_capture, on_disk};
    use super::*;
    use crate::crypto::{KeySource, MemoryKeys};
    use crate::model::Capture;

    #[test]
    fn whole_file_encryption_round_trips_and_never_opens_without_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let keys: Arc<dyn KeySource> = Arc::new(MemoryKeys::default());
        let open = || Store::open_with_keys(&path, Some(keys.clone()));
        // Another database that happens to sit beside it is left alone.
        let neighbour = dir.path().join("t.db.tmp");
        std::fs::write(&neighbour, "another database").unwrap();
        let mut s = open().unwrap();
        let note = "meeting notes about the quarterly budget";
        s.ingest(&Capture::from_text(note.into()), 1).unwrap();
        let key = s.ingest(&key_capture(VALUE), 2).unwrap().id();

        s.set_db_encryption(true).unwrap();
        assert!(is_encrypted(&path).unwrap());
        assert!(!on_disk(&path, "quarterly budget"));
        assert_eq!(s.search("quarterly", 10).unwrap().len(), 1);
        // Rotation re-keys the file too; pruning then retires the old key.
        let generation = s.rotate_key().unwrap();
        assert_eq!(keys.generations().unwrap().len(), 2);
        s.prune_keys().unwrap();
        assert_eq!(keys.generations().unwrap(), [generation]);
        assert!(is_encrypted(&path).unwrap());
        assert_eq!(s.get(key).unwrap().unwrap().text(), Some(VALUE));

        // Without the key it refuses, and never creates a fresh database over it.
        let size = std::fs::metadata(&path).unwrap().len();
        let none: Arc<dyn KeySource> = Arc::new(MemoryKeys::default());
        assert!(matches!(
            Store::open_with_keys(&path, Some(none)),
            Err(Error::Locked(_))
        ));
        assert!(matches!(
            Store::open_with_keys(&path, None),
            Err(Error::Locked(_))
        ));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), size);

        // A second connection blocks switching; the file stays as it was.
        let other = open().unwrap();
        assert_eq!(other.search("budget", 10).unwrap().len(), 1);
        assert!(matches!(s.set_db_encryption(false), Err(Error::Refused(_))));
        assert!(is_encrypted(&path).unwrap());
        assert_eq!(s.search("budget", 10).unwrap().len(), 1);
        drop(other);

        s.set_db_encryption(false).unwrap();
        assert!(!is_encrypted(&path).unwrap());
        assert!(neighbour.exists());
        let v: i64 = s
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        let av: i64 = s
            .conn
            .pragma_query_value(None, "auto_vacuum", |r| r.get(0))
            .unwrap();
        assert_eq!((v, av), (schema::VERSION, 2));
        assert_eq!(s.get(key).unwrap().unwrap().text(), Some(VALUE));

        // A file replaced underneath an open store is picked up again.
        let mut stale = open().unwrap();
        let elsewhere = dir.path().join("new.db");
        let mut fresh = Store::open_with_keys(&elsewhere, None).unwrap();
        fresh
            .ingest(&Capture::from_text("replacement clip".into()), 3)
            .unwrap();
        drop(fresh);
        std::fs::rename(&elsewhere, &path).unwrap();
        stale.revalidate().unwrap();
        assert_eq!(stale.search("replacement", 10).unwrap().len(), 1);
    }
}
