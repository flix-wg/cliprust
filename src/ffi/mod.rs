//! C ABI for the macOS app: JSON requests in, JSON responses out.
//! See docs/app-design.md for the contract; the methods are in `methods`.

mod methods;

use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use zeroize::Zeroize;

use serde::Serialize;
use serde_json::{Value, json};

use crate::model::SecretKind;
use crate::monitor::{self, Event};
use crate::store::{Ingest, Store};

pub struct CrHandle {
    /// Shared with the watcher thread: one connection per process, which
    /// switching database encryption needs.
    store: Arc<Mutex<Store>>,
    path: PathBuf,
    watching: AtomicBool,
}

impl CrHandle {
    fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// What kind of failure an error response reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    /// No such item or list.
    NotFound,
    /// A malformed request, or one the store refuses.
    Invalid,
    /// The key needed to read the data is missing from or refused by the Keychain.
    Locked,
    /// The database or the pasteboard failed.
    Io,
    /// A bug, such as a caught panic.
    Internal,
}

/// The `err` of a response.
#[derive(Debug, Serialize)]
pub struct Error {
    pub code: Code,
    pub message: String,
}

impl Error {
    fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<crate::Error> for Error {
    fn from(e: crate::Error) -> Self {
        use crate::Error as E;
        let code = match &e {
            E::Sql(rusqlite::Error::QueryReturnedNoRows) => Code::NotFound,
            E::Sql(s)
                if s.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation) =>
            {
                Code::Invalid
            }
            E::Json(_) | E::Refused(_) => Code::Invalid,
            E::Locked(_) => Code::Locked,
            E::Sql(_) | E::Io(_) | E::Clipboard(_) | E::Corrupt(_) => Code::Io,
        };
        Error::new(code, e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::new(Code::Invalid, e.to_string())
    }
}

fn string_out(s: String) -> *mut c_char {
    CString::new(s.replace('\0', "")).map_or(std::ptr::null_mut(), CString::into_raw)
}

unsafe fn str_in<'a>(p: *const c_char) -> Option<&'a str> {
    (!p.is_null())
        .then(|| unsafe { CStr::from_ptr(p) }.to_str().ok())
        .flatten()
}

/// The crate version, e.g. `0.1.0`; a static string, not freed.
#[unsafe(no_mangle)]
pub extern "C" fn cr_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
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
    let result = catch_unwind(|| -> Result<CrHandle, String> {
        let path = match unsafe { str_in(db_path) } {
            Some(p) => PathBuf::from(p),
            None => crate::default_db_path().ok_or("HOME is not set")?,
        };
        let mut store = Store::open(&path).map_err(|e| e.to_string())?;
        // Seals keys stored in plaintext; a locked or refused keychain waits for later.
        let _ = store.encrypt_pending();
        Ok(CrHandle {
            store: Arc::new(Mutex::new(store)),
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

/// Overwrites the string (a response may hold a revealed key) and frees it.
///
/// # Safety
/// `s` comes from this library and is freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cr_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe { CString::from_raw(s) }
            .into_bytes_with_nul()
            .zeroize();
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

/// Handles one request; always returns `{"ok": ...}` or
/// `{"err": {"code": ..., "message": ...}}`.
///
/// # Safety
/// `h` is a live handle; `request` is a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cr_call(h: *const CrHandle, request: *const c_char) -> *mut c_char {
    let response = match (unsafe { h.as_ref() }, unsafe { str_in(request) }) {
        (Some(h), Some(req)) => catch_unwind(AssertUnwindSafe(|| methods::call(h, req)))
            .unwrap_or_else(|_| Err(Error::new(Code::Internal, "internal error"))),
        _ => Err(Error::new(Code::Invalid, "invalid handle or request")),
    };
    string_out(
        match response {
            Ok(v) => json!({ "ok": v }),
            Err(e) => json!({ "err": e }),
        }
        .to_string(),
    )
}

/// Copies one stored representation (`format` as in `item.formats`, or
/// `"image"` for the first image format) into a new buffer. Returns 0 on
/// success, 1 if absent, 2 on error or for a masked key.
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
    // Keys only leave the core through reveal_key.
    let found = catch_unwind(AssertUnwindSafe(|| {
        let store = h.store();
        match store.summary(id) {
            Ok(Some(s)) if methods::is_masked(&s) => Err(()),
            Ok(_) => Ok(store.rep(id, format).ok().flatten()),
            Err(_) => Err(()),
        }
    }));
    match found {
        Ok(Ok(Some(data))) => {
            let boxed = data.into_boxed_slice();
            unsafe {
                *len = boxed.len();
                *out = Box::into_raw(boxed).cast::<u8>();
            }
            0
        }
        Ok(Ok(None)) => 1,
        _ => 2,
    }
}

/// Tells the watcher that the app itself wrote pasteboard change `count`.
#[unsafe(no_mangle)]
pub extern "C" fn cr_own_change(count: isize) {
    monitor::own_change(count);
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
    let store = Arc::clone(&h.store);
    let sink = Arc::new(Sink { cb, ctx });
    let thread_sink = Arc::clone(&sink);
    let spawned = std::thread::Builder::new()
        .name("cliprust-watch".into())
        .spawn(move || {
            let sink = thread_sink;
            let events = Arc::clone(&sink);
            let result = monitor::run(store, monitor::Config::default(), move |ev| {
                events.emit(event_json(&ev))
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
        Event::SecretSkipped(reason) => json!({ "type": "secret_skipped", "reason": reason }),
        Event::Failed(e) => json!({ "type": "failed", "error": e.to_string() }),
    }
}
