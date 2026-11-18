import AppKit
import Observation

@MainActor @Observable
final class AppState {
    let core: Core?
    var settings = CoreSettings()
    var lists: [ClipList] = []
    var recent: [Item] = []
    var pinnedIDs: Set<Int64> = []
    var excludes: [String] = []
    var stats: Stats?
    var lastError: String?
    var pinnedLists = Prefs.pinnedLists { didSet { Prefs.pinnedLists = pinnedLists } }
    var listOrder = Prefs.listOrder { didSet { Prefs.listOrder = listOrder } }

    init(core: Core?) {
        self.core = core
    }

    var userLists: [ClipList] { lists.filter { $0.name != Item.keysList } }
    var manualLists: [ClipList] { userLists.filter(\.manual) }
    var hasKeysList: Bool { lists.contains { $0.name == Item.keysList } }

    /// User lists in the order set in Settings; lists not placed yet come last.
    var orderedLists: [ClipList] {
        let rank = Dictionary(listOrder.enumerated().map { ($1, $0) }, uniquingKeysWith: { a, _ in a })
        return userLists.enumerated()
            .sorted { (rank[$0.element.name] ?? .max, $0.offset) < (rank[$1.element.name] ?? .max, $1.offset) }
            .map(\.element)
    }

    func refresh() async {
        guard let core else { return }
        do {
            settings = try await core.settings()
            lists = try await core.lists()
            excludes = try await core.excludes()
        } catch {
            lastError = error.localizedDescription
        }
        await refreshRecent()
        await refreshPins()
    }

    func refreshRecent() async {
        guard let core else { return }
        recent = (try? await core.items(limit: 5)) ?? recent
    }

    func refreshLists() async {
        guard let core else { return }
        lists = (try? await core.lists()) ?? lists
        await refreshPins()
    }

    /// Items in a manual list show a pin; lists are small, so this stays cheap.
    func refreshPins() async {
        guard let core else { return }
        var ids = Set<Int64>()
        for list in manualLists {
            if let items = try? await core.items(list: list.name, limit: 5000) {
                ids.formUnion(items.map(\.id))
            }
        }
        pinnedIDs = ids
    }

    func itemStored(secret: Bool) {
        Task {
            await refreshRecent()
            if secret, !hasKeysList { await refreshLists() }
        }
    }

    func update(_ change: (inout CoreSettings) -> Void) {
        var next = settings
        change(&next)
        settings = next
        guard let core else { return }
        Task {
            do { settings = try await core.saveSettings(next) } catch { lastError = error.localizedDescription }
        }
    }

    /// Creates a manual list named `name` if there is none.
    func addToList(_ name: String, _ id: Int64) async {
        try? await core?.addToList(name, id)
        if lists.contains(where: { $0.name == name }) { await refreshPins() } else { await refreshLists() }
    }

    func removeFromList(_ name: String, _ id: Int64) async {
        try? await core?.removeFromList(name, id)
        await refreshPins()
    }

    func createList(_ name: String, contains text: String? = nil, kind: ItemKind? = nil) async -> Bool {
        do {
            try await core?.createList(name, contains: text, kind: kind)
            await refreshLists()
            return true
        } catch {
            lastError = error.localizedDescription
            return false
        }
    }

    func togglePin(_ name: String) {
        if pinnedLists.contains(name) { pinnedLists.removeAll { $0 == name } } else { pinnedLists.append(name) }
    }

    func renameList(_ name: String, to newName: String) async -> Bool {
        do {
            try await core?.renameList(name, newName)
        } catch {
            lastError = error.localizedDescription
            return false
        }
        pinnedLists = pinnedLists.map { $0 == name ? newName : $0 }
        listOrder = listOrder.map { $0 == name ? newName : $0 }
        if Prefs.lastFilter == ItemFilter.list(name).key { Prefs.lastFilter = ItemFilter.list(newName).key }
        await refreshLists()
        return true
    }

    func deleteList(_ name: String) async {
        try? await core?.deleteList(name)
        pinnedLists.removeAll { $0 == name }
        listOrder.removeAll { $0 == name }
        await refreshLists()
    }

    func addExclude(_ bundleID: String) async {
        try? await core?.addExclude(bundleID)
        excludes = (try? await core?.excludes()) ?? excludes
    }

    func removeExclude(_ bundleID: String) async {
        try? await core?.removeExclude(bundleID)
        excludes = (try? await core?.excludes()) ?? excludes
    }

    func loadStats() async {
        guard let core else { return }
        stats = try? await core.stats()
    }

    func clearHistory() async -> Int? {
        guard let core else { return nil }
        let removed = try? await core.clear()
        await refreshRecent()
        await loadStats()
        return removed
    }
}
