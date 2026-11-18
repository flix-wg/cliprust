//! The stored formats of an item. Every payload is written and read here, so
//! this is the one place that compresses and seals clip content.

use rusqlite::{Connection, OptionalExtension, params};
use zeroize::Zeroize;

use super::Store;
use super::keys::{Keyring, Sealer};
use crate::model::Rep;
use crate::{Error, Result, crypto};

const ZSTD_LEVEL: i32 = 3;

/// One stored payload as read from its row.
struct Stored {
    position: i64,
    format: String,
    payload: Vec<u8>,
    raw_size: i64,
    key_gen: Option<u32>,
}

impl Store {
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
        let row = self
            .conn
            .prepare_cached(
                "SELECT position, format, payload, raw_size, key_gen FROM paste_item_data \
                 WHERE item_id = ?1 AND (format = ?2 OR (?2 = 'image' AND format IN \
                 ('public.png', 'public.tiff', 'public.jpeg', 'public.heic'))) \
                 ORDER BY position LIMIT 1",
            )?
            .query_row(params![id, format], stored_from_row)
            .optional()?;
        row.map(|s| unpack(&self.conn, &self.keys, id, &s))
            .transpose()
    }
}

fn stored_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Stored> {
    Ok(Stored {
        position: r.get(0)?,
        format: r.get(1)?,
        payload: r.get(2)?,
        raw_size: r.get(3)?,
        key_gen: r.get(4)?,
    })
}

/// Writes an item's formats, sealed with `sealer` when given.
pub(super) fn insert_reps(
    conn: &Connection,
    item_id: i64,
    reps: &[Rep],
    sealer: Option<&Sealer>,
) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        "INSERT INTO paste_item_data (item_id, position, format, payload, raw_size, key_gen) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for (pos, rep) in reps.iter().enumerate() {
        let mut packed = zstd::bulk::compress(&rep.data, ZSTD_LEVEL)?;
        let (payload, key_gen) = match sealer {
            Some((key_gen, keys)) => {
                let sealed = keys.seal(&crypto::aad(item_id, pos as i64, &rep.format), &packed);
                packed.zeroize();
                (sealed, Some(*key_gen))
            }
            None => (packed, None),
        };
        stmt.execute(params![
            item_id,
            pos as i64,
            rep.format,
            payload,
            rep.data.len() as i64,
            key_gen
        ])?;
    }
    Ok(())
}

/// An item's formats, opened and decompressed.
pub(super) fn load_reps(conn: &Connection, keys: &Keyring, item_id: i64) -> Result<Vec<Rep>> {
    let mut stmt = conn.prepare_cached(
        "SELECT position, format, payload, raw_size, key_gen FROM paste_item_data \
         WHERE item_id = ?1 ORDER BY position",
    )?;
    let rows = stmt
        .query_map([item_id], stored_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut reps = Vec::with_capacity(rows.len());
    for s in rows {
        let data = unpack(conn, keys, item_id, &s)?;
        reps.push(Rep {
            format: s.format,
            data,
        });
    }
    Ok(reps)
}

/// Overwrites decrypted content before it is freed.
pub(super) fn wipe(reps: &mut [Rep]) {
    for rep in reps {
        rep.data.zeroize();
    }
}

fn unpack(conn: &Connection, keys: &Keyring, item_id: i64, s: &Stored) -> Result<Vec<u8>> {
    let size = s.raw_size as usize;
    let Some(key_gen) = s.key_gen else {
        return Ok(zstd::bulk::decompress(&s.payload, size)?);
    };
    let packed = keys
        .get(conn, key_gen)?
        .open(&crypto::aad(item_id, s.position, &s.format), &s.payload)
        .map_err(|_| Error::Corrupt("a stored key failed its integrity check".into()))?;
    Ok(zstd::bulk::decompress(&packed, size)?)
}
