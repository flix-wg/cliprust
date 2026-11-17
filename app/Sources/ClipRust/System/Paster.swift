import AppKit
import ApplicationServices
import Carbon.HIToolbox

@MainActor
enum Paster {
    static var isTrusted: Bool { AXIsProcessTrusted() }

    /// Sends ⌘V to `target` once the pasteboard holds the item.
    static func paste(into target: NSRunningApplication?) {
        guard isTrusted else {
            explainOnce()
            return
        }
        // Without a known target the item just stays on the clipboard.
        guard let target, !target.isTerminated else { return }
        if !target.isActive { target.activate() }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.06) {
            guard NSWorkspace.shared.frontmostApplication?.processIdentifier == target.processIdentifier else { return }
            let source = CGEventSource(stateID: .combinedSessionState)
            for down in [true, false] {
                let e = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(kVK_ANSI_V), keyDown: down)
                e?.flags = .maskCommand
                e?.post(tap: .cghidEventTap)
            }
        }
    }

    static func openAccessibilitySettings() {
        let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: false] as CFDictionary
        _ = AXIsProcessTrustedWithOptions(options)
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") {
            NSWorkspace.shared.open(url)
        }
    }

    private static func explainOnce() {
        guard !Prefs.explainedAccessibility else { return }
        Prefs.explainedAccessibility = true
        let alert = NSAlert()
        alert.messageText = "Let ClipRust paste for you"
        alert.informativeText = "The item is on the clipboard. To paste straight into the app you were using, allow ClipRust in System Settings → Privacy & Security → Accessibility."
        alert.addButton(withTitle: "Open System Settings")
        alert.addButton(withTitle: "Not Now")
        NSApp.activate()
        if alert.runModal() == .alertFirstButtonReturn { openAccessibilitySettings() }
    }
}
