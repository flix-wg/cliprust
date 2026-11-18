import AppKit
import Observation

enum ItemFilter: Hashable {
    case all, kind(ItemKind), keys, contacts, list(String)

    var title: String {
        switch self {
        case .all: "All"
        case .kind(.text): "Text"
        case .kind(.url): "Links"
        case .kind(.image): "Images"
        case .kind(.file): "Files"
        case .keys: "Keys"
        case .contacts: "Contacts"
        case .list(let name): name
        }
    }
}

enum ChooseMode { case standard, plain, copyOnly }

enum PanelAction {
    case choose(ChooseMode)
    case addToList(String), newList, removeFromList(String), togglePin
    case rename, note, revealInFinder, quickLook, delete, openContact(String)
}

/// The note or title being edited inline under the selected row.
struct InlineEdit: Equatable {
    enum Field { case note, title }
    let field: Field
    let id: Int64
    var text: String
}

/// An app named by a search term; accepting it turns the term into the app token.
struct AppSuggestion: Equatable {
    let app: SourceApp
    let term: String
}

/// The contact token: `is:phone`, or a term like "phone" accepted from its suggestion.
enum ContactFilter: String, CaseIterable {
    case phone, email, address, any

    var title: String {
        switch self {
        case .phone: "Phone numbers"
        case .email: "Emails"
        case .address: "Addresses"
        case .any: "Contacts"
        }
    }

    var symbol: String { self == .any ? "person.crop.circle" : Item.contactSymbol(rawValue) }

    /// Words a search term of three or more letters may start.
    var words: [String] {
        switch self {
        case .phone: ["phones", "numbers"]
        case .email: ["emails", "e-mails"]
        case .address: ["addresses"]
        case .any: ["contacts"]
        }
    }

    static func named(_ term: String) -> ContactFilter? {
        let t = term.lowercased()
        return t.count < 3 ? nil : allCases.first { $0.words.contains { $0.hasPrefix(t) } }
    }

    /// The first of the leading terms naming a type, with the terms it used ("mail address" is email).
    static func named(in terms: [String]) -> (ContactFilter, [String])? {
        for (i, term) in terms.prefix(6).enumerated() {
            if term.lowercased() == "mail", i + 1 < terms.count, named(terms[i + 1]) == .address {
                return (.email, [term, terms[i + 1]])
            }
            if let type = named(term) { return (type, [term]) }
        }
        return nil
    }
}

/// The row above the results; ⇥, ⏎ or a click turns it into a token.
enum Suggestion: Equatable {
    case app(AppSuggestion)
    case contact(ContactFilter, count: Int, terms: [String])

    var terms: [String] {
        switch self {
        case .app(let s): [s.term]
        case .contact(_, _, let terms): terms
        }
    }
}

@MainActor @Observable
final class PanelModel {
    static let pageSize = 200
    static let countCap = 1000

    let app: AppState
    var query = "" { didSet { if query != oldValue { reload(resetSelection: true) } } }
    /// The app token in the search field: only items copied from that app.
    var source: SourceApp? { didSet { if source?.bundleId != oldValue?.bundleId { reload(resetSelection: true) } } }
    /// The contact token: only phone numbers, emails, addresses or any of them.
    var contactToken: ContactFilter? { didSet { if contactToken != oldValue { reload(resetSelection: true) } } }
    var suggestion: Suggestion?
    /// Items per contact type, counted once per open up to `countCap`.
    var contactCounts: [ContactFilter: Int]?
    var suggestionSelected = false
    var filter: ItemFilter = .all {
        didSet {
            guard filter != oldValue else { return }
            if isVisible { Prefs.lastFilter = filter.key }
            reload(resetSelection: true)
        }
    }
    var items: [Item] = []
    var selectedID: Int64? {
        didSet {
            guard selectedID != oldValue else { return }
            if editing?.id != selectedID { editing = nil }
            loadDetail()
        }
    }
    var editing: InlineEdit?
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

    /// Bar order: All, Pinned, the other pinned lists, Keys, Contacts, Images, Files, Links, the other lists, Text last.
    /// Pinned shows only while it holds items.
    func filters(kinds: Bool = Prefs.showTypeFilters) -> [ItemFilter] {
        let lists = app.orderedLists.map(\.name)
            .filter { $0 != Item.pinnedList || !app.pinnedIDs.isEmpty || filter == .list($0) }
        let pinned = lists.filter { $0 == Item.pinnedList } + lists.filter { $0 != Item.pinnedList && app.pinnedLists.contains($0) }
        var out: [ItemFilter] = [.all] + pinned.map(ItemFilter.list)
        if app.settings.detectKeys || app.hasKeysList { out.append(.keys) }
        if app.hasContacts || filter == .contacts { out.append(.contacts) }
        if kinds { out += [.kind(.image), .kind(.file), .kind(.url)] }
        out += lists.filter { !pinned.contains($0) }.map(ItemFilter.list)
        return kinds ? out + [.kind(.text)] : out
    }
    var selectedIndex: Int? { items.firstIndex { $0.id == selectedID } }
    var selected: Item? { selectedIndex.map { items[$0] } }
    var current: Item? {
        guard let selected else { return nil }
        guard let detail, detail.id == selected.id, detail.isSecret == selected.isSecret else { return selected }
        return detail
    }

    func showsCount(_ f: ItemFilter) -> Bool {
        loaded && filter == f && (f != .all || hasSearch)
    }

    var countLabel: String { items.count >= Self.pageSize ? "\(Self.pageSize)+" : "\(items.count)" }

    var hasSearch: Bool { !query.isEmpty || source != nil || contactToken != nil }

    var searchPlaceholder: String {
        let scope = [contactToken?.title, source?.title].compactMap { $0 }
        return scope.isEmpty ? "Search clipboard" : "Search \(scope.joined(separator: " from "))"
    }

    var emptyMessage: String {
        if !query.isEmpty { return "No matches for “\(query)”" + (source.map { " from \($0.title)" } ?? "") }
        if let contactToken {
            return "No \(contactToken.title.lowercased())" + (source.map { " from \($0.title)" } ?? "")
        }
        if let source { return "Nothing from \(source.title)" + (filter == .all ? "" : " in \(filter.title)") }
        switch filter {
        case .all: return "Nothing copied yet"
        case .keys: return "No keys yet"
        case .contacts: return "No phone numbers, emails or addresses yet"
        case .kind: return "No \(filter.title.lowercased()) yet"
        case .list(let name): return "\(name) is empty"
        }
    }

    var currentManualList: String? {
        if case .list(let name) = filter, app.manualLists.contains(where: { $0.name == name }) { return name }
        return nil
    }

    /// Shows the items already in memory at once, then always reloads them: other
    /// processes and retention change the store without bumping our revision.
    func prepareForShow() {
        now = Date()
        contactCounts = nil
        resetSearch()
        selectedID = items.first?.id
        reload()
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
        contactToken = nil
    }

    /// ⌫ at the start of the field drops the last token.
    func removeLastToken() {
        if contactToken != nil { contactToken = nil } else { source = nil }
    }

    func loadContactCounts() async -> [ContactFilter: Int] {
        if let contactCounts { return contactCounts }
        var out: [ContactFilter: Int] = [:]
        if let core = app.core, app.hasContacts {
            for type in ContactFilter.allCases {
                out[type] = (try? await core.items(contact: type.rawValue, limit: Self.countCap))?.count ?? 0
            }
        }
        contactCounts = out
        return out
    }

    static func countText(_ n: Int) -> String { n >= countCap ? "\(countCap)+" : "\(n)" }

    private var isStale: Bool { app.core.map { $0.revision.load(ordering: .relaxed) != loadedRevision } ?? false }

    private func resetSearch() {
        palette = nil
        editing = nil
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
        let (q, f, token, tokenType, hasKeys) = (query, filter, source, contactToken, app.hasKeysList)
        Task {
            let words = q.split(whereSeparator: \.isWhitespace).map(String.init)
            let terms = words.filter { Self.contactType(in: $0) == nil }
            let typed = words.last { Self.contactType(in: $0) != nil }
            let contact = typed.flatMap(Self.contactType) ?? tokenType?.rawValue ?? (f == .contacts ? "any" : nil)
            var named: AppSuggestion?
            var suggested: Suggestion?
            if token == nil { named = await Self.namedApp(in: terms, core: core) }
            if tokenType == nil, typed == nil, let hit = ContactFilter.named(in: terms),
               let n = await loadContactCounts()[hit.0], n > 0 {
                suggested = .contact(hit.0, count: n, terms: hit.1)
            }
            if token == nil, named == nil, suggested == nil {
                for term in terms.prefix(6) where Self.appName(in: term) == nil && term.count >= 2 {
                    if let hit = try? await core.apps(query: term).first {
                        suggested = .app(AppSuggestion(app: hit, term: term))
                        break
                    }
                }
            }
            let text = named == nil && terms.count == words.count ? q
                : (named.map { n in terms.filter { $0 != n.term } } ?? terms).joined(separator: " ")
            let bundle = token?.bundleId ?? named?.app.bundleId
            let found: [Item]?
            do {
                switch f {
                case .all, .contacts: found = try await core.items(query: text, app: bundle, contact: contact, limit: Self.pageSize)
                case .kind(let kind):
                    found = try await core.items(query: text, kind: kind, app: bundle, contact: contact, limit: Self.pageSize)
                case .keys:
                    found = hasKeys ? try await core.items(query: text, list: Item.keysList, app: bundle, contact: contact,
                                                           limit: Self.pageSize) : []
                case .list(let name):
                    found = try await core.items(query: text, list: name, app: bundle, contact: contact, limit: Self.pageSize)
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
            // `from:mail ` / `@mail ` / `is:phone ` becomes a token once a space follows it.
            let settled = { (term: String?) in term.map { $0 != words.last || q.last?.isWhitespace == true } ?? false }
            let namedApp = settled(named?.term) ? named?.app : nil
            let type = settled(typed) ? typed.flatMap(Self.contactType).flatMap(ContactFilter.init) : nil
            if namedApp != nil || type != nil {
                if let namedApp { source = namedApp }
                if let type { contactToken = type }
                query = words.filter { !(namedApp != nil && $0 == named?.term) && !(type != nil && Self.contactType(in: $0) != nil) }
                    .joined(separator: " ")
            }
        }
    }

    /// The first of the leading terms that names a stored app as `from:mail` or `@mail`.
    static func namedApp(in terms: [String], core: Core) async -> AppSuggestion? {
        for term in terms.prefix(6) {
            if let name = appName(in: term), let hit = try? await core.apps(query: name).first {
                return AppSuggestion(app: hit, term: term)
            }
        }
        return nil
    }

    /// `is:phone`, `is:email`, `is:address`, or `is:contact` for any of them.
    static func contactType(in term: String) -> String? {
        switch term.lowercased() {
        case "is:phone": "phone"
        case "is:email": "email"
        case "is:address": "address"
        case "is:contact": "any"
        default: nil
        }
    }

    /// The app named by `from:mail` or `@mail`.
    private static func appName(in term: String) -> String? {
        let name = term.lowercased().hasPrefix("from:") ? term.dropFirst(5) : term.hasPrefix("@") ? term.dropFirst() : ""
        return name.isEmpty ? nil : String(name)
    }

    /// Turns the suggestion into its token and drops its terms from the query.
    @discardableResult
    func acceptSuggestion() -> Bool {
        guard let s = suggestion else { return false }
        var terms = query.split(whereSeparator: \.isWhitespace).map(String.init)
        terms.removeAll { s.terms.contains($0) }
        suggestion = nil
        suggestionSelected = false
        switch s {
        case .app(let a): source = a.app
        case .contact(let type, _, _): contactToken = type
        }
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

    /// Opens the inline field, prefilled from a fresh copy so edits made elsewhere aren't lost.
    func beginEdit(_ field: InlineEdit.Field, _ item: Item) {
        palette = nil
        suggestionSelected = false
        selectedID = item.id
        Task {
            guard let full = try? await app.core?.item(item.id), selectedID == item.id else { return NSSound.beep() }
            detail = full
            editing = InlineEdit(field: field, id: item.id, text: (field == .note ? full.note : full.title) ?? "")
        }
    }

    /// Saves the inline field; an empty one removes the note or title.
    func commitEdit() {
        guard let edit = editing, let core = app.core else { return }
        editing = nil
        let text = edit.text.trimmingCharacters(in: .whitespacesAndNewlines)
        let value = text.isEmpty ? nil : text
        func apply(_ item: inout Item) {
            if edit.field == .note { item.note = value } else { item.title = value }
        }
        if let i = items.firstIndex(where: { $0.id == edit.id }) { apply(&items[i]) }
        if detail?.id == edit.id { apply(&detail!) }
        Task {
            do {
                if edit.field == .note { try await core.setNote(edit.id, value) } else { try await core.setTitle(edit.id, value) }
            } catch {
                NSSound.beep()
            }
            reload()
        }
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
