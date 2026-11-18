import AppKit
import SwiftUI

/// `--render-previews <dir>`: draws the main surfaces with sample data into PNGs, light and dark.
@MainActor
enum PreviewRenderer {
    static func run(into dir: URL) {
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        SampleData.seedImages()
        let app = SampleData.appState()

        for (suffix, name) in [("light", NSAppearance.Name.aqua), ("dark", .darkAqua)] {
            let appearance = NSAppearance(named: name)!
            for (shot, select, filter, query) in [("panel-text", Int64(1), ItemFilter.all, ""), ("panel-image", 3, .all, ""),
                                                  ("panel-key", 4, .all, ""), ("panel-file", 6, .all, ""),
                                                  ("panel-empty", 0, .kind(.text), "invoice")] {
                let model = PanelModel(app: app)
                model.filter = filter
                model.query = query
                model.items = query.isEmpty ? SampleData.items : []
                model.loaded = true
                model.isVisible = true
                model.selectedID = select == 0 ? nil : select
                model.detail = SampleData.items.first { $0.id == select }
                let view = PanelView(model: model).environment(\.renderingPreview, true)
                render(view, size: Prefs.defaultPanelSize, appearance: appearance, to: dir.appendingPathComponent("\(shot)-\(suffix).png"))
            }

            for (shot, text, held) in [("panel-palette", "", true), ("panel-palette-apps", "ma", false)] {
                let palette = PanelModel(app: app)
                palette.items = SampleData.items
                palette.loaded = true
                palette.isVisible = true
                palette.selectedID = 1
                palette.controlHeld = held
                palette.palette = ListPalette(mode: .go, text: text)
                render(PanelView(model: palette).environment(\.renderingPreview, true), size: Prefs.defaultPanelSize,
                       appearance: appearance, to: dir.appendingPathComponent("\(shot)-\(suffix).png"))
            }

            // "mail" suggests the app; the token then narrows to Safari.
            let mail = SampleData.apps[3], safari = SampleData.apps[0]
            for (shot, query, token) in [("panel-app-suggestion", "mail", nil as SourceApp?), ("panel-app-token", "rust", safari)] {
                let model = PanelModel(app: app)
                model.query = query
                model.source = token
                model.items = SampleData.items.filter { token == nil ? $0.app == mail.bundleId || $0.id == 5 : $0.app == safari.bundleId && !$0.isSecret }
                model.suggestion = token == nil ? AppSuggestion(app: mail, term: query) : nil
                model.loaded = true
                model.isVisible = true
                model.selectedID = model.items.first?.id
                model.detail = model.items.first
                render(PanelView(model: model).environment(\.renderingPreview, true), size: Prefs.defaultPanelSize,
                       appearance: appearance, to: dir.appendingPathComponent("\(shot)-\(suffix).png"))
            }

            let keys = KeysModel(app: app, items: SampleData.items.filter(\.isSecret))
            keys.selection = [4]
            let keysHost = NSHostingController(rootView: KeysView(model: keys).environment(\.renderingPreview, true))
            keysHost.sceneBridgingOptions = [.toolbars, .title]
            let keysWindow = NSWindow(contentViewController: keysHost)
            keysWindow.styleMask = [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView]
            keysWindow.toolbarStyle = .unified
            keysWindow.title = "Keys & IDs"
            keysWindow.setContentSize(NSSize(width: 900, height: 560))
            capture(keysWindow, appearance: appearance, to: dir.appendingPathComponent("keys-\(suffix).png"))

            let settings = SettingsWindowController.makeWindow(app: app)
            for (i, item) in settings.tabs.tabViewItems.enumerated() {
                settings.tabs.selectedTabViewItemIndex = i
                capture(settings.window, appearance: appearance,
                        to: dir.appendingPathComponent("settings-\(item.label.lowercased())-\(suffix).png"))
            }
        }
        renderMenuIcons(to: dir.appendingPathComponent("menu-icons.png"))
    }

    private static func render(_ view: some View, size: NSSize, appearance: NSAppearance, to url: URL) {
        let host = NSHostingView(rootView: view.frame(width: size.width, height: size.height))
        host.frame = NSRect(origin: .zero, size: size)
        let window = NSWindow(contentRect: host.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.appearance = appearance
        window.backgroundColor = .clear
        window.isOpaque = false
        window.contentView = host
        settle()
        write(host, to: url)
    }

    private static func capture(_ window: NSWindow, appearance: NSAppearance, to url: URL) {
        window.appearance = appearance
        window.alphaValue = 0.01
        window.setFrameOrigin(NSPoint(x: -6000, y: -6000))
        window.orderFront(nil)
        settle()
        window.alphaValue = 1
        if let frameView = window.contentView?.superview { write(frameView, to: url) }
        window.orderOut(nil)
    }

    private static func settle() {
        for _ in 0..<6 { RunLoop.main.run(until: Date().addingTimeInterval(0.08)) }
    }

    private static func write(_ view: NSView, to url: URL) {
        view.layoutSubtreeIfNeeded()
        guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
        view.cacheDisplay(in: view.bounds, to: rep)
        try? rep.representation(using: .png, properties: [:])?.write(to: url)
    }

    private static func renderMenuIcons(to url: URL) {
        let scale: CGFloat = 4
        let cell = NSSize(width: 44, height: 24)
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(cell.width * 2 * scale), pixelsHigh: Int(cell.height * 2 * scale),
                                   bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                                   colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        rep.size = NSSize(width: cell.width * 2, height: cell.height * 2)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        for (row, (bg, ink)) in [(NSColor(white: 0.93, alpha: 1), NSColor(white: 0.1, alpha: 1)),
                                 (NSColor(white: 0.16, alpha: 1), NSColor.white)].enumerated() {
            for (col, paused) in [false, true].enumerated() {
                let origin = NSPoint(x: CGFloat(col) * cell.width, y: CGFloat(1 - row) * cell.height)
                bg.setFill()
                NSRect(origin: origin, size: cell).fill()
                let icon = MenuIcon.image(paused: paused)
                let tinted = NSImage(size: icon.size, flipped: false) { rect in
                    icon.draw(in: rect)
                    ink.set()
                    rect.fill(using: .sourceAtop)
                    return true
                }
                tinted.draw(in: NSRect(x: origin.x + 14, y: origin.y + 4, width: 16, height: 16))
            }
        }
        NSGraphicsContext.restoreGraphicsState()
        try? rep.representation(using: .png, properties: [:])?.write(to: url)
    }
}
