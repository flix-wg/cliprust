import AppKit
import SwiftUI

struct PreviewPane: View {
    let item: Item?

    var body: some View {
        if let item {
            VStack(spacing: 0) {
                PreviewContent(item: item)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                Divider()
                PreviewFooter(item: item)
            }
        } else {
            Color.clear
        }
    }
}

private struct PreviewContent: View {
    let item: Item

    var body: some View {
        if item.isSecret {
            KeyPreview(item: item)
        } else {
            switch item.kind {
            case .image: ImagePreview(item: item)
            case .file: FilePreview(item: item)
            case .url: URLPreview(item: item)
            case .text: TextContentPreview(item: item)
            }
        }
    }
}

private struct TextContentPreview: View {
    let item: Item
    @State private var rich: NSAttributedString?

    var body: some View {
        let text = item.text ?? item.preview ?? ""
        TextPreview(text: text, rich: rich, monospaced: Format.looksLikeCode(text))
            .task(id: item.id) {
                rich = nil
                guard item.formats?.contains("rtf") == true, !Format.looksLikeCode(text),
                      let core = Thumbnails.shared.core,
                      let data = await core.data(id: item.id, format: "rtf") else { return }
                rich = NSAttributedString(rtf: data, documentAttributes: nil)
            }
    }
}

/// Selectable, scrollable text that stays fast for long clippings.
struct TextPreview: NSViewRepresentable {
    let text: String
    let rich: NSAttributedString?
    let monospaced: Bool

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSTextView.scrollableTextView()
        scroll.drawsBackground = false
        scroll.autohidesScrollers = true
        scroll.scrollerStyle = .overlay
        let tv = scroll.documentView as! NSTextView
        tv.isEditable = false
        tv.isSelectable = true
        tv.drawsBackground = false
        tv.textContainerInset = NSSize(width: 10, height: 12)
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        guard let tv = scroll.documentView as? NSTextView else { return }
        let key = "\(text.hashValue)-\(rich?.hash ?? 0)-\(monospaced)"
        guard context.coordinator.key != key else { return }
        context.coordinator.key = key
        let wrap = !monospaced || rich != nil
        scroll.hasHorizontalScroller = !wrap
        tv.isHorizontallyResizable = !wrap
        tv.textContainer?.widthTracksTextView = wrap
        tv.textContainer?.containerSize = NSSize(width: wrap ? scroll.contentSize.width : .greatestFiniteMagnitude,
                                                 height: .greatestFiniteMagnitude)
        if wrap { tv.frame.size.width = scroll.contentSize.width }
        tv.textStorage?.setAttributedString(content)
        tv.scroll(.zero)
    }

    func makeCoordinator() -> Coordinator { Coordinator() }
    final class Coordinator { var key = "" }

    private var content: NSAttributedString {
        let paragraph = NSMutableParagraphStyle()
        paragraph.lineSpacing = monospaced ? 2 : 3
        if let rich {
            let s = NSMutableAttributedString(attributedString: rich)
            let all = NSRange(location: 0, length: s.length)
            s.removeAttribute(.foregroundColor, range: all)
            s.removeAttribute(.backgroundColor, range: all)
            s.addAttribute(.foregroundColor, value: NSColor.labelColor, range: all)
            return s
        }
        let font = monospaced ? NSFont.monospacedSystemFont(ofSize: 12, weight: .regular) : NSFont.systemFont(ofSize: 13)
        return NSAttributedString(string: text, attributes: [
            .font: font, .foregroundColor: NSColor.labelColor, .paragraphStyle: paragraph,
        ])
    }
}

private struct ImagePreview: View {
    let item: Item
    @State private var image: Thumbnail?

    var body: some View {
        let shown = image ?? Thumbnails.shared.cached(item.id, large: true)
        VStack(spacing: 10) {
            if let shown {
                Image(nsImage: shown.image)
                    .resizable()
                    .interpolation(.high)
                    .aspectRatio(contentMode: .fit)
                    .clipShape(.rect(cornerRadius: 6, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).strokeBorder(Color.primary.opacity(0.1), lineWidth: 0.5))
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                Text("\(Format.pixels(shown.pixelSize)) pixels")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            } else {
                ProgressView().controlSize(.small)
            }
        }
        .padding(14)
        .task(id: item.id) {
            image = await Thumbnails.shared.image(for: item.id, large: true)
        }
    }
}

private struct FilePreview: View {
    let item: Item
    @State private var icon: NSImage?

    var body: some View {
        let urls = item.fileURLs
        VStack(spacing: 10) {
            Spacer(minLength: 0)
            if let url = urls.first {
                Image(nsImage: icon ?? NSWorkspace.shared.icon(forFile: url.path))
                    .resizable()
                    .aspectRatio(contentMode: .fit)
                    .frame(width: 96, height: 96)
                Text(url.lastPathComponent)
                    .font(.system(size: 13, weight: .semibold))
                    .multilineTextAlignment(.center)
                    .lineLimit(3)
                Text(ItemRow.folder(of: url))
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                    .truncationMode(.middle)
                    .multilineTextAlignment(.center)
                if urls.count > 1 {
                    Text("and \(urls.count - 1) more")
                        .font(.system(size: 11))
                        .foregroundStyle(.tertiary)
                }
                Button("Reveal in Finder") { NSWorkspace.shared.activateFileViewerSelecting(urls) }
                    .controlSize(.small)
                    .padding(.top, 4)
            }
            Spacer(minLength: 0)
        }
        .padding(16)
        .task(id: item.id) {
            if let url = urls.first { icon = await Thumbnails.fileIcon(url, size: 96) }
        }
    }
}

private struct URLPreview: View {
    let item: Item

    var body: some View {
        let raw = (item.text ?? item.preview ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let url = URL(string: raw)
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 7) {
                Image(systemName: "link").foregroundStyle(.secondary)
                Text(url?.host() ?? "Link")
                    .font(.system(size: 15, weight: .semibold))
                    .lineLimit(1)
            }
            Text(raw)
                .font(.system(size: 12, design: .monospaced))
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
                .lineLimit(10)
            if let url {
                Button("Open in Browser") { NSWorkspace.shared.open(url) }
                    .controlSize(.small)
                    .padding(.top, 2)
            }
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(16)
    }
}

private struct KeyPreview: View {
    let item: Item

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 7) {
                KeyBadge(isID: item.isID)
                Text(item.title ?? item.secretProvider ?? "Secret")
                    .font(.system(size: 15, weight: .semibold))
                    .lineLimit(1)
            }
            Text(item.maskedValue)
                .font(.system(size: 12, design: .monospaced))
                .lineLimit(3)
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.primary.opacity(0.05), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            if let domain = item.domain {
                Label(domain, systemImage: "globe")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
            }
            Text("The full value stays hidden. Reveal it in Keys & IDs (⌘K).")
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .padding(16)
    }
}

private struct PreviewFooter: View {
    let item: Item
    @State private var note = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 3) {
                row("App", item.app.map(AppInfo.name) ?? "Unknown")
                row("Copied", Format.date(item.copiedAt))
                if let formats = item.formats, !formats.isEmpty {
                    row("Formats", Format.formats(formats))
                }
                row("Size", Format.bytes(item.byteSize))
            }
            .font(.system(size: 11))
            if Features.notes {
                TextField("Add a note…", text: $note)
                    .textFieldStyle(.plain)
                    .font(.system(size: 12))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 5)
                    .background(Color.primary.opacity(0.05), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                    .onSubmit {
                        let id = item.id, text = note
                        Task { try? await Thumbnails.shared.core?.setNote(id, text.isEmpty ? nil : text) }
                    }
                    .onChange(of: item.id, initial: true) { note = item.note ?? "" }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
    }

    private func row(_ label: String, _ value: String) -> some View {
        GridRow {
            Text(label).foregroundStyle(.tertiary).gridColumnAlignment(.trailing)
            Text(value).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
        }
    }
}
