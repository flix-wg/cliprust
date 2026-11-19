import AppKit
import Observation

@MainActor @Observable
final class AppState {
    let core: Core?
    var settings = CoreSettings()
    var lists: [ClipList] = []
    var recent: [Item] = []
    var apps: [SourceApp] = []
    var pinnedIDs: Set<Int64> = []
    var hasContacts = false
    var excludes: [String] = []
    var stats: Stats?
    var encryption: EncryptionStatus?
    /// Why the last key copied was not saved; shown in the panel until dismissed.
    var skippedKey: String?
    /// Why marking an item as a key, or not a key, failed; shown in the panel until dismissed.
    var keyError: String?
    var pinnedLists = Prefs.pinnedLists { didSet { Prefs.pinnedLists = pinnedLists } }
    var listOrder = Prefs.listOrder { didSet { Prefs.listOrder = listOrder } }

    init(core: Core?) {
        self.core = core
    }

    var userLists: [ClipList] { lists.filter { $0.name != Item.keysList } }
    var manualLists: [ClipList] { userLists.filter(\.manual) }
    var hasKeysList: Bool { lists.contains { $0.name == Item.keysList } }

    /// User lists in the order set in Settings; Pinned comes first and other lists not placed yet last.
    var orderedLists: [ClipList] {
        let rank = Dictionary(listOrder.enumerated().map { ($1, $0) }, uniquingKeysWith: { a, _ in a })
        let key = { (name: String) in rank[name] ?? (name == Item.pinnedList ? -1 : .max) }
        return userLists.enumerated()
            .sorted { (key($0.element.name), $0.offset) < (key($1.element.name), $1.offset) }
            .map(\.element)
    }

    func refresh() async {
        guard let core else { return }
        settings = (try? await core.settings()) ?? settings
        lists = (try? await core.lists()) ?? lists
        excludes = (try? await core.excludes()) ?? excludes
        await refreshEncryption()
        await refreshRecent()
        await refreshPins()
        await refreshApps()
        await refreshContacts()
    }

    /// The Contacts chip shows once any item holds a phone number, email or address.
    func refreshContacts() async {
        guard let core, let found = try? await core.items(contact: "any", limit: 1) else { return }
        hasContacts = !found.isEmpty
    }

    func refreshEncryption() async {
        guard let core else { return }
        encryption = (try? await core.encryptionStatus()) ?? encryption
    }

    /// Deletes the keys no master key reads, after which new keys are saved again.
    func forgetUnreadableKeys() async throws -> Int {
        guard let core else { return 0 }
        let n = try await core.forgetUnreadableKeys()
        skippedKey = nil
        await refreshEncryption()
        await refreshRecent()
        return n
    }

    /// Marks an item as a key, or not a key; returns the id that remains, or nil after showing why not.
    func setKey(_ id: Int64, _ on: Bool) async -> Int64? {
        guard let core else { return nil }
        do {
            let kept = on ? try await core.markKey(id) : try await core.unmarkKey(id)
            keyError = nil
            await refreshRecent()
            if on, !hasKeysList { await refreshLists() }
            return kept
        } catch let error as CoreError where error.code == "locked" {
            await refreshEncryption()
            if encryption?.isLocked != true { keyError = error.message }
            return nil
        } catch {
            keyError = error.localizedDescription
            return nil
        }
    }

    func refreshApps() async {
        guard let core else { return }
        apps = (try? await core.apps()) ?? apps
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

    /// Items in the Pinned list show a pin.
    func refreshPins() async {
        guard let core else { return }
        if let ids = try? await core.listIDs(Item.pinnedList) { pinnedIDs = Set(ids) }
    }

    /// Pins or unpins an item; returns whether it is pinned now. The Pinned list
    /// is created on first use and pinned to the front of the filter bar.
    func togglePinned(_ id: Int64) async -> Bool {
        if pinnedIDs.contains(id) {
            await removeFromList(Item.pinnedList, id)
            return false
        }
        if !lists.contains(where: { $0.name == Item.pinnedList }), !pinnedLists.contains(Item.pinnedList) {
            pinnedLists.insert(Item.pinnedList, at: 0)
        }
        return await addToList(Item.pinnedList, id)
    }

    func itemStored(secret: Bool) {
        Task {
            await refreshRecent()
            if secret, !hasKeysList { await refreshLists() }
            if !secret, !hasContacts { await refreshContacts() }
        }
    }

    func update(_ change: (inout CoreSettings) -> Void) {
        var next = settings
        change(&next)
        settings = next
        guard let core else { return }
        Task {
            if let saved = try? await core.saveSettings(next) { settings = saved }
        }
    }

    /// Creates a manual list named `name` if there is none; false if the item was not added.
    @discardableResult
    func addToList(_ name: String, _ id: Int64) async -> Bool {
        var added = true
        do { try await core?.addToList(name, id) } catch { added = false }
        if lists.contains(where: { $0.name == name }) { await refreshPins() } else { await refreshLists() }
        return added
    }

    /// Pinning owns the name "Pinned"; no other list may take it.
    static func isReserved(_ name: String) -> Bool { name.caseInsensitiveCompare(Item.pinnedList) == .orderedSame }

    func removeFromList(_ name: String, _ id: Int64) async {
        try? await core?.removeFromList(name, id)
        await refreshPins()
    }

    func createList(_ name: String, terms: [String] = [], kind: ItemKind? = nil, app: String? = nil,
                    contact: String? = nil, site: String? = nil) async -> Bool {
        guard !Self.isReserved(name) else { return false }
        do {
            try await core?.createList(name, terms: terms, kind: kind, app: app, contact: contact, site: site)
            await refreshLists()
            return true
        } catch {
            return false
        }
    }

    func togglePin(_ name: String) {
        if pinnedLists.contains(name) { pinnedLists.removeAll { $0 == name } } else { pinnedLists.append(name) }
    }

    func renameList(_ name: String, to newName: String) async -> Bool {
        guard !Self.isReserved(newName) else { return false }
        do {
            try await core?.renameList(name, newName)
        } catch {
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
