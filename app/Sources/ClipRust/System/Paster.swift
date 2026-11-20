import AppKit
import ApplicationServices
import Carbon.HIToolbox

@MainActor
enum Paster {
    static var isTrusted: Bool { AXIsProcessTrusted() }

    /// Sends ⌘V to `target` once the pasteboard holds the item.
    static func paste(into target: NSRunningApplication?) {
        guard isTrusted else {
            AccessGuide.shared.show()
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
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") {
            NSWorkspace.shared.open(url)
        }
    }
}
