import Foundation
import Observation

enum ItemFilter: Hashable {
    case all, kind(ItemKind), keys, list(String)

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
    case addToList(String), newList, removeFromList(String), togglePin
    case rename, note, revealInFinder, quickLook, delete
}

/// An app named by a search term; accepting it turns the term into the app token.
struct AppSuggestion: Equatable {
    let app: SourceApp
    let term: String
}

@MainActor @Observable
final class PanelModel {
    static let pageSize = 200

    let app: AppState
    var query = "" { didSet { if query != oldValue { reload(resetSelection: true) } } }
    /// The app token in the search field: only items copied from that app.
    var source: SourceApp? { didSet { if source?.bundleId != oldValue?.bundleId { reload(resetSelection: true) } } }
    var suggestion: AppSuggestion?
    var suggestionSelected = false
    var filter: ItemFilter = .all {
        didSet {
            guard filter != oldValue else { return }
            if isVisible { Prefs.lastFilter = filter.key }
            reload(resetSelection: true)
        }
    }
    var items: [Item] = []
    var selectedID: Int64? { didSet { if selectedID != oldValue { loadDetail() } } }
    var detail: Item?
    var loaded = false
    var isVisible = false
    var palette: ListPalette?
    var toast: String?
    var toastSymbol = "checkmark"
    var controlHeld = false
    /// The time rows show their age against; set on open.
    var now = Date()

    @ObservationIgnored var perform: (PanelAction, Item) -> Void = { _, _ in }
    @ObservationIgnored private var generation = 0
    @ObservationIgnored private var toastGeneration = 0
    @ObservationIgnored private var loadedRevision = -1

    init(app: AppState) {
        self.app = app
    }

    /// Bar order: All, pinned lists, Keys, the type filters, then the other lists.
    /// Pinned shows only while it holds items.
    func filters(kinds: Bool = Prefs.showTypeFilters) -> [ItemFilter] {
        let lists = app.orderedLists.map(\.name)
            .filter { $0 != Item.pinnedList || !app.pinnedIDs.isEmpty || filter == .list($0) }
        let pinned = lists.filter(app.pinnedLists.contains)
        var out: [ItemFilter] = [.all] + pinned.map(ItemFilter.list)
        if app.hasKeysList { out.append(.keys) }
        if kinds { out += ItemFilter.kinds }
        return out + lists.filter { !pinned.contains($0) }.map(ItemFilter.list)
    }
    var selectedIndex: Int? { items.firstIndex { $0.id == selectedID } }
    var selected: Item? { selectedIndex.map { items[$0] } }
    var current: Item? {
        guard let selected else { return nil }
        guard let detail, detail.id == selected.id, detail.isSecret == selected.isSecret else { return selected }
        return detail
    }

    func showsCount(_ f: ItemFilter) -> Bool {
        loaded && filter == f && (f != .all || !query.isEmpty || source != nil)
    }

    var countLabel: String { items.count >= Self.pageSize ? "\(Self.pageSize)+" : "\(items.count)" }

    var emptyMessage: String {
        if !query.isEmpty { return "No matches for “\(query)”" + (source.map { " from \($0.title)" } ?? "") }
        if let source { return "Nothing from \(source.title)" + (filter == .all ? "" : " in \(filter.title)") }
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

    /// Shows the items already in memory; reloads only if the core changed since they were loaded.
    func prepareForShow() {
        now = Date()
        resetSearch()
        if isStale { reload(resetSelection: true) } else { selectedID = items.first?.id }
    }

    /// Resets the search while hidden, so the next open finds it loaded.
    func didHide() {
        resetSearch()
        if isStale { reload(resetSelection: true) }
    }

    /// A clip was stored: refresh the visible page, or prewarm the next open.
    func itemStored() {
        reload(resetSelection: !isVisible)
    }

    func clearSearch() {
        query = ""
        source = nil
    }

    private var isStale: Bool { app.core.map { $0.revision.load(ordering: .relaxed) != loadedRevision } ?? false }

    private func resetSearch() {
        palette = nil
        controlHeld = false
        suggestionSelected = false
        let last = Prefs.openOn == .last ? Prefs.lastFilter.flatMap(ItemFilter.init(key:)) : nil
        let target = last.flatMap { filters().contains($0) ? $0 : nil } ?? .all
        clearSearch()
        filter = target
    }

    func showToast(_ text: String, symbol: String = "checkmark") {
        toastGeneration += 1
        let gen = toastGeneration
        toast = text
        toastSymbol = symbol
        Task {
            try? await Task.sleep(for: .seconds(1.2))
            if gen == toastGeneration { toast = nil }
        }
    }

    func reload(resetSelection: Bool = false) {
        guard let core = app.core else { return }
        generation += 1
        let gen = generation
        loadedRevision = core.revision.load(ordering: .relaxed)
        let (q, f, token, hasKeys) = (query, filter, source, app.hasKeysList)
        Task {
            let terms = q.split(whereSeparator: \.isWhitespace).map(String.init)
            var named: AppSuggestion?
            var suggested: AppSuggestion?
            if token == nil {
                for term in terms.prefix(6) {
                    if let name = Self.appName(in: term) {
                        if let hit = try? await core.apps(query: name).first {
                            named = AppSuggestion(app: hit, term: term)
                            break
                        }
                    } else if suggested == nil, term.count >= 2, let hit = try? await core.apps(query: term).first {
                        suggested = AppSuggestion(app: hit, term: term)
                    }
                }
            }
            let text = named.map { n in terms.filter { $0 != n.term }.joined(separator: " ") } ?? q
            let bundle = token?.bundleId ?? named?.app.bundleId
            let found: [Item]?
            do {
                switch f {
                case .all: found = try await core.items(query: text, app: bundle, limit: Self.pageSize)
                case .kind(let kind): found = try await core.items(query: text, kind: kind, app: bundle, limit: Self.pageSize)
                case .keys: found = hasKeys ? try await core.items(query: text, list: Item.keysList, app: bundle, limit: Self.pageSize) : []
                case .list(let name): found = try await core.items(query: text, list: name, app: bundle, limit: Self.pageSize)
                }
            } catch {
                found = nil
            }
            guard gen == generation else { return }
            if found == nil { loadedRevision = -1 }
            items = found ?? []
            loaded = true
            suggestion = named == nil ? suggested : nil
            if suggestion == nil || resetSelection { suggestionSelected = false }
            if resetSelection || !items.contains(where: { $0.id == selectedID }) {
                selectedID = items.first?.id
            }
            // `from:mail ` / `@mail ` becomes the token once a space follows it.
            if let named, named.term != terms.last || q.last?.isWhitespace == true {
                source = named.app
                query = text
            }
        }
    }

    /// The app named by `from:mail` or `@mail`.
    private static func appName(in term: String) -> String? {
        let name = term.lowercased().hasPrefix("from:") ? term.dropFirst(5) : term.hasPrefix("@") ? term.dropFirst() : ""
        return name.isEmpty ? nil : String(name)
    }

    /// Turns the suggested app into the token and drops its term from the query.
    @discardableResult
    func acceptSuggestion() -> Bool {
        guard let s = suggestion else { return false }
        var terms = query.split(whereSeparator: \.isWhitespace).map(String.init)
        terms.removeAll { $0 == s.term }
        suggestion = nil
        suggestionSelected = false
        source = s.app
        query = terms.joined(separator: " ")
        return true
    }

    /// The suggestion row sits above the first item.
    func move(by delta: Int) {
        if suggestionSelected {
            if delta > 0, !items.isEmpty {
                suggestionSelected = false
                if selectedID == nil { selectedID = items.first?.id }
            }
            return
        }
        if suggestion != nil, delta < 0 ? (selectedIndex ?? 0) == 0 : items.isEmpty {
            suggestionSelected = true
            return
        }
        guard !items.isEmpty else { return }
        let i = selectedIndex.map { $0 + delta } ?? 0
        selectedID = items[min(max(i, 0), items.count - 1)].id
    }

    func cycleFilter(by delta: Int) {
        let all = filters()
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
