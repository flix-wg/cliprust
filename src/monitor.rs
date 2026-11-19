//! The capture loop: clipboard change → filters → store.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clipboard_rs::ClipboardContext;

use crate::capture::{self, Skipped};
use crate::model::{Kind, SecretKind};
use crate::store::{Ingest, Store};
use crate::{Error, Result, platform, secrets};

#[derive(Debug)]
pub enum Event {
    Stored {
        outcome: Ingest,
        kind: Kind,
        secret: Option<SecretKind>,
    },
    Skipped(Skipped),
    /// A key was copied but not stored: its encryption key is unavailable.
    /// The key is still on the clipboard; nothing reached the disk.
    SecretSkipped(String),
    Failed(Error),
}

/// The store the watcher writes to: its own, or one shared with the caller
/// (the app), so a process has one connection to the file.
pub trait StoreAccess: Send + 'static {
    fn with<R>(&mut self, f: impl FnOnce(&mut Store) -> R) -> R;
}

impl StoreAccess for Store {
    fn with<R>(&mut self, f: impl FnOnce(&mut Store) -> R) -> R {
        f(self)
    }
}

impl StoreAccess for Arc<Mutex<Store>> {
    fn with<R>(&mut self, f: impl FnOnce(&mut Store) -> R) -> R {
        f(&mut self.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Poll interval right after a change; idle polling backs off to `max_interval`.
    pub interval: Duration,
    pub max_interval: Duration,
    pub max_item_bytes: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            interval: Duration::from_millis(250),
            max_interval: Duration::from_millis(1000),
            max_item_bytes: 32 << 20,
        }
    }
}

/// How long asking Safari for its tab may hold up a capture; osascript
/// itself takes much of it.
const SAFARI_TIMEOUT: Duration = Duration::from_millis(400);
/// After a failed Safari lookup (denied, timed out) copies are stored
/// without one for this long, so a burst of copies does not ask each time.
const SAFARI_BACKOFF: Duration = Duration::from_secs(30);
/// Asking a browser for the page behind a key may wait for its permission prompt.
const KEY_PAGE_TIMEOUT: Duration = Duration::from_secs(2);

struct Handler<S, F> {
    store: S,
    ctx: ClipboardContext,
    max_item_bytes: usize,
    on_event: F,
    /// No Safari lookups before this, after one failed.
    safari_retry: Option<Instant>,
}

impl<S: StoreAccess, F: FnMut(Event)> Handler<S, F> {
    fn on_change(&mut self) {
        let event =
            platform::with_autorelease_pool(|| self.capture().unwrap_or_else(Event::Failed));
        (self.on_event)(event);
    }
}

#[cfg(not(target_os = "macos"))]
impl<S: StoreAccess, F: FnMut(Event)> clipboard_rs::ClipboardHandler for Handler<S, F> {
    fn on_clipboard_change(&mut self) {
        self.on_change();
    }
}

impl<S: StoreAccess, F> Handler<S, F> {
    fn capture(&mut self) -> Result<Event> {
        let settings = self.store.with(|s| {
            s.revalidate()?;
            s.settings()
        })?;
        if settings.paused {
            return Ok(Event::Skipped(Skipped::Paused));
        }
        // Check exclusions before touching the content at all.
        let app = platform::frontmost_app();
        if let Some(app) = &app
            && self.store.with(|s| s.is_excluded(&app.bundle_id))?
        {
            return Ok(Event::Skipped(Skipped::Excluded(app.bundle_id.clone())));
        }
        let mut cap = match capture::read(&self.ctx, self.max_item_bytes) {
            Ok(cap) => cap,
            Err(skipped) => return Ok(Event::Skipped(skipped)),
        };
        cap.app = app;
        let bundle_id = cap.app.as_ref().map(|a| a.bundle_id.clone());
        let mut asked = false;
        if let Some(safari) = bundle_id
            .as_deref()
            .filter(|b| platform::SAFARI.contains(b))
            && settings.safari_pages
            && cap.source_url.is_none()
            && self.safari_retry.is_none_or(|at| Instant::now() >= at)
        {
            asked = true;
            match platform::browser_page(safari, SAFARI_TIMEOUT) {
                Some(page) => {
                    cap.source_url = Some(page.url);
                    cap.source_title = page.title;
                    self.safari_retry = None;
                }
                None => self.safari_retry = Some(Instant::now() + SAFARI_BACKOFF),
            }
        }
        if settings.detect_keys {
            secrets::annotate(&mut cap, || {
                (!asked)
                    .then(|| platform::browser_page(bundle_id.as_deref()?, KEY_PAGE_TIMEOUT))
                    .flatten()
            });
        }
        let now = crate::now_ms();
        let stored = self.store.with(|s| {
            let outcome = s.ingest(&cap, now)?;
            if matches!(outcome, Ingest::Inserted(_)) {
                s.apply_retention(&settings, now)?;
            }
            Ok(outcome)
        });
        let is_key = cap.secret.as_ref().map(|s| s.kind) == Some(SecretKind::Key);
        if is_key {
            for rep in &mut cap.reps {
                zeroize::Zeroize::zeroize(&mut rep.data);
            }
        }
        let outcome = match stored {
            Err(Error::Locked(why)) if is_key => return Ok(Event::SecretSkipped(why)),
            other => other?,
        };
        Ok(Event::Stored {
            outcome,
            kind: cap.kind,
            secret: cap.secret.as_ref().map(|s| s.kind),
        })
    }
}

/// The pasteboard change count this process produced last; the watcher skips it.
static OWN_CHANGE: AtomicIsize = AtomicIsize::new(0);

/// Marks the pasteboard's current contents as written by us, e.g. a clip put
/// back for pasting, so the watcher does not store it again.
pub fn own_change(count: isize) {
    OWN_CHANGE.store(count, Ordering::Relaxed);
}

/// Watches the clipboard until the process exits, reporting every change.
pub fn run(
    store: impl StoreAccess,
    config: Config,
    on_event: impl FnMut(Event) + Send + 'static,
) -> Result<()> {
    let handler = Handler {
        store,
        ctx: ClipboardContext::new().map_err(clip_err)?,
        max_item_bytes: config.max_item_bytes,
        on_event,
        safari_retry: None,
    };
    watch(handler, config)
}

fn clip_err(e: Box<dyn std::error::Error + Send + Sync>) -> Error {
    Error::Clipboard(e.to_string())
}

/// Polls `changeCount` like clipboard-rs did, but backs off by 1.25x per idle
/// poll toward `max_interval`; a clipboard change or a switch of frontmost app
/// snaps back to `interval`. The wait is a park, so `unpark` wakes it at once.
#[cfg(target_os = "macos")]
fn watch<S: StoreAccess, F: FnMut(Event)>(
    mut handler: Handler<S, F>,
    config: Config,
) -> Result<()> {
    let mut last = platform::pasteboard_state();
    let mut delay = config.interval;
    loop {
        std::thread::park_timeout(delay);
        let state = platform::pasteboard_state();
        // As in clipboard-rs, a zero count is adopted as the baseline silently.
        if last.0 != 0 && state.0 != last.0 && state.0 != OWN_CHANGE.load(Ordering::Relaxed) {
            handler.on_change();
        }
        delay = if state == last {
            (delay * 5 / 4).min(config.max_interval)
        } else {
            config.interval
        };
        last = state;
    }
}

#[cfg(not(target_os = "macos"))]
fn watch<S: StoreAccess, F: FnMut(Event) + Send + 'static>(
    handler: Handler<S, F>,
    config: Config,
) -> Result<()> {
    use clipboard_rs::{ClipboardWatcher, ClipboardWatcherContext};
    let mut watcher =
        ClipboardWatcherContext::new_with_interval(config.interval).map_err(clip_err)?;
    watcher.add_handler(handler);
    watcher.start_watch();
    Ok(())
}
