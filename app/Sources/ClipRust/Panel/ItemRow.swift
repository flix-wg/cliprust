import AppKit
import SwiftUI

struct ItemRow: View {
    let item: Item
    let index: Int
    let selected: Bool
    let pinned: Bool
    var now = Date()
    @AppStorage(Prefs.Key.density) private var density = Density.compact
    @AppStorage(Prefs.Key.showAppIcons) private var showAppIcons = true
    @AppStorage(Prefs.Key.showHints) private var showHints = true
    @Environment(\.colorScheme) private var colorScheme
    @State private var hovering = false
    @State private var thumbnail: Thumbnail?

    var body: some View {
        HStack(spacing: 8) {
            leading
                .frame(width: 18, height: 18)
            if !item.contacts.isEmpty {
                HStack(spacing: 2) {
                    ForEach(item.contacts, id: \.self) { Image(systemName: Item.contactSymbol($0)) }
                }
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
                .padding(.leading, -3)
            }
            content
                .frame(maxWidth: .infinity, alignment: .leading)
            trailing
        }
        .padding(.horizontal, 10)
        .frame(height: density.rowHeight)
        .background(RoundedRectangle(cornerRadius: 7, style: .continuous).fill(fill))
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
    }

    private var fill: Color {
        if selected { return Color.accentColor.opacity(colorScheme == .dark ? 0.32 : 0.2) }
        return hovering ? Color.primary.opacity(0.05) : .clear
    }

    @ViewBuilder private var leading: some View {
        if showAppIcons, let icon = AppInfo.icon(item.app) {
            Image(nsImage: icon).resizable().interpolation(.high).frame(width: 16, height: 16)
        } else {
            Image(systemName: item.symbol)
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
        }
    }

    @ViewBuilder private var content: some View {
        if item.isSecret {
            HStack(spacing: 7) {
                if let title = item.title {
                    Text(title).font(.system(size: 13)).lineLimit(1)
                } else {
                    Text(item.maskedValue)
                        .font(.system(size: 12, design: .monospaced))
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .layoutPriority(1)
                }
                KeyBadge(isID: item.isID)
                if let provider = item.secretProvider {
                    Text(provider).font(.system(size: 11)).foregroundStyle(.secondary).fixedSize()
                }
                if let note = item.note, !note.isEmpty {
                    Text(note.collapsingWhitespace).font(.system(size: 11)).foregroundStyle(.secondary).lineLimit(1)
                }
            }
        } else if item.kind == .image, item.title == nil {
            HStack(spacing: 8) {
                imageThumb
                Text(thumb.map { Format.pixels($0.pixelSize) } ?? "Image")
                    .font(.system(size: 13))
                    .monospacedDigit()
                Text(Format.bytes(item.byteSize))
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
        } else if item.kind == .file, item.title == nil, let url = item.fileURLs.first {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(url.lastPathComponent).font(.system(size: 13)).lineLimit(1)
                Text(Self.folder(of: url) + (item.fileURLs.count > 1 ? "  +\(item.fileURLs.count - 1)" : ""))
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.head)
            }
        } else {
            Text(item.oneLine.isEmpty ? " " : item.oneLine)
                .font(.system(size: 13))
                .lineLimit(1)
        }
    }

    private var thumb: Thumbnail? { thumbnail ?? Thumbnails.shared.cached(item.id) }

    private var imageThumb: some View {
        let side: CGFloat = density == .compact ? 26 : 34
        return Group {
            if let thumb {
                Image(nsImage: thumb.image).resizable().aspectRatio(contentMode: .fill)
            } else {
                Color.primary.opacity(0.06)
            }
        }
        .frame(width: side * 1.4, height: side)
        .clipShape(.rect(cornerRadius: 4, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 4, style: .continuous).strokeBorder(Color.primary.opacity(0.1), lineWidth: 0.5))
        .task(id: item.id) {
            if thumb == nil { thumbnail = await Thumbnails.shared.image(for: item.id) }
        }
    }

    private var trailing: some View {
        HStack(spacing: 6) {
            if item.hasNote {
                Image(systemName: "note.text").font(.system(size: 10)).foregroundStyle(.tertiary)
            }
            if pinned {
                Image(systemName: "pin.fill").font(.system(size: 9)).foregroundStyle(.tertiary)
            }
            Text(Format.ago(item.copiedAt, now: now))
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .monospacedDigit()
            if showHints {
                Text(index < 9 ? "⌘\(index + 1)" : "")
                    .font(.system(size: 11))
                    .foregroundStyle(.tertiary)
                    .frame(width: 22, alignment: .trailing)
            }
        }
    }

    static func folder(of url: URL) -> String {
        (url.deletingLastPathComponent().path as NSString).abbreviatingWithTildeInPath
    }
}

struct KeyBadge: View {
    let isID: Bool

    var body: some View {
        let tint: Color = isID ? .blue : .orange
        Text(isID ? "ID" : "KEY")
            .font(.system(size: 9, weight: .semibold))
            .tracking(0.4)
            .foregroundStyle(tint)
            .padding(.horizontal, 5)
            .padding(.vertical, 1.5)
            .background(tint.opacity(0.15), in: Capsule())
            .fixedSize()
    }
}
