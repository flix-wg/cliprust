# Roadmap

ClipRust stays small: the default app has no network access, no background helpers and idles at about 17 MB. Anything heavier is an **add-on**. Add-ons aren't in the default download. When one is off it isn't loaded and uses no memory or CPU.

## Add-on model
- **Separate binaries.** Each add-on is its own executable (`cliprust-mcp`, `cliprust-ocr`, …) in the same Cargo workspace, built behind a cargo feature and shipped as a separate release asset and Homebrew formula.
- **Same database.** An add-on reads the shared database through the core library, the same way the CLI does. The app never links it.
- **Opt-in.** Settings → Add-ons lists the ones installed (found on `PATH` or in `~/Library/Application Support/cliprust/addons`) with a toggle each. Turning one off stops its process.
- **Swift-only features** (for example Vision OCR) live in a lazily loaded bundle or helper. They do nothing until enabled.

## Planned
| Feature | Why | Form |
|---|---|---|
| MCP server | Let coding agents search clips, list recent ones and paste; keys only after Touch ID | add-on `cliprust-mcp` (stdio, launched by the MCP client, no idle process) |
| OCR on images | Search text inside screenshots | add-on, Apple Vision, runs on capture or as a backfill job |
| Paste stack | Copy several things, paste them in order | core |
| Text transforms | JSON pretty-print and minify, base64, URL encode/decode, case, trim; optional shell filters | core (built-in) + user shell filters |
| Auto-expire sensitive clips | Keys and IDs delete themselves after N hours | core |
| Regex search and regex smart lists | `re:` token | core |
| Snippets | Saved text with placeholders | core |
| Merge clips | Join selected clips with a separator | core |
| Shortcuts actions and URL scheme | Automation | app |
| Ignore list per app | Never record from chosen apps (password managers already skipped) | app |
| Opt-in E2E sync | Several Macs | add-on, later; never on by default |

## Not planned
Cloud accounts, telemetry, ads or paid tiers. ClipRust stays free and MIT.
