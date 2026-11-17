import AppKit
import Carbon.HIToolbox
import Observation

struct Shortcut: Codable, Equatable {
    var keyCode: UInt32
    var modifiers: UInt32
    var key: String

    static let standard = Shortcut(keyCode: UInt32(kVK_ANSI_V), modifiers: UInt32(cmdKey | shiftKey), key: "V")
    static let fallback = Shortcut(keyCode: UInt32(kVK_ANSI_V), modifiers: UInt32(cmdKey | shiftKey | optionKey), key: "V")

    init(keyCode: UInt32, modifiers: UInt32, key: String) {
        self.keyCode = keyCode
        self.modifiers = modifiers
        self.key = key
    }

    init?(event: NSEvent) {
        let flags = event.modifierFlags.intersection([.command, .option, .control, .shift])
        guard !flags.intersection([.command, .option, .control]).isEmpty else { return nil }
        var mods = 0
        if flags.contains(.command) { mods |= cmdKey }
        if flags.contains(.option) { mods |= optionKey }
        if flags.contains(.control) { mods |= controlKey }
        if flags.contains(.shift) { mods |= shiftKey }
        let label = Self.specialKeys[Int(event.keyCode)]
            ?? (event.characters(byApplyingModifiers: []) ?? "").uppercased()
        guard !label.isEmpty else { return nil }
        self.init(keyCode: UInt32(event.keyCode), modifiers: UInt32(mods), key: label)
    }

    var display: String {
        var s = ""
        if modifiers & UInt32(controlKey) != 0 { s += "⌃" }
        if modifiers & UInt32(optionKey) != 0 { s += "⌥" }
        if modifiers & UInt32(shiftKey) != 0 { s += "⇧" }
        if modifiers & UInt32(cmdKey) != 0 { s += "⌘" }
        return s + key
    }

    var menuModifiers: NSEvent.ModifierFlags {
        var f: NSEvent.ModifierFlags = []
        if modifiers & UInt32(controlKey) != 0 { f.insert(.control) }
        if modifiers & UInt32(optionKey) != 0 { f.insert(.option) }
        if modifiers & UInt32(shiftKey) != 0 { f.insert(.shift) }
        if modifiers & UInt32(cmdKey) != 0 { f.insert(.command) }
        return f
    }

    private static let specialKeys: [Int: String] = [
        kVK_Space: "Space", kVK_Return: "↩", kVK_Tab: "⇥", kVK_Delete: "⌫", kVK_ForwardDelete: "⌦",
        kVK_LeftArrow: "←", kVK_RightArrow: "→", kVK_UpArrow: "↑", kVK_DownArrow: "↓",
        kVK_Home: "↖", kVK_End: "↘", kVK_PageUp: "⇞", kVK_PageDown: "⇟",
        kVK_F1: "F1", kVK_F2: "F2", kVK_F3: "F3", kVK_F4: "F4", kVK_F5: "F5", kVK_F6: "F6",
        kVK_F7: "F7", kVK_F8: "F8", kVK_F9: "F9", kVK_F10: "F10", kVK_F11: "F11", kVK_F12: "F12",
    ]
}

/// The global open shortcut, registered through Carbon.
@MainActor @Observable
final class Hotkey {
    static let shared = Hotkey()

    private(set) var active: Shortcut?
    private(set) var message: String?
    @ObservationIgnored var onPress: () -> Void = {}
    @ObservationIgnored private var ref: EventHotKeyRef?
    @ObservationIgnored private var handlerInstalled = false

    var preferred: Shortcut {
        get {
            guard let data = UserDefaults.standard.data(forKey: Prefs.Key.hotkey),
                  let s = try? JSONDecoder().decode(Shortcut.self, from: data) else { return .standard }
            return s
        }
        set {
            UserDefaults.standard.set(try? JSONEncoder().encode(newValue), forKey: Prefs.Key.hotkey)
            register()
        }
    }

    func register() {
        unregister()
        installHandler()
        let wanted = preferred
        if let owner = otherOwner(of: wanted) {
            if wanted == .standard, otherOwner(of: .fallback) == nil, tryRegister(.fallback) {
                active = .fallback
                message = "\(owner) uses \(wanted.display), so ClipRust uses \(Shortcut.fallback.display) while \(owner) is running."
            } else {
                active = nil
                message = "\(owner) uses \(wanted.display). Choose another shortcut."
            }
        } else if tryRegister(wanted) {
            active = wanted
            message = nil
        } else if wanted == .standard, tryRegister(.fallback) {
            active = .fallback
            message = "\(wanted.display) is taken by another app, so ClipRust uses \(Shortcut.fallback.display)."
        } else {
            active = nil
            message = "\(wanted.display) is taken by another app. Choose another shortcut."
        }
    }

    /// Carbon does not report shortcuts other apps registered, so PasteNow, the app
    /// ClipRust replaces and often runs beside, is checked by name.
    private func otherOwner(of s: Shortcut) -> String? {
        let id = "app.pastenow.PasteNow"
        guard !NSRunningApplication.runningApplications(withBundleIdentifier: id).isEmpty else { return nil }
        var taken = Shortcut.standard
        if let d = UserDefaults(suiteName: id)?.dictionary(forKey: "shortcutkit-show-interface"),
           let code = d["carbonKeyCode"] as? Int, let mods = d["carbonModifiers"] as? Int {
            taken = Shortcut(keyCode: UInt32(code), modifiers: UInt32(mods), key: "")
        }
        return taken.keyCode == s.keyCode && taken.modifiers == s.modifiers ? "PasteNow" : nil
    }

    func unregister() {
        if let ref { UnregisterEventHotKey(ref) }
        ref = nil
    }

    private func tryRegister(_ s: Shortcut) -> Bool {
        var newRef: EventHotKeyRef?
        let id = EventHotKeyID(signature: OSType(0x4372_5374), id: 1)
        let status = RegisterEventHotKey(s.keyCode, s.modifiers, id, GetApplicationEventTarget(),
                                         OptionBits(kEventHotKeyExclusive), &newRef)
        guard status == noErr, let newRef else { return false }
        ref = newRef
        return true
    }

    private func installHandler() {
        guard !handlerInstalled else { return }
        handlerInstalled = true
        let center = NSWorkspace.shared.notificationCenter
        for name in [NSWorkspace.didLaunchApplicationNotification, NSWorkspace.didTerminateApplicationNotification] {
            center.addObserver(forName: name, object: nil, queue: .main) { note in
                let app = note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication
                guard app?.bundleIdentifier == "app.pastenow.PasteNow" else { return }
                MainActor.assumeIsolated { Hotkey.shared.register() }
            }
        }
        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        InstallEventHandler(GetApplicationEventTarget(), { _, _, _ in
            MainActor.assumeIsolated { Hotkey.shared.onPress() }
            return noErr
        }, 1, &spec, nil, nil)
    }
}
