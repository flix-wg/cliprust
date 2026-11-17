import AppKit
import Carbon.HIToolbox
import SwiftUI

struct ShortcutRecorder: View {
    @Bindable var hotkey: Hotkey
    @State private var recording = false
    @State private var monitor: Any?

    var body: some View {
        HStack(spacing: 6) {
            Button(action: toggle) {
                Text(recording ? "Type shortcut…" : hotkey.preferred.display)
                    .font(.system(size: 12, weight: recording ? .regular : .medium))
                    .foregroundStyle(recording ? .secondary : .primary)
                    .frame(minWidth: 96)
            }
            .buttonStyle(.bordered)
            if hotkey.preferred != .standard, !recording {
                Button {
                    hotkey.preferred = .standard
                } label: {
                    Image(systemName: "arrow.counterclockwise")
                }
                .buttonStyle(.borderless)
                .help("Reset to \(Shortcut.standard.display)")
            }
        }
        .onDisappear(perform: stop)
    }

    private func toggle() {
        recording ? stop() : start()
    }

    private func start() {
        recording = true
        hotkey.unregister()
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { e in
            nonisolated(unsafe) let event = e
            MainActor.assumeIsolated {
                if event.keyCode == UInt16(kVK_Escape) {
                    stop()
                } else if let shortcut = Shortcut(event: event) {
                    recording = false
                    removeMonitor()
                    hotkey.preferred = shortcut
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
        removeMonitor()
        hotkey.register()
    }

    private func removeMonitor() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
    }
}
