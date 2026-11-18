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
        Task { await app.refreshEncryption() }
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
        quickLook.clear()
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
        if model.editing != nil { return handleEditKey(code, mods) }
        let command = PanelShortcuts.shared.command(for: e)
        if let command, paletteKey(command) { return true }
        if model.palette != nil { return handlePaletteKey(code, mods, key) }
        if handleFixedKey(code, mods, key) { return true }
        // A shortcut without ⌘, ⌥ or ⌃ (␣ for Quick Look) only acts while the search is empty.
        guard let command, !mods.isDisjoint(with: [.command, .option, .control]) || model.query.isEmpty else { return false }
        let item = model.selected
        switch command {
        case .goToList, .addToList, .saveSearch: break
        case .plainPaste: run(.choose(.plain), item)
        case .copyOnly: run(.choose(.copyOnly), item)
        case .pin: run(.togglePin, item)
        case .note: run(.note, item)
        case .rename: run(.rename, item)
        case .quickLook: run(.quickLook, item)
        case .delete: run(.delete, item)
        case .keys: openKeys()
        case .preview: UserDefaults.standard.set(!UserDefaults.standard.bool(forKey: Prefs.Key.showPreview), forKey: Prefs.Key.showPreview)
        case .settings: openSettings()
        case .close: close()
        }
        return true
    }

    private func handleFixedKey(_ code: Int, _ mods: NSEvent.ModifierFlags, _ key: String) -> Bool {
        switch (code, mods) {
        case (kVK_DownArrow, []): model.move(by: 1)
        case (kVK_UpArrow, []): model.move(by: -1)
        case (kVK_PageDown, []): model.move(by: 8)
        case (kVK_PageUp, []): model.move(by: -8)
        case (kVK_LeftArrow, []) where caretAtEdge(end: false): model.cycleFilter(by: -1)
        case (kVK_RightArrow, []) where caretAtEdge(end: true): model.cycleFilter(by: 1)
        case (kVK_Return, []), (kVK_ANSI_KeypadEnter, []):
            model.whenCurrent { [weak self] in
                guard let self else { return }
                if model.suggestionSelected || (model.items.isEmpty && model.suggestion != nil) {
                    model.acceptSuggestion()
                } else {
                    run(.choose(.standard), model.selected)
                }
            }
        case (kVK_Escape, []): model.hasSearch ? model.clearSearch() : close()
        case (kVK_Tab, []): if !model.acceptSuggestion() { model.cycleFilter(by: 1) }
        case (kVK_Delete, []) where (model.source != nil || model.contactToken != nil) && caretAtEdge(end: false):
            model.removeLastToken()
        case (kVK_Tab, .shift): model.cycleFilter(by: -1)
        case (kVK_LeftArrow, .command): model.cycleFilter(by: -1)
        case (kVK_RightArrow, .command): model.cycleFilter(by: 1)
        case (_, .control) where ("1"..."9").contains(key) && key.count == 1:
            if !model.jump(to: Int(key)! - 1) { NSSound.beep() }
        case (_, .command) where ("1"..."9").contains(key) && key.count == 1:
            let i = Int(key)! - 1
            guard i < model.items.count else { NSSound.beep(); return true }
            perform(.choose(.standard), model.items[i])
        case (_, .control) where key == "n": model.move(by: 1)
        case (_, .control) where key == "p": model.move(by: -1)
        default: return false
        }
        return true
    }

    /// ⏎ saves the inline note or title, Esc cancels; everything else goes to its field.
    private func handleEditKey(_ code: Int, _ mods: NSEvent.ModifierFlags) -> Bool {
        switch (code, mods) {
        case (kVK_Escape, []): model.editing = nil
        case (kVK_Return, []), (kVK_ANSI_KeypadEnter, []): model.commitEdit()
        case (kVK_Tab, _): break
        default: return false
        }
        return true
    }

    /// True while the focused field is empty or its caret sits at the start (or end).
    private func caretAtEdge(end: Bool) -> Bool {
        guard let field = panel.firstResponder as? NSTextView else { return model.query.isEmpty }
        return field.selectedRange() == NSRange(location: end ? (field.string as NSString).length : 0, length: 0)
    }

    /// Go to list, add to list and save search open the list palette in any state.
    private func paletteKey(_ command: PanelCommand) -> Bool {
        let mode: ListPalette.Mode
        switch command {
        case .goToList: mode = .go
        case .addToList:
            guard let id = model.selectedID else { NSSound.beep(); return true }
            mode = .add(id)
        case .saveSearch: mode = .saveSearch
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
            Task { if !(await app.addToList(name, item.id)) { NSSound.beep(); model.reload() } }
        case .newList:
            _ = model.openPalette(.add(item.id))
        case .togglePin:
            Task {
                let was = app.pinnedIDs.contains(item.id)
                let pinned = await app.togglePinned(item.id)
                if pinned == was { return NSSound.beep() }
                model.showToast(pinned ? "Pinned" : "Unpinned", symbol: pinned ? "pin.fill" : "pin.slash")
                if model.filter == .list(Item.pinnedList) { model.reload() }
            }
        case .removeFromList(let name):
            Task {
                await app.removeFromList(name, item.id)
                model.reload()
            }
        case .rename:
            model.beginEdit(.title, item)
        case .note:
            model.beginEdit(.note, item)
        case .revealInFinder:
            let urls = (model.detail?.id == item.id ? model.detail! : item).fileURLs
            guard !urls.isEmpty else { NSSound.beep(); return }
            close()
            NSWorkspace.shared.activateFileViewerSelecting(urls)
        case .quickLook:
            toggleQuickLook(item)
        case .delete:
            model.delete(item)
        case .openContact(let type):
            Task {
                let text = (try? await app.core?.item(item.id))?.text ?? item.preview ?? ""
                guard let url = Item.contactURL(type, in: text) else { return NSSound.beep() }
                close()
                NSWorkspace.shared.open(url)
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
