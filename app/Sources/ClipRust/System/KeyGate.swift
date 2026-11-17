import AppKit
import LocalAuthentication

/// Touch ID or password before a key's value leaves the database.
@MainActor
enum KeyGate {
    private static var lastSuccess: Date?
    private static let grace: TimeInterval = 60

    static func authorize(_ reason: String) async -> Bool {
        guard Prefs.requireAuth else { return true }
        if let lastSuccess, Date().timeIntervalSince(lastSuccess) < grace { return true }
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
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(value, forType: .string)
        pb.setData(Data(), forType: NSPasteboard.PasteboardType("org.nspasteboard.ConcealedType"))
    }

    static func copy(_ value: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(value, forType: .string)
    }
}
