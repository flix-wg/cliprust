import AppKit

/// `--render-icon <dir.iconset>`: the app icon, drawn in code.
enum AppIcon {
    static func renderIconset(into dir: URL) -> Bool {
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        for points in [16, 32, 128, 256, 512] {
            for scale in [1, 2] {
                let px = points * scale
                let name = scale == 1 ? "icon_\(points)x\(points).png" : "icon_\(points)x\(points)@2x.png"
                guard let png = png(pixels: px), (try? png.write(to: dir.appendingPathComponent(name))) != nil else { return false }
            }
        }
        return true
    }

    private static func png(pixels: Int) -> Data? {
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels, bitsPerSample: 8,
                                         samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                                         bytesPerRow: 0, bitsPerPixel: 0) else { return nil }
        rep.size = NSSize(width: 1024, height: 1024)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        draw()
        NSGraphicsContext.restoreGraphicsState()
        return rep.representation(using: .png, properties: [:])
    }

    private static func draw() {
        let body = NSRect(x: 100, y: 100, width: 824, height: 824)
        let squircle = NSBezierPath(roundedRect: body, xRadius: 186, yRadius: 186)
        NSGraphicsContext.saveGraphicsState()
        let shadow = NSShadow()
        shadow.shadowColor = NSColor(white: 0, alpha: 0.28)
        shadow.shadowOffset = NSSize(width: 0, height: -10)
        shadow.shadowBlurRadius = 24
        shadow.set()
        NSColor.white.setFill()
        squircle.fill()
        NSGraphicsContext.restoreGraphicsState()
        NSGradient(starting: NSColor(white: 0.995, alpha: 1), ending: NSColor(calibratedRed: 0.86, green: 0.87, blue: 0.9, alpha: 1))?
            .draw(in: squircle, angle: -90)

        let graphite = NSColor(calibratedRed: 0.17, green: 0.18, blue: 0.21, alpha: 1)
        let back = NSRect(x: 400, y: 330, width: 330, height: 400)
        let backPath = NSBezierPath(roundedRect: back, xRadius: 70, yRadius: 70)
        backPath.lineWidth = 46
        graphite.withAlphaComponent(0.55).setStroke()
        backPath.stroke()

        let front = NSRect(x: 290, y: 240, width: 330, height: 400)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current?.compositingOperation = .copy
        NSGradient(starting: NSColor(white: 0.97, alpha: 1), ending: NSColor(calibratedRed: 0.9, green: 0.91, blue: 0.93, alpha: 1))?
            .draw(in: NSBezierPath(roundedRect: front.insetBy(dx: -40, dy: -40), xRadius: 104, yRadius: 104), angle: -90)
        NSGraphicsContext.restoreGraphicsState()
        NSGradient(starting: NSColor(calibratedRed: 0.27, green: 0.29, blue: 0.34, alpha: 1), ending: graphite)?
            .draw(in: NSBezierPath(roundedRect: front, xRadius: 70, yRadius: 70), angle: -90)

        NSColor(white: 1, alpha: 0.92).setFill()
        NSBezierPath(roundedRect: NSRect(x: 360, y: 470, width: 190, height: 34), xRadius: 17, yRadius: 17).fill()
        NSBezierPath(roundedRect: NSRect(x: 360, y: 390, width: 130, height: 34), xRadius: 17, yRadius: 17).fill()
    }
}
