import AppKit
import CCliprust
import LocalAuthentication

/// Touch ID or password before a key's value leaves the database.
@MainActor
enum KeyGate {
    private static var lastSuccess: Date?
    private static let grace: TimeInterval = 60

    /// `always` asks even when the setting is off or a recent success is still fresh.
    static func authorize(_ reason: String, always: Bool = false) async -> Bool {
        if !always {
            guard Prefs.requireAuth else { return true }
            if let lastSuccess, Date().timeIntervalSince(lastSuccess) < grace { return true }
        }
        let context = LAContext()
        var error: NSError?
        guard context.canEvaluatePolicy(.deviceOwnerAuthentication, error: &error) else { return false }
        do {
            try await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason)
            lastSuccess = Date()
            return true
        } catch {
            return false
        }
    }
}

enum Clipboard {
    /// Marked concealed so clipboard managers, ClipRust included, skip it.
    static func copySecret(_ value: String) {
        // One write, so no reader sees the value without the marker.
        let entry = NSPasteboardItem()
        entry.setData(Data(), forType: NSPasteboard.PasteboardType("org.nspasteboard.ConcealedType"))
        entry.setString(value, forType: .string)
        // clearContents returns the count of this write (writing adds no change); reading it after the
        // write could pick up another app's copy made in between, which the watcher would then skip.
        let count = NSPasteboard.general.clearContents()
        NSPasteboard.general.writeObjects([entry])
        cr_own_change(count)
    }

    static func copy(_ value: String) {
        let count = NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(value, forType: .string)
        cr_own_change(count)
    }
}
