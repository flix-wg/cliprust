//! C ABI for the macOS app: JSON requests in, JSON responses out.
//! See docs/app-design.md for the contract.

use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::model::{ItemSummary, Kind, SecretKind, Settings};
use crate::monitor::{self, Event};
use crate::rules::{Rule, SortType};
use crate::store::{Ingest, PasteList, SecretFilter, Store};
use crate::{capture, now_ms, platform};

type R<T> = std::result::Result<T, String>;

pub struct CrHandle {
    store: Mutex<Store>,
    path: PathBuf,
    watching: AtomicBool,
}

impl CrHandle {
    fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub fn default_db_path() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let base = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        std::env::var_os("XDG_DATA_HOME").map_or_else(|| home.join(".local/share"), PathBuf::from)
    };
    Some(base.join("cliprust/cliprust.db"))
}

fn string_out(s: String) -> *mut c_char {
    CString::new(s.replace('\0', "")).map_or(std::ptr::null_mut(), CString::into_raw)
}

unsafe fn str_in<'a>(p: *const c_char) -> Option<&'a str> {
    (!p.is_null())
        .then(|| unsafe { CStr::from_ptr(p) }.to_str().ok())
        .flatten()
}

/// Opens (creating if needed) the database. `db_path` NULL means the default
/// location shared with the CLI. On failure returns NULL and sets `err_out`.
///
/// # Safety
/// `db_path` is NULL or a valid C string; `err_out` is NULL or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cr_open(
    db_path: *const c_char,
    err_out: *mut *mut c_char,
) -> *mut CrHandle {
    let result = catch_unwind(|| -> R<CrHandle> {
        let path = match unsafe { str_in(db_path) } {
            Some(p) => PathBuf::from(p),
            None => default_db_path().ok_or("HOME is not set")?,
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let store = Store::open(&path).map_err(|e| e.to_string())?;
        Ok(CrHandle {
            store: Mutex::new(store),
            path,
            watching: AtomicBool::new(false),
        })
    })
    .unwrap_or_else(|_| Err("panic while opening the database".into()));
    match result {
        Ok(h) => Box::into_raw(Box::new(h)),
        Err(e) => {
            if !err_out.is_null() {
                unsafe { *err_out = string_out(e) };
            }
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// `h` comes from `cr_open` and is not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cr_close(h: *mut CrHandle) {
    if !h.is_null() {
        drop(unsafe { Box::from_raw(h) });
    }
}

/// # Safety
/// `s` comes from this library and is freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cr_free_string(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}

/// # Safety
/// `p`/`len` come from `cr_item_data` and are freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cr_free_bytes(p: *mut u8, len: usize) {
    if !p.is_null() {
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(p, len)) });
    }
}

/// Handles one request; always returns `{"ok": ...}` or `{"err": "..."}`.
///
/// # Safety
/// `h` is a live handle; `request` is a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cr_call(h: *const CrHandle, request: *const c_char) -> *mut c_char {
    let response = match (unsafe { h.as_ref() }, unsafe { str_in(request) }) {
        (Some(h), Some(req)) => catch_unwind(AssertUnwindSafe(|| call(h, req)))
            .unwrap_or_else(|_| Err("internal error".into())),
        _ => Err("invalid handle or request".into()),
    };
    string_out(
        match response {
            Ok(v) => json!({ "ok": v }),
            Err(e) => json!({ "err": e }),
        }
        .to_string(),
    )
}

/// Copies one stored representation (`format` as in `item.formats`) into a
/// new buffer. Returns 0 on success, 1 if absent, 2 on error.
///
/// # Safety
/// `h` is a live handle; `format` a valid C string; `out`/`len` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cr_item_data(
    h: *const CrHandle,
    id: i64,
    format: *const c_char,
    out: *mut *mut u8,
    len: *mut usize,
) -> i32 {
    let (Some(h), Some(format)) = (unsafe { h.as_ref() }, unsafe { str_in(format) }) else {
        return 2;
    };
    if out.is_null() || len.is_null() {
        return 2;
    }
    let found = catch_unwind(AssertUnwindSafe(|| {
        let item = h.store().get(id).ok()??;
        item.reps
            .into_iter()
            .find(|r| r.format == format)
            .map(|r| r.data)
    }));
    match found {
        Ok(Some(data)) => {
            let boxed = data.into_boxed_slice();
            unsafe {
                *len = boxed.len();
                *out = Box::into_raw(boxed).cast::<u8>();
            }
            0
        }
        Ok(None) => 1,
        Err(_) => 2,
    }
}

pub type EventCallback = extern "C" fn(event_json: *const c_char, ctx: *mut c_void);

struct Sink {
    cb: EventCallback,
    ctx: *mut c_void,
}
// The callback and its context are documented as callable from any thread.
unsafe impl Send for Sink {}
unsafe impl Sync for Sink {}

impl Sink {
    fn emit(&self, v: Value) {
        if let Ok(s) = CString::new(v.to_string()) {
            (self.cb)(s.as_ptr(), self.ctx);
        }
    }
}

/// Starts the clipboard watcher on its own thread (once per handle). `cb`
/// receives one JSON event per clipboard change, on that thread.
///
/// # Safety
/// `h` stays alive while the process runs; `cb`/`ctx` are thread-safe.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cr_watch(h: *const CrHandle, cb: EventCallback, ctx: *mut c_void) {
    let Some(h) = (unsafe { h.as_ref() }) else {
        return;
    };
    if h.watching.swap(true, Ordering::SeqCst) {
        return;
    }
    let path = h.path.clone();
    let sink = Arc::new(Sink { cb, ctx });
    let thread_sink = Arc::clone(&sink);
    let spawned = std::thread::Builder::new()
        .name("cliprust-watch".into())
        .spawn(move || {
            let sink = thread_sink;
            let events = Arc::clone(&sink);
            let result = Store::open(&path).and_then(|store| {
                monitor::run(store, monitor::Config::default(), move |ev| {
                    events.emit(event_json(&ev))
                })
            });
            if let Err(e) = result {
                sink.emit(json!({ "type": "failed", "error": e.to_string() }));
            }
        });
    if let Err(e) = spawned {
        h.watching.store(false, Ordering::SeqCst);
        sink.emit(json!({ "type": "failed", "error": e.to_string() }));
    }
}

fn event_json(ev: &Event) -> Value {
    match ev {
        Event::Stored {
            outcome,
            kind,
            secret,
        } => json!({
            "type": "stored",
            "id": outcome.id(),
            "new": matches!(outcome, Ingest::Inserted(_)),
            "kind": kind.as_str(),
            "secret": secret.is_some(),
            "secret_kind": secret.map(SecretKind::as_str),
        }),
        Event::Skipped(reason) => json!({ "type": "skipped", "reason": reason.to_string() }),
        Event::Failed(e) => json!({ "type": "failed", "error": e.to_string() }),
    }
}

fn summary(s: &ItemSummary) -> Value {
    json!({
        "id": s.id,
        "kind": s.kind.as_str(),
        "title": s.title,
        "preview": s.preview,
        "app": s.app_bundle_id,
        "source_url": s.source_url,
        "source_domain": s.source_domain,
        "secret_kind": s.secret_kind.map(SecretKind::as_str),
        "secret_provider": s.secret_provider,
        "note": s.note,
        "created_at": s.created_at,
        "last_used_at": s.last_used_at,
        "byte_size": s.byte_size,
    })
}

fn list_json(l: &PasteList) -> Value {
    json!({
        "name": l.name,
        "rule": l.rule,
        "sort": l.sort.as_str(),
        "manual": l.rule.is_none(),
    })
}

#[derive(Deserialize)]
struct Request {
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Deserialize)]
struct Id {
    id: i64,
}

#[derive(Deserialize)]
struct ItemsQuery {
    query: Option<String>,
    kind: Option<Kind>,
    list: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    200
}

/// Items in a list or of a kind are matched against the query in memory over
/// this many candidates.
const FILTER_SCAN: usize = 5000;
const DETAIL_TEXT_LIMIT: usize = 256 * 1024;

fn params<T: DeserializeOwned>(v: Value) -> R<T> {
    serde_json::from_value(if v.is_null() { json!({}) } else { v }).map_err(|e| e.to_string())
}

fn call(h: &CrHandle, request: &str) -> R<Value> {
    let req: Request = serde_json::from_str(request).map_err(|e| e.to_string())?;
    let p = req.params;
    let now = now_ms();
    let mut store = h.store();
    let e = |e: crate::Error| e.to_string();
    Ok(match req.method.as_str() {
        "items" => Value::from_iter(items(&store, params(p)?, now)?.iter().map(summary)),
        "item" => {
            let Id { id } = params(p)?;
            let item = store.get(id).map_err(e)?.ok_or("no such item")?;
            let mut v = summary(&item.summary);
            let text = item
                .text()
                .map(|t| truncate(t, DETAIL_TEXT_LIMIT).to_owned());
            v["text"] = json!(if is_masked(&item.summary) {
                item.summary.preview.clone()
            } else {
                text
            });
            v["formats"] = json!(item.reps.iter().map(|r| &r.format).collect::<Vec<_>>());
            v
        }
        "paste_prep" => {
            #[derive(Deserialize)]
            struct P {
                id: i64,
                #[serde(default)]
                plain: bool,
            }
            let P { id, plain } = params(p)?;
            let item = store.get(id).map_err(e)?.ok_or("no such item")?;
            // Keys go through reveal_key so the app can gate and conceal them.
            if is_masked(&item.summary) {
                return Err("item is a key".into());
            }
            platform::with_autorelease_pool(|| -> R<()> {
                let ctx = clipboard_rs::ClipboardContext::new().map_err(|e| e.to_string())?;
                capture::write(&ctx, &item.reps, plain).map_err(e)
            })?;
            store.touch(id, now).map_err(e)?;
            Value::Null
        }
        "reveal_key" => {
            let Id { id } = params(p)?;
            let item = store.get(id).map_err(e)?.ok_or("no such item")?;
            json!(item.text())
        }
        "secrets" => {
            #[derive(Deserialize)]
            struct P {
                #[serde(flatten)]
                filter: SecretFilter,
                #[serde(default = "default_limit")]
                limit: usize,
            }
            let P { filter, limit } = params(p)?;
            Value::from_iter(
                store
                    .secrets(&filter, limit)
                    .map_err(e)?
                    .iter()
                    .map(summary),
            )
        }
        "secret_facets" => {
            #[derive(Deserialize)]
            struct P {
                kind: Option<SecretKind>,
            }
            let P { kind } = params(p)?;
            json!(store.secret_facets(kind).map_err(e)?)
        }
        "set_note" => {
            #[derive(Deserialize)]
            struct P {
                id: i64,
                note: Option<String>,
            }
            let P { id, note } = params(p)?;
            json!(store.set_note(id, note.as_deref()).map_err(e)?)
        }
        "delete" => {
            let Id { id } = params(p)?;
            json!(store.delete(id).map_err(e)?)
        }
        "set_title" => {
            #[derive(Deserialize)]
            struct P {
                id: i64,
                title: Option<String>,
            }
            let P { id, title } = params(p)?;
            let title = title.filter(|t| !t.trim().is_empty());
            json!(store.set_title(id, title.as_deref()).map_err(e)?)
        }
        "lists" => Value::from_iter(store.lists().map_err(e)?.iter().map(list_json)),
        "create_list" => {
            #[derive(Deserialize)]
            struct P {
                name: String,
                rule: Option<Rule>,
            }
            let P { name, rule } = params(p)?;
            json!(
                store
                    .create_list(&name, rule.as_ref(), SortType::Recent)
                    .map_err(e)?
            )
        }
        "rename_list" => {
            #[derive(Deserialize)]
            struct P {
                name: String,
                new_name: String,
            }
            let P { name, new_name } = params(p)?;
            json!(store.rename_list(&name, &new_name).map_err(e)?)
        }
        "delete_list" | "add_to_list" | "remove_from_list" => {
            #[derive(Deserialize)]
            struct P {
                name: String,
                id: Option<i64>,
            }
            let P { name, id } = params(p)?;
            match (req.method.as_str(), id) {
                ("delete_list", _) => json!(store.delete_list(&name).map_err(e)?),
                ("add_to_list", Some(id)) => {
                    store.add_to_list(&name, id, now).map_err(e)?;
                    json!(true)
                }
                ("remove_from_list", Some(id)) => {
                    json!(store.remove_from_list(&name, id).map_err(e)?)
                }
                _ => return Err("missing id".into()),
            }
        }
        "settings" => json!(store.settings().map_err(e)?),
        "save_settings" => {
            let settings: Settings = params(p)?;
            store.save_settings(&settings).map_err(e)?;
            store.apply_retention(&settings, now).map_err(e)?;
            json!(settings)
        }
        "excludes" => json!(store.excludes().map_err(e)?),
        "add_exclude" | "remove_exclude" => {
            #[derive(Deserialize)]
            struct P {
                bundle_id: String,
            }
            let P { bundle_id } = params(p)?;
            if req.method == "add_exclude" {
                store.add_exclude(&bundle_id).map_err(e)?;
                json!(true)
            } else {
                json!(store.remove_exclude(&bundle_id).map_err(e)?)
            }
        }
        "stats" => {
            let s = store.stats().map_err(e)?;
            json!({
                "items": s.items,
                "raw_bytes": s.raw_bytes,
                "stored_bytes": s.stored_bytes,
                "file_bytes": s.file_bytes,
                "path": h.path.display().to_string(),
            })
        }
        "clear" => json!(store.clear().map_err(e)?),
        #[cfg(feature = "pastenow-import")]
        "import_pastenow" => {
            #[derive(Deserialize)]
            struct P {
                path: Option<PathBuf>,
            }
            let P { path } = params(p)?;
            let r = crate::import_pastenow::import_live(&mut store, path).map_err(e)?;
            json!({
                "imported": r.imported,
                "duplicates": r.duplicates,
                "skipped": r.skipped.len(),
                "lists_created": r.lists_created,
                "excludes": r.excludes,
            })
        }
        other => return Err(format!("unknown method {other:?}")),
    })
}

fn items(store: &Store, q: ItemsQuery, now: i64) -> R<Vec<ItemSummary>> {
    let e = |e: crate::Error| e.to_string();
    let query = q.query.filter(|t| !t.trim().is_empty());
    let filtered = q.list.is_some() || q.kind.is_some();
    let mut found = match (&q.list, &query, q.kind) {
        (None, Some(text), _) => store.search(text, if filtered { FILTER_SCAN } else { q.limit }),
        (None, None, Some(kind)) => {
            let list = PasteList {
                id: 0,
                name: String::new(),
                rule: Some(Rule::Kind { kind }),
                sort: SortType::Recent,
                sort_weight: 0.0,
            };
            store.list_items(&list, q.limit, now)
        }
        (None, None, None) => store.recent(q.limit),
        (Some(name), _, _) => {
            let list = store
                .list_by_name(name)
                .map_err(e)?
                .ok_or_else(|| format!("no list named {name:?}"))?;
            let scan = if query.is_some() || q.kind.is_some() {
                FILTER_SCAN
            } else {
                q.limit
            };
            store.list_items(&list, scan, now)
        }
    }
    .map_err(e)?;
    if let Some(kind) = q.kind {
        found.retain(|s| s.kind == kind);
    }
    if let (Some(_), Some(text)) = (&q.list, &query) {
        let terms: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
        found.retain(|s| {
            let hay = format!(
                "{} {} {}",
                s.title.as_deref().unwrap_or_default(),
                s.preview.as_deref().unwrap_or_default(),
                s.note.as_deref().unwrap_or_default()
            )
            .to_lowercase();
            terms.iter().all(|t| hay.contains(t.as_str()))
        });
    }
    found.truncate(q.limit);
    Ok(found)
}

/// Keys stay masked; IDs are shown in full.
fn is_masked(s: &ItemSummary) -> bool {
    s.secret_kind == Some(SecretKind::Key)
        || (s.secret_provider.is_some() && s.secret_kind != Some(SecretKind::Id))
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Capture, Secret};

    #[test]
    fn keys_filter_by_kind_and_notes_round_trip_through_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let h = CrHandle {
            store: Mutex::new(Store::open(&path).unwrap()),
            path,
            watching: AtomicBool::new(false),
        };
        let mut key = Capture::from_text("sk-live-secret-value".into());
        key.secret = Some(Secret {
            kind: SecretKind::Key,
            provider: "Test key".into(),
            name: "TEST_KEY".into(),
            masked: "sk-liv…alue".into(),
        });
        let id = h.store().ingest(&key, 1).unwrap().id();
        let req =
            |m: &str, p: Value| call(&h, &json!({"method": m, "params": p}).to_string()).unwrap();

        assert_eq!(
            req("set_note", json!({"id": id, "note": "billing"})),
            json!(true)
        );
        let keys = req("secrets", json!({"kind": "key", "provider": "test KEY"}));
        assert_eq!(keys[0]["note"], "billing");
        assert_eq!(keys[0]["secret_kind"], "key");
        assert_eq!(req("secrets", json!({"kind": "id"})), json!([]));
        assert_eq!(
            req("secret_facets", Value::Null)["providers"][0]["count"],
            1
        );
        assert_ne!(
            req("item", json!({"id": id}))["text"],
            "sk-live-secret-value"
        );
        req("create_list", json!({"name": "Work"}));
        assert_eq!(
            req("rename_list", json!({"name": "Work", "new_name": "Job"})),
            json!(true)
        );
        assert!(
            req("lists", Value::Null)
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l["name"] == "Job")
        );
        let paste = json!({"method": "paste_prep", "params": {"id": id}});
        assert!(call(&h, &paste.to_string()).is_err());
    }
}
