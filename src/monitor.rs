//! The capture loop: clipboard change → filters → store.

use std::time::Duration;

use clipboard_rs::{ClipboardContext, ClipboardHandler, ClipboardWatcher, ClipboardWatcherContext};

use crate::capture::{self, Skipped};
use crate::model::Kind;
use crate::store::{Ingest, Store};
use crate::{Error, Result, platform, secrets};

#[derive(Debug)]
pub enum Event {
    Stored {
        outcome: Ingest,
        kind: Kind,
        secret: bool,
    },
    Skipped(Skipped),
    Failed(Error),
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// How often the pasteboard change counter is polled.
    pub interval: Duration,
    pub max_item_bytes: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            interval: Duration::from_millis(300),
            max_item_bytes: 64 << 20,
        }
    }
}

struct Handler<F> {
    store: Store,
    ctx: ClipboardContext,
    max_item_bytes: usize,
    on_event: F,
}

impl<F: FnMut(Event)> ClipboardHandler for Handler<F> {
    fn on_clipboard_change(&mut self) {
        let event =
            platform::with_autorelease_pool(|| self.capture().unwrap_or_else(Event::Failed));
        (self.on_event)(event);
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
            secret: cap.secret.is_some(),
        })
    }
}

/// Watches the clipboard until the process exits, reporting every change.
pub fn run(
    store: Store,
    config: Config,
    on_event: impl FnMut(Event) + Send + 'static,
) -> Result<()> {
    let clip_err = |e: Box<dyn std::error::Error + Send + Sync>| Error::Clipboard(e.to_string());
    let ctx = ClipboardContext::new().map_err(clip_err)?;
    let mut watcher =
        ClipboardWatcherContext::new_with_interval(config.interval).map_err(clip_err)?;
    watcher.add_handler(Handler {
        store,
        ctx,
        max_item_bytes: config.max_item_bytes,
        on_event,
    });
    watcher.start_watch();
    Ok(())
}
