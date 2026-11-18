import AppKit
import Carbon.HIToolbox
import Observation

/// Panel actions with editable shortcuts. ⏎, Esc, ⇥, the arrows, ⌘1–⌘9 and ⌃1–⌃9 stay fixed.
enum PanelCommand: String, CaseIterable, Identifiable {
    case plainPaste, copyOnly, goToList, addToList, saveSearch, pin, note, rename, quickLook, delete, keys, preview,
         settings, close

    var id: String { rawValue }

    var title: String {
        switch self {
        case .plainPaste: "Paste as plain text"
        case .copyOnly: "Copy without pasting"
        case .goToList: "Go to list or app"
        case .addToList: "Add to list"
        case .saveSearch: "Save search as list"
        case .pin: "Pin or unpin"
        case .note: "Edit note"
        case .rename: "Rename"
        case .quickLook: "Quick Look"
        case .delete: "Delete"
        case .keys: "Keys & IDs"
        case .preview: "Show or hide preview"
        case .settings: "Settings"
        case .close: "Close panel"
        }
    }

    var standard: Shortcut {
        switch self {
        case .plainPaste: Shortcut(kVK_Return, optionKey, "↩")
        case .copyOnly: Shortcut(kVK_Return, cmdKey, "↩")
        case .goToList: Shortcut(kVK_ANSI_L, cmdKey, "L")
        case .addToList: Shortcut(kVK_ANSI_D, cmdKey, "D")
        case .saveSearch: Shortcut(kVK_ANSI_S, cmdKey | shiftKey, "S")
        case .pin: Shortcut(kVK_ANSI_P, cmdKey | shiftKey, "P")
        case .note: Shortcut(kVK_ANSI_N, cmdKey, "N")
        case .rename: Shortcut(kVK_ANSI_R, cmdKey, "R")
        case .quickLook: Shortcut(kVK_Space, 0, "Space")
        case .delete: Shortcut(kVK_Delete, cmdKey, "⌫")
        case .keys: Shortcut(kVK_ANSI_K, cmdKey, "K")
        case .preview: Shortcut(kVK_ANSI_P, cmdKey, "P")
        case .settings: Shortcut(kVK_ANSI_Comma, cmdKey, ",")
        case .close: Shortcut(kVK_ANSI_W, cmdKey, "W")
        }
    }
}

/// Custom bindings, stored as one dictionary of command → shortcut.
@MainActor @Observable
final class PanelShortcuts {
    static let shared = PanelShortcuts()

    private var custom: [String: Shortcut] = {
        guard let data = UserDefaults.standard.data(forKey: Prefs.Key.panelShortcuts) else { return [:] }
        return (try? JSONDecoder().decode([String: Shortcut].self, from: data)) ?? [:]
    }()

    subscript(_ c: PanelCommand) -> Shortcut { custom[c.rawValue] ?? c.standard }

    func command(for e: NSEvent) -> PanelCommand? {
        PanelCommand.allCases.first { self[$0].matches(e) }
    }

    /// Nil when bound, else why the shortcut was refused.
    func set(_ s: Shortcut, for c: PanelCommand) -> String? {
        if let why = Self.fixed(s) { return why }
        if s == Hotkey.shared.preferred { return "\(s.display) opens ClipRust." }
        if let other = PanelCommand.allCases.first(where: { $0 != c && self[$0] == s }) {
            return "\(s.display) is used by \(other.title)."
        }
        custom[c.rawValue] = s == c.standard ? nil : s
        save()
        return nil
    }

    func resetAll() {
        custom = [:]
        save()
    }

    var isCustomized: Bool { !custom.isEmpty }

    private func save() {
        UserDefaults.standard.set(try? JSONEncoder().encode(custom), forKey: Prefs.Key.panelShortcuts)
    }

    private static func fixed(_ s: Shortcut) -> String? {
        let code = Int(s.keyCode), mods = Int(s.modifiers)
        if [kVK_Escape, kVK_Tab, kVK_LeftArrow, kVK_RightArrow, kVK_UpArrow, kVK_DownArrow,
            kVK_PageUp, kVK_PageDown].contains(code) { return "Arrows, ⇥ and Esc are fixed." }
        if (mods == cmdKey || mods == controlKey), ("1"..."9").contains(s.key), s.key.count == 1 {
            return "⌘1–⌘9 and ⌃1–⌃9 are fixed."
        }
        if mods == controlKey, ["N", "P"].contains(s.key) { return "⌃N and ⌃P move the selection." }
        if mods == cmdKey, ["A", "C", "V", "X", "Z"].contains(s.key) { return "\(s.display) edits text." }
        return nil
    }
}
