import AppKit
import Observation

@MainActor
final class StatusItemController: NSObject {
    private let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
    private let app: AppState
    private let panel: PanelController
    private let openKeys: () -> Void
    private let openSettings: () -> Void

    var button: NSStatusBarButton? { item.button }

    init(app: AppState, panel: PanelController, openKeys: @escaping () -> Void, openSettings: @escaping () -> Void) {
        self.app = app
        self.panel = panel
        self.openKeys = openKeys
        self.openSettings = openSettings
        super.init()
        item.button?.target = self
        item.button?.action = #selector(clicked)
        item.button?.sendAction(on: [.leftMouseUp, .rightMouseUp])
        item.autosaveName = "ClipRust"
        observePaused()
        updateVisibility()
        NotificationCenter.default.addObserver(forName: UserDefaults.didChangeNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.updateVisibility() }
        }
    }

    private func observePaused() {
        withObservationTracking {
            item.button?.image = MenuIcon.image(paused: app.settings.paused)
        } onChange: { [weak self] in
            DispatchQueue.main.async { MainActor.assumeIsolated { self?.observePaused() } }
        }
    }

    private func updateVisibility() {
        if item.isVisible != Prefs.showMenuIcon { item.isVisible = Prefs.showMenuIcon }
    }

    @objc private func clicked() {
        let event = NSApp.currentEvent
        let wantsMenu = event?.type == .rightMouseUp
            || event?.modifierFlags.contains(.option) == true
            || event?.modifierFlags.contains(.control) == true
        wantsMenu ? showMenu() : panel.toggle()
    }

    private func showMenu() {
        panel.close()
        let target = NSWorkspace.shared.frontmostApplication
        let menu = NSMenu()
        for (i, recent) in app.recent.prefix(5).enumerated() {
            let title = recent.oneLine.isEmpty ? "Untitled" : String(recent.oneLine.prefix(44)) + (recent.oneLine.count > 44 ? "…" : "")
            let entry = ActionMenuItem(title, key: "\(i + 1)") { [weak self] in
                self?.panel.paste(recent, mode: .standard, target: target)
            }
            let icon = AppInfo.icon(recent.app) ?? NSImage(systemSymbolName: recent.symbol, accessibilityDescription: nil)
            icon?.size = NSSize(width: 16, height: 16)
            entry.image = icon
            menu.addItem(entry)
        }
        if !app.recent.isEmpty { menu.addItem(.separator()) }
        let shortcut = Hotkey.shared.active
        menu.addItem(ActionMenuItem("Open ClipRust", key: shortcut?.key.lowercased() ?? "",
                                    modifiers: shortcut?.menuModifiers ?? []) { [weak self] in self?.panel.show() })
        let pause = ActionMenuItem("Pause Recording") { [weak self] in
            self?.app.update { $0.paused.toggle() }
        }
        pause.state = app.settings.paused ? .on : .off
        menu.addItem(pause)
        menu.addItem(.separator())
        menu.addItem(ActionMenuItem("Keys & IDs…", key: "k", handler: openKeys))
        menu.addItem(ActionMenuItem("Settings…", key: ",", handler: openSettings))
        menu.addItem(.separator())
        menu.addItem(ActionMenuItem("Quit ClipRust", key: "q") { NSApp.terminate(nil) })

        item.menu = menu
        item.button?.performClick(nil)
        item.menu = nil
    }
}
