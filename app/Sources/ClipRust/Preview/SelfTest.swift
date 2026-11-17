import AppKit

/// `--self-test <png>`, a development aid: drives the panel with synthetic key events
/// on the configured database, logs the model after each step, then quits.
/// It ends with ⌘2, which puts the second item on the clipboard.
@MainActor
enum SelfTest {
    static func run(_ panel: PanelController, snapshot: URL) async {
        let model = panel.model
        func log(_ step: String) {
            print("self-test: \(step) | visible=\(panel.isVisible) filter=\(model.filter.title) query=\"\(model.query)\" items=\(model.items.count) selected=\(model.selectedIndex ?? -1)")
        }
        func key(_ code: Int, _ chars: String, _ mods: NSEvent.ModifierFlags = []) async {
            if let e = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: mods,
                                        timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: panel.windowNumber,
                                        context: nil, characters: chars, charactersIgnoringModifiers: chars,
                                        isARepeat: false, keyCode: UInt16(code)) {
                NSApp.postEvent(e, atStart: false)
            }
            try? await Task.sleep(for: .milliseconds(300))
        }

        panel.show()
        try? await Task.sleep(for: .seconds(1))
        log("shown")
        print("self-test: hotkey=\(Hotkey.shared.active?.display ?? "none") message=\(Hotkey.shared.message ?? "-")")
        panel.snapshot(to: snapshot)
        await key(125, "\u{F701}"); await key(125, "\u{F701}"); log("down down")
        await key(126, "\u{F700}"); log("up")
        await key(48, "\t"); log("tab")
        await key(48, "\t", .shift); log("shift-tab")
        for ch in "brew" { await key(0, String(ch)) }
        log("typed brew")
        await key(53, "\u{1B}"); log("escape")
        await key(19, "2", .command); log("cmd-2")
        try? await Task.sleep(for: .milliseconds(300))
        print("self-test: clipboard chars=\(NSPasteboard.general.string(forType: .string)?.count ?? 0)")
        panel.show()
        try? await Task.sleep(for: .milliseconds(500))
        await key(40, "k", .command)
        NSApp.sendAction(#selector(AppDelegate.showSettings(_:)), to: nil, from: nil)
        try? await Task.sleep(for: .seconds(1))
        let titles = NSApp.windows.filter(\.isVisible).map(\.title)
        print("self-test: panel visible=\(panel.isVisible) windows=\(titles)")
        exit(0)
    }
}
