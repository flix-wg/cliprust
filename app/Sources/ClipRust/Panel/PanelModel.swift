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
    var query = "" { didSet { if query != oldValue { reload(resetSelection: true, debounce: true) } } }
    /// The app token in the search field: only items copied from that app.
    var source: SourceApp? { didSet { if source?.bundleId != oldValue?.bundleId { reload(resetSelection: true) } } }
    /// The contact token: only phone numbers, emails, addresses or any of them.
    var contactToken: ContactFilter? { didSet { if contactToken != oldValue { reload(resetSelection: true) } } }
    var suggestion: Suggestion?
    /// Items per contact type, counted once per open.
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
    /// The core's data version the items were loaded at; nil after a failed load.
    @ObservationIgnored private var loadedVersion: [Int]?
    /// The search and filter the items were loaded for.
    @ObservationIgnored private var loadedSearch: SearchState?
    /// False while a reload is pending, so the items may not match the search field yet.
    @ObservationIgnored private var settled = true
    /// A pending reload resets the selection, also when a newer one replaces it.
    @ObservationIgnored private var pendingReset = false
    @ObservationIgnored private var whenSettled: (() -> Void)?

    private struct SearchState: Equatable {
        let query: String, source: String?, contact: ContactFilter?, filter: ItemFilter
    }

    private var searchState: SearchState {
        SearchState(query: query, source: source?.bundleId, contact: contactToken, filter: filter)
    }

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

    /// Resets the search before the panel is drawn. The page in memory is kept unless the store changed
    /// since (in any process), the filter changed, or it is a smart list, whose rules may depend on the
    /// time; then it is reloaded on the spot (a millisecond or two) so the first frame is never stale.
    /// The main thread waits for the core 30 ms at most: if a capture or a long call holds it, the
    /// panel shows the page it has and loads the current one right after.
    func prepareForShow() {
        now = Date()
        contactCounts = nil
        resetSearch()
        selectedID = items.first?.id
        guard let core = app.core else { return }
        var smart = false
        if case .list = filter { smart = currentManualList == nil }
        let deadline = DispatchTime.now() + .milliseconds(30)
        guard let versionResult = core.dataVersionSoon(by: deadline) else { return loadAfterShowing() }
        let version = try? versionResult.get()
        guard smart || loadedSearch != searchState || version == nil || version != loadedVersion else { return }
        var found: [Item]? = []
        if let scope = Self.scope(filter, hasKeys: app.hasKeysList) {
            guard let result = core.itemsSoon(kind: scope.kind, list: scope.list, contact: scope.contact,
                                              limit: Self.pageSize, by: deadline) else { return loadAfterShowing() }
            found = try? result.get()
        }
        generation += 1 // drops the reloads the reset queued
        (settled, pendingReset, whenSettled) = (true, false, nil)
        loadedVersion = found == nil ? nil : version
        loadedSearch = searchState
        items = found ?? []
        loaded = true
        suggestion = nil
        selectedID = items.first?.id
    }

    /// The core was busy: show the page in memory now and replace it once the core answers.
    private func loadAfterShowing() {
        loadedVersion = nil
        reload(resetSelection: true)
    }

    /// Resets the search while hidden and lets the WAL drain.
    func didHide() {
        whenSettled = nil
        resetSearch()
        if let core = app.core { Task { await core.checkpoint() } }
    }

    /// A clip was stored: refresh the visible page; a hidden one waits for the next open.
    func itemStored() {
        if isVisible { reload() }
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
        if let core = app.core, app.hasContacts, let counts = try? await core.contactCounts() {
            for (name, n) in counts { if let type = ContactFilter(rawValue: name) { out[type] = n } }
        }
        contactCounts = out
        return out
    }

    static func countText(_ n: Int) -> String { n >= countCap ? "\(countCap)+" : "\(n)" }

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

    /// What a filter chip narrows to; nil for Keys while there are none.
    private static func scope(_ f: ItemFilter, hasKeys: Bool) -> (kind: ItemKind?, list: String?, contact: String?)? {
        switch f {
        case .all: (nil, nil, nil)
        case .contacts: (nil, nil, "any")
        case .kind(let kind): (kind, nil, nil)
        case .keys: hasKeys ? (nil, Item.keysList, nil) : nil
        case .list(let name): (nil, name, nil)
        }
    }

    /// Typing reloads after a short pause; a newer reload abandons this one at its next step.
    /// The core reads the query (`from:`, `is:` and plain words); the parse it returns here only
    /// drives the suggestion row and turns settled `from:` and `is:` words into tokens.
    func reload(resetSelection: Bool = false, debounce: Bool = false) {
        guard let core = app.core else { return }
        generation += 1
        let gen = generation
        let resetSelection = resetSelection || pendingReset
        (settled, pendingReset) = (false, resetSelection)
        let (q, f, token, tokenType, hasKeys) = (query, filter, source, contactToken, app.hasKeysList)
        let search = searchState
        Task {
            if debounce { try? await Task.sleep(for: .milliseconds(60)) }
            guard gen == generation else { return }
            let version = try? await core.dataVersion()
            let parsed = try? await core.parseQuery(q, app: token?.bundleId)
            let words = q.split(whereSeparator: \.isWhitespace).map(String.init)
            let terms = parsed.map { $0.text.split(separator: " ").map(String.init) } ?? words
            let named = parsed.flatMap { p in p.app.flatMap { app in p.appTerm.map { AppSuggestion(app: app, term: $0) } } }
            let typed = parsed?.contactTerms.last
            var suggested: Suggestion?
            guard gen == generation else { return }
            if tokenType == nil, typed == nil, let hit = ContactFilter.named(in: terms),
               let n = await loadContactCounts()[hit.0], n > 0 {
                suggested = .contact(hit.0, count: n, terms: hit.1)
            }
            if token == nil, named == nil, suggested == nil {
                for term in terms.prefix(6) where term.count >= 2 {
                    guard gen == generation else { return }
                    if let hit = try? await core.apps(query: term).first {
                        suggested = .app(AppSuggestion(app: hit, term: term))
                        break
                    }
                }
            }
            guard gen == generation else { return }
            var found: [Item]? = []
            if let scope = Self.scope(f, hasKeys: hasKeys) {
                found = try? await core.items(query: q, kind: scope.kind, list: scope.list, app: token?.bundleId,
                                              contact: tokenType?.rawValue ?? scope.contact, limit: Self.pageSize)
            }
            guard gen == generation else { return }
            (settled, pendingReset) = (true, false)
            let action = whenSettled
            whenSettled = nil
            defer { action?() }
            loadedVersion = found == nil ? nil : version
            loadedSearch = search
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
            let type = settled(typed) ? parsed?.contact.flatMap(ContactFilter.init) : nil
            if namedApp != nil || type != nil {
                let contactTerms = parsed?.contactTerms ?? []
                if let namedApp { source = namedApp }
                if let type { contactToken = type }
                query = words.filter { !(namedApp != nil && $0 == named?.term) && !(type != nil && contactTerms.contains($0)) }
                    .joined(separator: " ")
            }
        }
    }

    /// Runs `action` once the items match the search field: now, or when the pending reload (started at
    /// once, skipping its typing pause) lands; so ⏎ never picks a row of the previous search.
    func whenCurrent(_ action: @escaping () -> Void) {
        if settled { return action() }
        whenSettled = action
        reload()
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
        let before = detail
        if detail?.id == edit.id { apply(&detail!) }
        Task {
            do {
                if edit.field == .note { try await core.setNote(edit.id, value) } else { try await core.setTitle(edit.id, value) }
                // The core may have masked a key in it.
                if detail?.id == edit.id, let fresh = try? await core.item(edit.id) { detail = fresh }
            } catch {
                NSSound.beep()
                if detail?.id == edit.id, before?.id == edit.id { detail = before }
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
