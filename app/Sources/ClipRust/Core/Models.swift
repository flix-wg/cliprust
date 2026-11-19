import Foundation

enum ItemKind: String, Codable, CaseIterable, Sendable {
    case text, url, image, file
}

struct Item: Codable, Identifiable, Hashable, Sendable {
    static let keysList = "Keys"
    static let idsList = "IDs"
    static let pinnedList = "Pinned"

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
    /// Detected `phone`, `email`, `address`; missing when none.
    var contact: [String]?
    // Only in `item` responses.
    var text: String?
    var formats: [String]?
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
    var contacts: [String] { contact ?? [] }

    static func contactSymbol(_ type: String) -> String {
        switch type {
        case "phone": "phone"
        case "email": "envelope"
        default: "mappin.and.ellipse"
        }
    }

    static func contactAction(_ type: String) -> String {
        switch type {
        case "phone": "Call"
        case "email": "New Email"
        default: "Open in Maps"
        }
    }

    /// `tel:` or `mailto:` for the first number or address found in `text`; Maps for the whole text.
    static func contactURL(_ type: String, in text: String) -> URL? {
        let range = NSRange(text.startIndex..., in: text)
        switch type {
        case "phone":
            let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.phoneNumber.rawValue)
            let number = detector?.firstMatch(in: text, range: range)?.phoneNumber ?? ""
            // In "+49 (0)30 …" the bracketed trunk zero is not dialled.
            let intl = number.replacingOccurrences(of: #"^(\+\d{1,3})\s*\(0\)"#, with: "$1", options: .regularExpression)
            let dial = intl.filter { $0.isNumber || $0 == "+" }
            return dial.isEmpty ? nil : URL(string: "tel:\(dial)")
        case "email":
            let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)
            return detector?.matches(in: text, range: range).lazy.compactMap(\.url).first { $0.scheme == "mailto" }
        default:
            var allowed = CharacterSet.urlQueryAllowed
            allowed.remove(charactersIn: "&+=?")
            let q = text.collapsingWhitespace.addingPercentEncoding(withAllowedCharacters: allowed) ?? ""
            return q.isEmpty ? nil : URL(string: "maps://?q=\(q)")
        }
    }

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

/// An app that stored items were copied from.
struct SourceApp: Codable, Hashable, Identifiable, Sendable {
    let bundleId: String
    var name: String?
    var count: Int
    var id: String { bundleId }
    var title: String { name ?? AppInfo.name(bundleId) }
}

/// A search as the core reads it: the plain words, the app a `from:` word named, the last `is:` type.
struct ParsedQuery: Decodable, Sendable {
    var text: String
    var app: SourceApp?
    var appTerm: String?
    /// `phone`, `email`, `address` or `any`.
    var contact: String?
    var contactTerms: [String]
}

struct ClipList: Codable, Identifiable, Hashable, Sendable {
    let name: String
    var sort: String?
    var manual: Bool
    var id: String { name }
}

struct CoreSettings: Codable, Equatable, Sendable {
    var paused = false
    var detectKeys = true
    /// Ask Safari for its current tab when a copy there brings no source URL.
    var safariPages = false
    var maxItems: Int? = 5000
    var maxAgeDays: Int?

    var json: [String: Any] {
        [
            "paused": paused,
            "detect_keys": detectKeys,
            "safari_pages": safariPages,
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

/// Keys at rest and whole-database encryption, as `encryption_status` reports them.
struct EncryptionStatus: Decodable, Equatable, Sendable {
    /// `ready`, `no_key`, `locked`, `unavailable` or `unsupported`.
    var keys: String
    var generation: Int?
    var message: String?
    /// Keys not yet sealed with the current master key.
    var pending: Int
    /// Keys whose master key is missing from the Keychain or wrong.
    var unreadable: Int
    var dbEncrypted: Bool
    var dbEncryptionAvailable: Bool

    var isLocked: Bool { keys == "locked" || unreadable > 0 }
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
