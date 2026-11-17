import AppKit
import ImageIO
import QuickLookThumbnailing

final class Thumbnail {
    let image: NSImage
    let pixelSize: CGSize
    init(image: NSImage, pixelSize: CGSize) {
        self.image = image
        self.pixelSize = pixelSize
    }
}

/// Downscaled images for rows and the preview pane, loaded lazily.
@MainActor
final class Thumbnails {
    static let shared = Thumbnails()
    var core: Core?
    private let small = NSCache<NSNumber, Thumbnail>()
    private let large = NSCache<NSNumber, Thumbnail>()

    init() {
        small.countLimit = 300
        large.countLimit = 6
    }

    func cached(_ id: Int64, large wantLarge: Bool = false) -> Thumbnail? {
        (wantLarge ? large : small).object(forKey: NSNumber(value: id))
    }

    func seed(_ id: Int64, _ thumb: Thumbnail) {
        small.setObject(thumb, forKey: NSNumber(value: id))
        large.setObject(thumb, forKey: NSNumber(value: id))
    }

    func image(for id: Int64, large wantLarge: Bool = false) async -> Thumbnail? {
        if let hit = cached(id, large: wantLarge) { return hit }
        guard let core, let data = await core.imageData(id) else { return nil }
        let maxPixel = wantLarge ? 1400 : 96
        let thumb = await Task.detached { Self.downscale(data, maxPixel: maxPixel) }.value
        if let thumb { (wantLarge ? large : small).setObject(thumb, forKey: NSNumber(value: id)) }
        return thumb
    }

    nonisolated static func downscale(_ data: Data, maxPixel: Int) -> Thumbnail? {
        guard let src = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
        let props = CGImageSourceCopyPropertiesAtIndex(src, 0, nil) as? [CFString: Any]
        let w = props?[kCGImagePropertyPixelWidth] as? Int ?? 0
        let h = props?[kCGImagePropertyPixelHeight] as? Int ?? 0
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: maxPixel,
        ]
        guard let cg = CGImageSourceCreateThumbnailAtIndex(src, 0, options as CFDictionary) else { return nil }
        let image = NSImage(cgImage: cg, size: NSSize(width: cg.width, height: cg.height))
        return Thumbnail(image: image, pixelSize: CGSize(width: w, height: h))
    }

    static func fileIcon(_ url: URL, size: CGFloat) async -> NSImage {
        let scale = NSScreen.main?.backingScaleFactor ?? 2
        let request = QLThumbnailGenerator.Request(fileAt: url, size: CGSize(width: size, height: size),
                                                   scale: scale, representationTypes: .all)
        if let rep = try? await QLThumbnailGenerator.shared.generateBestRepresentation(for: request) {
            return rep.nsImage
        }
        return NSWorkspace.shared.icon(forFile: url.path)
    }
}
