import Foundation
import Observation

enum ItemFilter: Hashable {
    case all, kind(ItemKind), keys, list(String)

    static let fixed: [ItemFilter] = [.all, .kind(.text), .kind(.url), .kind(.image), .kind(.file), .keys]

    var title: String {
        switch self {
        case .all: "All"
        case .kind(.text): "Text"
        case .kind(.url): "Links"
        case .kind(.image): "Images"
        case .kind(.file): "Files"
        case .keys: "Keys"
        case .list(let name): name
        }
    }
}

enum ChooseMode { case standard, plain, copyOnly }

enum PanelAction {
    case choose(ChooseMode)
    case addToList(String), newList, removeFromList(String)
    case rename, note, revealInFinder, quickLook, delete
}

@MainActor @Observable
final class PanelModel {
    static let pageSize = 200

    let app: AppState
    var query = "" { didSet { if query != oldValue { reload(resetSelection: true) } } }
    var filter: ItemFilter = .all { didSet { if filter != oldValue { reload(resetSelection: true) } } }
    var items: [Item] = []
    var selectedID: Int64? { didSet { if selectedID != oldValue { loadDetail() } } }
    var detail: Item?
    var loaded = false
    var isVisible = false

    @ObservationIgnored var rowFrames: [Int64: CGRect] = [:]
    @ObservationIgnored var perform: (PanelAction, Item) -> Void = { _, _ in }
    @ObservationIgnored private var generation = 0

    init(app: AppState) {
        self.app = app
    }

    var filters: [ItemFilter] { ItemFilter.fixed + app.userLists.map { .list($0.name) } }
    var selectedIndex: Int? { items.firstIndex { $0.id == selectedID } }
    var selected: Item? { selectedIndex.map { items[$0] } }
    var current: Item? {
        guard let selected else { return nil }
        guard let detail, detail.id == selected.id, detail.isSecret == selected.isSecret else { return selected }
        return detail
    }

    func showsCount(_ f: ItemFilter) -> Bool {
        loaded && filter == f && (f != .all || !query.isEmpty)
    }

    var countLabel: String { items.count >= Self.pageSize ? "\(Self.pageSize)+" : "\(items.count)" }

    var emptyMessage: String {
        if !query.isEmpty { return "No matches for “\(query)”" }
        switch filter {
        case .all: return "Nothing copied yet"
        case .keys: return "No keys yet"
        case .kind: return "No \(filter.title.lowercased()) yet"
        case .list(let name): return "\(name) is empty"
        }
    }

    var currentManualList: String? {
        if case .list(let name) = filter, app.manualLists.contains(where: { $0.name == name }) { return name }
        return nil
    }

    func prepareForShow() {
        if query.isEmpty { reload(resetSelection: true) } else { query = "" }
    }

    func reload(resetSelection: Bool = false) {
        guard let core = app.core else { return }
        generation += 1
        let gen = generation
        let (q, f) = (query, filter)
        let hasKeys = app.hasKeysList
        Task {
            let found: [Item]
            do {
                switch f {
                case .all: found = try await core.items(query: q, limit: Self.pageSize)
                case .kind(let kind): found = try await core.items(query: q, kind: kind, limit: Self.pageSize)
                case .keys: found = hasKeys ? try await core.items(query: q, list: Item.keysList, limit: Self.pageSize) : []
                case .list(let name): found = try await core.items(query: q, list: name, limit: Self.pageSize)
                }
            } catch {
                found = []
            }
            guard gen == generation else { return }
            items = found
            loaded = true
            if resetSelection || !found.contains(where: { $0.id == selectedID }) {
                selectedID = found.first?.id
            }
        }
    }

    func move(by delta: Int) {
        guard !items.isEmpty else { return }
        let i = selectedIndex.map { $0 + delta } ?? 0
        selectedID = items[min(max(i, 0), items.count - 1)].id
    }

    func cycleFilter(by delta: Int) {
        let all = filters
        let i = all.firstIndex(of: filter) ?? 0
        filter = all[(i + delta + all.count) % all.count]
    }

    func delete(_ item: Item) {
        guard let core = app.core else { return }
        let index = items.firstIndex { $0.id == item.id } ?? 0
        items.removeAll { $0.id == item.id }
        if selectedID == item.id {
            selectedID = items.isEmpty ? nil : items[min(index, items.count - 1)].id
        }
        Task {
            try? await core.delete(item.id)
            await app.refreshRecent()
        }
    }

    private func loadDetail() {
        guard let id = selectedID, let core = app.core else { return }
        if detail?.id == id { return }
        Task {
            let full = try? await core.item(id)
            if selectedID == id { detail = full }
        }
    }
}
