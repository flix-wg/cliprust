//! Imports a PasteNow store (CoreData SQLite): history, lists and exclusions.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use serde_json::Value as Json;

use crate::Result;
use crate::model::{AppInfo, Capture, Kind, Rep, format};
use crate::rules::{Rule, SortType};
use crate::store::{Ingest, PINNED_LIST, Store};

/// Where CoreData keeps blobs too large to store inline, relative to the store.
pub const EXTERNAL_DATA_DIR: &str = ".PasteNow_SUPPORT/_EXTERNAL_DATA";

/// Seconds from the Unix epoch to 2001-01-01, CoreData's reference date.
const APPLE_EPOCH: f64 = 978_307_200.0;
/// Bound on a blob before and after decompression, so a damaged store
/// cannot exhaust memory.
const MAX_BLOB_BYTES: u64 = 64 << 20;
const TEXT_UTI: &str = "public.utf8-plain-text";
const FILE_UTI: &str = "public.file-url";
const IMAGE_UTIS: &[&str] = &["public.png", "public.tiff", "public.jpeg", "public.heic"];

const ITEMS_SQL: &str = "SELECT i.Z_PK, i.ZTIMESTAMP, i.ZBASEUNIFORMTYPE, i.ZTEXT, i.ZTITLE, i.ZLIST, \
     coalesce(i.ZAPPBUNDLEID, a.ZBUNDLEID), \
     (SELECT n.ZNAME FROM ZAPPLICATION n \
      WHERE n.ZBUNDLEID = coalesce(i.ZAPPBUNDLEID, a.ZBUNDLEID) ORDER BY n.Z_PK LIMIT 1), \
     d.ZDATA \
     FROM ZPASTEITEM i \
     LEFT JOIN ZPASTEITEMDATA d ON d.Z_PK = i.ZITEMDATA \
     LEFT JOIN ZAPPLICATION a ON a.Z_PK = i.ZAPP \
     ORDER BY i.ZTIMESTAMP, i.Z_PK";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub imported: usize,
    pub duplicates: usize,
    /// PasteNow item key and why it was not imported.
    pub skipped: Vec<(i64, String)>,
    pub lists_created: usize,
    /// List name and why it was not imported.
    pub lists_skipped: Vec<(String, String)>,
    pub excludes: usize,
}

struct SourceItem {
    timestamp: Option<f64>,
    base_type: Option<String>,
    text: Option<String>,
    bundle_id: Option<String>,
    app_name: Option<String>,
    data: Option<Vec<u8>>,
}

/// PasteNow's store in its group container.
pub fn default_store_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(
        "Library/Group Containers/KAQ3M6FSM7.app.pastenow.PasteNow/Documents/PasteNow.sqlite",
    ))
}

/// Imports from a private snapshot of a store that may still be in use.
pub fn import_live(store: &mut Store, path: Option<PathBuf>) -> Result<ImportReport> {
    let src = path
        .or_else(default_store_path)
        .filter(|p| p.is_file())
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no PasteNow store found")
        })?;
    // A fresh 0700 directory with a random name, removed when dropped.
    let tmp = tempfile::Builder::new()
        .prefix("cliprust-pastenow-")
        .tempdir()?;
    let copy = snapshot(&src, tmp.path())?;
    import(store, &copy)
}

/// Copies the store with its WAL and external blobs, so the live files are
/// never opened.
fn snapshot(src: &Path, dir: &Path) -> Result<PathBuf> {
    let name = src
        .file_name()
        .ok_or_else(|| std::io::Error::other("store path has no file name"))?;
    for suffix in ["", "-wal", "-shm"] {
        let mut file = name.to_owned();
        file.push(suffix);
        let from = src.with_file_name(&file);
        if suffix.is_empty() || from.exists() {
            std::fs::copy(&from, dir.join(&file))?;
        }
    }
    let external_dir = EXTERNAL_DATA_DIR;
    let external = src.with_file_name(external_dir);
    if external.is_dir() {
        let to = dir.join(external_dir);
        std::fs::create_dir_all(&to)?;
        for entry in std::fs::read_dir(&external)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                std::fs::copy(entry.path(), to.join(entry.file_name()))?;
            }
        }
    }
    Ok(dir.join(name))
}

/// Imports `PasteNow.sqlite`, oldest item first. Re-running adds nothing new:
/// items dedup on ingest and existing lists are left alone.
pub fn import(store: &mut Store, store_path: &Path) -> Result<ImportReport> {
    let src = Connection::open_with_flags(
        store_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let external = store_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(EXTERNAL_DATA_DIR);
    let detect_keys = store.settings()?.detect_keys;
    let mut report = ImportReport::default();
    let mut members: HashMap<i64, Vec<(i64, i64)>> = HashMap::new();

    let mut stmt = src.prepare(ITEMS_SQL)?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let pk: i64 = row.get(0)?;
        let title: Option<String> = row.get(4)?;
        let list: Option<i64> = row.get(5)?;
        let item = SourceItem {
            timestamp: row.get(1)?,
            base_type: row.get(2)?,
            text: row.get(3)?,
            bundle_id: row.get(6)?,
            app_name: row.get(7)?,
            data: row.get(8)?,
        };
        let (mut cap, now) = match to_capture(item, &external) {
            Ok(found) => found,
            Err(reason) => {
                report.skipped.push((pk, reason));
                continue;
            }
        };
        if detect_keys {
            crate::secrets::annotate(&mut cap, || None);
        }
        let id = match store.ingest(&cap, now)? {
            Ingest::Inserted(id) => {
                report.imported += 1;
                id
            }
            Ingest::Duplicate(id) => {
                report.duplicates += 1;
                id
            }
        };
        if cap.secret.is_none()
            && let Some(title) = title.as_deref().filter(|t| {
                !t.trim().is_empty() && !(detect_keys && crate::secrets::detect(t).is_some())
            })
        {
            // A key item that is locked keeps its title rather than failing the import.
            match store.set_title(id, Some(title)) {
                Ok(_) | Err(crate::Error::Locked(_)) => {}
                Err(e) => return Err(e),
            }
        }
        if let Some(list) = list {
            members.entry(list).or_default().push((id, now));
        }
    }
    drop(rows);

    import_lists(store, &src, &members, &mut report)?;

    let mut stmt = src.prepare(
        "SELECT a.ZBUNDLEID FROM ZEXCLUDERULE e JOIN ZAPPLICATION a ON a.Z_PK = e.ZAPP \
         WHERE e.ZENABLED AND a.ZBUNDLEID IS NOT NULL",
    )?;
    let bundles: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for bundle in bundles {
        store.add_exclude(&bundle)?;
        report.excludes += 1;
    }
    Ok(report)
}

fn import_lists(
    store: &Store,
    src: &Connection,
    members: &HashMap<i64, Vec<(i64, i64)>>,
    report: &mut ImportReport,
) -> Result<()> {
    // ZLISTTYPE -1 is the built-in "All" history.
    let mut stmt = src.prepare(
        "SELECT Z_PK, ZNAME, ZLISTTYPE, ZFILTERRULEDATA FROM ZPASTELIST \
         WHERE ZLISTTYPE >= 0 ORDER BY ZSORTWEIGHT, Z_PK",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let pk: i64 = row.get(0)?;
        let list_type: i64 = row.get(2)?;
        let filter: Option<Vec<u8>> = row.get(3)?;
        let mut name = row
            .get::<_, Option<String>>(1)?
            .unwrap_or_else(|| format!("PasteNow list {pk}"));
        let rule = match list_type {
            // PasteNow's pin list fills the built-in one.
            0 if name == "📌"
                || ["pinned", "pins"]
                    .iter()
                    .any(|n| name.trim().eq_ignore_ascii_case(n)) =>
            {
                name = PINNED_LIST.to_owned();
                None
            }
            0 => None,
            1 => match filter.as_deref().map(translate_filter) {
                Some(Ok(rule)) => match only_kind(&rule) {
                    Some(kind) => {
                        let chip = match kind {
                            Kind::Text => "Text",
                            Kind::Url => "Links",
                            Kind::Image => "Images",
                            Kind::File => "Files",
                        };
                        report
                            .lists_skipped
                            .push((name, format!("same as the built-in {chip} filter")));
                        continue;
                    }
                    None => Some(rule),
                },
                Some(Err(reason)) => {
                    report.lists_skipped.push((name, reason));
                    continue;
                }
                None => {
                    report
                        .lists_skipped
                        .push((name, "smart list without a filter".into()));
                    continue;
                }
            },
            other => {
                report
                    .lists_skipped
                    .push((name, format!("unknown list type {other}")));
                continue;
            }
        };
        match store.list_by_name(&name)? {
            Some(list) if list.rule != rule => {
                report
                    .lists_skipped
                    .push((name, "a different list with this name exists".into()));
                continue;
            }
            Some(_) => {}
            None if rule.is_some() && name == PINNED_LIST => {
                report
                    .lists_skipped
                    .push((name, "this name is reserved for pinned items".into()));
                continue;
            }
            None => {
                store.create_list(&name, rule.as_ref(), SortType::Recent)?;
                report.lists_created += 1;
            }
        }
        if rule.is_none() {
            for &(id, added) in members.get(&pk).into_iter().flatten() {
                store.add_to_list(&name, id, added)?;
            }
        }
    }
    Ok(())
}

/// The kind of a rule that only filters by one kind, which a type chip already does.
fn only_kind(rule: &Rule) -> Option<Kind> {
    match rule {
        Rule::Kind { kind } => Some(*kind),
        Rule::All { rules } | Rule::Any { rules } if rules.len() == 1 => only_kind(&rules[0]),
        _ => None,
    }
}

fn read_bounded(reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    reader.take(MAX_BLOB_BYTES + 1).read_to_end(&mut out)?;
    if out.len() as u64 > MAX_BLOB_BYTES {
        return Err(std::io::Error::other("larger than 64 MiB"));
    }
    Ok(out)
}

fn to_capture(item: SourceItem, external: &Path) -> std::result::Result<(Capture, i64), String> {
    let ts = item.timestamp.ok_or("no timestamp")?;
    let now = ((ts + APPLE_EPOCH) * 1000.0).round() as i64;
    let blob = item.data.ok_or("no pasteboard data")?;
    let payload: Cow<[u8]> = match blob.split_first() {
        Some((1, inline)) => Cow::Borrowed(inline),
        Some((2, reference)) => {
            let uuid = std::str::from_utf8(reference)
                .unwrap_or_default()
                .trim_end_matches('\0');
            if uuid.len() != 36 || !uuid.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
                return Err("malformed external data reference".into());
            }
            let file = std::fs::File::open(external.join(uuid))
                .map_err(|e| format!("external data {uuid}: {e}"))?;
            Cow::Owned(read_bounded(file).map_err(|e| format!("external data {uuid}: {e}"))?)
        }
        _ => return Err("unknown data envelope".into()),
    };
    let plist = if payload.starts_with(b"bplist00") {
        payload.into_owned()
    } else {
        let mut decoder = lzfse_rust::LzfseRingDecoder::default();
        read_bounded(decoder.reader_bytes(&payload)).map_err(|e| format!("decompress: {e}"))?
    };
    let entries = pasteboard_entries(&plist)?;

    let mut files = Vec::new();
    let (mut text, mut rtf, mut html) = (None, None, None);
    let mut images = Vec::new();
    let mut others = Vec::new();
    let mut source_url = None;
    for (ty, data) in entries {
        match ty.as_str() {
            TEXT_UTI if text.is_none() => text = Some(data),
            "public.rtf" if rtf.is_none() => rtf = Some(Rep::new(format::RTF, data)),
            "public.html" if html.is_none() => html = Some(Rep::new(format::HTML, data)),
            FILE_UTI => match file_url_path(&data) {
                Some(path) => files.push(path),
                None => others.push(Rep { format: ty, data }),
            },
            t if IMAGE_UTIS.contains(&t) => images.push(Rep::new(t, data)),
            _ => {
                if ty == "org.chromium.source-url" {
                    source_url = Some(
                        String::from_utf8_lossy(&data)
                            .trim_matches('\0')
                            .trim()
                            .to_owned(),
                    )
                    .filter(|u| !u.is_empty());
                }
                others.push(Rep { format: ty, data });
            }
        }
    }
    let base = item.base_type.as_deref().unwrap_or_default();
    // PasteNow's own text rendering stands in when the pasteboard had no plain text.
    if text.is_none() && !matches!(base, "public.image" | FILE_UTI) {
        text = item.text.filter(|t| !t.is_empty()).map(String::into_bytes);
    }

    let kind = match base {
        FILE_UTI if !files.is_empty() => Kind::File,
        "public.image" if !images.is_empty() => Kind::Image,
        _ if !files.is_empty() => Kind::File,
        _ if text.is_some() => Kind::Text,
        _ if !images.is_empty() => Kind::Image,
        _ => return Err("no text, file or image representation".into()),
    };
    let text_str = text
        .as_deref()
        .map(|t| String::from_utf8_lossy(t).into_owned());
    let mut files_rep =
        (!files.is_empty()).then(|| Rep::new(format::FILES, files.join("\n").into_bytes()));
    let mut text_rep = text_str.map(|t| Rep::new(format::TEXT, t.into_bytes()));

    let mut reps = Vec::new();
    let (kind, base_uniform_type) = match kind {
        Kind::File => {
            reps.extend(files_rep.take());
            (Kind::File, FILE_UTI.to_owned())
        }
        Kind::Image => {
            let image = images.remove(0);
            let ty = image.format.clone();
            reps.push(image);
            (Kind::Image, ty)
        }
        _ => {
            let text = text_rep
                .take()
                .unwrap_or_else(|| Rep::new(format::TEXT, Vec::new()));
            let kind = if base == "public.url" {
                Kind::Url
            } else {
                Capture::from_text(String::from_utf8_lossy(&text.data).into_owned()).kind
            };
            reps.push(text);
            (kind, TEXT_UTI.to_owned())
        }
    };
    reps.extend(files_rep);
    reps.extend(text_rep);
    reps.extend(rtf);
    reps.extend(html);
    reps.extend(images);
    reps.extend(others);

    let app = item.bundle_id.map(|bundle_id| AppInfo {
        bundle_id,
        name: item.app_name,
        path: None,
    });
    Ok((
        Capture {
            kind,
            base_uniform_type,
            reps,
            app,
            source_url,
            source_title: None,
            secret: None,
        },
        now,
    ))
}

/// The decompressed blob is a binary plist array of `{type, data}`, one per
/// pasteboard type.
fn pasteboard_entries(bytes: &[u8]) -> std::result::Result<Vec<(String, Vec<u8>)>, String> {
    let value = plist::Value::from_reader(Cursor::new(bytes)).map_err(|e| format!("plist: {e}"))?;
    let array = value.into_array().ok_or("plist is not an array")?;
    Ok(array
        .into_iter()
        .filter_map(|entry| {
            let mut dict = entry.into_dictionary()?;
            let ty = dict.remove("type")?.into_string()?;
            let data = dict.remove("data")?.into_data()?;
            Some((ty, data))
        })
        .collect())
}

fn file_url_path(data: &[u8]) -> Option<String> {
    let url = std::str::from_utf8(data).ok()?.trim_matches('\0').trim();
    let rest = url.strip_prefix("file://")?;
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok().filter(|p| p.starts_with('/'))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Filter {
    compose_type: i64,
    predicate_types: Vec<Json>,
}

/// Translates a smart-list filter, or says why it can't be. composeType's
/// values are unverified, so it only matters when predicates are mixed.
fn translate_filter(json: &[u8]) -> std::result::Result<Rule, String> {
    let filter: Filter = serde_json::from_slice(json).map_err(|e| format!("filter: {e}"))?;
    let mut rules = filter
        .predicate_types
        .iter()
        .map(translate_predicate)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    match rules.len() {
        0 => Err("filter has no predicates".into()),
        1 => Ok(rules.remove(0)),
        // Items have exactly one type, so several type predicates only make
        // sense as alternatives.
        _ if rules.iter().all(|r| matches!(r, Rule::Kind { .. })) => Ok(Rule::Any { rules }),
        _ => Err(format!(
            "composeType {} over mixed predicates is not understood",
            filter.compose_type
        )),
    }
}

fn translate_predicate(predicate: &Json) -> std::result::Result<Rule, String> {
    if predicate
        .pointer("/creationDate/dynamicDate")
        .and_then(Json::as_str)
        == Some("today")
    {
        return Ok(Rule::CreatedToday);
    }
    let kind = match predicate.get("uniformType").and_then(Json::as_str) {
        Some("public.text") => Kind::Text,
        Some("public.url") => Kind::Url,
        Some("public.image") => Kind::Image,
        Some(FILE_UTI) => Kind::File,
        _ => return Err(format!("unsupported predicate {predicate}")),
    };
    Ok(Rule::Kind { kind })
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMAGE_UUID: &str = "0A1B2C3D-0000-4000-8000-00000000000A";
    const MISSING_UUID: &str = "0A1B2C3D-0000-4000-8000-00000000000B";
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake image";

    fn pasteboard(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let array = entries
            .iter()
            .map(|(ty, data)| {
                let mut dict = plist::Dictionary::new();
                dict.insert("type".into(), plist::Value::String((*ty).into()));
                dict.insert("data".into(), plist::Value::Data(data.to_vec()));
                plist::Value::Dictionary(dict)
            })
            .collect();
        let mut bytes = Vec::new();
        plist::Value::Array(array)
            .to_writer_binary(&mut bytes)
            .unwrap();
        let mut compressed = Vec::new();
        lzfse_rust::encode_bytes(&bytes, &mut compressed).unwrap();
        compressed
    }

    fn external_ref(uuid: &str) -> Vec<u8> {
        let mut blob = vec![2];
        blob.extend_from_slice(uuid.as_bytes());
        blob.push(0);
        blob
    }

    fn fixture(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("PasteNow.sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "PRAGMA synchronous = OFF;
             CREATE TABLE ZPASTEITEM (Z_PK INTEGER PRIMARY KEY, ZAPP INTEGER, ZITEMDATA INTEGER, \
               ZLIST INTEGER, ZTIMESTAMP TIMESTAMP, ZAPPBUNDLEID VARCHAR, ZBASEUNIFORMTYPE VARCHAR, \
               ZTEXT VARCHAR, ZTITLE VARCHAR);
             CREATE TABLE ZPASTEITEMDATA (Z_PK INTEGER PRIMARY KEY, ZPASTEITEM INTEGER, ZDATA BLOB);
             CREATE TABLE ZAPPLICATION (Z_PK INTEGER PRIMARY KEY, ZBUNDLEID VARCHAR, ZNAME VARCHAR);
             CREATE TABLE ZEXCLUDERULE (Z_PK INTEGER PRIMARY KEY, ZAPP INTEGER, ZENABLED INTEGER);
             CREATE TABLE ZPASTELIST (Z_PK INTEGER PRIMARY KEY, ZLISTTYPE INTEGER, ZSORTWEIGHT INTEGER, \
               ZNAME VARCHAR, ZFILTERRULEDATA BLOB);
             INSERT INTO ZAPPLICATION VALUES (1, 'com.apple.Safari', 'Safari'), \
               (2, 'com.agilebits.onepassword7', '1Password');
             INSERT INTO ZEXCLUDERULE VALUES (1, 2, 1);
             INSERT INTO ZPASTELIST VALUES (1, -1, -1, NULL, NULL), (2, 0, 1, '📌', NULL), \
               (3, 1, 2, 'Images', \
                CAST('{\"composeType\":1,\"predicateTypes\":[{\"uniformType\":\"public.image\"}],\"ruleName\":\"Images\"}' AS BLOB)), \
               (4, 1, 3, 'Today', \
                CAST('{\"composeType\":1,\"predicateTypes\":[{\"creationDate\":{\"dynamicDate\":\"today\"}}],\"ruleName\":\"Today\"}' AS BLOB));
             INSERT INTO ZPASTEITEM VALUES \
               (1, 1, 1, 2, 800000000.5, 'com.apple.Safari', 'public.text', 'hello world', 'Greeting'), \
               (2, 1, 2, NULL, 800000100, 'com.apple.Safari', 'public.image', NULL, NULL), \
               (3, 1, 3, NULL, 800000200, 'com.apple.Safari', 'public.image', NULL, NULL);",
        )
        .unwrap();
        let mut inline = vec![1];
        inline.extend(pasteboard(&[
            ("public.rtf", b"{\\rtf1 hello world}"),
            (TEXT_UTI, b"hello world"),
        ]));
        let insert = "INSERT INTO ZPASTEITEMDATA VALUES (?1, ?1, ?2)";
        db.execute(insert, rusqlite::params![1, inline]).unwrap();
        db.execute(insert, rusqlite::params![2, external_ref(IMAGE_UUID)])
            .unwrap();
        db.execute(insert, rusqlite::params![3, external_ref(MISSING_UUID)])
            .unwrap();

        let external = dir.join(EXTERNAL_DATA_DIR);
        std::fs::create_dir_all(&external).unwrap();
        std::fs::write(
            external.join(IMAGE_UUID),
            pasteboard(&[("public.png", PNG)]),
        )
        .unwrap();
        path
    }

    #[test]
    fn imports_items_lists_and_excludes_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = fixture(dir.path());
        let mut store = Store::open_in_memory().unwrap();

        let report = import(&mut store, &path).unwrap();
        assert_eq!((report.imported, report.duplicates), (2, 0));
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].0, 3);
        assert_eq!(report.lists_created, 2);
        assert_eq!(
            report.lists_skipped,
            [(
                "Images".to_owned(),
                "same as the built-in Images filter".to_owned()
            )]
        );
        assert_eq!(report.excludes, 1);

        let recent = store.recent(10).unwrap();
        assert_eq!(recent.len(), 2);
        let (image, text) = (&recent[0], &recent[1]);
        assert_eq!(image.kind, Kind::Image);
        assert_eq!(text.kind, Kind::Text);
        assert_eq!(text.title.as_deref(), Some("Greeting"));
        assert_eq!(text.app_bundle_id.as_deref(), Some("com.apple.Safari"));
        assert_eq!(text.created_at, 1_778_307_200_500);

        let item = store.get(text.id).unwrap().unwrap();
        let formats: Vec<&str> = item.reps.iter().map(|r| r.format.as_str()).collect();
        assert_eq!(formats, [format::TEXT, format::RTF]);
        assert_eq!(item.text(), Some("hello world"));
        let image_item = store.get(image.id).unwrap().unwrap();
        assert_eq!(image_item.reps, [Rep::new("public.png", PNG.to_vec())]);

        let pinned = store.list_by_name(PINNED_LIST).unwrap().unwrap();
        let members: Vec<i64> = store
            .list_items(&pinned, 10, 0)
            .unwrap()
            .iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(members, [text.id]);
        assert_eq!(store.excludes().unwrap(), ["com.agilebits.onepassword7"]);
        let today = store.list_by_name("Today").unwrap().unwrap();
        let copied_today = |now| store.list_items(&today, 10, now).unwrap();
        assert!(
            copied_today(text.created_at + 1)
                .iter()
                .any(|i| i.id == text.id)
        );
        assert!(copied_today(text.created_at + 2 * 86_400_000).is_empty());
        assert!(copied_today(text.created_at - 2 * 86_400_000).is_empty());

        let again = import(&mut store, &path).unwrap();
        assert_eq!((again.imported, again.duplicates), (0, 2));
        assert_eq!(again.lists_created, 0);
        assert_eq!(store.recent(10).unwrap(), recent);
        assert_eq!(store.lists().unwrap().len(), 2);
        assert_eq!(store.list_items(&pinned, 10, 0).unwrap().len(), 1);
    }
}
