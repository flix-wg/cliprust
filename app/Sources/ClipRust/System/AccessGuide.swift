import AppKit
import ApplicationServices
import SwiftUI

/// Walks the user through granting Accessibility, which pasting into other apps needs.
@MainActor
final class AccessGuide: NSObject, NSWindowDelegate {
    static let shared = AccessGuide()

    private var window: NSWindow?
    private var timer: Timer?
    private let model = AccessGuideModel()

    func show() {
        model.trusted = Paster.isTrusted
        if window == nil {
            let view = AccessGuideView(model: model, fix: { [weak self] in self?.fix() }, copyOnly: { [weak self] in
                UserDefaults.standard.set(AfterChoosing.copy.rawValue, forKey: Prefs.Key.afterChoosing)
                self?.window?.close()
            })
            let host = NSHostingController(rootView: view)
            host.sizingOptions = .preferredContentSize
            let window = NSWindow(contentViewController: host)
            window.styleMask = [.titled, .closable]
            window.title = "Allow Pasting"
            window.isReleasedWhenClosed = false
            window.delegate = self
            window.center()
            self.window = window
        }
        startPolling()
        NSApp.activate()
        window?.makeKeyAndOrderFront(nil)
    }

    func windowWillClose(_ notification: Notification) {
        timer?.invalidate()
        timer = nil
        model.waiting = false
    }

    /// Clears a stale entry left by an earlier build, adds ClipRust to the list and opens it.
    /// macOS ties the grant to the code signature, so after an update the old switch can look on yet not count.
    private func fix() {
        model.waiting = true
        Task {
            model.resetFailed = await !Self.resetEntry()
            let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
            _ = AXIsProcessTrustedWithOptions(options)
            Paster.openAccessibilitySettings()
        }
    }

    private static func resetEntry() async -> Bool {
        guard let id = Bundle.main.bundleIdentifier else { return false }
        return await Task.detached {
            let p = Process()
            p.executableURL = URL(fileURLWithPath: "/usr/bin/tccutil")
            p.arguments = ["reset", "Accessibility", id]
            p.standardOutput = FileHandle.nullDevice
            p.standardError = FileHandle.nullDevice
            guard (try? p.run()) != nil else { return false }
            p.waitUntilExit()
            return p.terminationStatus == 0
        }.value
    }

    private func startPolling() {
        guard timer == nil else { return }
        timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { _ in
            MainActor.assumeIsolated {
                let shared = AccessGuide.shared
                guard !shared.model.trusted, Paster.isTrusted else { return }
                shared.model.trusted = true
                shared.timer?.invalidate()
                shared.timer = nil
                DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { shared.window?.close() }
            }
        }
    }
}

@MainActor @Observable
final class AccessGuideModel {
    var trusted = false
    var waiting = false
    var resetFailed = false
}

struct AccessGuideView: View {
    let model: AccessGuideModel
    let fix: () -> Void
    let copyOnly: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 14) {
                Image(nsImage: NSApp.applicationIconImage).resizable().frame(width: 52, height: 52)
                VStack(alignment: .leading, spacing: 3) {
                    Text("Let ClipRust paste for you").font(.headline)
                    Text("Your item is on the clipboard, so ⌘V still pastes it.").foregroundStyle(.secondary)
                }
            }
            Text("To paste straight into the app you were using, ClipRust needs Accessibility access. After an update, macOS can stop accepting the earlier permission even though ClipRust still looks switched on.")
                .fixedSize(horizontal: false, vertical: true)
            VStack(alignment: .leading, spacing: 8) {
                Step(n: 1, text: "Click Open Accessibility Settings. ClipRust clears its old entry first.")
                Step(n: 2, text: "Turn on ClipRust in the list.")
                if model.resetFailed {
                    Text("ClipRust couldn’t clear its old entry. If the switch is already on, select ClipRust, remove it with −, then add it again with + from Applications.")
                        .font(.callout).foregroundStyle(.orange)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Divider()
            HStack {
                status
                Spacer()
                Button("Use Copy Only", action: copyOnly)
                Button("Open Accessibility Settings", action: fix)
                    .keyboardShortcut(.defaultAction)
                    .disabled(model.trusted)
            }
        }
        .padding(20)
        .frame(width: 500)
    }

    @ViewBuilder private var status: some View {
        if model.trusted {
            Label("ClipRust can paste now", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
        } else if model.waiting {
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text("Waiting for access…").foregroundStyle(.secondary)
            }
        }
    }
}

private struct Step: View {
    let n: Int
    let text: String

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text("\(n)").font(.callout.monospacedDigit().bold())
                .frame(width: 20, height: 20)
                .background(Circle().fill(.quaternary))
            Text(text).fixedSize(horizontal: false, vertical: true)
        }
    }
}
