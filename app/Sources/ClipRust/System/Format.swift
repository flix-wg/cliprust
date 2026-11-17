import Foundation

enum Format {
    static func ago(_ date: Date, now: Date = Date()) -> String {
        let s = max(0, now.timeIntervalSince(date))
        switch s {
        case ..<60: return "now"
        case ..<3600: return "\(Int(s / 60))m"
        case ..<86400: return "\(Int(s / 3600))h"
        case ..<(7 * 86400): return "\(Int(s / 86400))d"
        default: return date.formatted(.dateTime.day().month(.abbreviated))
        }
    }

    private static let full: DateFormatter = {
        let f = DateFormatter()
        f.dateStyle = .medium
        f.timeStyle = .short
        f.doesRelativeDateFormatting = true
        return f
    }()

    static func date(_ date: Date) -> String { full.string(from: date) }

    static func bytes(_ n: Int64) -> String {
        ByteCountFormatter.string(fromByteCount: n, countStyle: .file)
    }

    static func formats(_ formats: [String]) -> String {
        formats.map { f in
            switch f {
            case "text": "Text"
            case "rtf": "RTF"
            case "html": "HTML"
            case "files": "File"
            default: f.hasPrefix("public.") ? String(f.dropFirst(7)).uppercased() : f
            }
        }.joined(separator: ", ")
    }

    static func pixels(_ size: CGSize) -> String {
        "\(Int(size.width)) × \(Int(size.height))"
    }

    static func looksLikeCode(_ text: String) -> Bool {
        let lines = text.split(whereSeparator: \.isNewline).prefix(60)
        guard let first = lines.first else { return false }
        if lines.count == 1 {
            let commands = ["$ ", "sudo ", "git ", "brew ", "cd ", "npm ", "cargo ", "ssh ", "curl ", "docker ", "kubectl ", "swift "]
            return commands.contains { first.hasPrefix($0) }
        }
        let markers = ["{", "}", ";", "=>", "->", "func ", "def ", "fn ", "let ", "const ", "import ", "#include", "</", "()", "==", "&&", "return "]
        let score = lines.filter { line in
            line.hasPrefix("  ") || line.hasPrefix("\t") || markers.contains { line.contains($0) }
        }.count
        return Double(score) / Double(lines.count) >= 0.5
    }
}
