import AppKit
import SwiftUI

extension EnvironmentValues {
    /// True while rendering offscreen previews, where materials render flat.
    @Entry var renderingPreview = false
}

struct PanelView: View {
    @Bindable var model: PanelModel
    @AppStorage(Prefs.Key.showPreview) private var showPreview = true
    @FocusState private var searchFocused: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(spacing: 0) {
            searchBar
            FilterBar(model: model)
            Divider()
            GeometryReader { geo in
                HStack(spacing: 0) {
                    ItemList(model: model)
                    if showPreview, !(model.loaded && model.items.isEmpty) {
                        Divider()
                        PreviewPane(item: model.current)
                            .frame(width: (geo.size.width * 0.42).rounded())
                    }
                }
            }
        }
        .background { PanelBackground() }
        .clipShape(.rect(cornerRadius: 14, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .strokeBorder(Color(nsColor: .separatorColor), lineWidth: 0.5)
        }
        .coordinateSpace(.named("panel"))
        .scaleEffect(model.isVisible || reduceMotion ? 1 : 0.98)
        .opacity(model.isVisible ? 1 : 0)
        .animation(.easeOut(duration: 0.12), value: model.isVisible)
        .onChange(of: model.isVisible, initial: true) { _, visible in
            if visible { searchFocused = true }
        }
    }

    private var searchBar: some View {
        HStack(spacing: 10) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 15, weight: .medium))
                .foregroundStyle(.secondary)
            TextField("Search clipboard", text: $model.query)
                .textFieldStyle(.plain)
                .font(.system(size: 15))
                .focused($searchFocused)
            if model.app.settings.paused {
                Label("Paused", systemImage: "pause.fill")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3)
                    .background(Color.primary.opacity(0.07), in: Capsule())
            }
        }
        .padding(.horizontal, 16)
        .frame(height: 48)
    }
}

struct PanelBackground: View {
    @Environment(\.renderingPreview) private var rendering

    var body: some View {
        if rendering {
            Color(nsColor: .windowBackgroundColor)
        } else {
            Color.clear.glassEffect(.regular, in: .rect(cornerRadius: 14, style: .continuous))
        }
    }
}

struct FilterBar: View {
    @Bindable var model: PanelModel

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    ForEach(model.filters, id: \.self) { filter in
                        Chip(title: filter.title,
                             count: model.showsCount(filter) ? model.countLabel : nil,
                             selected: model.filter == filter) { model.filter = filter }
                            .id(filter)
                    }
                }
                .padding(.horizontal, 8)
            }
            .onChange(of: model.filter) { _, f in proxy.scrollTo(f) }
        }
        .frame(height: 26)
        .padding(.bottom, 10)
    }
}

struct Chip: View {
    let title: String
    let count: String?
    let selected: Bool
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 5) {
                Text(title)
                if let count {
                    Text(count).foregroundStyle(.secondary).monospacedDigit()
                }
            }
            .font(.system(size: 12, weight: selected ? .semibold : .regular))
            .foregroundStyle(selected ? .primary : .secondary)
            .padding(.horizontal, 9)
            .frame(height: 24)
            .background(Capsule().fill(Color.primary.opacity(selected ? 0.1 : hovering ? 0.05 : 0)))
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}

struct ItemList: View {
    @Bindable var model: PanelModel

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(spacing: 2) {
                    ForEach(Array(model.items.enumerated()), id: \.element.id) { index, item in
                        ItemRow(item: item, index: index, selected: item.id == model.selectedID,
                                pinned: model.app.pinnedIDs.contains(item.id))
                            .id(item.id)
                            .onGeometryChange(for: CGRect.self, of: { $0.frame(in: .named("panel")) }) { frame in
                                model.rowFrames[item.id] = frame
                            }
                            .onTapGesture {
                                model.selectedID = item.id
                                if NSApp.currentEvent?.clickCount == 2 { model.perform(.choose(.standard), item) }
                            }
                            .contextMenu { RowMenu(model: model, item: item) }
                    }
                }
                .padding(6)
            }
            .scrollIndicators(.automatic)
            .onChange(of: model.selectedID) { _, id in
                if let id { proxy.scrollTo(id) }
            }
        }
        .overlay {
            if model.loaded, model.items.isEmpty {
                Text(model.emptyMessage)
                    .font(.system(size: 13))
                    .foregroundStyle(.secondary)
                    .padding(24)
                    .multilineTextAlignment(.center)
            }
        }
    }
}

struct RowMenu: View {
    let model: PanelModel
    let item: Item

    var body: some View {
        Button("Paste") { model.perform(.choose(.standard), item) }
        Button("Paste as Plain Text") { model.perform(.choose(.plain), item) }
        Button("Copy") { model.perform(.choose(.copyOnly), item) }
        Menu("Add to List") {
            ForEach(model.app.manualLists) { list in
                Button(list.name) { model.perform(.addToList(list.name), item) }
            }
            if !model.app.manualLists.isEmpty { Divider() }
            Button("New List…") { model.perform(.newList, item) }
        }
        if let list = model.currentManualList {
            Button("Remove from \(list)") { model.perform(.removeFromList(list), item) }
        }
        Divider()
        Button("Rename…") { model.perform(.rename, item) }
        if Features.notes {
            Button("Edit Note…") { model.perform(.note, item) }
        }
        if item.kind == .file {
            Button("Reveal in Finder") { model.perform(.revealInFinder, item) }
        }
        Divider()
        Button("Delete", role: .destructive) { model.perform(.delete, item) }
    }
}
