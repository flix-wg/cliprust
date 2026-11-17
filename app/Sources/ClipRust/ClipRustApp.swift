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
        Task { await start() }
    }

    private func start() async {
        let path = ProcessInfo.processInfo.environment["CLIPRUST_DB"].flatMap { $0.isEmpty ? nil : $0 }
        let core: Core
        do {
            core = try await Core.open(path: path)
        } catch {
            let alert = NSAlert()
            alert.messageText = "ClipRust couldn’t open its database"
            alert.informativeText = error.localizedDescription
            NSApp.activate()
            alert.runModal()
            NSApp.terminate(nil)
            return
        }
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
        }
    }

    private func handle(_ event: CoreEvent) {
        guard event.type == "stored" else { return }
        state?.itemStored(secret: event.secret == true)
        if panel?.isVisible == true { panel?.model.reload() }
        keysWindow.reloadIfOpen()
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

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        openSettings()
        return false
    }
}
