import AppKit

/// In-memory data for `--render-previews`.
@MainActor
enum SampleData {
    static let now = Int64(Date().timeIntervalSince1970 * 1000)
    static func ago(_ minutes: Double) -> Int64 { now - Int64(minutes * 60_000) }

    static let code = """
    func greet(_ name: String) -> String {
        let hour = Calendar.current.component(.hour, from: .now)
        return hour < 12 ? "Good morning, \\(name)" : "Hello, \\(name)"
    }
    """

    static let items: [Item] = [
        item(1, .text, code, app: "com.apple.dt.Xcode", minutes: 1, size: 162, formats: ["text"]),
        item(2, .url, "https://developer.apple.com/documentation/swiftui/applying-liquid-glass-to-custom-views",
             app: "com.apple.Safari", minutes: 6, size: 86, formats: ["text"]),
        item(3, .image, nil, app: "com.apple.Preview", minutes: 14, size: 1_284_000, formats: ["public.png"]),
        secret(4, "sk-proj-••••••••••••••••3f9Q", provider: "OpenAI", url: "https://platform.openai.com/api-keys", minutes: 52),
        item(5, .text, "Ship ClipRust 0.1 on Friday, then collect feedback for a week before touching sync.",
             app: "com.apple.Notes", minutes: 95, size: 82, formats: ["text", "rtf"]),
        item(6, .file, "/Users/sam/Documents/Invoices/2026-11 Hosting.pdf", app: "com.apple.finder", minutes: 180, size: 48, formats: ["files"]),
        item(7, .text, "brew install --cask cliprust", app: "com.apple.Terminal", minutes: 300, size: 28, formats: ["text"]),
        secret(8, "ghp_••••••••••••••••••••Xk2a", provider: "GitHub", url: "https://github.com/settings/tokens", minutes: 1500),
        item(9, .url, "https://github.com/rust-lang/rust/issues/123456", app: "com.apple.Safari", minutes: 2900, size: 47, formats: ["text"]),
        item(10, .text, "Meeting moved to Thursday 14:00, room 2.14", app: "com.apple.mail", minutes: 4400, size: 42, formats: ["text"]),
        secret(11, "••••••••••••••••a91c", provider: "Cloudflare", url: "https://dash.cloudflare.com/profile/api-tokens", minutes: 9000),
        item(12, .text, "Grüezi mitenand", app: "com.apple.Notes", minutes: 14000, size: 16, formats: ["text"]),
    ]

    static func item(_ id: Int64, _ kind: ItemKind, _ text: String?, app: String, minutes: Double, size: Int64, formats: [String]) -> Item {
        Item(id: id, kind: kind, title: nil, preview: text.map { String($0.prefix(300)) }, app: app, sourceUrl: nil,
             secretProvider: nil, createdAt: ago(minutes * 3), lastUsedAt: ago(minutes), byteSize: size,
             text: text, formats: formats)
    }

    static func secret(_ id: Int64, _ masked: String, provider: String, url: String, minutes: Double) -> Item {
        Item(id: id, kind: .text, title: nil, preview: masked, app: "com.apple.Safari", sourceUrl: url,
             secretProvider: provider, createdAt: ago(minutes * 2), lastUsedAt: ago(minutes), byteSize: 64,
             text: masked, formats: ["text"])
    }

    static func appState() -> AppState {
        let app = AppState(core: nil)
        app.lists = [
            ClipList(name: Item.keysList, sort: "recent", manual: true),
            ClipList(name: "Snippets", sort: "recent", manual: true),
            ClipList(name: "Today", sort: "recent", manual: false),
        ]
        app.pinnedIDs = [5, 7]
        app.excludes = ["com.apple.keychainaccess", "com.1password.1password"]
        app.settings = CoreSettings(paused: false, detectKeys: true, maxItems: 5000, maxAgeDays: nil)
        app.stats = Stats(items: 4812, rawBytes: 91_000_000, storedBytes: 38_400_000, fileBytes: 0,
                          path: NSHomeDirectory() + "/Library/Application Support/cliprust/cliprust.db")
        return app
    }

    static func seedImages() {
        let size = NSSize(width: 1600, height: 1000)
        let image = NSImage(size: size, flipped: false) { rect in
            NSGradient(colors: [NSColor(calibratedRed: 0.32, green: 0.45, blue: 0.86, alpha: 1),
                                NSColor(calibratedRed: 0.62, green: 0.38, blue: 0.78, alpha: 1)])?.draw(in: rect, angle: -35)
            NSColor(white: 1, alpha: 0.9).setFill()
            NSBezierPath(roundedRect: NSRect(x: 260, y: 170, width: 1080, height: 660), xRadius: 36, yRadius: 36).fill()
            NSColor(white: 0.9, alpha: 1).setFill()
            for i in 0..<5 {
                NSBezierPath(roundedRect: NSRect(x: 340, y: 680 - CGFloat(i) * 96, width: CGFloat(760 - i * 90), height: 36),
                             xRadius: 18, yRadius: 18).fill()
            }
            return true
        }
        guard let tiff = image.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
              let png = rep.representation(using: .png, properties: [:]) else { return }
        if let small = Thumbnails.downscale(png, maxPixel: 96), let large = Thumbnails.downscale(png, maxPixel: 1400) {
            Thumbnails.shared.seed(3, small)
            Thumbnails.shared.seed(3, large)
        }
    }
}
