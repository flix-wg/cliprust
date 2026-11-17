// swift-tools-version: 6.0
import PackageDescription

let rustLib = Context.packageDirectory + "/../target/release"

let package = Package(
    name: "ClipRust",
    platforms: [.macOS("26.0")],
    targets: [
        .systemLibrary(name: "CCliprust", path: "Sources/CCliprust/include"),
        .executableTarget(
            name: "ClipRust",
            dependencies: ["CCliprust"],
            path: "Sources/ClipRust",
            swiftSettings: [.swiftLanguageMode(.v5)],
            linkerSettings: [
                .unsafeFlags(["-L", rustLib]),
                .linkedFramework("AppKit"),
                .linkedFramework("Carbon"),
                .linkedFramework("CoreGraphics"),
                .linkedFramework("LocalAuthentication"),
                .linkedFramework("Quartz"),
                .linkedFramework("QuickLookThumbnailing"),
                .linkedFramework("ServiceManagement"),
                .linkedLibrary("objc"),
                .linkedLibrary("iconv"),
            ]
        ),
    ]
)
