# cliprust

A low-memory clipboard history for macOS, written as a Rust core with a CLI.
A native SwiftUI shell will sit on top of the same library. The aim is to
replace PasteNow with something that has no helper process, no telemetry and
real full-text search.

## What it does

- Watches the clipboard and stores every representation of a copy (plain
  text, RTF, HTML, images, file lists) with the app it came from.
- Deduplicates on ingest. Copying something again moves the existing item to
  the top instead of adding a second one.
- Compresses payloads with zstd and keeps them out of the table used for
  listing.
- Searches with an FTS5 trigram index, so any substring of three or more
  characters matches. Shorter terms fall back to a scan.
- Skips content that password managers mark as concealed or transient, and
  apps you exclude.
- Manual lists, plus smart lists defined by JSON rules.
- Retention by item count or age. Items in manual lists are always kept.
- Optional key detection: recognises API keys and tokens, names them from
  `NAME=value` or the page they came from, and files them in a Keys list.
  Search and previews only ever see a masked form.
- Imports an existing PasteNow history.

## Usage

```sh
cliprust watch                          # record until interrupted
cliprust list -n 20
cliprust search invoice 2024
cliprust copy 42 [--plain]              # put item 42 back on the clipboard
cliprust lists add Links --rule '{"type":"kind","kind":"url"}'
cliprust lists put Snippets 42
cliprust exclude add com.agilebits.onepassword7
cliprust settings detect-keys on
cliprust keys
cliprust import-pastenow
```

The database lives in `~/Library/Application Support/cliprust/cliprust.db`;
set `CLIPRUST_DB` or `--db` to use another file.

Smart-list rules combine `all`, `any` and `not` with `kind`, `app`,
`contains`, `copied_within` (seconds), `created_today`, `larger_than` (bytes)
and `secret`.

## Development

```sh
cargo test
xvfb-run cargo test -- --ignored        # live clipboard round trip (Linux)
```

The macOS-only code (frontmost app, browser page lookup, run loop) builds on
Linux with `cargo zigbuild --target aarch64-apple-darwin --lib`.
