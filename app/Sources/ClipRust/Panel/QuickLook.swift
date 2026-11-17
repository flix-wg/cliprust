import AppKit
import Quartz

/// Feeds the Quick Look panel one item, written to a temporary file when needed.
final class QuickLookSource: NSObject, QLPreviewPanelDataSource {
    var url: URL?

    func numberOfPreviewItems(in panel: QLPreviewPanel!) -> Int { url == nil ? 0 : 1 }
    func previewPanel(_ panel: QLPreviewPanel!, previewItemAt index: Int) -> QLPreviewItem! { url as NSURL? }

    @MainActor
    func prepare(_ item: Item, core: Core?, detail: Item?) async -> URL? {
        guard let core else { return nil }
        let full: Item? = detail?.id == item.id ? detail : try? await core.item(item.id)
        guard let full else { return nil }
        if item.kind == .file { return full.fileURLs.first }
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("ClipRust Quick Look", isDirectory: true)
        try? FileManager.default.removeItem(at: dir)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        if item.kind == .image {
            let format = full.formats?.first { $0.hasPrefix("public.") } ?? "public.png"
            guard let data = await core.data(id: item.id, format: format) else { return nil }
            let ext = format == "public.jpeg" ? "jpg" : format == "public.tiff" ? "tiff" : "png"
            let url = dir.appendingPathComponent("Image.\(ext)")
            return (try? data.write(to: url)) == nil ? nil : url
        }
        if full.formats?.contains("rtf") == true, let rtf = await core.data(id: item.id, format: "rtf") {
            let url = dir.appendingPathComponent("Clipping.rtf")
            if (try? rtf.write(to: url)) != nil { return url }
        }
        guard let text = full.text else { return nil }
        let url = dir.appendingPathComponent("Clipping.txt")
        return (try? text.write(to: url, atomically: true, encoding: .utf8)) == nil ? nil : url
    }
}
