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
  2. **Filter bar.** A row of compact capsule chips: All · Text · Links · Images · Files · Keys, followed by the user's lists (manual and smart, with an SF Symbol or emoji name). A chip shows a count only when a filter is active. ⌘←/⌘→ or ⇥/⇧⇥ moves between chips.
  3. **Body.** The list is on the left. A preview pane (toggled with ⌘P, on by default) takes 42 % of the width on the right.
- **List rows.** A row is 34 pt high in Compact and 44 pt in Comfortable. Each row contains:
  - **leading:** the source app's icon at 16 pt, or a kind glyph when the app is unknown
  - **main:** one line of preview text, or the title if one is set. For an image, a 28 pt thumbnail plus its size. For a file, the file name plus the path tail in secondary colour. For a key, the masked value in monospace plus the provider.
  - **trailing:** relative time in secondary colour, then a `⌘1`–`⌘9` hint on the first nine rows
  - a pin dot when the item belongs to a manual list
  - a note glyph when the item has a note
- **Selection.** The selected row shows an accent-tinted rounded highlight (radius 7). Hover shows a subtle fill.
- **Preview pane** content by kind:
  - **text:** selectable monospace when it looks like code, otherwise the system font
  - **rich text:** rendered RTF or HTML
  - **image:** fitted, with its pixel size
  - **file:** a Quick Look thumbnail and the path
  - **URL:** the URL plus its host
- **Preview footer:** source app · copied time · times used · formats · size, and an editable note field ("Add a note…").
- **Empty states:** a single centred line in secondary colour, such as "Nothing copied yet" or "No matches for "foo"".

**Keyboard in the panel:**

| Key | Action |
| --- | --- |
| type | search |
| ↑ ↓ / ⌃N ⌃P | move selection |
| ⏎ | paste into the previous app (or copy only, per setting) |
| ⌥⏎ | paste as plain text |
| ⌘⏎ | copy only, no paste |
| ⌘1…⌘9 | paste row N |
| ⌘P | toggle preview |
| ␣ (when search is empty) | Quick Look the item |
| ⌘⌫ | delete item |
| ⌘D | add to list… (popover menu of manual lists, plus "New list…") |
| ⌘R | rename (title) |
| ⌘N | edit note |
| ⌘K | open Keys & IDs |
| ⌘, | Settings |
| Esc | clear search, else close |

Clicking outside the panel closes it.

**Right-click on a row:** Paste · Paste as Plain Text · Copy · Add to List ▸ · Rename… · Edit Note… · Reveal in Finder (files) · Delete.

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
  - Open shortcut (recorder, default ⇧⌘V)
  - Launch at login (`SMAppService.mainApp`)
  - Panel position
  - After choosing an item: Paste into the active app / Copy only
  - Always paste as plain text
  - Show menu-bar icon
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
- **Import:** Import from PasteNow… (shows counts when done) and the database location with "Show in Finder".

## Architecture

```
app/
  Package.swift                 SwiftPM, macOS 26, executable target ClipRust, C target CCliprust
  Sources/CCliprust/            cliprust.h + module.modulemap (links ../../target/release/libcliprust.a)
  Sources/ClipRust/
    ClipRustApp.swift           @main; AppDelegate; LSUIElement; owns Core, StatusItem, PanelController
    Core/Core.swift             FFI wrapper: a serial background queue; async methods; event stream
    Core/Models.swift           Codable mirrors of the Rust JSON
    MenuBar/StatusItem.swift    NSStatusItem, icon drawing, menu
    Panel/PanelController.swift NSPanel subclass, show/hide/position, key handling
    Panel/PanelView.swift       search + chips + list + preview (SwiftUI)
    Panel/ItemRow.swift
    Panel/PreviewPane.swift
    Keys/KeysView.swift         Keys & IDs window
    Settings/SettingsView.swift
    System/Hotkey.swift         Carbon RegisterEventHotKey wrapper + ShortcutRecorder view
    System/Paster.swift         Accessibility check/prompt, CGEvent ⌘V to the previous app
    System/Prefs.swift          @AppStorage keys and defaults
  scripts/build-app.sh          cargo build --release --features ffi → swift build -c release
                                → assemble ClipRust.app (Info.plist, icon) → codesign
                                with the "Apple Development" identity (stable, so TCC grants survive rebuilds)
```

- No third-party Swift packages.
- The Swift language mode is 5, with strict concurrency off.
- UI state lives on `@MainActor` `@Observable` models.

### FFI contract (Rust `src/ffi.rs`, `crate-type = ["lib", "staticlib"]`, feature `ffi`)

A small C ABI carries JSON requests and responses, so Swift decodes them with `Codable` and no code generation is needed.

```c
typedef struct CrHandle CrHandle;
typedef void (*cr_event_cb)(const char *event_json, void *ctx);

CrHandle *cr_open(const char *db_path /* NULL = default */, char **err_out);
char     *cr_call(CrHandle *h, const char *request_json);   // always returns JSON, caller frees
int       cr_item_data(CrHandle *h, int64_t id, const char *format, uint8_t **out, size_t *len); // 0 = ok
void      cr_watch(CrHandle *h, cr_event_cb cb, void *ctx); // starts the watcher thread once
void      cr_free_string(char *s);
void      cr_free_bytes(uint8_t *p, size_t len);
void      cr_close(CrHandle *h);
```

**Requests and responses.**
- A request has the form `{"method": "...", "params": {...}}`.
- The response is `{"ok": <value>}` on success and `{"err": "message"}` on failure.
- Any Rust panic is caught at the boundary and returned as `err`.

**Methods** (the params mirror the existing store API):

| Method | Params / returns |
| --- | --- |
| `items` | `{query?, kind?, list?, limit}` → `[ItemSummary]` |
| `item` | `{id}` → summary plus `formats: [String]`, full `text?`, `note?` |
| `paste_prep` | `{id, plain}` writes every representation to the pasteboard (or plain text only) and touches the item |
| `delete` | `{id}` |
| `set_title` | `{id, title?}` |
| `set_note` | `{id, note?}` |
| `lists` | `[{name, rule?, sort, manual}]` |
| `create_list` | `{name, rule?}` |
| `delete_list` | `{name}` |
| `add_to_list` | `{name, id}` |
| `remove_from_list` | `{name, id}` |
| `secrets` | `{kind?, provider?, domain?, limit}` → `[ItemSummary]` (keys and IDs) |
| `secret_facets` | `{kind?}` → `{providers: [{name, count}], domains: [{name, count}]}` |
| `reveal_key` | `{id}` → full value (Swift gates it behind LocalAuthentication) |
| `settings` / `save_settings` | `Settings` |
| `excludes` / `add_exclude` / `remove_exclude` | the ignored apps |
| `stats` | |
| `clear` | |
| `import_pastenow` | `{path?}` → counts |

**Details:**
- **Events:** `{"type": "stored", "id": 12}` · `{"type": "skipped", "reason": "..."}` · `{"type": "failed", "error": "..."}`. Swift refreshes the visible page on `stored`.
- **Connections:** the handle holds `Mutex<Store>` for calls, and the watcher thread opens its own `Store` connection (WAL).
- **Main thread:** Swift never calls the FFI there. `Core` hops to a serial `DispatchQueue` and back.

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
