import AppKit
import SwiftUI

/// A small panel pinned under System Settings with ClipRust's icon to drag into the Accessibility list.
/// It finds the Settings window through the window server, which needs no permission, and tracks it
/// until the guide closes.
@MainActor
final class AccessDragPanel {
    private static let settingsID = "com.apple.systempreferences"
    private var panel: NSPanel?
    private var timer: Timer?

    func show() {
        if panel == nil { panel = Self.makePanel() }
        guard timer == nil else { return }
        timer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.follow() }
        }
    }

    func hide() {
        timer?.invalidate()
        timer = nil
        panel?.orderOut(nil)
    }

    private func follow() {
        guard let panel else { return }
        // Only beside Settings in front, so it never floats over other apps.
        guard NSWorkspace.shared.frontmostApplication?.bundleIdentifier == Self.settingsID,
              let frame = Self.settingsWindowFrame() else {
            panel.orderOut(nil)
            return
        }
        let size = panel.frame.size
        var origin = NSPoint(x: frame.midX - size.width / 2, y: frame.minY - size.height - 10)
        if let screen = NSScreen.screens.first(where: { $0.frame.intersects(frame) }) {
            // Short of room below, overlap Settings only as far as the screen edge requires.
            origin.y = max(origin.y, screen.visibleFrame.minY + 4)
        }
        if panel.frame.origin != origin { panel.setFrameOrigin(origin) }
        if !panel.isVisible { panel.orderFrontRegardless() }
    }

    private static func settingsWindowFrame() -> NSRect? {
        guard let pid = NSRunningApplication.runningApplications(withBundleIdentifier: settingsID)
            .first(where: { $0.activationPolicy == .regular })?.processIdentifier,
            let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]]
        else { return nil }
        // Window-server bounds start at the primary screen's top-left; AppKit's at its bottom-left.
        let primaryHeight = NSScreen.screens.first?.frame.height ?? 0
        return windows.compactMap { w -> NSRect? in
            guard w[kCGWindowOwnerPID as String] as? pid_t == pid, w[kCGWindowLayer as String] as? Int == 0,
                  let dict = w[kCGWindowBounds as String] as? NSDictionary,
                  let r = CGRect(dictionaryRepresentation: dict), r.width > 320, r.height > 240 else { return nil }
            return NSRect(x: r.minX, y: primaryHeight - r.maxY, width: r.width, height: r.height)
        }.max { $0.width * $0.height < $1.width * $1.height }
    }

    private static func makePanel() -> NSPanel {
        let host = NSHostingView(rootView: AccessDragView())
        host.setFrameSize(host.fittingSize)
        let panel = NSPanel(contentRect: NSRect(origin: .zero, size: host.fittingSize),
                            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.contentView = host
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.level = .floating
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        return panel
    }
}

struct AccessDragView: View {
    var body: some View {
        HStack(spacing: 12) {
            DragTile().frame(width: 44, height: 44)
            VStack(alignment: .leading, spacing: 2) {
                Text("Drag ClipRust into the list above").font(.headline)
                Text("Then make sure its switch is on.").foregroundStyle(.secondary)
            }
            Image(systemName: "arrow.up").font(.title3.weight(.semibold)).foregroundStyle(.tint)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(.separator))
    }
}

private struct DragTile: NSViewRepresentable {
    func makeNSView(context: Context) -> DragTileView { DragTileView() }
    func updateNSView(_ view: DragTileView, context: Context) {}
}

/// Drags the app bundle's file URL, which the Accessibility list accepts like an app from Finder.
private final class DragTileView: NSView, NSDraggingSource {
    private var started = false

    override func draw(_ dirtyRect: NSRect) { NSApp.applicationIconImage.draw(in: bounds) }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .openHand) }
    override func mouseDown(with event: NSEvent) { started = false }

    override func mouseDragged(with event: NSEvent) {
        guard !started else { return }
        started = true
        let item = NSDraggingItem(pasteboardWriter: Bundle.main.bundleURL as NSURL)
        item.setDraggingFrame(bounds, contents: NSApp.applicationIconImage)
        beginDraggingSession(with: [item], event: event, source: self)
    }

    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation { .copy }
}
