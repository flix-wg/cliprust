import AppKit
import Carbon.HIToolbox
import Quartz
import SwiftUI

final class ClipPanel: NSPanel {
    var quickLook: QuickLookSource?
    var onQuickLookEnd: () -> Void = {}

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    override func acceptsPreviewPanelControl(_ panel: QLPreviewPanel!) -> Bool { true }
    override func beginPreviewPanelControl(_ panel: QLPreviewPanel!) { panel.dataSource = quickLook }
    override func endPreviewPanelControl(_ panel: QLPreviewPanel!) {
        panel.dataSource = nil
        DispatchQueue.main.async { self.onQuickLookEnd() }
    }
}

@MainActor
final class PanelController: NSObject, NSWindowDelegate {
    let model: PanelModel
    private let app: AppState
    private let panel: ClipPanel
    private let host: NSHostingView<PanelView>
    private let quickLook = QuickLookSource()
    private var monitors: [Any] = []
    private var target: NSRunningApplication?
    weak var statusButton: NSStatusBarButton?
    var openKeys: () -> Void = {}
    var openSettings: () -> Void = {}

    init(app: AppState) {
        self.app = app
        model = PanelModel(app: app)
        panel = ClipPanel(contentRect: NSRect(origin: .zero, size: Prefs.panelSize),
                          styleMask: [.borderless, .nonactivatingPanel, .resizable],
                          backing: .buffered, defer: true)
        host = NSHostingView(rootView: PanelView(model: model))
        super.init()
        host.sizingOptions = []
        panel.contentView = host
        panel.level = .floating
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .ignoresCycle]
        panel.isFloatingPanel = true
        panel.hidesOnDeactivate = false
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.isReleasedWhenClosed = false
        panel.animationBehavior = .none
        panel.minSize = NSSize(width: 520, height: 320)
        panel.delegate = self
        panel.quickLook = quickLook
        panel.onQuickLookEnd = { [weak self] in
            guard let self, panel.isVisible else { return }
            panel.makeKey()
        }
        model.perform = { [unowned self] action, item in perform(action, item) }
        NotificationCenter.default.addObserver(forName: .panelSizeReset, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.panel.setContentSize(Prefs.defaultPanelSize) }
        }
    }

    var isVisible: Bool { panel.isVisible }
    var windowNumber: Int { panel.windowNumber }

    func toggle() {
        panel.isVisible ? close() : show()
    }

    func show() {
        guard !panel.isVisible else { return }
        let front = NSWorkspace.shared.frontmostApplication
        target = front?.processIdentifier == ProcessInfo.processInfo.processIdentifier ? nil : front
        model.prepareForShow()
        panel.setFrame(frameForShow(), display: false)
        panel.orderFrontRegardless()
        panel.makeKey()
        model.isVisible = true
        installMonitors()
    }

    func close() {
        guard panel.isVisible else { return }
        removeMonitors()
        if QLPreviewPanel.sharedPreviewPanelExists(), QLPreviewPanel.shared().isVisible {
            QLPreviewPanel.shared().orderOut(nil)
        }
        model.isVisible = false
        panel.orderOut(nil)
        model.didHide()
        Thumbnails.shared.trim()
    }

    /// Pastes `item` into `target` (or the app that was active when the panel opened).
    func paste(_ item: Item, mode: ChooseMode, target explicitTarget: NSRunningApplication? = nil) {
        let target = explicitTarget ?? self.target
        guard let core = app.core else { return }
        Task {
            do {
                if item.isSecret {
                    // Written by hand and marked concealed, so other clipboard managers skip it.
                    guard await KeyGate.authorize("paste a saved key"),
                          let value = try await core.revealKey(item.id) else { return }
                    Clipboard.copySecret(value)
                } else {
                    try await core.pastePrep(item.id, plain: mode == .plain || Prefs.plainText)
                }
            } catch {
                NSSound.beep()
                return
            }
            close()
            if mode != .copyOnly, Prefs.afterChoosing == .paste { Paster.paste(into: target) }
        }
    }

    func snapshot(to url: URL) {
        host.layoutSubtreeIfNeeded()
        guard let rep = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { return }
        host.cacheDisplay(in: host.bounds, to: rep)
        try? rep.representation(using: .png, properties: [:])?.write(to: url)
    }

    // MARK: - Placement

    private func frameForShow() -> NSRect {
        let size = panel.frame.size
        let mouse = NSEvent.mouseLocation
        let screen = NSScreen.screens.first { NSMouseInRect(mouse, $0.frame, false) } ?? NSScreen.main
        let visible = screen?.visibleFrame ?? NSRect(origin: .zero, size: size)
        var origin = NSPoint(x: visible.midX - size.width / 2, y: visible.midY - size.height / 2 + visible.height * 0.08)
        switch Prefs.panelPosition {
        case .center:
            break
        case .mouse:
            origin = NSPoint(x: mouse.x - 24, y: mouse.y - size.height + 12)
        case .menuBar:
            if let frame = statusButton?.window?.frame, statusButton?.window?.screen != nil {
                origin = NSPoint(x: frame.midX - size.width / 2, y: frame.minY - size.height - 6)
            }
        }
        origin.x = min(max(origin.x, visible.minX + 8), visible.maxX - size.width - 8)
        origin.y = min(max(origin.y, visible.minY + 8), visible.maxY - size.height - 8)
        return NSRect(origin: origin, size: size)
    }

    func windowDidEndLiveResize(_ notification: Notification) {
        Prefs.panelSize = panel.frame.size
    }

    func windowDidResignKey(_ notification: Notification) {
        if panel.attachedSheet != nil { return }
        if QLPreviewPanel.sharedPreviewPanelExists(), QLPreviewPanel.shared().isVisible { return }
        close()
    }

    // MARK: - Keyboard

    private func installMonitors() {
        removeMonitors()
        if let m = NSEvent.addLocalMonitorForEvents(matching: .keyDown, handler: { [weak self] e in
            nonisolated(unsafe) let event = e
            return MainActor.assumeIsolated {
                guard let self, event.window === self.panel else { return event }
                return self.handleKey(event) ? nil : event
            }
        }) { monitors.append(m) }
        if let m = NSEvent.addLocalMonitorForEvents(matching: .flagsChanged, handler: { [weak self] e in
            let control = e.modifierFlags.intersection([.command, .option, .control, .shift]) == .control
            MainActor.assumeIsolated { self?.model.controlHeld = control }
            return e
        }) { monitors.append(m) }
        if let m = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown, .otherMouseDown], handler: { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                if QLPreviewPanel.sharedPreviewPanelExists(), QLPreviewPanel.shared().isVisible { return }
                self.close()
            }
        }) { monitors.append(m) }
    }

    private func removeMonitors() {
        monitors.forEach(NSEvent.removeMonitor)
        monitors = []
    }

    private func handleKey(_ e: NSEvent) -> Bool {
        let mods = e.modifierFlags.intersection([.command, .option, .control, .shift])
        let key = e.charactersIgnoringModifiers?.lowercased() ?? ""
        let code = Int(e.keyCode)
        let item = model.selected
        if paletteKey(key, mods) { return true }
        if model.palette != nil { return handlePaletteKey(code, mods, key) }

        switch (code, mods) {
        case (kVK_DownArrow, []): model.move(by: 1)
        case (kVK_UpArrow, []): model.move(by: -1)
        case (kVK_PageDown, []): model.move(by: 8)
        case (kVK_PageUp, []): model.move(by: -8)
        case (kVK_Return, []), (kVK_ANSI_KeypadEnter, []):
            if model.suggestionSelected || (model.items.isEmpty && model.suggestion != nil) {
                model.acceptSuggestion()
            } else {
                run(.choose(.standard), item)
            }
        case (kVK_Return, .option): run(.choose(.plain), item)
        case (kVK_Return, .command): run(.choose(.copyOnly), item)
        case (kVK_Escape, []): model.query.isEmpty && model.source == nil ? close() : model.clearSearch()
        case (kVK_Tab, []): if !model.acceptSuggestion() { model.cycleFilter(by: 1) }
        case (kVK_Delete, []) where model.source != nil && caretAtStart: model.source = nil
        case (kVK_Tab, .shift): model.cycleFilter(by: -1)
        case (kVK_LeftArrow, .command): model.cycleFilter(by: -1)
        case (kVK_RightArrow, .command): model.cycleFilter(by: 1)
        case (kVK_Space, []) where model.query.isEmpty: run(.quickLook, item)
        case (kVK_Delete, .command): run(.delete, item)
        case (_, .control) where ("1"..."9").contains(key) && key.count == 1:
            if !model.jump(to: Int(key)! - 1) { NSSound.beep() }
        case (_, [.command, .shift]) where key == "p": run(.togglePin, item)
        case (_, .control) where key == "n": model.move(by: 1)
        case (_, .control) where key == "p": model.move(by: -1)
        case (_, .command):
            switch key {
            case "1", "2", "3", "4", "5", "6", "7", "8", "9":
                let i = Int(key)! - 1
                guard i < model.items.count else { NSSound.beep(); return true }
                perform(.choose(.standard), model.items[i])
            case "p": UserDefaults.standard.set(!UserDefaults.standard.bool(forKey: Prefs.Key.showPreview), forKey: Prefs.Key.showPreview)
            case "r": run(.rename, item)
            case "n": run(.note, item)
            case "k": openKeys()
            case ",": openSettings()
            case "w": close()
            default: return false
            }
        default:
            return false
        }
        return true
    }

    private var caretAtStart: Bool {
        (panel.firstResponder as? NSTextView)?.selectedRange() == NSRange(location: 0, length: 0)
    }

    /// ⌘L, ⌘D and ⌘⇧S open the list palette in any state.
    private func paletteKey(_ key: String, _ mods: NSEvent.ModifierFlags) -> Bool {
        let mode: ListPalette.Mode
        switch (key, mods) {
        case ("l", .command): mode = .go
        case ("d", .command):
            guard let id = model.selectedID else { NSSound.beep(); return true }
            mode = .add(id)
        case ("s", [.command, .shift]): mode = .saveSearch
        default: return false
        }
        if !model.openPalette(mode) { NSSound.beep() }
        return true
    }

    private func handlePaletteKey(_ code: Int, _ mods: NSEvent.ModifierFlags, _ key: String) -> Bool {
        switch (code, mods) {
        case (kVK_Escape, []): model.palette = nil
        case (kVK_DownArrow, []): model.movePalette(by: 1)
        case (kVK_UpArrow, []): model.movePalette(by: -1)
        case (_, .control) where key == "n": model.movePalette(by: 1)
        case (_, .control) where key == "p": model.movePalette(by: -1)
        case (kVK_Return, []), (kVK_ANSI_KeypadEnter, []): if !model.confirmPalette() { NSSound.beep() }
        case (kVK_Tab, _): break
        default: return false
        }
        return true
    }

    private func run(_ action: PanelAction, _ item: Item?) {
        guard let item else { NSSound.beep(); return }
        perform(action, item)
    }

    // MARK: - Actions

    private func perform(_ action: PanelAction, _ item: Item) {
        switch action {
        case .choose(let mode):
            paste(item, mode: mode)
        case .addToList(let name):
            Task { await app.addToList(name, item.id) }
        case .newList:
            _ = model.openPalette(.add(item.id))
        case .togglePin:
            Task {
                let pinned = await app.togglePinned(item.id)
                model.showToast(pinned ? "Pinned" : "Unpinned", symbol: pinned ? "pin.fill" : "pin.slash")
                if model.filter == .list(Item.pinnedList) { model.reload() }
            }
        case .removeFromList(let name):
            Task {
                await app.removeFromList(name, item.id)
                model.reload()
            }
        case .rename:
            prompt(title: "Rename Item", message: "A title replaces the preview in the list. Leave it empty to remove it.",
                   value: item.title ?? "", placeholder: item.oneLine, confirm: "Rename") { [weak self] title in
                guard let self, let core = app.core else { return }
                Task {
                    try? await core.setTitle(item.id, title.trimmingCharacters(in: .whitespaces))
                    self.model.reload()
                }
            }
        case .note:
            guard Features.notes else { NSSound.beep(); return }
            prompt(title: "Note", message: nil, value: item.note ?? "", placeholder: "Add a note…", confirm: "Save") { [weak self] note in
                guard let self, let core = app.core else { return }
                Task {
                    try? await core.setNote(item.id, note.isEmpty ? nil : note)
                    self.model.reload()
                }
            }
        case .revealInFinder:
            let urls = (model.detail?.id == item.id ? model.detail! : item).fileURLs
            guard !urls.isEmpty else { NSSound.beep(); return }
            close()
            NSWorkspace.shared.activateFileViewerSelecting(urls)
        case .quickLook:
            toggleQuickLook(item)
        case .delete:
            model.delete(item)
        }
    }

    private func prompt(title: String, message: String?, value: String, placeholder: String, confirm: String,
                        completion: @escaping (String) -> Void) {
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = message ?? ""
        let field = NSTextField(string: value)
        field.placeholderString = placeholder
        field.frame = NSRect(x: 0, y: 0, width: 280, height: 24)
        alert.accessoryView = field
        alert.addButton(withTitle: confirm)
        alert.addButton(withTitle: "Cancel")
        alert.window.initialFirstResponder = field
        alert.beginSheetModal(for: panel) { [weak self] response in
            MainActor.assumeIsolated {
                if response == .alertFirstButtonReturn { completion(field.stringValue) }
                self?.panel.makeKey()
            }
        }
    }

    // MARK: - Quick Look

    private func toggleQuickLook(_ item: Item) {
        let ql = QLPreviewPanel.shared()!
        if QLPreviewPanel.sharedPreviewPanelExists(), ql.isVisible {
            ql.orderOut(nil)
            return
        }
        guard !item.isSecret else { NSSound.beep(); return }
        Task {
            guard let url = await quickLook.prepare(item, core: app.core, detail: model.detail) else {
                NSSound.beep()
                return
            }
            quickLook.url = url
            ql.level = NSWindow.Level(rawValue: panel.level.rawValue + 1)
            ql.makeKeyAndOrderFront(nil)
            ql.reloadData()
        }
    }
}
