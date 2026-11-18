# ClipRust for Mac: app design

A menu-bar clipboard manager: a thin SwiftUI/AppKit shell over the Rust core in this repo. A single process with no helper and no network. It sits at about 30 MB including AppKit and SwiftUI, and the Rust watcher accounts for about 5 MB of that.

## Principles

- **Keyboard first, mouse friendly.** Every action has a key. Nothing needs the mouse.
- **Quiet.** No Dock icon, no onboarding carousel and no badges. The only permission prompts are Accessibility (for pasting) and, when key detection is on, Automation for the browser in use.
- **Native, not themed.** System materials (Liquid Glass on macOS 26), SF Pro and SF Symbols, the system accent colour, and light and dark modes. No custom brand colours.
- **Customizable where it matters.** The shortcut, panel position, density, preview pane, paste behaviour, retention, exclusions and key detection can all be changed. Everything else is fixed.

## Surfaces

### 1. Menu-bar icon

- The icon is a template image drawn in code as two offset rounded rectangles: the back one at 1.25 pt stroke, the front one filled at 1.25 pt corner. It fits a 16×16 pt box, so the system tints it for light, dark and highlighted states.
- **Paused:** the icon draws at 45 % opacity with a small diagonal slash. In the setting where the icon is hidden, nothing is shown.
- **Left click** toggles the panel (same as the hotkey).
- **Right click or ⌥-click** opens a menu:
  - the 5 most recent items, as one-line previews, where ⌘1–⌘5 paste
  - Open ClipRust (with the shortcut shown)
  - Pause recording ✓
  - Keys & IDs… (⌘K)
  - Settings… (⌘,)
  - Quit (⌘Q)

### 2. Panel (the main surface)

- **Window:** a non-activating floating `NSPanel` at the `.floating` level, which joins all Spaces and works over full-screen apps. The app that had focus keeps it, so pasting goes back to that app.
- **Size:** 680 × 440 pt by default and resizable (min 520 × 320), remembered across launches.
- **Background:** a glass material with a 14 pt corner radius and a hairline separator colour for the border. No heavy shadow.
- **Position setting:**
  - centred on the active screen (default)
  - at the mouse cursor
  - under the menu-bar icon
- **Layout, top to bottom:**
  1. **Search field.** It is always focused, uses 15 pt text and has a magnifier glyph. Placeholder: "Search clipboard". Typing filters at once, searching text, title and note, across the whole history and not just the loaded page.
     - **Source app.** A term that names an app (prefix of its name, a word of it, or a bundle-id component, so "codex" finds `com.openai.codex`) shows a "Copied from Mail · 42" row above the results. ⇥, a click, or ↑ then ⏎ turns it into an app token (icon + name capsule) before the text; the rest of the query then searches that app only. `from:mail` or `@mail` filters directly and becomes the token once a space follows. ⌫ at the start of the field or the token's × removes it. ⌘L lists apps under "Apps" too.
     - **Contacts.** Typing "phone", "number", "email", "mail address", "address" or "contact" (any prefix of three letters or more) shows a "Phone numbers · 12" row above the results; ⇥, a click, or ↑ then ⏎ turns it into a contact token, like the app token. `is:phone`, `is:email`, `is:address` or `is:contact` (any of them) become the same token once a space follows. It keeps only clips the core recognised as a phone number, email address or postal address, narrows the Contacts chip further, and the rest of the query searches within them. ⌘L lists Phone numbers, Emails and Addresses under "Contacts" when they hold clips. ⌘⇧S saves the type into the smart list.
  2. **Filter bar.** A row of compact capsule chips: All, Pinned (the list of pinned items, while it holds any), the other pinned lists, Keys (while key detection is on, or once a key exists), Contacts (once a phone number, email or address exists), Images · Files · Links, the other lists in the order set in Settings (manual and smart, with an SF Symbol or emoji name), and Text last. The four type chips can be hidden in Settings. A chip shows a count only when a filter is active. ←/→ (with an empty search, or the caret at its start or end), ⌘←/⌘→ or ⇥/⇧⇥ move between chips, wrapping, and the bar scrolls the chip into view; ⌃1–⌃9 jumps to chip N, and holding ⌃ shows the numbers. The panel opens on All or the last used list (setting).
  3. **Body.** The list is on the left. A preview pane (toggled with ⌘P, on by default) takes 42 % of the width on the right.
- **List rows.** A row is 34 pt high in Compact and 44 pt in Comfortable. Each row contains:
  - **leading:** the source app's icon at 16 pt, or a kind glyph when the app is unknown
  - **main:** one line of preview text, or the title if one is set. For an image, a 28 pt thumbnail plus its size. For a file, the file name plus the path tail in secondary colour. For a key, the title or the masked value in monospace, the provider and the note in secondary colour.
  - **trailing:** relative time in secondary colour, then a `⌘1`–`⌘9` hint on the first nine rows
  - a pin when the item is pinned (⌘⇧P); pinned items are never pruned
  - a note glyph when the item has a note
  - a phone, envelope or map-pin glyph after the leading icon when the clip is a phone number, email or address
- **Selection.** The selected row shows an accent-tinted rounded highlight (radius 7). Hover shows a subtle fill.
- **Preview pane** content by kind:
  - **text:** selectable monospace when it looks like code, otherwise the system font
  - **rich text:** rendered RTF or HTML
  - **image:** fitted, with its pixel size
  - **file:** a Quick Look thumbnail and the path
  - **URL:** the URL plus its host
- **Preview footer:** source app · copied time · times used · formats · size, and an editable note field ("Add a note…").
- **Inline edit:** ⌘N (note) or ⌘R (title) opens a field under the selected row, prefilled and focused. ⏎ saves, Esc cancels, saving it empty removes the note or title. Selecting another row cancels it. Works for keys and IDs, whose note labels them ("prod Cloudflare, DNS edit") and is searchable.
- **Opening:** the panel keeps the page already in memory unless the core changed since (`data_version`), the filter changed or it is a smart list. Then it reloads the first page synchronously before the panel is drawn (a millisecond or two), so the first frame is current. The search is reset and the page reloaded when it hides, and a stored clip reloads a visible panel in the background.
- **Empty states:** a single centred line in secondary colour, such as "Nothing copied yet" or "No matches for "foo"".

**Keyboard in the panel:**

| Key | Action |
| --- | --- |
| type | search |
| ↑ ↓ / ⌃N ⌃P | move selection |
| ← → (empty search, or caret at the start / end) | previous / next filter chip |
| ⇥ ⇧⇥ / ⌘← ⌘→ | previous / next filter chip |
| ⏎ | paste into the previous app (or copy only, per setting) |
| ⌥⏎ | paste as plain text |
| ⌘⏎ | copy only, no paste |
| ⌘1…⌘9 | paste row N |
| ⌘P | toggle preview |
| ␣ (when search is empty) | Quick Look the item |
| ⌘⌫ | delete item |
| ⇥ (with an app suggestion) | make the app the search token |
| ⌫ (at the start of the search field) | remove the app token |
| ⌘⇧P | pin or unpin the item |
| ⌘L | go to list: palette of every chip, then apps; ⏎ on an app shows its clips, ⏎ on an unknown name creates a manual list and switches to it |
| ⌘D | add to list: palette of manual lists; ⏎ on an unknown name creates the list and adds; "Added to X" toast |
| ⌘⇧S | save search as a smart list (text, app token, `is:` type and the active type or Contacts filter) and switch to it |
| ⌃1…⌃9 | jump to filter chip N |
| ⌘R | rename (title), inline |
| ⌘N | edit note, inline |
| ⌘K | open Keys & IDs |
| ⌘, | Settings |
| ⌘W | close the panel |
| Esc | close the list palette, else clear search and app token, else close |

Settings → Shortcuts rebinds every action above except ⏎, Esc, ⇥, the arrows, ⌘1–⌘9, ⌃1–⌃9 and ⌃N/⌃P, which stay fixed. A shortcut held by another action, the open shortcut, a fixed key or ⌘A/⌘C/⌘V/⌘X/⌘Z is refused with the reason next to it.

The list palette is a 280 pt glass overlay under the search field: type to filter (prefix matches first), ↑↓ or ⌃N/⌃P to move, ⏎ to confirm, Esc or a click outside to close. Lists are reordered, pinned, renamed and deleted in Settings → Lists.

Clicking outside the panel closes it.

**Right-click on a row:** Paste · Paste as Plain Text · Copy · Call / New Email / Open in Maps (contacts; the first number or address found, Maps gets the whole clip) · Pin · Add to List ▸ · Rename · Edit Note · Reveal in Finder (files) · Delete.

### 3. Keys & IDs window

A standard window (900 × 560), opened from the menu or with ⌘K.

- **Sidebar** (source list):
  - All Keys
  - IDs
  - **Providers:** each provider name with its count, such as OpenAI 3, Cloudflare 2, GitHub 1
  - **Sites:** each domain with its count
- **Main:** a `Table` with the columns Name, Value (masked, monospace), Provider, Site, App, Copied and Note. It sorts by any column (default Copied, newest first) and has a search field in the toolbar.
- **Detail pane / inspector** (toggle ⌥⌘I):
  - a large name field (editable title)
  - the masked value with a Reveal button (needs Touch ID or the password, when the setting is on) and a Copy button (same rule)
  - the source URL as a link, the app, the time
  - the note, as a multi-line editor
- **Actions:**
  - Copy (⌘C): needs the authentication described above
  - Copy masked
  - Open source page
  - Delete
  - Remove from Keys (to correct a false positive)

### 4. Settings window (tabs, standard `Settings` scene)

- **General:**
  - Launch at login (`SMAppService.mainApp`)
  - Panel position
  - After choosing an item: Paste into the active app / Copy only
  - Always paste as plain text
  - Show menu-bar icon
- **Shortcuts:**
  - Open ClipRust (recorder, default ⇧⌘V)
  - In the Panel: a recorder per action with a per-row reset and Reset All; stored as one `panelShortcuts` dictionary in UserDefaults
- **Appearance:**
  - Density: Compact / Comfortable
  - Show preview pane
  - Show app icons
  - Show shortcut hints
  - Panel size: Reset
- **History:**
  - Keep: 500 / 1 000 / 5 000 / 10 000 / unlimited items
  - Delete items older than: never / 7 / 30 / 90 / 365 days
  - Pause recording
  - Ignored apps (list with + to pick an app via `NSOpenPanel` in /Applications, − to remove)
  - Clear history… (confirmation dialog; items in lists are kept)
- **Keys:**
  - Detect API keys and IDs
  - Require Touch ID to reveal or copy keys
  - Look up the browser page when a key is copied (explains the one-time Automation prompt)
- **Security:**
  - Saved keys: status (encrypted, none yet, locked or unavailable) and how many still wait to be encrypted
  - Encryption key: Rotate… (confirm); Old keys: Remove… (confirm; older copies then need their recovery code)
  - Recovery code: Show… always asks for Touch ID or the password, then Copy (concealed) or Hide; Restore key from a pasted code
  - Unreadable keys: Forget… (confirm), shown only while some exist
  - Encrypt the whole database (confirm; warns that older backups and snapshots keep the old data)
- **Import:** Import from PasteNow… (shows counts when done) and the database location with "Show in Finder".

## Architecture

```
src/                            Rust core, CLI and C ABI
  lib.rs                        public API, Error, default database path
  main.rs                       the `cliprust` CLI
  model.rs                      Capture, Item, ItemSummary, Settings
  schema.rs                     SQLite schema and migrations
  store/                        the Store, one file per responsibility
    mod.rs                      open, data_version, checkpoint, stats
    items.rs                    ingest, single items, edits, deletion, retention
    reps.rs                     reading and writing payloads (compression and sealing)
    keys.rs                     sealing key items, the locked state, rotation, recovery codes
    file.rs                     plaintext or SQLCipher file, switching modes, replaced-file check
  crypto.rs                     master keys (Keychain), HKDF subkeys, XChaCha20-Poly1305
    search.rs                   find, apps, contact counts
    lists.rs                    manual and smart lists
    secrets.rs                  detected keys and IDs, rescans
    settings.rs                 settings and excluded apps
  query.rs                      the search grammar shared by the panel and the CLI
  rules.rs                      smart-list rules compiled to SQL
  secrets.rs, contacts.rs       key, ID and contact detection
  capture.rs, monitor.rs, platform.rs   pasteboard reading/writing and the watch loop
  import_pastenow.rs            PasteNow import
  ffi/mod.rs                    C ABI, error codes, watcher events
  ffi/methods.rs                the cr_call methods
app/
  Package.swift                 SwiftPM, macOS 26, executable target ClipRust, C target CCliprust
  Sources/CCliprust/            cliprust.h + module.modulemap (links ../../target/release/libcliprust.a)
  Sources/ClipRust/
    ClipRustApp.swift           @main; AppDelegate; LSUIElement; owns Core, StatusItem, PanelController
    Core/                       Core (FFI wrapper on a serial queue), AppState, Codable models
    MenuBar/                    NSStatusItem, icon drawing, menu
    Panel/                      NSPanel, search + chips + list + preview, list palette, Quick Look
    Keys/                       Keys & IDs window
    Settings/                   Settings window
    System/                     hotkey, panel shortcuts, recorder, paster, prefs, thumbnails, Touch ID gate
    Preview/                    sample data and renderers for screenshots and the app icon (dev aids)
  scripts/build-app.sh          cargo build --release --features ffi → swift build -c release
                                → assemble ClipRust.app (Info.plist, icon) → codesign
                                with the "Apple Development" identity (stable, so TCC grants survive rebuilds)
```

- No third-party Swift packages.
- The Swift language mode is 5, with strict concurrency off.
- UI state lives on `@MainActor` `@Observable` models.

### Search grammar (`src/query.rs`)

The core reads every search; the app and `cliprust search` accept the same syntax.

- Plain words must each occur in the text, title or note (substring, case-insensitive).
- `from:mail` or `@mail` keeps clips copied from the best stored app whose name, a word of it or a bundle-id component starts with "mail". Only the first six words are read as apps. A word that names no stored app is searched as text. An explicit `app` (the panel's app token) turns this off.
- `is:phone`, `is:email`, `is:address`, or `is:contact` for any of them, keeps clips recognised as that contact type. All `is:` words leave the text; the last one wins and overrides an explicit `contact`.

Swift only draws the result: `parse_query` tells it which word named an app and which words were `is:` words, and those become capsules once a space follows them. The suggestion rows ("Copied from Mail", "Phone numbers") are app UI and stay in Swift.

### FFI contract (`src/ffi/`, `crate-type = ["lib", "staticlib"]`, feature `ffi`)

A small C ABI carries JSON requests and responses, so Swift decodes them with `Codable` and no code generation is needed.

```c
typedef struct CrHandle CrHandle;
typedef void (*cr_event_cb)(const char *event_json, void *ctx);

CrHandle   *cr_open(const char *db_path /* NULL = default */, char **err_out); // err_out: {"code", "message"}
int64_t     cr_restore_key(const char *db_path, const char *code, char **err_out); // generation, or -1
char       *cr_call(const CrHandle *h, const char *request_json); // always returns JSON, caller frees
int32_t     cr_item_data(const CrHandle *h, int64_t id, const char *format, uint8_t **out, size_t *len);
void        cr_watch(const CrHandle *h, cr_event_cb cb, void *ctx); // starts the watcher thread once
void        cr_own_change(intptr_t count); // the app wrote pasteboard change `count`; the watcher skips it
const char *cr_version(void);              // crate version, static
void        cr_free_string(char *s);
void        cr_free_bytes(uint8_t *p, size_t len);
void        cr_close(CrHandle *h);
```

**Requests and responses.**
- A request has the form `{"method": "...", "params": {...}}`; `params` may be left out when a method takes none. Rust parses it into one enum variant per method.
- The response is `{"ok": <value>}` on success and `{"err": {"code": "...", "message": "..."}}` on failure.
- Error codes: `not_found` (no such item or list), `invalid` (a malformed request, an unknown method, or one the store refuses, such as a duplicate list name), `locked` (the master key is missing from, or refused by, the Keychain), `io` (database or pasteboard failure), `internal` (a bug; any Rust panic is caught at the boundary and returned with this code).
- `cr_item_data` returns 0 with the bytes, 1 when the format is absent and 2 on error or for a masked key. `format` is one of `item.formats`, or `"image"` for the first image format.

**Methods:**

| Method | Params / returns |
| --- | --- |
| `items` | `{query?, kind?, list?, app?, contact?, limit}` → `[ItemSummary]`; every given filter applies. `query` is in the search grammar, `app` is a bundle id, `contact` is `phone`, `email`, `address` or `any` |
| `parse_query` | `{query, app?}` → `{text, app?: {bundle_id, name?, count}, app_term?, contact?, contact_terms}`, how `items` reads the query |
| `item` | `{id}` → summary plus `formats: [String]`, full `text?`, `note?`; only the text format is decompressed |
| `apps` | `{query?}` → `[{bundle_id, name?, count}]`, apps of stored items, most copied first; a query matches a prefix of the name, a word of it or a bundle-id component |
| `counts` | → `{phone, email, address, any}`, items per contact type |
| `data_version` | → an opaque value that changes once any process commits a change; the panel compares it before reloading |
| `checkpoint` | folds the WAL back without waiting for other connections (sent when the panel hides) |
| `paste_prep` | `{id, plain}` writes every representation to the pasteboard (or plain text only) and touches the item; `invalid` for a key |
| `delete` | `{id}` |
| `set_title` | `{id, title?}` |
| `set_note` | `{id, note?}` |
| `lists` | `[{name, rule?, sort, manual}]` |
| `list_ids` | `{name}` → ids of the items in a manual list |
| `create_list` | `{name, rule?}` |
| `rename_list` | `{name, new_name}` |
| `delete_list` | `{name}` |
| `add_to_list` | `{name, id}` |
| `remove_from_list` | `{name, id}` |
| `secrets` | `{kind?, provider?, domain?, limit}` → `[ItemSummary]` (keys and IDs) |
| `secret_facets` | `{kind?}` → `{providers: [{name, count}], domains: [{name, count}]}` |
| `reveal_key` | `{id}` → full value (Swift gates it behind LocalAuthentication) |
| `settings` / `save_settings` | `Settings`; saving applies retention |
| `excludes` / `add_exclude` / `remove_exclude` | the ignored apps, `{bundle_id}` |
| `stats` | → `{items, raw_bytes, stored_bytes, file_bytes, path}` |
| `clear` | → number of deleted items |
| `import_pastenow` | `{path?}` → counts |
| `encryption_status` | → `{keys, generation?, message?, pending, unreadable, db_encrypted, db_encryption_available}`; `keys` is `ready`, `no_key`, `locked`, `unavailable` or `unsupported` |
| `set_db_encryption` | `{on}` encrypts the database file with SQLCipher or decrypts it → status; `invalid` while another process has the file open |
| `rotate_key` | → the new key generation; re-seals every key and re-keys an encrypted file. The old key stays in the Keychain for older copies |
| `prune_keys` | deletes older generations no row or file uses from the Keychain, after a completed checkpoint → their generations; `invalid` while busy |
| `recovery_code` | → the master key as `cliprust-key-<gen>-<hex>` (Swift gates it behind LocalAuthentication) |
| `restore_key` | `{code}` puts the key back after checking it belongs to this database → its generation |
| `forget_unreadable_keys` | deletes the key items whose key is missing → number deleted; only then can a new key be created |

**Details:**
- **Events:** `{"type": "stored", "id": 12, "new": true, "kind": "text", "secret": false, "secret_kind": null}` · `{"type": "skipped", "reason": "..."}` · `{"type": "secret_skipped", "reason": "..."}` (a copied key was not stored because its key is unavailable; it is still on the clipboard) · `{"type": "failed", "error": "..."}`. Swift refreshes the visible page on `stored`.
- **Own changes:** after writing the pasteboard itself (a revealed key), the app calls `cr_own_change` with the count `clearContents` returned, so the watcher does not store it again; reading `changeCount` after the write could adopt another app's copy. The core's `paste_prep` marks its write only when it was the single change.
- **Opening a locked database:** when `cr_open` fails with `locked` (an encrypted file that no Keychain key opens), the app asks for a recovery code, passes it to `cr_restore_key` and opens again.
- **Notices:** the panel shows a notice while `encryption_status` reports unreadable keys (with an inline confirm for `forget_unreadable_keys`) and after a `secret_skipped` event until dismissed.
- **Connections:** the handle holds one `Arc<Mutex<Store>>` that calls and the watcher thread share, so the app has a single connection; switching database encryption needs it to be the only one.
- **Main thread:** `Core` hops to a serial `DispatchQueue` and back. The one exception is opening the panel, which waits up to 30 ms for `data_version` and, if needed, the first page so the first frame is current; when the queue is busy it shows the page it has and reloads right after.

### Paste flow

1. The hotkey records `NSWorkspace.frontmostApplication` as the target, then shows the panel without activating ClipRust.
2. ⏎ calls `paste_prep` (Rust writes the pasteboard; the watcher sees the change and moves that item to the top). The panel then closes, and the target app is re-activated if needed.
3. After 60 ms, a CGEvent ⌘V goes to the target. Without Accessibility permission the item is only copied, and a one-time sheet explains how to grant the permission.

## Visual details

- **Type:** SF Pro Text 13 for rows, 11 for secondary text, 15 for search. SF Mono 12 for code, keys and IDs.
- **Spacing:** an 8 pt grid. The panel has 10 pt insets and 6 pt between chips. Rows have 10 pt horizontal padding.
- **Colour:** only `.primary`, `.secondary` and `.tertiary`, the accent colour, and system fills. Keys get a small capsule in `.orange.opacity(0.15)` with an orange "KEY" label (or "ID" in blue), so they stand out without being loud.
- **Motion:** the panel fades and scales in from 0.98 over 120 ms, and the selection moves without animation (instant feels faster). Reduce Motion is respected.
- **Icons:** SF Symbols `doc.text`, `link`, `photo`, `folder`, `key.horizontal`, `number`, `pin.fill`, `note.text`.
