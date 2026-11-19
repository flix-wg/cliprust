# ClipRust

A fast, low-memory clipboard history for macOS. A Rust core handles capture, storage and search; a native SwiftUI app sits on top.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/panel-dark.png">
  <img alt="The ClipRust panel" src="docs/images/panel-light.png" width="680">
</picture>

## Features

### Find things

- **Search everything.** Full-text search over all history, not only previews.
- **Search by app.** Type `mail` or `codex` to filter by the app you copied from, or use `from:mail`.
- **Search by site.** `site:github` (or `source:github.com`) keeps clips copied from that website.
- **Phone numbers, emails, addresses.** Recognised in short clips; type `phone`, `email` or `address` (or `is:phone`), or use the Contacts chip, then call, mail or map them from the row menu.

### Organise

- **Lists.** Manual lists, saved searches, pinned items. Jump between them from the keyboard.
- **Keyboard first.** ← and → walk the filter chips, ⌘N labels the selected clip with a note right in the list (handy for keys), and every panel shortcut can be changed in Settings → Shortcuts.

### Capture and privacy

- **Keys stay hidden.** API keys, tokens and generated passwords are recognised, encrypted on disk with a key kept in your login Keychain, masked everywhere, and revealed only after Touch ID. Copied keys are marked so other clipboard managers skip them. The whole history can be encrypted too; see [SECURITY.md](SECURITY.md) for what this protects against.
- **Mark as Key.** Hide a clip detection missed with ⌘⇧K or the row menu, or turn a false positive back into searchable text. Later copies keep your choice.
- **Where it came from.** The preview shows the page a clip was copied from as a link. Chromium browsers say so themselves; for Safari, turn on Settings → History → Remember the page for copies from Safari; Firefox and Zen need the optional [ClipRust Source add-on](extensions/firefox), which records the site only.
- **Respects privacy.** Skips content that password managers mark as concealed, and any apps you exclude.
- **Every format.** Plain text, RTF, HTML, images and files, deduplicated and compressed with zstd.
- **Lightweight.** About 17 MB of memory at idle, no helper process, no telemetry, no network access.
- **Imports PasteNow.** Brings over your history and lists.

## Requirements

- macOS 26. Release downloads are built for Apple silicon only.
- To build from source: Xcode 26 and Rust 1.89 or later.

## Install

### Download

1. Download `ClipRust-<version>-macos-arm64.zip` from the [latest release](https://github.com/flix-wg/cliprust/releases/latest) and check it against `SHA256SUMS.txt` (`shasum -a 256 -c SHA256SUMS.txt --ignore-missing`).
2. Unzip it and move `ClipRust.app` to `/Applications`.
3. Releases are not yet signed with an Apple Developer ID, so macOS blocks the first launch. Either open the app once, then choose **Open Anyway** in System Settings → Privacy & Security, or remove the quarantine flag:

   ```sh
   xattr -dr com.apple.quarantine /Applications/ClipRust.app
   ```

Pasting into other apps needs Accessibility permission, which macOS asks for on first use.

### Build from source

```sh
git clone https://github.com/flix-wg/cliprust.git
cd cliprust
app/scripts/build-app.sh        # → app/build/ClipRust.app
```

Set `CLIPRUST_SIGN_IDENTITY` to sign with your own certificate. See [CONTRIBUTING.md](CONTRIBUTING.md) for details.

### CLI

The core also ships as a command-line tool that works on the same database. Install it from the release tarball (`cliprust-<version>-macos-arm64.tar.gz`, then run `xattr -d com.apple.quarantine cliprust` and move it onto your `PATH`) or with Cargo:

```sh
cargo install --git https://github.com/flix-wg/cliprust
```

```sh
cliprust search invoice 2024
cliprust copy 42 --plain
cliprust keys --provider cloudflare
cliprust keys mark 42
cliprust import-pastenow
```

Run `cliprust --help` for the full list.

## Data and privacy

History lives in `~/Library/Application Support/cliprust/cliprust.db`; the CLI reads another file if you set `CLIPRUST_DB`. Settings live in the `app.cliprust.ClipRust` defaults domain.

Detected API keys are stored encrypted with a key kept in your login keychain (service `app.cliprust`), and the whole database file can be encrypted too (`cliprust encryption on`, macOS only).

ClipRust makes no network connections and collects no telemetry. Nothing leaves your Mac. When key detection is on, it asks the browser in front for the current page's address (Automation permission) so you can tell keys apart later; with Safari pages remembered it asks Safari for every copy made there, private windows included. That address is stored only in the local database.

## Uninstall

1. Turn off **Launch at login** in Settings, then quit ClipRust from the menu-bar icon.
2. Delete the app and, if you want, your data and settings:

   ```sh
   rm -rf /Applications/ClipRust.app
   rm -rf ~/Library/Application\ Support/cliprust
   defaults delete app.cliprust.ClipRust
   tccutil reset Accessibility app.cliprust.ClipRust
   tccutil reset AppleEvents app.cliprust.ClipRust
   # the key that encrypts stored API keys (repeat until none is left)
   security delete-generic-password -s app.cliprust
   ```

3. Remove the CLI with `cargo uninstall cliprust`, or delete the `cliprust` binary you copied onto your `PATH`.

If ClipRust still shows up in System Settings → General → Login Items, remove it there.

## Shortcuts

| Key | Action |
| --- | --- |
| ⇧⌘V | Open the panel |
| ⏎ / ⌥⏎ / ⌘⏎ | Paste / paste as plain text / copy only |
| ⌘1 … ⌘9 | Paste row N |
| ↑ ↓ / ⌃N ⌃P | Move the selection |
| ← → | Previous / next filter (with an empty search, or the caret at its start / end) |
| ⇥ ⇧⇥ / ⌘← ⌘→ | Previous / next filter |
| ⌃1 … ⌃9 | Jump to filter N |
| ⌘L | Go to a list or app |
| ⌘D | Add to list |
| ⌘⇧P | Pin |
| ⌘⇧K | Mark as key, or not a key |
| ⌘⇧S | Save search as list |
| ⌘N | Edit the note inline (⏎ saves, Esc cancels, empty removes it) |
| ⌘R | Rename inline |
| ␣ | Quick Look (with an empty search) |
| ⌘⌫ | Delete |
| ⌘P | Show or hide the preview |
| ⌘K | Keys & IDs |
| ⌘, | Settings |
| Esc / ⌘W | Clear the search, then close / close |

The open shortcut and every panel shortcut except ⏎, Esc, ⇥, the arrows, ⌘1–⌘9 and ⌃1–⌃9 can be changed in Settings → Shortcuts. More detail in [docs/app-design.md](docs/app-design.md).

## Layout

```
src/        Rust core: capture, SQLite store, search, rules, key detection, C API
app/        SwiftUI app (Swift package) and build script
docs/       Design notes and roadmap
extensions/ Optional browser add-ons
```

## More

- [CHANGELOG.md](CHANGELOG.md): what changed in each release
- [CONTRIBUTING.md](CONTRIBUTING.md): development setup, checks and releases
- [docs/roadmap.md](docs/roadmap.md): planned features and add-ons

ClipRust is released under the [MIT License](LICENSE).

## Third-party notices

The macOS build includes [SQLCipher](https://www.zetetic.net/sqlcipher/) (community edition), which is distributed under this licence:

```text
Copyright (c) 2025, ZETETIC LLC
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:
    * Redistributions of source code must retain the above copyright
      notice, this list of conditions and the following disclaimer.
    * Redistributions in binary form must reproduce the above copyright
      notice, this list of conditions and the following disclaimer in the
      documentation and/or other materials provided with the distribution.
    * Neither the name of the ZETETIC LLC nor the
      names of its contributors may be used to endorse or promote products
      derived from this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY ZETETIC LLC ''AS IS'' AND ANY
EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL ZETETIC LLC BE LIABLE FOR ANY
DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND
ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```
