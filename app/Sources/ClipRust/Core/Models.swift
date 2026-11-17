import Foundation

enum ItemKind: String, Codable, CaseIterable, Sendable {
    case text, url, image, file
}

struct Item: Codable, Identifiable, Hashable, Sendable {
    static let keysList = "Keys"

    let id: Int64
    let kind: ItemKind
    var title: String?
    var preview: String?
    var app: String?
    var sourceUrl: String?
    var secretProvider: String?
    var createdAt: Int64
    var lastUsedAt: Int64
    var byteSize: Int64
    // Only in `item` responses.
    var text: String?
    var formats: [String]?
    // Arrive with the core update; nil until then.
    var note: String?
    var secretKind: String?
    var sourceDomain: String?
}

extension Item {
    var isSecret: Bool { secretProvider != nil || secretKind != nil }
    var isID: Bool { secretKind == "id" }
    var hasNote: Bool { !(note ?? "").isEmpty }
    var copiedAt: Date { Date(milliseconds: lastUsedAt) }
    var firstCopiedAt: Date { Date(milliseconds: createdAt) }
    var oneLine: String { (title ?? preview ?? "").collapsingWhitespace }

    var symbol: String {
        if isSecret { return isID ? "number" : "key.horizontal" }
        switch kind {
        case .text: return "doc.text"
        case .url: return "link"
        case .image: return "photo"
        case .file: return "folder"
        }
    }

    var domain: String? {
        if let sourceDomain { return sourceDomain }
        guard let host = sourceUrl.flatMap({ URL(string: $0)?.host() }) else { return nil }
        return host.hasPrefix("www.") ? String(host.dropFirst(4)) : host
    }

    var fileURLs: [URL] {
        (text ?? preview ?? "").split(whereSeparator: \.isNewline).compactMap { line in
            let s = String(line).trimmingCharacters(in: .whitespaces)
            if s.hasPrefix("file://") { return URL(string: s) }
            return s.hasPrefix("/") || s.hasPrefix("~") ? URL(fileURLWithPath: (s as NSString).expandingTildeInPath) : nil
        }
    }

    var maskedValue: String { preview ?? "" }
    var keyName: String { title ?? secretProvider.map { "\($0) \(isID ? "ID" : "key")" } ?? maskedValue }
    var providerLabel: String { secretProvider ?? "" }
    var siteLabel: String { domain ?? "" }
    var appLabel: String { app.map(AppInfo.name) ?? "" }
    var noteLabel: String { note ?? "" }
}

struct ClipList: Codable, Identifiable, Hashable, Sendable {
    let name: String
    var sort: String?
    var manual: Bool
    var id: String { name }
}

struct CoreSettings: Codable, Equatable, Sendable {
    var paused = false
    var detectKeys = false
    var maxItems: Int? = 5000
    var maxAgeDays: Int?

    var json: [String: Any] {
        [
            "paused": paused,
            "detect_keys": detectKeys,
            "max_items": maxItems.map { $0 as Any } ?? NSNull(),
            "max_age_days": maxAgeDays.map { $0 as Any } ?? NSNull(),
        ]
    }
}

struct Stats: Codable, Sendable {
    var items: Int
    var rawBytes: Int64
    var storedBytes: Int64
    var fileBytes: Int64
    var path: String
}

struct ImportResult: Codable, Sendable {
    var imported: Int
    var duplicates: Int
    var skipped: Int
    var listsCreated: Int
    var excludes: Int
}

struct CoreEvent: Decodable, Sendable {
    var type: String
    var id: Int64?
    var new: Bool?
    var secret: Bool?
    var reason: String?
    var error: String?
}

extension Date {
    init(milliseconds: Int64) { self.init(timeIntervalSince1970: TimeInterval(milliseconds) / 1000) }
}

extension String {
    var collapsingWhitespace: String {
        split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
    }
}
