import AppKit

enum Features {
    /// Notes need `set_note` and the `note` field from the core.
    static let notes = false
}

enum PanelPosition: String, CaseIterable, Identifiable {
    case center, mouse, menuBar
    var id: String { rawValue }
    var title: String {
        switch self {
        case .center: "Centred on the active screen"
        case .mouse: "At the mouse pointer"
        case .menuBar: "Under the menu-bar icon"
        }
    }
}

enum Density: String, CaseIterable, Identifiable {
    case compact, comfortable
    var id: String { rawValue }
    var rowHeight: CGFloat { self == .compact ? 34 : 44 }
}

enum AfterChoosing: String, CaseIterable, Identifiable {
    case paste, copy
    var id: String { rawValue }
}

enum Prefs {
    enum Key {
        static let panelPosition = "panelPosition"
        static let afterChoosing = "afterChoosing"
        static let plainText = "alwaysPlainText"
        static let showMenuIcon = "showMenuIcon"
        static let density = "density"
        static let showPreview = "showPreview"
        static let showAppIcons = "showAppIcons"
        static let showHints = "showShortcutHints"
        static let panelWidth = "panelWidth"
        static let panelHeight = "panelHeight"
        static let requireAuth = "requireAuthForKeys"
        static let hotkey = "openShortcut"
        static let explainedAccessibility = "explainedAccessibility"
    }

    static let defaultPanelSize = NSSize(width: 680, height: 440)
    private static var d: UserDefaults { .standard }

    static func register() {
        d.register(defaults: [
            Key.panelPosition: PanelPosition.center.rawValue,
            Key.afterChoosing: AfterChoosing.paste.rawValue,
            Key.plainText: false,
            Key.showMenuIcon: true,
            Key.density: Density.compact.rawValue,
            Key.showPreview: true,
            Key.showAppIcons: true,
            Key.showHints: true,
            Key.requireAuth: true,
        ])
    }

    static var panelPosition: PanelPosition { PanelPosition(rawValue: d.string(forKey: Key.panelPosition) ?? "") ?? .center }
    static var afterChoosing: AfterChoosing { AfterChoosing(rawValue: d.string(forKey: Key.afterChoosing) ?? "") ?? .paste }
    static var plainText: Bool { d.bool(forKey: Key.plainText) }
    static var requireAuth: Bool { d.bool(forKey: Key.requireAuth) }
    static var showMenuIcon: Bool { d.bool(forKey: Key.showMenuIcon) }

    static var panelSize: NSSize {
        get {
            let w = d.double(forKey: Key.panelWidth), h = d.double(forKey: Key.panelHeight)
            return w > 0 && h > 0 ? NSSize(width: w, height: h) : defaultPanelSize
        }
        set {
            d.set(newValue.width, forKey: Key.panelWidth)
            d.set(newValue.height, forKey: Key.panelHeight)
        }
    }

    static func resetPanelSize() {
        d.removeObject(forKey: Key.panelWidth)
        d.removeObject(forKey: Key.panelHeight)
        NotificationCenter.default.post(name: .panelSizeReset, object: nil)
    }

    static var explainedAccessibility: Bool {
        get { d.bool(forKey: Key.explainedAccessibility) }
        set { d.set(newValue, forKey: Key.explainedAccessibility) }
    }
}

extension Notification.Name {
    static let panelSizeReset = Notification.Name("ClipRustPanelSizeReset")
}
