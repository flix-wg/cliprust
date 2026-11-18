import AppKit
import Quartz

/// Feeds the Quick Look panel one item, written to a temporary file when needed.
final class QuickLookSource: NSObject, QLPreviewPanelDataSource {
    var url: URL?
    /// Bumped by each request and by `clear()`, so a request that finishes late knows it is stale.
    private var generation = 0

    private static let folder = FileManager.default.temporaryDirectory.appendingPathComponent("ClipRust Quick Look", isDirectory: true)

    /// Deletes the copies written for Quick Look; the panel calls this when it closes.
    static func removeFolder() {
        try? FileManager.default.removeItem(at: folder)
    }

    func clear() {
        generation += 1
        url = nil
        Self.removeFolder()
    }

    /// Starts a request; it stays current until the next one or `clear()`.
    func newRequest() -> Int {
        generation += 1
        return generation
    }

    func isCurrent(_ request: Int) -> Bool { request == generation }

    func numberOfPreviewItems(in panel: QLPreviewPanel!) -> Int { url == nil ? 0 : 1 }
    func previewPanel(_ panel: QLPreviewPanel!, previewItemAt index: Int) -> QLPreviewItem! { url as NSURL? }

    @MainActor
    func prepare(_ item: Item, core: Core?, detail: Item?, request: Int) async -> URL? {
        guard let core else { return nil }
        let full: Item? = detail?.id == item.id ? detail : try? await core.item(item.id)
        guard isCurrent(request), let full else { return nil }
        if item.kind == .file { return full.fileURLs.first }
        let dir = Self.folder
        Self.removeFolder()
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        if item.kind == .image {
            let format = full.formats?.first { $0.hasPrefix("public.") } ?? "public.png"
            guard let data = await core.data(id: item.id, format: format), isCurrent(request) else { return nil }
            let ext = format == "public.jpeg" ? "jpg" : format == "public.tiff" ? "tiff" : "png"
            let url = dir.appendingPathComponent("Image.\(ext)")
            return (try? data.write(to: url)) == nil ? nil : url
        }
        if full.formats?.contains("rtf") == true, let rtf = await core.data(id: item.id, format: "rtf"), isCurrent(request) {
            let url = dir.appendingPathComponent("Clipping.rtf")
            if (try? rtf.write(to: url)) != nil { return url }
        }
        guard isCurrent(request), let text = full.text else { return nil }
        let url = dir.appendingPathComponent("Clipping.txt")
        return (try? text.write(to: url, atomically: true, encoding: .utf8)) == nil ? nil : url
    }
}
