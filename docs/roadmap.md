# Roadmap

ClipRust stays small: the default app has no network access, no background helpers and idles at about 17 MB. Anything heavier is an **add-on**. Add-ons aren't in the default download. When one is off it isn't loaded and uses no memory or CPU.

## Add-on model
- **Separate binaries.** Each add-on is its own executable (`cliprust-mcp`, `cliprust-ocr`, …) in the same Cargo workspace, built behind a cargo feature and shipped as a separate release asset and Homebrew formula.
- **Same database.** An add-on reads the shared database through the core library, the same way the CLI does. The app never links it.
- **Opt-in.** Settings → Add-ons lists the ones installed (found on `PATH` or in `~/Library/Application Support/cliprust/addons`) with a toggle each. Turning one off stops its process.
- **Browser add-ons** live in `extensions/` and ship through the browser's store, not with the app. The Firefox one ([extensions/firefox](../extensions/firefox)) adds the site (origin only) to copies, so clips from Firefox and Zen get a source.
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
| Opt-in E2E sync | Several Macs | add-on, later; never on by default |

## Distribution
| Item | Why |
|---|---|
| Firefox add-on on addons.mozilla.org | Install it without loading a temporary add-on by hand |
| Homebrew cask and formula | `brew install --cask cliprust` for the app, a formula for the CLI |
| Intel build | Releases are Apple silicon only today |
| Update notice | Tell you when a new release is out; opt-in, since the app makes no network requests by default |

## Not planned
Cloud accounts, telemetry, ads or paid tiers. ClipRust stays free and MIT.
