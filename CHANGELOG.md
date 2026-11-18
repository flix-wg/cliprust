# Changelog

All notable changes to this project are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - Unreleased

First public release.

### Core and CLI

- Clipboard capture of plain text, RTF, HTML, images and files, deduplicated and compressed with zstd in a SQLite database.
- Full-text search with FTS5 over all history, multi-term search and search by source app (`from:`).
- Manual lists, saved searches, pinned items and ordered lists.
- Retention rules that keep listed and pinned items when history is cleared.
- Detection of API keys, tokens and IDs, with provider and site facets, notes and masking. Stored clips are rescanned when detection rules change.
- Detection of phone numbers, email addresses and postal addresses, searchable by type.
- Skips content that password managers mark as concealed, and excluded apps.
- Idle clipboard polling backs off; the WAL is checkpointed.
- PasteNow importer for history and lists.
- `cliprust` command-line tool working on the same database.
- C API for the macOS app.

### macOS app

- Menu-bar app with a floating panel opened with ⇧⌘V.
- Keyboard-first panel: filter chips, ⌃1–9 jumps, list palette (⌘L), add to list (⌘D), save search (⌘⇧S), pin, inline notes (⌘N) and rename (⌘R).
- Paste, paste as plain text or copy only; keys paste as concealed text and plain paste of keys is refused.
- Keys & IDs window; keys are revealed only after Touch ID.
- Preview pane with Quick Look and images capped at 800 px.
- Settings for retention, exclusions, key detection, launch at login and every panel shortcut.

### Security

- Detected API keys are encrypted at rest (XChaCha20-Poly1305) with a master key in the login Keychain; their checksums are keyed, and the search index never holds them.
- Optional encryption of the whole database with SQLCipher (CommonCrypto), from Settings → Security or `cliprust encryption on`.
- Key rotation keeps the old key for older copies until `cliprust keys prune` or Settings removes it; recovery codes restore a key on another Mac, also before an encrypted database can open.
- A missing key locks saved keys instead of deleting them; the panel explains it and offers to forget them.
- `cliprust show` and `cliprust copy` need `--reveal` for keys; the CLI copies keys as concealed.
- Quick Look copies are deleted when the panel closes.
- [SECURITY.md](SECURITY.md) describes the threat model and how to report a vulnerability.

[0.1.0]: https://github.com/flix-wg/cliprust/releases/tag/v0.1.0
