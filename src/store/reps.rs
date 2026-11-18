//! The stored formats of an item. Every payload is written and read here, so
//! this is the one place that compresses (and would seal) clip content.

use rusqlite::{Connection, OptionalExtension, params};

use super::Store;
use crate::Result;
use crate::model::Rep;

const ZSTD_LEVEL: i32 = 3;

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
        let row: Option<(Vec<u8>, i64)> = self
            .conn
            .prepare_cached(
                "SELECT payload, raw_size FROM paste_item_data WHERE item_id = ?1 AND (format = ?2 \
                 OR (?2 = 'image' AND format IN ('public.png', 'public.tiff', 'public.jpeg', 'public.heic'))) \
                 ORDER BY position LIMIT 1",
            )?
            .query_row(params![id, format], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        row.map(|(payload, raw_size)| unpack(&payload, raw_size))
            .transpose()
    }
}

pub(super) fn insert_reps(conn: &Connection, item_id: i64, reps: &[Rep]) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        "INSERT INTO paste_item_data (item_id, position, format, payload, raw_size) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for (pos, rep) in reps.iter().enumerate() {
        stmt.execute(params![
            item_id,
            pos as i64,
            rep.format,
            zstd::bulk::compress(&rep.data, ZSTD_LEVEL)?,
            rep.data.len() as i64
        ])?;
    }
    Ok(())
}

pub(super) fn load_reps(conn: &Connection, item_id: i64) -> Result<Vec<Rep>> {
    let mut stmt = conn.prepare_cached(
        "SELECT format, payload, raw_size FROM paste_item_data WHERE item_id = ?1 ORDER BY position",
    )?;
    let rows = stmt.query_map([item_id], |r| {
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
            data: unpack(&payload, raw_size)?,
        });
    }
    Ok(reps)
}

fn unpack(payload: &[u8], raw_size: i64) -> Result<Vec<u8>> {
    Ok(zstd::bulk::decompress(payload, raw_size as usize)?)
}
