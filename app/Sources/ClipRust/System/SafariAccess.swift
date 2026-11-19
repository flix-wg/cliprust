import AppKit
import Carbon

/// Whether macOS lets ClipRust ask Safari for its current tab (Automation permission).
enum SafariAccess {
    case allowed, denied, unknown

    /// With `ask`, macOS shows its prompt if the user has not decided yet. Blocks, so it runs off the main thread.
    static func check(ask: Bool) async -> SafariAccess {
        await Task.detached(priority: .utility) { () -> SafariAccess in
            let target = NSAppleEventDescriptor(bundleIdentifier: "com.apple.Safari")
            guard let desc = target.aeDesc else { return .unknown }
            let status = AEDeterminePermissionToAutomateTarget(desc, AEEventClass(typeWildCard), AEEventID(typeWildCard), ask)
            switch Int(status) {
            case Int(noErr): return .allowed
            case Int(errAEEventNotPermitted): return .denied
            // Not asked yet, or Safari is not running.
            default: return .unknown
            }
        }.value
    }

    static func openSettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation") {
            NSWorkspace.shared.open(url)
        }
    }
}
