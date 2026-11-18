//! The capture loop: clipboard change → filters → store.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Duration;

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
    Failed(Error),
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

struct Handler<F> {
    store: Store,
    ctx: ClipboardContext,
    max_item_bytes: usize,
    on_event: F,
}

impl<F: FnMut(Event)> Handler<F> {
    fn on_change(&mut self) {
        let event =
            platform::with_autorelease_pool(|| self.capture().unwrap_or_else(Event::Failed));
        (self.on_event)(event);
    }
}

#[cfg(not(target_os = "macos"))]
impl<F: FnMut(Event)> clipboard_rs::ClipboardHandler for Handler<F> {
    fn on_clipboard_change(&mut self) {
        self.on_change();
    }
}

impl<F> Handler<F> {
    fn capture(&mut self) -> Result<Event> {
        let settings = self.store.settings()?;
        if settings.paused {
            return Ok(Event::Skipped(Skipped::Paused));
        }
        // Check exclusions before touching the content at all.
        let app = platform::frontmost_app();
        if let Some(app) = &app
            && self.store.is_excluded(&app.bundle_id)?
        {
            return Ok(Event::Skipped(Skipped::Excluded(app.bundle_id.clone())));
        }
        let mut cap = match capture::read(&self.ctx, self.max_item_bytes) {
            Ok(cap) => cap,
            Err(skipped) => return Ok(Event::Skipped(skipped)),
        };
        cap.app = app;
        if settings.detect_keys {
            let bundle_id = cap.app.as_ref().map(|a| a.bundle_id.clone());
            secrets::annotate(&mut cap, || platform::browser_page(bundle_id.as_deref()?));
        }
        let now = crate::now_ms();
        let outcome = self.store.ingest(&cap, now)?;
        if matches!(outcome, Ingest::Inserted(_)) {
            self.store.apply_retention(&settings, now)?;
        }
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
    store: Store,
    config: Config,
    on_event: impl FnMut(Event) + Send + 'static,
) -> Result<()> {
    let handler = Handler {
        store,
        ctx: ClipboardContext::new().map_err(clip_err)?,
        max_item_bytes: config.max_item_bytes,
        on_event,
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
fn watch<F: FnMut(Event)>(mut handler: Handler<F>, config: Config) -> Result<()> {
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
fn watch<F: FnMut(Event) + Send + 'static>(handler: Handler<F>, config: Config) -> Result<()> {
    use clipboard_rs::{ClipboardWatcher, ClipboardWatcherContext};
    let mut watcher =
        ClipboardWatcherContext::new_with_interval(config.interval).map_err(clip_err)?;
    watcher.add_handler(handler);
    watcher.start_watch();
    Ok(())
}
