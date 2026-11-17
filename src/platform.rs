//! macOS integration. Other platforms get inert fallbacks so the core builds
//! and tests anywhere.

use crate::model::AppInfo;
use crate::secrets::PageContext;

#[cfg(target_os = "macos")]
pub fn frontmost_app() -> Option<AppInfo> {
    use objc2_app_kit::NSWorkspace;

    let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    Some(AppInfo {
        bundle_id: app.bundleIdentifier()?.to_string(),
        name: app.localizedName().map(|n| n.to_string()),
        path: app
            .bundleURL()
            .and_then(|u| u.path())
            .map(|p| p.to_string()),
    })
}

#[cfg(not(target_os = "macos"))]
pub fn frontmost_app() -> Option<AppInfo> {
    None
}

/// Drains Objective-C temporaries created while reading the pasteboard; the
/// watcher thread has no pool of its own, so they would otherwise pile up.
#[cfg(target_os = "macos")]
pub fn with_autorelease_pool<R>(f: impl FnOnce() -> R) -> R {
    objc2::rc::autoreleasepool(|_| f())
}

#[cfg(not(target_os = "macos"))]
pub fn with_autorelease_pool<R>(f: impl FnOnce() -> R) -> R {
    f()
}

/// NSWorkspace only refreshes `frontmostApplication` while the main run loop
/// turns, so a command-line watcher must park its main thread here.
#[cfg(target_os = "macos")]
pub fn run_main_loop() -> ! {
    use objc2_foundation::NSRunLoop;

    loop {
        NSRunLoop::mainRunLoop().run();
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// URL and title of the front tab, for browsers that answer AppleScript.
/// macOS asks the user once per browser before allowing this.
pub fn browser_page(bundle_id: &str) -> Option<PageContext> {
    const CHROMIUM: &[&str] = &[
        "com.google.Chrome",
        "com.google.Chrome.canary",
        "com.brave.Browser",
        "com.microsoft.edgemac",
        "company.thebrowser.Browser",
        "com.vivaldi.Vivaldi",
        "com.operasoftware.Opera",
    ];
    const SAFARI: &[&str] = &["com.apple.Safari", "com.apple.SafariTechnologyPreview"];

    let tab = if CHROMIUM.contains(&bundle_id) {
        "{URL, title} of active tab of front window"
    } else if SAFARI.contains(&bundle_id) {
        "{URL, name} of current tab of front window"
    } else {
        return None;
    };
    let script = format!(
        "tell application id \"{bundle_id}\" to set p to {tab}\nreturn (item 1 of p) & linefeed & (item 2 of p)"
    );
    let out = osascript(&script)?;
    let (url, title) = out.split_once('\n').unwrap_or((&out, ""));
    let url = url.trim();
    (!url.is_empty()).then(|| PageContext {
        url: url.to_owned(),
        title: Some(title.trim())
            .filter(|t| !t.is_empty())
            .map(str::to_owned),
    })
}

/// Runs osascript with a short deadline: a pending permission prompt or a
/// hung browser must not stall clipboard capture.
fn osascript(script: &str) -> Option<String> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let mut child = Command::new("/usr/bin/osascript")
        .args(["-e", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Drain stdout while waiting so a long title cannot fill the pipe.
    let stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ =
            std::io::Read::read_to_string(&mut std::io::Read::take(stdout, 16 * 1024), &mut out);
        out
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let out = reader.join().ok()?;
    success.then_some(out)
}
