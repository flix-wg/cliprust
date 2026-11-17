import AppKit
import SwiftUI

@MainActor
final class KeysWindowController: NSObject, NSWindowDelegate {
    private var window: NSWindow?
    private var model: KeysModel?

    func show(app: AppState) {
        if window == nil {
            let model = KeysModel(app: app)
            let host = NSHostingController(rootView: KeysView(model: model))
            host.sceneBridgingOptions = [.toolbars, .title]
            let window = NSWindow(contentViewController: host)
            window.styleMask = [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView]
            window.title = "Keys & IDs"
            window.toolbarStyle = .unified
            window.isReleasedWhenClosed = false
            window.setContentSize(NSSize(width: 900, height: 560))
            window.contentMinSize = NSSize(width: 700, height: 360)
            window.center()
            window.setFrameAutosaveName("KeysWindow")
            window.delegate = self
            self.window = window
            self.model = model
        }
        Task { await model?.reload() }
        NSApp.activate()
        window?.makeKeyAndOrderFront(nil)
    }

    func reloadIfOpen() {
        guard window?.isVisible == true else { return }
        Task { await model?.reload() }
    }

    func windowWillClose(_ notification: Notification) {
        model?.revealed = [:]
    }
}
