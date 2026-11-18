import AppKit
import SwiftUI

/// The ⌘L / ⌘D / ⌘⇧S overlay: a type-ahead field over lists.
struct ListPalette: Equatable {
    enum Mode: Equatable { case go, add(Int64), saveSearch }

    let mode: Mode
    var text: String { didSet { if text != oldValue { selection = 0 } } }
    var selection = 0

    var placeholder: String {
        switch mode {
        case .go: "Go to list…"
        case .add: "Add to list…"
        case .saveSearch: "Save search as…"
        }
    }
}

enum PaletteRow: Hashable {
    case filter(ItemFilter), create(String), save(String)
}

extension ItemFilter {
    static let kinds: [ItemFilter] = [.kind(.text), .kind(.url), .kind(.image), .kind(.file)]

    var key: String {
        switch self {
        case .all: "all"
        case .kind(let kind): "kind:\(kind.rawValue)"
        case .keys: "keys"
        case .list(let name): "list:\(name)"
        }
    }

    var kind: ItemKind? {
        if case .kind(let kind) = self { return kind }
        return nil
    }

    var symbol: String {
        switch self {
        case .all: "square.stack"
        case .kind(.text): "text.alignleft"
        case .kind(.url): "link"
        case .kind(.image): "photo"
        case .kind(.file): "doc"
        case .keys: "key.horizontal"
        case .list: "list.bullet"
        }
    }

    init?(key: String) {
        let (head, tail) = (key.prefix { $0 != ":" }, String(key.drop { $0 != ":" }.dropFirst()))
        switch head {
        case "all": self = .all
        case "keys": self = .keys
        case "kind": guard let kind = ItemKind(rawValue: tail) else { return nil }; self = .kind(kind)
        case "list": self = .list(tail)
        default: return nil
        }
    }
}

extension PanelModel {
    /// Opening the open mode again closes it; false means there is nothing to open.
    func openPalette(_ mode: ListPalette.Mode) -> Bool {
        if palette?.mode == mode {
            palette = nil
            return true
        }
        let text = mode == .saveSearch ? query.trimmingCharacters(in: .whitespaces) : ""
        if mode == .saveSearch, text.isEmpty { return false }
        palette = ListPalette(mode: mode, text: text)
        return true
    }

    var paletteRows: [PaletteRow] {
        guard let palette else { return [] }
        let text = palette.text.trimmingCharacters(in: .whitespaces)
        switch palette.mode {
        case .go:
            return ranked(filters(), text)
        case .add:
            return ranked(app.orderedLists.filter { $0.manual && $0.name != Item.idsList }.map { .list($0.name) }, text)
        case .saveSearch:
            return text.isEmpty || isTaken(text) ? [] : [.save(text)]
        }
    }

    func movePalette(by delta: Int) {
        guard let selection = palette?.selection, !paletteRows.isEmpty else { return }
        palette?.selection = min(max(selection + delta, 0), paletteRows.count - 1)
    }

    func confirmPalette(_ index: Int? = nil) -> Bool {
        guard let palette else { return false }
        let rows = paletteRows
        let i = index ?? palette.selection
        guard rows.indices.contains(i) else { return false }
        self.palette = nil
        switch (rows[i], palette.mode) {
        case (.filter(let f), .add(let id)): add(id, to: f.title)
        case (.create(let name), .add(let id)): add(id, to: name)
        case (.filter(let f), _): filter = f
        case (.create(let name), _):
            Task { if await app.createList(name) { filter = .list(name) } else { NSSound.beep() } }
        case (.save(let name), _):
            let (text, kind) = (query.trimmingCharacters(in: .whitespaces), filter.kind)
            Task {
                guard await app.createList(name, contains: text, kind: kind) else { return NSSound.beep() }
                query = ""
                filter = .list(name)
            }
        }
        return true
    }

    private func add(_ id: Int64, to name: String) {
        Task {
            await app.addToList(name, id)
            if filter == .list(name) { reload() }
            showToast("Added to \(name)")
        }
    }

    func jump(to index: Int) -> Bool {
        let all = filters()
        guard all.indices.contains(index) else { return false }
        filter = all[index]
        return true
    }

    private func isTaken(_ name: String) -> Bool {
        ([Item.keysList, Item.idsList] + app.lists.map(\.name)).contains { $0.caseInsensitiveCompare(name) == .orderedSame }
    }

    private func ranked(_ filters: [ItemFilter], _ text: String) -> [PaletteRow] {
        guard !text.isEmpty else { return filters.map(PaletteRow.filter) }
        let hits = filters.filter { $0.title.localizedCaseInsensitiveContains(text) }
        let prefix = hits.filter { $0.title.lowercased().hasPrefix(text.lowercased()) }
        let rows = (prefix + hits.filter { !prefix.contains($0) }).map(PaletteRow.filter)
        let exact = filters.contains { $0.title.caseInsensitiveCompare(text) == .orderedSame }
        return exact || isTaken(text) ? rows : rows + [.create(text)]
    }
}

struct ListPaletteView: View {
    @Bindable var model: PanelModel
    @FocusState private var focused: Bool

    private static let rowHeight: CGFloat = 26

    var body: some View {
        let rows = model.paletteRows
        let selection = model.palette?.selection ?? 0
        VStack(spacing: 0) {
            TextField(model.palette?.placeholder ?? "", text: Binding(get: { model.palette?.text ?? "" },
                                                                     set: { model.palette?.text = $0 }))
                .textFieldStyle(.plain)
                .font(.system(size: 13))
                .focused($focused)
                .padding(.horizontal, 12)
                .frame(height: 34)
            if !rows.isEmpty {
                Divider()
                ScrollViewReader { proxy in
                    ScrollView {
                        VStack(spacing: 1) {
                            ForEach(Array(rows.enumerated()), id: \.element) { i, row in
                                PaletteRowView(row: row, mode: model.palette?.mode ?? .go, hint: hint(row),
                                               smart: isSmart(row), selected: i == selection)
                                    .id(i)
                                    .onTapGesture { _ = model.confirmPalette(i) }
                            }
                        }
                        .padding(5)
                    }
                    .frame(height: CGFloat(min(rows.count, 8)) * (Self.rowHeight + 1) + 9)
                    .onChange(of: selection) { _, i in proxy.scrollTo(i) }
                }
            }
        }
        .frame(width: 280)
        .background { PanelBackground(radius: 10) }
        .clipShape(.rect(cornerRadius: 10, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(Color(nsColor: .separatorColor), lineWidth: 0.5)
        }
        .shadow(color: .black.opacity(0.18), radius: 14, y: 6)
        .onAppear { focused = true }
    }

    private func hint(_ row: PaletteRow) -> String? {
        guard model.palette?.mode == .go, case .filter(let f) = row,
              let i = model.filters().firstIndex(of: f), i < 9 else { return nil }
        return "⌃\(i + 1)"
    }

    private func isSmart(_ row: PaletteRow) -> Bool {
        guard case .filter(.list(let name)) = row else { return false }
        return model.app.lists.contains { $0.name == name && !$0.manual }
    }
}

private struct PaletteRowView: View {
    let row: PaletteRow
    let mode: ListPalette.Mode
    let hint: String?
    let smart: Bool
    let selected: Bool
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: symbol)
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
                .frame(width: 16)
            Text(title).font(.system(size: 13)).lineLimit(1)
            Spacer(minLength: 4)
            if let hint {
                Text(hint).font(.system(size: 11)).foregroundStyle(.tertiary).monospacedDigit()
            }
        }
        .padding(.horizontal, 8)
        .frame(height: 26)
        .background(RoundedRectangle(cornerRadius: 6, style: .continuous)
            .fill(selected ? Color.accentColor.opacity(colorScheme == .dark ? 0.32 : 0.2) : .clear))
        .contentShape(Rectangle())
    }

    private var symbol: String {
        switch row {
        case .filter(let f): smart ? "line.3.horizontal.decrease" : f.symbol
        case .create: "plus"
        case .save: "line.3.horizontal.decrease"
        }
    }

    private var title: String {
        switch row {
        case .filter(let f): f.title
        case .create(let name): if case .add = mode { "Create “\(name)” and add" } else { "Create list “\(name)”" }
        case .save(let name): "Save search as “\(name)”"
        }
    }
}
