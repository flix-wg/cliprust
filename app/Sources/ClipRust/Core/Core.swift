import CCliprust
import Foundation
import Synchronization

struct CoreError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

/// The Rust core. Every FFI call runs on one serial background queue.
final class Core: @unchecked Sendable {
    private let handle: OpaquePointer
    private let queue: DispatchQueue
    private var onEvent: (@MainActor (CoreEvent) -> Void)?
    /// Bumped by every call that may change items and by every stored clip.
    let revision = Atomic<Int>(0)
    private static let reads: Set<String> = ["items", "item", "apps", "secrets", "secret_facets", "lists", "list_ids",
                                             "settings", "excludes", "stats", "reveal_key"]

    static let decoder: JSONDecoder = {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return d
    }()

    private init(handle: OpaquePointer, queue: DispatchQueue) {
        self.handle = handle
        self.queue = queue
    }

    static func open(path: String?) async throws -> Core {
        let queue = DispatchQueue(label: "app.cliprust.core", qos: .userInitiated)
        return try await withCheckedThrowingContinuation { cont in
            queue.async {
                var err: UnsafeMutablePointer<CChar>?
                let h = path.map { p in p.withCString { cr_open($0, &err) } } ?? cr_open(nil, &err)
                if let h {
                    cont.resume(returning: Core(handle: h, queue: queue))
                } else {
                    let message = err.map { String(cString: $0) } ?? "Could not open the database."
                    if let err { cr_free_string(err) }
                    cont.resume(throwing: CoreError(message: message))
                }
            }
        }
    }

    private func send(_ method: String, _ params: [String: Any]) throws -> Data {
        let request = try JSONSerialization.data(withJSONObject: ["method": method, "params": params])
        guard let out = String(decoding: request, as: UTF8.self).withCString({ cr_call(handle, $0) }) else {
            throw CoreError(message: "The core returned no response.")
        }
        defer { cr_free_string(out) }
        let envelope = try JSONSerialization.jsonObject(with: Data(bytes: out, count: strlen(out)))
        guard let dict = envelope as? [String: Any] else { throw CoreError(message: "Malformed response.") }
        if let err = dict["err"] as? String { throw CoreError(message: err) }
        if !Self.reads.contains(method) { revision.wrappingAdd(1, ordering: .relaxed) }
        return try JSONSerialization.data(withJSONObject: dict["ok"] ?? NSNull(), options: .fragmentsAllowed)
    }

    func call<T: Decodable>(_ method: String, _ params: [String: Any] = [:], as type: T.Type = T.self) async throws -> T {
        try await withCheckedThrowingContinuation { cont in
            queue.async {
                cont.resume(with: Result { try Self.decoder.decode(T.self, from: self.send(method, params)) })
            }
        }
    }

    func run(_ method: String, _ params: [String: Any] = [:]) async throws {
        _ = try await call(method, params, as: Ignored.self)
    }

    func data(id: Int64, format: String) async -> Data? {
        await withCheckedContinuation { cont in
            queue.async {
                var ptr: UnsafeMutablePointer<UInt8>?
                var len = 0
                let rc = format.withCString { cr_item_data(self.handle, id, $0, &ptr, &len) }
                guard rc == 0, let ptr else { return cont.resume(returning: nil) }
                let data = Data(bytes: ptr, count: len)
                cr_free_bytes(ptr, len)
                cont.resume(returning: data)
            }
        }
    }

    /// Starts the clipboard watcher; `handler` runs on the main actor.
    func watch(_ handler: @escaping @MainActor (CoreEvent) -> Void) {
        onEvent = handler
        let ctx = Unmanaged.passUnretained(self).toOpaque()
        cr_watch(handle, { json, ctx in
            guard let json, let ctx else { return }
            let core = Unmanaged<Core>.fromOpaque(ctx).takeUnretainedValue()
            let data = Data(bytes: json, count: strlen(json))
            guard let event = try? Core.decoder.decode(CoreEvent.self, from: data) else { return }
            if event.type == "stored" { core.revision.wrappingAdd(1, ordering: .relaxed) }
            DispatchQueue.main.async { MainActor.assumeIsolated { core.onEvent?(event) } }
        }, ctx)
    }

    private struct Ignored: Decodable {
        init(from decoder: Decoder) throws {}
    }
}

extension Core {
    func items(query: String? = nil, kind: ItemKind? = nil, list: String? = nil, app: String? = nil, limit: Int) async throws -> [Item] {
        var p: [String: Any] = ["limit": limit]
        if let query, !query.isEmpty { p["query"] = query }
        if let kind { p["kind"] = kind.rawValue }
        if let list { p["list"] = list }
        if let app { p["app"] = app }
        return try await call("items", p)
    }

    /// Apps of stored items, most copied first; a query keeps the matching ones, best first.
    func apps(query: String? = nil) async throws -> [SourceApp] {
        try await call("apps", query.map { ["query": $0] } ?? [:])
    }
    func listIDs(_ name: String) async throws -> [Int64] { try await call("list_ids", ["name": name]) }

    /// Keys and IDs together; the Keys window facets them itself.
    func secrets(limit: Int) async throws -> [Item] { try await call("secrets", ["limit": limit]) }
    func item(_ id: Int64) async throws -> Item { try await call("item", ["id": id]) }
    func pastePrep(_ id: Int64, plain: Bool) async throws { try await run("paste_prep", ["id": id, "plain": plain]) }
    func revealKey(_ id: Int64) async throws -> String? { try await call("reveal_key", ["id": id]) }
    func delete(_ id: Int64) async throws { try await run("delete", ["id": id]) }
    func setTitle(_ id: Int64, _ title: String?) async throws {
        try await run("set_title", ["id": id, "title": title.map { $0 as Any } ?? NSNull()])
    }
    func setNote(_ id: Int64, _ note: String?) async throws {
        try await run("set_note", ["id": id, "note": note.map { $0 as Any } ?? NSNull()])
    }
    func lists() async throws -> [ClipList] { try await call("lists") }
    /// With `contains` or `app`, a smart list matching them (and `kind`, if given).
    func createList(_ name: String, contains text: String? = nil, kind: ItemKind? = nil, app: String? = nil) async throws {
        var p: [String: Any] = ["name": name]
        if text != nil || app != nil {
            var rules: [[String: Any]] = []
            if let text { rules.append(["type": "contains", "text": text]) }
            if let kind { rules.append(["type": "kind", "kind": kind.rawValue]) }
            if let app { rules.append(["type": "app", "bundle_id": app]) }
            p["rule"] = ["type": "all", "rules": rules]
        }
        try await run("create_list", p)
    }
    func renameList(_ name: String, _ newName: String) async throws { try await run("rename_list", ["name": name, "new_name": newName]) }
    func deleteList(_ name: String) async throws { try await run("delete_list", ["name": name]) }
    func addToList(_ name: String, _ id: Int64) async throws { try await run("add_to_list", ["name": name, "id": id]) }
    func removeFromList(_ name: String, _ id: Int64) async throws { try await run("remove_from_list", ["name": name, "id": id]) }
    func settings() async throws -> CoreSettings { try await call("settings") }
    func saveSettings(_ s: CoreSettings) async throws -> CoreSettings { try await call("save_settings", s.json) }
    func excludes() async throws -> [String] { try await call("excludes") }
    func addExclude(_ bundleID: String) async throws { try await run("add_exclude", ["bundle_id": bundleID]) }
    func removeExclude(_ bundleID: String) async throws { try await run("remove_exclude", ["bundle_id": bundleID]) }
    func stats() async throws -> Stats { try await call("stats") }
    func clear() async throws -> Int { try await call("clear") }
    func importPasteNow() async throws -> ImportResult { try await call("import_pastenow") }

    /// The stored bitmap of an image item, whichever format it was copied in.
    func imageData(_ id: Int64) async -> Data? { await data(id: id, format: "image") }
}
