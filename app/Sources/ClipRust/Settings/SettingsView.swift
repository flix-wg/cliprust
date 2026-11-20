import AppKit
import ServiceManagement
import SwiftUI
import UniformTypeIdentifiers

private extension View {
    func settingsPane() -> some View {
        formStyle(.grouped)
            .scrollDisabled(true)
            .frame(width: 500)
            .fixedSize(horizontal: false, vertical: true)
    }
}

private struct Caption: View {
    let text: String
    init(_ text: String) { self.text = text }
    var body: some View {
        Text(text).font(.system(size: 11)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
    }
}

struct GeneralPane: View {
    let app: AppState
    @AppStorage(Prefs.Key.panelPosition) private var position = PanelPosition.center
    @AppStorage(Prefs.Key.afterChoosing) private var after = AfterChoosing.paste
    @AppStorage(Prefs.Key.plainText) private var plainText = false
    @AppStorage(Prefs.Key.showMenuIcon) private var showMenuIcon = true
    @State private var launchAtLogin = SMAppService.mainApp.status == .enabled
    @State private var trusted = Paster.isTrusted

    var body: some View {
        Form {
            Section {
                Picker("Panel position", selection: $position) {
                    ForEach(PanelPosition.allCases) { Text($0.title).tag($0) }
                }
            }
            Section {
                Picker("After choosing an item", selection: $after) {
                    Text("Paste into the active app").tag(AfterChoosing.paste)
                    Text("Copy only").tag(AfterChoosing.copy)
                }
                Toggle("Always paste as plain text", isOn: $plainText)
                LabeledContent {
                    if trusted {
                        Label("Allowed", systemImage: "checkmark.circle.fill")
                            .foregroundStyle(.secondary)
                    } else {
                        Button("Allow…") { AccessGuide.shared.show() }
                    }
                } label: {
                    Text("Accessibility")
                    Text("Needed to paste into other apps.")
                }
            }
            Section {
                Toggle("Launch at login", isOn: Binding(get: { launchAtLogin }, set: setLaunchAtLogin))
                Toggle("Show menu-bar icon", isOn: $showMenuIcon)
                if !showMenuIcon {
                    Caption("Open settings from the panel with ⌘,.")
                }
            }
        }
        .settingsPane()
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            trusted = Paster.isTrusted
            launchAtLogin = SMAppService.mainApp.status == .enabled
        }
    }

    private func setLaunchAtLogin(_ on: Bool) {
        if on { try? SMAppService.mainApp.register() } else { try? SMAppService.mainApp.unregister() }
        launchAtLogin = SMAppService.mainApp.status == .enabled
    }
}

struct AppearancePane: View {
    @AppStorage(Prefs.Key.density) private var density = Density.compact
    @AppStorage(Prefs.Key.showPreview) private var showPreview = true
    @AppStorage(Prefs.Key.showAppIcons) private var showAppIcons = true
    @AppStorage(Prefs.Key.showHints) private var showHints = true

    var body: some View {
        Form {
            Section {
                Picker("Density", selection: $density) {
                    Text("Compact").tag(Density.compact)
                    Text("Comfortable").tag(Density.comfortable)
                }
                .pickerStyle(.segmented)
                Toggle("Show preview pane", isOn: $showPreview)
                Toggle("Show app icons", isOn: $showAppIcons)
                Toggle("Show shortcut hints", isOn: $showHints)
            }
            Section {
                LabeledContent {
                    Button("Reset") { Prefs.resetPanelSize() }
                } label: {
                    Text("Panel size")
                    Text("Drag the panel’s edges to resize it.")
                }
            }
        }
        .settingsPane()
    }
}

struct ListsPane: View {
    let app: AppState
    @AppStorage(Prefs.Key.showTypeFilters) private var showTypeFilters = true
    @AppStorage(Prefs.Key.openOn) private var openOn = OpenOn.all
    @State private var deleting: ClipList?

    var body: some View {
        Form {
            Section {
                Toggle("Show type filters (Images, Files, Links, Text)", isOn: $showTypeFilters)
                Picker("Open panel on", selection: $openOn) {
                    Text("All").tag(OpenOn.all)
                    Text("Last used list").tag(OpenOn.last)
                }
            }
            Section {
                if app.orderedLists.isEmpty {
                    Text("No lists yet. Press \(PanelShortcuts.shared[.goToList].display) in the panel to create one.").foregroundStyle(.secondary)
                }
                ForEach(app.orderedLists) { list in
                    ListRow(app: app, list: list) { deleting = list }
                }
                .onMove { from, to in
                    var names = app.orderedLists.map(\.name)
                    names.move(fromOffsets: from, toOffset: to)
                    app.listOrder = names
                }
            } header: {
                Text("Lists")
            } footer: {
                Caption("Drag to reorder. Pinned lists come first in the filter bar. ←/→ or ⌃1–⌃9 move between chips, \(PanelShortcuts.shared[.goToList].display) goes to any list.")
            }
        }
        .settingsPane()
        .confirmationDialog("Delete “\(deleting?.name ?? "")”?", isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }),
                            presenting: deleting) { list in
            Button("Delete List", role: .destructive) { Task { await app.deleteList(list.name) } }
        } message: { list in
            Text(list.manual ? "Its items stay in your history but are no longer kept by the list." : "Its items stay in your history.")
        }
    }
}

private struct ListRow: View {
    let app: AppState
    let list: ClipList
    let delete: () -> Void
    @State private var name = ""

    var body: some View {
        let pinned = app.pinnedLists.contains(list.name)
        HStack(spacing: 8) {
            Image(systemName: list.manual ? "list.bullet" : "line.3.horizontal.decrease")
                .foregroundStyle(.secondary)
                .frame(width: 18)
            TextField("Name", text: $name)
                .labelsHidden()
                .textFieldStyle(.plain)
                .multilineTextAlignment(.leading)
                .onSubmit(rename)
            Button { app.togglePin(list.name) } label: {
                Image(systemName: pinned ? "pin.fill" : "pin")
                    .foregroundStyle(pinned ? AnyShapeStyle(.tint) : AnyShapeStyle(.secondary))
            }
            .buttonStyle(.borderless)
            .help(pinned ? "Unpin" : "Pin to the front of the filter bar")
            Button(action: delete) {
                Image(systemName: "minus.circle.fill").foregroundStyle(.secondary)
            }
            .buttonStyle(.borderless)
            .help("Delete \(list.name)")
        }
        .onChange(of: list.name, initial: true) { _, n in name = n }
    }

    private func rename() {
        let new = name.trimmingCharacters(in: .whitespaces)
        guard !new.isEmpty, new != list.name else { name = list.name; return }
        Task {
            guard await app.renameList(list.name, to: new) else { name = list.name; NSSound.beep(); return }
        }
    }
}

struct ShortcutsPane: View {
    @Bindable private var hotkey = Hotkey.shared
    @Bindable private var shortcuts = PanelShortcuts.shared

    var body: some View {
        Form {
            Section {
                LabeledContent("Open ClipRust") {
                    ShortcutRecorder(shortcut: hotkey.preferred, standard: .standard) { hotkey.preferred = $0; return nil }
                }
                if let message = hotkey.message {
                    Caption(message)
                }
            }
            Section {
                ForEach(PanelCommand.allCases) { command in
                    LabeledContent(command.title) {
                        ShortcutRecorder(shortcut: shortcuts[command], standard: command.standard) {
                            shortcuts.set($0, for: command)
                        }
                    }
                }
            } header: {
                Text("In the Panel")
            } footer: {
                HStack(alignment: .top) {
                    Caption("⏎ pastes, ↑↓ move, ←/→ and ⇥ switch chips, ⌘1–⌘9 paste row N, ⌃1–⌃9 jump to chip N, Esc clears or closes. These stay fixed.")
                    Spacer()
                    Button("Reset All", action: shortcuts.resetAll).disabled(!shortcuts.isCustomized)
                }
            }
        }
        .settingsPane()
    }
}

struct HistoryPane: View {
    let app: AppState
    @State private var confirmClear = false
    @State private var cleared: Int?
    @State private var safari = SafariAccess.unknown

    private static let keepOptions = [500, 1000, 5000, 10000]
    private static let ageOptions = [7, 30, 90, 365]

    var body: some View {
        Form {
            Section {
                Picker("Keep", selection: binding(\.maxItems)) {
                    ForEach(Self.keepOptions, id: \.self) { Text("\($0.formatted()) items").tag(Optional($0)) }
                    Text("Unlimited").tag(Int?.none)
                }
                Picker("Delete items older than", selection: binding(\.maxAgeDays)) {
                    Text("Never").tag(Int?.none)
                    ForEach(Self.ageOptions, id: \.self) { Text($0 == 365 ? "1 year" : "\($0) days").tag(Optional($0)) }
                }
                Toggle("Pause recording", isOn: binding(\.paused))
            } footer: {
                Caption("Items in lists are always kept.")
            }
            Section {
                Toggle(isOn: Binding(get: { app.settings.safariPages }, set: setSafariPages)) {
                    Text("Remember the page for copies from Safari")
                    Text("ClipRust asks Safari for the address of its current tab when you copy there, also in private windows. macOS asks once for permission.")
                }
                if app.settings.safariPages, safari == .denied {
                    LabeledContent {
                        Button("Allow…", action: SafariAccess.openSettings)
                    } label: {
                        Text("Safari access is off")
                        Text("Allow ClipRust to control Safari in Privacy & Security › Automation.")
                    }
                }
            }
            Section("Ignored Apps") {
                if app.excludes.isEmpty {
                    Text("Copies from every app are recorded.")
                        .foregroundStyle(.secondary)
                }
                ForEach(app.excludes, id: \.self) { bundleID in
                    HStack(spacing: 8) {
                        if let icon = AppInfo.icon(bundleID) {
                            Image(nsImage: icon).resizable().frame(width: 18, height: 18)
                        } else {
                            Image(systemName: "app.dashed").frame(width: 18, height: 18).foregroundStyle(.secondary)
                        }
                        Text(AppInfo.name(bundleID))
                        Spacer()
                        Button {
                            Task { await app.removeExclude(bundleID) }
                        } label: {
                            Image(systemName: "minus.circle.fill").foregroundStyle(.secondary)
                        }
                        .buttonStyle(.borderless)
                        .help("Stop ignoring \(AppInfo.name(bundleID))")
                    }
                }
                Button("Add App…", action: pickApp)
            }
            Section {
                LabeledContent {
                    Button("Clear History…", role: .destructive) { confirmClear = true }
                } label: {
                    Text("Clear history")
                    Text(cleared.map { "Removed \($0.formatted()) items." } ?? "Removes everything except items in lists.")
                }
            }
        }
        .settingsPane()
        .task { safari = await SafariAccess.check(ask: false) }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            Task { safari = await SafariAccess.check(ask: false) }
        }
        .confirmationDialog("Clear the clipboard history?", isPresented: $confirmClear) {
            Button("Clear History", role: .destructive) {
                Task { cleared = await app.clearHistory() }
            }
        } message: {
            Text("Items in lists, including Keys, are kept. This can’t be undone.")
        }
    }

    /// Turning it on asks for permission right away, not at the next copy.
    private func setSafariPages(_ on: Bool) {
        app.update { $0.safariPages = on }
        if on { Task { safari = await SafariAccess.check(ask: true) } }
    }

    private func binding<T>(_ path: WritableKeyPath<CoreSettings, T>) -> Binding<T> {
        Binding(get: { app.settings[keyPath: path] }, set: { value in app.update { $0[keyPath: path] = value } })
    }

    private func pickApp() {
        let panel = NSOpenPanel()
        panel.directoryURL = URL(fileURLWithPath: "/Applications")
        panel.allowedContentTypes = [.application]
        panel.allowsMultipleSelection = true
        panel.prompt = "Ignore"
        panel.message = "Copies from these apps won’t be recorded."
        guard panel.runModal() == .OK else { return }
        let ids = panel.urls.compactMap { Bundle(url: $0)?.bundleIdentifier }
        Task { for id in ids { await app.addExclude(id) } }
    }
}

struct KeysPane: View {
    let app: AppState
    @AppStorage(Prefs.Key.requireAuth) private var requireAuth = true

    var body: some View {
        Form {
            Section {
                Toggle(isOn: Binding(get: { app.settings.detectKeys }, set: { v in app.update { $0.detectKeys = v } })) {
                    Text("Detect API keys, passwords and IDs")
                    Text("Keys are masked in the list and filed under Keys.")
                }
                Toggle(isOn: $requireAuth) {
                    Text("Require Touch ID to reveal or copy keys")
                    Text("Your password works too.")
                }
            } footer: {
                Caption("When you copy a key in a browser, ClipRust looks up the page it came from so you can tell keys apart. macOS asks once for permission to read the browser’s current page.")
            }
        }
        .settingsPane()
    }
}

struct SecurityPane: View {
    let app: AppState
    @State private var busy = false
    @State private var result: String?
    @State private var confirm: Confirm?
    @State private var recoveryCode: String?
    @State private var restoreCode = ""

    private enum Confirm: Identifiable {
        case encrypt(Bool), rotate, prune, forget
        var id: String { "\(self)" }
    }

    var body: some View {
        Form {
            if let status = app.encryption {
                keysSection(status)
                if status.dbEncryptionAvailable { databaseSection(status) }
            } else {
                Text("Loading…").foregroundStyle(.secondary)
            }
            if let result {
                Section { Caption(result) }
            }
        }
        .settingsPane()
        .disabled(busy)
        .task { await app.refreshEncryption() }
        .onDisappear { recoveryCode = nil }
        .confirmationDialog(title, isPresented: Binding(get: { confirm != nil }, set: { if !$0 { confirm = nil } }),
                            presenting: confirm) { c in
            switch c {
            case .encrypt(let on):
                Button(on ? "Encrypt Database" : "Decrypt Database") {
                    run { core in
                        let s = try await core.setDbEncryption(on)
                        return s.dbEncrypted ? "The database file is encrypted." : "The database file is stored unencrypted."
                    }
                }
            case .rotate:
                Button("Rotate Key") { run { "Saved keys now use key \(try await $0.rotateKey())." } }
            case .prune:
                Button("Remove Old Keys", role: .destructive) {
                    run { core in
                        let gone = try await core.pruneKeys()
                        return gone.isEmpty ? "There were no old keys to remove." : "Removed \(gone.count) old key\(gone.count == 1 ? "" : "s") from the Keychain."
                    }
                }
            case .forget:
                Button("Delete Unreadable Keys", role: .destructive) {
                    run { _ in "Deleted \(try await app.forgetUnreadableKeys()) unreadable keys." }
                }
            }
        } message: { c in
            switch c {
            case .encrypt(true):
                Text("ClipRust rewrites its database encrypted with a key kept in your Keychain. Copies made before, such as Time Machine backups and APFS snapshots, keep the old unencrypted data. Quit “cliprust watch” first.")
            case .encrypt(false):
                Text("ClipRust rewrites its database without encryption. Saved keys stay encrypted either way.")
            case .rotate:
                Text("Saved keys are encrypted again with a new key. The old key stays in the Keychain so older copies of the database still open, until you remove it.")
            case .prune:
                Text("Older copies and backups of the database, Time Machine included, then open only with the recovery code that was current when they were made.")
            case .forget:
                Text("Deletes the saved keys no key in the Keychain can read. This can’t be undone; restoring the key from its recovery code keeps them.")
            }
        }
    }

    private var title: String {
        switch confirm {
        case .encrypt(true): "Encrypt the whole database?"
        case .encrypt(false): "Store the database unencrypted?"
        case .rotate: "Rotate the encryption key?"
        case .prune: "Remove old keys from the Keychain?"
        case .forget: "Delete unreadable keys?"
        case nil: ""
        }
    }

    @ViewBuilder
    private func keysSection(_ status: EncryptionStatus) -> some View {
        Section {
            LabeledContent("Saved keys") {
                Text(Self.describe(status)).foregroundStyle(.secondary).multilineTextAlignment(.trailing)
            }
            if status.pending > 0 {
                Caption("\(status.pending) saved key\(status.pending == 1 ? "" : "s") still wait to be encrypted.")
            }
            if status.keys != "unsupported" {
                LabeledContent {
                    Button("Rotate…") { confirm = .rotate }.disabled(status.keys != "ready")
                } label: {
                    Text("Encryption key")
                    Text(status.generation.map { "Key \($0) in the login Keychain." } ?? "Created with the first saved key.")
                }
                LabeledContent {
                    Button("Remove…") { confirm = .prune }.disabled(status.keys != "ready")
                } label: {
                    Text("Old keys")
                    Text("Kept after a rotation so older copies of the database still open.")
                }
                recoverySection(status)
            }
            if status.unreadable > 0 {
                LabeledContent {
                    Button("Forget…", role: .destructive) { confirm = .forget }
                } label: {
                    Text("Unreadable keys")
                    Text("\(status.unreadable) saved key\(status.unreadable == 1 ? "" : "s") can’t be read with the keys in the Keychain.")
                }
            }
        } header: {
            Text("Saved Keys")
        } footer: {
            Caption("Saved API keys are always encrypted. Keys copied before ClipRust encrypted them may survive in free disk space, APFS snapshots and Time Machine backups; FileVault protects those.")
        }
    }

    @ViewBuilder
    private func recoverySection(_ status: EncryptionStatus) -> some View {
        LabeledContent {
            if recoveryCode == nil {
                Button("Show…", action: showRecoveryCode).disabled(status.keys != "ready")
            } else {
                HStack {
                    Button("Copy") { recoveryCode.map(Clipboard.copySecret) }
                    Button("Hide") { recoveryCode = nil }
                }
            }
        } label: {
            Text("Recovery code")
            Text("Keep it in a password manager: it opens your saved keys on another Mac or after the Keychain item is lost.")
        }
        if let recoveryCode {
            Text(recoveryCode).font(.system(size: 11, design: .monospaced)).textSelection(.enabled)
        }
        LabeledContent {
            HStack {
                SecureField("Recovery code", text: $restoreCode).labelsHidden().frame(width: 180)
                Button("Restore", action: restore).disabled(restoreCode.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        } label: {
            Text("Restore key")
        }
    }

    @ViewBuilder
    private func databaseSection(_ status: EncryptionStatus) -> some View {
        Section {
            Toggle(isOn: Binding(get: { status.dbEncrypted }, set: { confirm = .encrypt($0) })) {
                Text("Encrypt the whole database")
                Text("Covers every clip, not only keys. Opening takes a few milliseconds longer.")
            }
            .disabled(status.keys == "locked" || status.keys == "unavailable")
        } footer: {
            Caption("Older backups and snapshots keep the data as it was when they were made.")
        }
    }

    private static func describe(_ s: EncryptionStatus) -> String {
        switch s.keys {
        case "ready": "Encrypted"
        case "no_key": "None saved yet"
        case "unsupported": "Not encrypted on this system"
        default: s.message.map { $0.prefix(1).uppercased() + $0.dropFirst() } ?? "Unavailable"
        }
    }

    private func showRecoveryCode() {
        Task {
            guard await KeyGate.authorize("show the recovery code", always: true) else { return }
            run { core in
                recoveryCode = try await core.recoveryCode()
                return nil
            }
        }
    }

    private func restore() {
        let code = restoreCode.trimmingCharacters(in: .whitespacesAndNewlines)
        run { core in
            let generation = try await core.restoreKey(code)
            restoreCode = ""
            app.skippedKey = nil
            return "Restored key \(generation)."
        }
    }

    /// Runs one change at a time and reports its outcome or error under the form.
    private func run(_ action: @escaping (Core) async throws -> String?) {
        guard let core = app.core else { return }
        busy = true
        Task {
            do { result = try await action(core) } catch { result = error.localizedDescription }
            await app.refreshEncryption()
            busy = false
        }
    }
}

struct ImportPane: View {
    let app: AppState
    @State private var importing = false
    @State private var result: String?

    var body: some View {
        Form {
            Section {
                LabeledContent {
                    if importing {
                        ProgressView().controlSize(.small)
                    } else {
                        Button("Import…", action: runImport)
                    }
                } label: {
                    Text("Import from PasteNow")
                    Text(result ?? "Copies history, lists and ignored apps. Running it twice adds nothing new.")
                }
            }
            Section("Database") {
                LabeledContent {
                    Button("Show in Finder") {
                        if let path = app.stats?.path {
                            NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
                        }
                    }
                } label: {
                    Text("Location")
                    Text(app.stats.map { ($0.path as NSString).abbreviatingWithTildeInPath } ?? "—")
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                LabeledContent("Items", value: app.stats.map { $0.items.formatted() } ?? "—")
                LabeledContent("Stored", value: app.stats.map { Format.bytes($0.storedBytes + $0.fileBytes) } ?? "—")
                LabeledContent("Core", value: Core.version)
            }
        }
        .settingsPane()
        .task { await app.loadStats() }
    }

    private func runImport() {
        guard let core = app.core else { return }
        importing = true
        Task {
            do {
                let r = try await core.importPasteNow()
                result = "Imported \(r.imported.formatted()) items, skipped \((r.duplicates + r.skipped).formatted()), created \(r.listsCreated) lists and \(r.excludes) ignored apps."
                await app.refresh()
                await app.loadStats()
            } catch {
                result = error.localizedDescription
            }
            importing = false
        }
    }
}
