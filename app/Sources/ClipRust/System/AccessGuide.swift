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
    private let drag = AccessDragPanel()

    /// `afterPaste`: a chosen item just landed on the clipboard instead of being pasted.
    func show(afterPaste: Bool = false) {
        guard !Paster.isTrusted else { return }
        model.trusted = false
        model.afterPaste = afterPaste
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
        drag.hide()
    }

    /// Clears a stale entry left by an earlier build, opens the pane and pins a drag helper under it.
    /// macOS ties the grant to the code signature, so after an update the old switch can look on yet not count.
    private func fix() {
        guard !model.resetting, !Paster.isTrusted else { return }
        model.waiting = true
        model.resetting = true
        Task {
            let reset = await Self.resetEntry()
            model.resetting = false
            // Closed, switched to Copy Only or granted meanwhile: leave Settings alone.
            guard window?.isVisible == true, Prefs.afterChoosing == .paste, !Paster.isTrusted else { return }
            model.resetFailed = !reset
            Paster.openAccessibilitySettings()
            drag.show()
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
    var resetting = false
    var resetFailed = false
    var afterPaste = false
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
                    Text(model.afterPaste ? "Your item is on the clipboard, so ⌘V still pastes it."
                                          : "Until then, chosen items are only copied, so ⌘V pastes them.")
                        .foregroundStyle(.secondary)
                }
            }
            Text("To paste straight into the app you were using, ClipRust needs Accessibility access. After an update, macOS can stop accepting the earlier permission even though ClipRust still looks switched on.")
                .fixedSize(horizontal: false, vertical: true)
            VStack(alignment: .leading, spacing: 8) {
                Step(n: 1, text: "Click Open Accessibility Settings. ClipRust clears its old entry first.")
                Step(n: 2, text: "Drag ClipRust from the helper under Settings into the list, and make sure its switch is on.")
                if model.resetFailed {
                    Text("ClipRust couldn’t clear its old entry. If the switch is already on, select ClipRust, remove it with −, then drag it in again.")
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
                    .disabled(model.trusted || model.resetting)
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
