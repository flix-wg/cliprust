import AppKit
import Carbon.HIToolbox
import SwiftUI

/// Records a shortcut with ⌘, ⌥ or ⌃. `set` returns why a shortcut was refused, shown next to it.
struct ShortcutRecorder: View {
    let shortcut: Shortcut
    let standard: Shortcut
    let set: (Shortcut) -> String?
    @State private var recording = false
    @State private var monitor: Any?
    @State private var error: String?

    var body: some View {
        HStack(spacing: 6) {
            if let error {
                Text(error)
                    .font(.system(size: 11))
                    .foregroundStyle(.red)
                    .multilineTextAlignment(.trailing)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Button(action: toggle) {
                Text(recording ? "Type shortcut…" : shortcut.display)
                    .font(.system(size: 12, weight: recording ? .regular : .medium))
                    .foregroundStyle(recording ? .secondary : .primary)
                    .frame(minWidth: 96)
            }
            .buttonStyle(.bordered)
            let custom = shortcut != standard && !recording
            Button {
                error = set(standard)
            } label: {
                Image(systemName: "arrow.counterclockwise")
            }
            .buttonStyle(.borderless)
            .help("Reset to \(standard.display)")
            .opacity(custom ? 1 : 0)
            .disabled(!custom)
        }
        .onDisappear(perform: stop)
    }

    private func toggle() {
        recording ? stop() : start()
    }

    /// The open shortcut is unregistered while recording, so it can be typed here.
    private func start() {
        recording = true
        error = nil
        Hotkey.shared.unregister()
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { e in
            nonisolated(unsafe) let event = e
            MainActor.assumeIsolated {
                if event.keyCode == UInt16(kVK_Escape) {
                    stop()
                } else if let shortcut = Shortcut(event: event) {
                    error = set(shortcut)
                    stop()
                } else {
                    NSSound.beep()
                }
            }
            return nil
        }
    }

    private func stop() {
        guard recording || monitor != nil else { return }
        recording = false
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
        Hotkey.shared.register()
    }
}
