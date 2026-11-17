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
    @Bindable private var hotkey = Hotkey.shared
    @AppStorage(Prefs.Key.panelPosition) private var position = PanelPosition.center
    @AppStorage(Prefs.Key.afterChoosing) private var after = AfterChoosing.paste
    @AppStorage(Prefs.Key.plainText) private var plainText = false
    @AppStorage(Prefs.Key.showMenuIcon) private var showMenuIcon = true
    @State private var launchAtLogin = SMAppService.mainApp.status == .enabled
    @State private var trusted = Paster.isTrusted

    var body: some View {
        Form {
            Section {
                LabeledContent("Open ClipRust") {
                    ShortcutRecorder(hotkey: hotkey)
                }
                if let message = hotkey.message {
                    Caption(message)
                }
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
                        Button("Allow…") { Paster.openAccessibilitySettings() }
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
        do {
            if on { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() }
        } catch {
            app.lastError = error.localizedDescription
        }
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

struct HistoryPane: View {
    let app: AppState
    @State private var confirmClear = false
    @State private var cleared: Int?

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
        .confirmationDialog("Clear the clipboard history?", isPresented: $confirmClear) {
            Button("Clear History", role: .destructive) {
                Task { cleared = await app.clearHistory() }
            }
        } message: {
            Text("Items in lists, including Keys, are kept. This can’t be undone.")
        }
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
                    Text("Detect API keys and IDs")
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
