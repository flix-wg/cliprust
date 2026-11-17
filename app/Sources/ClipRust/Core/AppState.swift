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

    init(core: Core?) {
        self.core = core
    }

    var userLists: [ClipList] { lists.filter { $0.name != Item.keysList } }
    var manualLists: [ClipList] { userLists.filter(\.manual) }
    var hasKeysList: Bool { lists.contains { $0.name == Item.keysList } }

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

    func addToList(_ name: String, _ id: Int64) async {
        try? await core?.addToList(name, id)
        await refreshPins()
    }

    func removeFromList(_ name: String, _ id: Int64) async {
        try? await core?.removeFromList(name, id)
        await refreshPins()
    }

    func createList(_ name: String) async -> Bool {
        do {
            try await core?.createList(name)
            await refreshLists()
            return true
        } catch {
            lastError = error.localizedDescription
            return false
        }
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
