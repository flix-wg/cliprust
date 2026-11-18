import AppKit
import SwiftUI

@MainActor
final class SettingsWindowController {
    private var window: NSWindow?

    func show(app: AppState) {
        if window == nil {
            window = Self.makeWindow(app: app).window
        }
        NSApp.activate()
        window?.makeKeyAndOrderFront(nil)
    }

    static func makeWindow(app: AppState) -> (window: NSWindow, tabs: NSTabViewController) {
        let tabs = NSTabViewController()
        tabs.tabStyle = .toolbar
        let panes: [(String, String, AnyView)] = [
            ("General", "gearshape", AnyView(GeneralPane(app: app))),
            ("Shortcuts", "keyboard", AnyView(ShortcutsPane())),
            ("Appearance", "paintbrush", AnyView(AppearancePane())),
            ("Lists", "list.bullet", AnyView(ListsPane(app: app))),
            ("History", "clock.arrow.circlepath", AnyView(HistoryPane(app: app))),
            ("Keys", "key.horizontal", AnyView(KeysPane(app: app))),
            ("Import", "square.and.arrow.down", AnyView(ImportPane(app: app))),
        ]
        for (title, symbol, view) in panes {
            let host = NSHostingController(rootView: view)
            host.sizingOptions = .preferredContentSize
            host.title = title
            let item = NSTabViewItem(viewController: host)
            item.label = title
            item.image = NSImage(systemSymbolName: symbol, accessibilityDescription: title)
            tabs.addTabViewItem(item)
        }
        let window = NSWindow(contentViewController: tabs)
        window.styleMask = [.titled, .closable]
        window.toolbarStyle = .preference
        window.isReleasedWhenClosed = false
        window.center()
        return (window, tabs)
    }
}
