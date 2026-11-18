import AppKit
import SwiftUI

struct AppIconView: View {
    let bundleID: String
    var size: CGFloat = 16

    var body: some View {
        if let icon = AppInfo.icon(bundleID) {
            Image(nsImage: icon).resizable().interpolation(.high).frame(width: size, height: size)
        } else {
            Image(systemName: "app.dashed")
                .font(.system(size: size * 0.75))
                .foregroundStyle(.secondary)
                .frame(width: size, height: size)
        }
    }
}

/// An app or contact filter in the search field; ⌫ at the start of the field or × removes it.
struct SearchToken<Icon: View>: View {
    let title: String
    let help: String
    let remove: () -> Void
    @ViewBuilder let icon: Icon
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 5) {
            icon
            Text(title).font(.system(size: 13, weight: .medium)).lineLimit(1)
            Button(action: remove) {
                Image(systemName: "xmark")
                    .font(.system(size: 8, weight: .bold))
                    .foregroundStyle(hovering ? .primary : .secondary)
                    .frame(width: 14, height: 14)
                    .background(Circle().fill(Color.primary.opacity(hovering ? 0.12 : 0)))
            }
            .buttonStyle(.plain)
            .onHover { hovering = $0 }
            .help(help)
        }
        .padding(.leading, 5)
        .padding(.trailing, 4)
        .frame(height: 24)
        .background(Capsule().fill(Color.accentColor.opacity(0.15)))
        .overlay(Capsule().strokeBorder(Color.accentColor.opacity(0.3), lineWidth: 0.5))
        .fixedSize()
    }
}

/// "Copied from Mail · 42" or "Phone numbers · 12" above the results; ⇥, ⏎ or a click makes it the token.
struct SuggestionRow: View {
    let suggestion: Suggestion
    let selected: Bool
    @AppStorage(Prefs.Key.density) private var density = Density.compact
    @Environment(\.colorScheme) private var colorScheme
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 8) {
            Group {
                switch suggestion {
                case .app(let s): AppIconView(bundleID: s.app.bundleId)
                case .contact(let type, _, _): Image(systemName: type.symbol).font(.system(size: 13)).foregroundStyle(.secondary)
                }
            }
            .frame(width: 18, height: 18)
            HStack(spacing: 4) {
                switch suggestion {
                case .app(let s):
                    Text("Copied from").foregroundStyle(.secondary)
                    Text(s.app.title).fontWeight(.semibold)
                    Text("·  \(s.app.count)").foregroundStyle(.secondary).monospacedDigit()
                case .contact(let type, let count, _):
                    Text(type.title).fontWeight(.semibold)
                    Text("·  \(PanelModel.countText(count))").foregroundStyle(.secondary).monospacedDigit()
                }
            }
            .font(.system(size: 13))
            .lineLimit(1)
            .frame(maxWidth: .infinity, alignment: .leading)
            Text("tab")
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(.secondary)
                .padding(.horizontal, 5)
                .frame(height: 16)
                .overlay(RoundedRectangle(cornerRadius: 4, style: .continuous).strokeBorder(Color.secondary.opacity(0.4), lineWidth: 0.5))
        }
        .padding(.horizontal, 10)
        .frame(height: density.rowHeight)
        .background(RoundedRectangle(cornerRadius: 7, style: .continuous).fill(fill))
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
    }

    private var fill: Color {
        if selected { return Color.accentColor.opacity(colorScheme == .dark ? 0.32 : 0.2) }
        return Color.primary.opacity(hovering ? 0.07 : 0.035)
    }
}
