# ClipRust

A fast, low-memory clipboard history for macOS. A Rust core handles capture, storage and search; a native SwiftUI app sits on top.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/panel-dark.png">
  <img alt="The ClipRust panel" src="docs/images/panel-light.png" width="680">
</picture>

## Features

- **Search everything.** Full-text search over all history, not only previews.
- **Search by app.** Type `mail` or `codex` to filter by the app you copied from, or use `from:mail`.
- **Phone numbers, emails, addresses.** Recognised in short clips; filter with `is:phone` or the Contacts chip, then call, mail or map them from the row menu.
- **Lists.** Manual lists, saved searches, pinned items. Jump between them from the keyboard.
- **Keys stay hidden.** API keys and tokens are recognised, masked everywhere, and revealed only after Touch ID. Copied keys are marked so other clipboard managers skip them.
- **Lightweight.** About 17 MB of memory at idle, no helper process, no telemetry, no network access.
- **Every format.** Plain text, RTF, HTML, images and files, deduplicated and compressed with zstd.
- **Respects privacy.** Skips content that password managers mark as concealed, and any apps you exclude.
- **Imports PasteNow.** Brings over your history and lists.

## Shortcuts

| Key | Action |
| --- | --- |
| ⇧⌘V | Open the panel |
| ⏎ / ⌥⏎ / ⌘⏎ | Paste / paste as plain text / copy only |
| ⌘1 … ⌘9 | Paste row N |
| ⌘L | Go to a list or app |
| ⌃1 … ⌃9 | Jump to filter N |
| ⌘D | Add to list |
| ⌘⇧P | Pin |
| ⌘⇧S | Save search as list |
| ␣ | Quick Look |
| ⌘K | Keys & IDs |

All shortcuts are listed in [docs/app-design.md](docs/app-design.md).

## Build

Requires macOS 26, Xcode 26 and a Rust toolchain.

```sh
app/scripts/build-app.sh        # → app/build/ClipRust.app
```

Set `CLIPRUST_SIGN_IDENTITY` to sign with your own certificate. Pasting into other apps needs Accessibility permission, which macOS asks for on first use.

## CLI

The core also ships as a command-line tool that works on the same database:

```sh
cargo install --path .
cliprust search invoice 2024
cliprust copy 42 --plain
cliprust keys --provider cloudflare
cliprust import-pastenow
```

Run `cliprust --help` for the full list. Data lives in `~/Library/Application Support/cliprust/cliprust.db`; override it with `CLIPRUST_DB`.

## Layout

```
src/        Rust core: capture, SQLite store, search, rules, key detection, C API
app/        SwiftUI app (Swift package) and build script
docs/       Design notes
```

## Development

```sh
cargo test --features ffi
cargo clippy --all-targets --features ffi -- -D warnings
```
