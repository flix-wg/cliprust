import AppKit

@main
enum ClipRustMain {
    @MainActor static func main() {
        let app = NSApplication.shared
        let delegate = AppDelegate()
        app.delegate = delegate
        app.setActivationPolicy(.accessory)
        withExtendedLifetime(delegate) { app.run() }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var state: AppState?
    private var panel: PanelController?
    private var statusItem: StatusItemController?
    private let settingsWindow = SettingsWindowController()
    private let keysWindow = KeysWindowController()

    func applicationDidFinishLaunching(_ notification: Notification) {
        Prefs.register()
        let args = CommandLine.arguments
        if let i = args.firstIndex(of: "--render-previews"), i + 1 < args.count {
            PreviewRenderer.run(into: URL(fileURLWithPath: args[i + 1]))
            exit(0)
        }
        if let i = args.firstIndex(of: "--render-icon"), i + 1 < args.count {
            exit(AppIcon.renderIconset(into: URL(fileURLWithPath: args[i + 1])) ? 0 : 1)
        }
        MainMenu.install()
        // Hand focus back once the last window closes; the app has no Dock icon.
        NotificationCenter.default.addObserver(forName: NSWindow.willCloseNotification, object: nil, queue: .main) { note in
            let closing = note.object as? NSWindow
            DispatchQueue.main.async {
                let open = NSApp.windows.contains { $0 !== closing && $0.isVisible && !($0 is NSPanel) && $0.canBecomeMain }
                if !open, closing?.canBecomeMain == true { NSApp.hide(nil) }
            }
        }
        Task { await start() }
    }

    private func start() async {
        let path = ProcessInfo.processInfo.environment["CLIPRUST_DB"].flatMap { $0.isEmpty ? nil : $0 }
        guard let core = await openCore(path: path) else { return NSApp.terminate(nil) }
        let state = AppState(core: core)
        Thumbnails.shared.core = core
        let panel = PanelController(app: state)
        panel.openKeys = { [unowned self] in openKeys() }
        panel.openSettings = { [unowned self] in openSettings() }
        let statusItem = StatusItemController(app: state, panel: panel,
                                              openKeys: { [unowned self] in openKeys() },
                                              openSettings: { [unowned self] in openSettings() })
        panel.statusButton = statusItem.button
        (self.state, self.panel, self.statusItem) = (state, panel, statusItem)

        Hotkey.shared.onPress = { [weak panel] in panel?.toggle() }
        Hotkey.shared.register()
        core.watch { [weak self] event in self?.handle(event) }
        await state.refresh()

        let args = CommandLine.arguments
        if let i = args.firstIndex(of: "--self-test"), i + 1 < args.count {
            await SelfTest.run(panel, snapshot: URL(fileURLWithPath: args[i + 1]))
        } else if Prefs.afterChoosing == .paste, !Paster.isTrusted {
            // Updates can invalidate the grant, so say so now rather than on the first failed paste.
            AccessGuide.shared.show()
        }
    }

    /// Opens the database; while no stored key opens an encrypted one, offers to restore it from a recovery code.
    private func openCore(path: String?) async -> Core? {
        var restoreError: String?
        while true {
            let error: Error
            do { return try await Core.open(path: path) } catch let e { error = e }
            let locked = (error as? CoreError)?.code == "locked"
            let alert = NSAlert()
            alert.messageText = "ClipRust couldn’t open its database"
            alert.informativeText = restoreError ?? error.localizedDescription
            let field = NSSecureTextField(frame: NSRect(x: 0, y: 0, width: 320, height: 24))
            if locked {
                alert.informativeText += "\n\nIf you saved a recovery code, paste it to put the key back into the Keychain."
                field.placeholderString = "Recovery code"
                alert.accessoryView = field
                alert.addButton(withTitle: "Restore Key")
            }
            alert.addButton(withTitle: "Quit")
            NSApp.activate()
            alert.window.initialFirstResponder = field
            guard locked, alert.runModal() == .alertFirstButtonReturn else {
                if !locked { alert.runModal() }
                return nil
            }
            do {
                _ = try await Core.restoreKey(path: path, code: field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines))
                restoreError = nil
            } catch {
                restoreError = error.localizedDescription
            }
        }
    }

    private func handle(_ event: CoreEvent) {
        switch event.type {
        case "stored":
            state?.itemStored(secret: event.secret == true)
            panel?.model.itemStored()
            keysWindow.reloadIfOpen()
        case "secret_skipped":
            state?.skippedKey = event.reason ?? "its encryption key is unavailable"
            Task { await state?.refreshEncryption() }
        default:
            break
        }
    }

    @objc func showSettings(_ sender: Any?) { openSettings() }

    func openSettings() {
        guard let state else { return }
        panel?.close()
        settingsWindow.show(app: state)
    }

    func openKeys() {
        guard let state else { return }
        panel?.close()
        keysWindow.show(app: state)
    }

    func applicationWillTerminate(_ notification: Notification) {
        QuickLookSource.removeFolder()
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        openSettings()
        return false
    }
}
