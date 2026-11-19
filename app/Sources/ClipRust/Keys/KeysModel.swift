import AppKit
import Observation

enum KeyScope: Hashable {
    case all, ids, provider(String), site(String)
}

struct Facet: Identifiable, Hashable {
    let name: String
    let count: Int
    var id: String { name }
}

@MainActor @Observable
final class KeysModel {
    let app: AppState
    var items: [Item] = []
    var scope: KeyScope? = .all
    var search = ""
    var selection: Set<Int64> = []
    var sortOrder = [KeyPathComparator(\Item.lastUsedAt, order: .reverse)]
    var revealed: [Int64: String] = [:]

    init(app: AppState, items: [Item] = []) {
        self.app = app
        self.items = items
    }

    var keyCount: Int { items.filter { !$0.isID }.count }
    var idCount: Int { items.filter(\.isID).count }
    var providers: [Facet] { Self.facets(items.compactMap(\.secretProvider)) }
    var sites: [Facet] { Self.facets(items.compactMap(\.domain)) }

    var rows: [Item] {
        var rows: [Item]
        switch scope ?? .all {
        case .all: rows = items.filter { !$0.isID }
        case .ids: rows = items.filter(\.isID)
        case .provider(let p): rows = items.filter { $0.secretProvider == p }
        case .site(let s): rows = items.filter { $0.domain == s }
        }
        let terms = search.lowercased().split(separator: " ")
        if !terms.isEmpty {
            rows = rows.filter { item in
                let hay = [item.keyName, item.providerLabel, item.siteLabel, item.appLabel, item.noteLabel]
                    .joined(separator: " ").lowercased()
                return terms.allSatisfy { hay.contains($0) }
            }
        }
        return rows.sorted(using: sortOrder)
    }

    var selectedItem: Item? {
        guard selection.count == 1, let id = selection.first else { return nil }
        return items.first { $0.id == id }
    }

    private static func facets(_ names: [String]) -> [Facet] {
        Dictionary(grouping: names, by: { $0 })
            .map { Facet(name: $0.key, count: $0.value.count) }
            .sorted { ($0.count, $1.name) > ($1.count, $0.name) }
    }

    func reload() async {
        guard let core = app.core else { return }
        await app.refreshLists()
        items = (try? await core.secrets(limit: 5000)) ?? []
        selection.formIntersection(Set(items.map(\.id)))
    }

    func toggleReveal(_ item: Item) async {
        if revealed[item.id] != nil {
            revealed[item.id] = nil
            return
        }
        guard let value = await value(of: item, reason: "reveal a saved key") else { return }
        revealed[item.id] = value
        let id = item.id
        Task {
            try? await Task.sleep(for: .seconds(30))
            revealed[id] = nil
        }
    }

    func copy(_ item: Item) async {
        guard let value = await value(of: item, reason: "copy a saved key") else { return }
        Clipboard.copySecret(value)
    }

    func copyMasked(_ item: Item) {
        Clipboard.copy(item.maskedValue)
    }

    private func value(of item: Item, reason: String) async -> String? {
        guard await KeyGate.authorize(reason) else { return nil }
        if let shown = revealed[item.id] { return shown }
        return try? await app.core?.revealKey(item.id)
    }

    func openSource(_ item: Item) {
        if let url = item.sourceUrl.flatMap(URL.init(string:)) { NSWorkspace.shared.open(url) }
    }

    func rename(_ item: Item, to title: String) async {
        let title = title.trimmingCharacters(in: .whitespaces)
        try? await app.core?.setTitle(item.id, title.isEmpty ? nil : title)
        await reload()
    }

    func setNote(_ item: Item, _ note: String) async {
        let note = note.trimmingCharacters(in: .whitespacesAndNewlines)
        guard note != (item.note ?? "") else { return }
        try? await app.core?.setNote(item.id, note.isEmpty ? nil : note)
        await reload()
    }

    func delete(_ ids: Set<Int64>) async {
        for id in ids { try? await app.core?.delete(id) }
        await reload()
    }

    /// Stores a key as plain text again; it leaves this window.
    func unmark(_ item: Item) async {
        if await app.setKey(item.id, false) == nil { NSSound.beep() }
        await reload()
    }

    func removeFromKeys(_ ids: Set<Int64>) async {
        for item in items where ids.contains(item.id) {
            try? await app.core?.removeFromList(item.isID ? Item.idsList : Item.keysList, item.id)
        }
        await reload()
    }
}
