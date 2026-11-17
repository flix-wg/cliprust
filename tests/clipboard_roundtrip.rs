//! Needs a real clipboard: `xvfb-run cargo test -- --ignored` on Linux, or a
//! logged-in session on macOS. It overwrites the clipboard, so everything that
//! touches it lives in this one test.

use std::sync::mpsc;
use std::time::Duration;

use clipboard_rs::{ClipboardContext, ClipboardHandler, ClipboardWatcher, ClipboardWatcherContext};
use cliprust::{Kind, Rep, capture, format};

struct Notify(mpsc::Sender<()>);

impl ClipboardHandler for Notify {
    fn on_clipboard_change(&mut self) {
        let _ = self.0.send(());
    }
}

#[test]
#[ignore = "touches the system clipboard"]
fn writes_read_back_and_wake_the_watcher() {
    let (tx, changes) = mpsc::channel();
    let mut watcher =
        ClipboardWatcherContext::new_with_interval(Duration::from_millis(50)).unwrap();
    watcher.add_handler(Notify(tx));
    let shutdown = watcher.get_shutdown_channel();
    let thread = std::thread::spawn(move || watcher.start_watch());
    std::thread::sleep(Duration::from_millis(200));

    let ctx = ClipboardContext::new().unwrap();
    let reps = vec![
        Rep::new(format::TEXT, b"round trip".to_vec()),
        Rep::new(format::HTML, b"<i>round trip</i>".to_vec()),
    ];
    capture::write(&ctx, &reps, false).unwrap();
    changes
        .recv_timeout(Duration::from_secs(3))
        .expect("watcher saw the write");
    let cap = capture::read(&ctx, 1 << 20).unwrap();
    assert_eq!(cap.kind, Kind::Text);
    assert_eq!(cap.reps, reps);

    capture::write(&ctx, &reps, true).unwrap();
    assert_eq!(capture::read(&ctx, 1 << 20).unwrap().reps, reps[..1]);

    shutdown.stop();
    thread.join().unwrap();
}
