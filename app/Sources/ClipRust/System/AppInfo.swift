import AppKit

/// Names and icons of apps by bundle id, cached.
enum AppInfo {
    nonisolated(unsafe) private static var icons: [String: NSImage?] = [:]
    nonisolated(unsafe) private static var names: [String: String] = [:]

    static func url(_ bundleID: String) -> URL? {
        NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleID)
    }

    static func icon(_ bundleID: String?) -> NSImage? {
        guard let bundleID else { return nil }
        if let cached = icons[bundleID] { return cached }
        let icon = url(bundleID).map { NSWorkspace.shared.icon(forFile: $0.path) }
        icons[bundleID] = icon
        return icon
    }

    static func name(_ bundleID: String) -> String {
        if let cached = names[bundleID] { return cached }
        var name = bundleID
        if let url = url(bundleID) {
            name = FileManager.default.displayName(atPath: url.path)
            if name.hasSuffix(".app") { name = String(name.dropLast(4)) }
        }
        names[bundleID] = name
        return name
    }
}
