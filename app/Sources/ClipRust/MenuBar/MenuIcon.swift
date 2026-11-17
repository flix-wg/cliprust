import AppKit

/// Two stacked cards, drawn as a template image so the menu bar tints it.
enum MenuIcon {
    static func image(paused: Bool) -> NSImage {
        let image = NSImage(size: NSSize(width: 16, height: 16), flipped: true) { _ in
            draw(paused: paused)
            return true
        }
        image.isTemplate = true
        image.accessibilityDescription = paused ? "ClipRust (paused)" : "ClipRust"
        return image
    }

    private static func draw(paused: Bool) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        ctx.setAlpha(paused ? 0.45 : 1)
        ctx.beginTransparencyLayer(auxiliaryInfo: nil)
        let ink = NSColor.black

        let back = NSBezierPath(roundedRect: NSRect(x: 5.6, y: 1.6, width: 8.8, height: 10.4), xRadius: 2.2, yRadius: 2.2)
        back.lineWidth = 1.25
        ink.setStroke()
        back.stroke()

        let frontRect = NSRect(x: 1.6, y: 4.4, width: 8.8, height: 10.4)
        let front = NSBezierPath(roundedRect: frontRect, xRadius: 2.2, yRadius: 2.2)
        NSGraphicsContext.current?.compositingOperation = .clear
        let gap = NSBezierPath(roundedRect: frontRect.insetBy(dx: -1.3, dy: -1.3), xRadius: 3.4, yRadius: 3.4)
        gap.fill()
        NSGraphicsContext.current?.compositingOperation = .sourceOver
        ink.setFill()
        front.fill()

        NSGraphicsContext.current?.compositingOperation = .clear
        NSBezierPath(roundedRect: NSRect(x: 3.6, y: 8.0, width: 4.8, height: 1.1), xRadius: 0.55, yRadius: 0.55).fill()
        NSBezierPath(roundedRect: NSRect(x: 3.6, y: 10.6, width: 3.2, height: 1.1), xRadius: 0.55, yRadius: 0.55).fill()

        if paused {
            let slash = NSBezierPath()
            slash.move(to: NSPoint(x: 1.2, y: 15.2))
            slash.line(to: NSPoint(x: 14.8, y: 0.8))
            slash.lineCapStyle = .round
            slash.lineWidth = 3.6
            slash.stroke()
            NSGraphicsContext.current?.compositingOperation = .sourceOver
            slash.lineWidth = 1.3
            ink.setStroke()
            slash.stroke()
        }
        NSGraphicsContext.current?.compositingOperation = .sourceOver
        ctx.endTransparencyLayer()
    }
}
