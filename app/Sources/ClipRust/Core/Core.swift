import CCliprust
import Foundation

/// An `err` from the core. `code` is `not_found`, `invalid`, `locked`, `io` or `internal`.
struct CoreError: LocalizedError, Decodable {
    var code = "internal"
    let message: String
    var errorDescription: String? { message }
}

/// The Rust core. Every FFI call runs on one serial background queue.
final class Core: @unchecked Sendable {
    private let handle: OpaquePointer
    private let queue: DispatchQueue
    private var onEvent: (@MainActor (CoreEvent) -> Void)?

    /// The Rust crate's version.
    static let version = String(cString: cr_version())

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

    private func send<T: Decodable>(_ method: String, _ params: [String: Any]) throws -> T {
        let request = try JSONSerialization.data(withJSONObject: ["method": method, "params": params])
        guard let out = String(decoding: request, as: UTF8.self).withCString({ cr_call(handle, $0) }) else {
            throw CoreError(message: "The core returned no response.")
        }
        defer { cr_free_string(out) }
        let envelope = try Self.decoder.decode(Envelope<T>.self, from: Data(bytesNoCopy: out, count: strlen(out), deallocator: .none))
        if let err = envelope.err { throw err }
        guard let ok = envelope.ok else { throw CoreError(message: "Malformed response.") }
        return ok
    }

    func call<T: Decodable>(_ method: String, _ params: [String: Any] = [:], as type: T.Type = T.self) async throws -> T {
        try await withCheckedThrowingContinuation { cont in
            queue.async {
                cont.resume(with: Result { try self.send(method, params) })
            }
        }
    }

    /// Blocks the caller until the core answers; only for the panel's first frame.
    func callNow<T: Decodable>(_ method: String, _ params: [String: Any] = [:], as type: T.Type = T.self) throws -> T {
        try queue.sync { try send(method, params) }
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
            DispatchQueue.main.async { MainActor.assumeIsolated { core.onEvent?(event) } }
        }, ctx)
    }

    private struct Ignored: Decodable {
        init(from decoder: Decoder) throws {}
    }

    /// `{"ok": T}` or `{"err": {"code", "message"}}`; `ok` may be JSON null, which only an optional or `Ignored` accepts.
    private struct Envelope<T: Decodable>: Decodable {
        let ok: T?
        let err: CoreError?

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: Key.self)
            err = try c.decodeIfPresent(CoreError.self, forKey: .err)
            ok = err == nil ? try c.decode(T.self, forKey: .ok) : nil
        }

        private enum Key: CodingKey { case ok, err }
    }
}

extension Core {
    /// `query` is in the core's search grammar (`from:mail`, `is:phone`, plain words); `app` (a bundle id)
    /// overrides its `from:` word and its `is:` word overrides `contact` (`phone`, `email`, `address` or `any`).
    func items(query: String? = nil, kind: ItemKind? = nil, list: String? = nil, app: String? = nil, contact: String? = nil,
               limit: Int) async throws -> [Item] {
        try await call("items", Self.itemsParams(query, kind, list, app, contact, limit))
    }

    /// `items` on the calling thread.
    func itemsNow(kind: ItemKind? = nil, list: String? = nil, contact: String? = nil, limit: Int) throws -> [Item] {
        try callNow("items", Self.itemsParams(nil, kind, list, nil, contact, limit))
    }

    private static func itemsParams(_ query: String?, _ kind: ItemKind?, _ list: String?, _ app: String?, _ contact: String?,
                                    _ limit: Int) -> [String: Any] {
        var p: [String: Any] = ["limit": limit]
        if let contact { p["contact"] = contact }
        if let query, !query.isEmpty { p["query"] = query }
        if let kind { p["kind"] = kind.rawValue }
        if let list { p["list"] = list }
        if let app { p["app"] = app }
        return p
    }

    /// How the core reads a search; with `app` set, `from:` words stay text.
    func parseQuery(_ query: String, app: String?) async throws -> ParsedQuery {
        try await call("parse_query", app.map { ["query": query, "app": $0] } ?? ["query": query])
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
    /// Items per contact type, and `any`.
    func contactCounts() async throws -> [String: Int] { try await call("counts") }
    /// Differs from an earlier value once any process committed a change.
    func dataVersion() async throws -> [Int] { try await call("data_version") }
    func dataVersionNow() throws -> [Int] { try callNow("data_version") }
    /// Folds the WAL back without waiting for other connections.
    func checkpoint() async { try? await run("checkpoint") }
    func lists() async throws -> [ClipList] { try await call("lists") }
    /// With `terms`, `app` or `contact`, a smart list matching every term (and `kind`, if given).
    func createList(_ name: String, terms: [String] = [], kind: ItemKind? = nil, app: String? = nil,
                    contact: String? = nil) async throws {
        var p: [String: Any] = ["name": name]
        if !terms.isEmpty || app != nil || contact != nil {
            var rules: [[String: Any]] = terms.map { ["type": "contains", "text": $0] }
            if let kind { rules.append(["type": "kind", "kind": kind.rawValue]) }
            if let app { rules.append(["type": "app", "bundle_id": app]) }
            if let contact { rules.append(contact == "any" ? ["type": "contact"] : ["type": "contact", "contact": contact]) }
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
