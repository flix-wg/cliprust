import AppKit
import SwiftUI

struct KeysView: View {
    @Bindable var model: KeysModel
    @State private var showInspector = true
    @State private var columns = TableColumnCustomization<Item>()
    @State private var unmarking: Item?
    @Environment(\.renderingPreview) private var rendering

    var body: some View {
        Group {
            if rendering {
                // Offscreen renders cannot capture the split view's glass panes.
                HStack(spacing: 0) {
                    sidebar.frame(width: 190)
                    Divider()
                    table
                    Divider()
                    KeyInspector(model: model).frame(width: 260)
                }
            } else {
                NavigationSplitView {
                    sidebar.navigationSplitViewColumnWidth(min: 170, ideal: 190, max: 280)
                } detail: {
                    table.inspector(isPresented: $showInspector) {
                        KeyInspector(model: model).inspectorColumnWidth(min: 240, ideal: 260, max: 360)
                    }
                }
            }
        }
        .searchable(text: $model.search, placement: .toolbar, prompt: "Search keys")
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    showInspector.toggle()
                } label: {
                    Label("Inspector", systemImage: "sidebar.trailing")
                }
                .keyboardShortcut("i", modifiers: [.command, .option])
                .help("Show or hide the inspector (⌥⌘I)")
            }
        }
        .navigationTitle("Keys & IDs")
    }

    private var sidebar: some View {
        List(selection: $model.scope) {
            Section {
                Label("All Keys", systemImage: "key.horizontal").badge(model.keyCount).tag(KeyScope.all)
                Label("IDs", systemImage: "number").badge(model.idCount).tag(KeyScope.ids)
            }
            if !model.providers.isEmpty {
                Section("Providers") {
                    ForEach(model.providers) { f in
                        Label(f.name, systemImage: "building.2").badge(f.count).tag(KeyScope.provider(f.name))
                    }
                }
            }
            if !model.sites.isEmpty {
                Section("Sites") {
                    ForEach(model.sites) { f in
                        Label(f.name, systemImage: "globe").badge(f.count).tag(KeyScope.site(f.name))
                    }
                }
            }
        }
        .listStyle(.sidebar)
    }

    private var table: some View {
        Table(model.rows, selection: $model.selection, sortOrder: $model.sortOrder, columnCustomization: $columns) {
            TableColumn("Name", value: \.keyName) { item in
                Text(item.keyName).lineLimit(1)
            }
            .width(min: 90, ideal: 120)
            .customizationID("name")
            TableColumn("Value", value: \.maskedValue) { item in
                Text(model.revealed[item.id] ?? item.maskedValue)
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            .width(min: 100, ideal: 150)
            .customizationID("value")
            TableColumn("Provider", value: \.providerLabel)
                .width(min: 60, ideal: 80)
                .customizationID("provider")
            TableColumn("Site", value: \.siteLabel)
                .width(min: 60, ideal: 120)
                .customizationID("site")
            TableColumn("App", value: \.appLabel)
                .width(min: 50, ideal: 80)
                .customizationID("app")
                .defaultVisibility(.hidden)
            TableColumn("Copied", value: \.lastUsedAt) { item in
                Text(Format.ago(item.copiedAt)).foregroundStyle(.secondary).monospacedDigit()
            }
            .width(min: 44, ideal: 56)
            .customizationID("copied")
            TableColumn("Note", value: \.noteLabel)
                .customizationID("note")
        }
        .contextMenu(forSelectionType: Int64.self) { ids in
            if let item = ids.count == 1 ? model.items.first(where: { $0.id == ids.first }) : nil {
                Button("Copy") { Task { await model.copy(item) } }
                Button("Copy Masked") { model.copyMasked(item) }
                Button("Open Source Page") { model.openSource(item) }
                    .disabled(item.sourceUrl == nil)
                if !item.isID {
                    Button("Not a Key…") { unmarking = item }
                }
                Divider()
            }
            Button("Remove from Keys") { Task { await model.removeFromKeys(ids) } }
            Button("Delete", role: .destructive) { Task { await model.delete(ids) } }
        } primaryAction: { ids in
            if let id = ids.first, let item = model.items.first(where: { $0.id == id }) {
                Task { await model.copy(item) }
            }
        }
        .onCopyCommand {
            if let item = model.selectedItem { Task { await model.copy(item) } }
            return []
        }
        .onDeleteCommand { Task { await model.delete(model.selection) } }
        .confirmationDialog("Store this key as plain text?", isPresented: Binding(get: { unmarking != nil },
                                                                                  set: { if !$0 { unmarking = nil } }),
                            presenting: unmarking) { item in
            Button("Not a Key", role: .destructive) { Task { await model.unmark(item) } }
        } message: { _ in
            Text("It is no longer encrypted or hidden, its text becomes searchable, and ClipRust won’t take it for a key again.")
        }
        .overlay {
            if model.rows.isEmpty {
                ContentUnavailableView {
                    Label(model.search.isEmpty ? "No Keys" : "No Matches", systemImage: "key.horizontal")
                } description: {
                    Text(model.search.isEmpty
                         ? "API keys and IDs you copy show up here. Turn on detection in Settings → Keys."
                         : "Nothing matches “\(model.search)”.")
                }
            }
        }
    }
}

struct KeyInspector: View {
    @Bindable var model: KeysModel
    @State private var name = ""
    @State private var note = ""

    var body: some View {
        if let item = model.selectedItem {
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    VStack(alignment: .leading, spacing: 6) {
                        TextField(item.keyName, text: $name)
                            .textFieldStyle(.plain)
                            .font(.system(size: 18, weight: .semibold))
                            .onSubmit { Task { await model.rename(item, to: name) } }
                        HStack(spacing: 6) {
                            KeyBadge(isID: item.isID)
                            if let provider = item.secretProvider {
                                Text(provider).font(.system(size: 12)).foregroundStyle(.secondary)
                            }
                        }
                    }
                    valueBox(item)
                    details(item)
                    VStack(alignment: .leading, spacing: 6) {
                        Text("Note").font(.system(size: 11, weight: .medium)).foregroundStyle(.secondary)
                        TextField("Add a note…", text: $note, axis: .vertical)
                            .textFieldStyle(.plain)
                            .font(.system(size: 12))
                            .lineLimit(3...8)
                            .padding(8)
                            .background(Color.primary.opacity(0.05), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                            .onSubmit { Task { await model.setNote(item, note) } }
                    }
                    Divider()
                    HStack {
                        Button(item.isID ? "Remove from IDs" : "Remove from Keys") { Task { await model.removeFromKeys([item.id]) } }
                        Spacer()
                        Button("Delete", role: .destructive) { Task { await model.delete([item.id]) } }
                    }
                    .controlSize(.small)
                }
                .padding(16)
            }
            .onChange(of: item.id, initial: true) { old, _ in
                if let previous = model.items.first(where: { $0.id == old }), previous.id != item.id {
                    let text = note
                    Task { await model.setNote(previous, text) }
                }
                name = item.title ?? ""
                note = item.note ?? ""
            }
        } else {
            Text(model.selection.count > 1 ? "\(model.selection.count) keys selected" : "No Selection")
                .font(.system(size: 13))
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private func valueBox(_ item: Item) -> some View {
        let shown = model.revealed[item.id]
        return VStack(alignment: .leading, spacing: 8) {
            // Not selectable: ⌘C would bypass the concealed copy.
            Text(shown ?? item.maskedValue)
                .font(.system(size: 12, design: .monospaced))
                .lineLimit(5)
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.primary.opacity(0.05), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            HStack(spacing: 8) {
                Button {
                    Task { await model.toggleReveal(item) }
                } label: {
                    Label(shown == nil ? "Reveal" : "Hide", systemImage: shown == nil ? "eye" : "eye.slash")
                }
                Button {
                    Task { await model.copy(item) }
                } label: {
                    Label("Copy", systemImage: "doc.on.doc")
                }
            }
            .controlSize(.small)
        }
    }

    private func details(_ item: Item) -> some View {
        Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 6) {
            if let url = item.sourceUrl.flatMap(URL.init(string:)) {
                GridRow {
                    label("Source")
                    Link(item.domain ?? url.absoluteString, destination: url)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
            if let app = item.app {
                GridRow { label("App"); Text(AppInfo.name(app)) }
            }
            GridRow { label("Copied"); Text(Format.date(item.copiedAt)) }
            GridRow { label("First seen"); Text(Format.date(item.firstCopiedAt)) }
        }
        .font(.system(size: 12))
    }

    private func label(_ s: String) -> some View {
        Text(s).foregroundStyle(.secondary).gridColumnAlignment(.trailing)
    }
}
