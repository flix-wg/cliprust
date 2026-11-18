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
        .overlay(alignment: .topLeading) {
            if model.palette != nil {
                ZStack(alignment: .topLeading) {
                    Color.black.opacity(0.001).onTapGesture { model.palette = nil }
                    ListPaletteView(model: model).padding(.top, 46).padding(.leading, 12)
                }
            }
        }
        .overlay(alignment: .bottom) {
            if let toast = model.toast {
                Label(toast, systemImage: model.toastSymbol)
                    .font(.system(size: 12, weight: .medium))
                    .padding(.horizontal, 12)
                    .frame(height: 28)
                    .background { PanelBackground(radius: 14) }
                    .overlay { Capsule().strokeBorder(Color(nsColor: .separatorColor), lineWidth: 0.5) }
                    .padding(.bottom, 14)
                    .transition(.opacity)
            }
        }
        .animation(.easeOut(duration: 0.15), value: model.toast)
        .clipShape(.rect(cornerRadius: 14, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .strokeBorder(Color(nsColor: .separatorColor), lineWidth: 0.5)
        }
        .scaleEffect(model.isVisible || reduceMotion ? 1 : 0.98)
        .opacity(model.isVisible ? 1 : 0)
        .animation(.easeOut(duration: 0.12), value: model.isVisible)
        .onChange(of: model.isVisible, initial: true) { _, visible in
            if visible { searchFocused = true }
        }
        .onChange(of: model.palette == nil && model.editing == nil) { _, closed in
            if closed { searchFocused = true }
        }
    }

    private var searchBar: some View {
        HStack(spacing: 10) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 15, weight: .medium))
                .foregroundStyle(.secondary)
            HStack(spacing: 6) {
                if let source = model.source {
                    SearchToken(title: source.title, help: "Show all apps", remove: { model.source = nil }) {
                        AppIconView(bundleID: source.bundleId, size: 15)
                    }
                }
                if let type = model.contactToken {
                    SearchToken(title: type.title, help: "Show all clips", remove: { model.contactToken = nil }) {
                        Image(systemName: type.symbol).font(.system(size: 11, weight: .medium)).frame(width: 15, height: 15)
                    }
                }
                TextField(model.searchPlaceholder, text: $model.query)
                    .textFieldStyle(.plain)
                    .font(.system(size: 15))
                    .focused($searchFocused)
            }
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
    var radius: CGFloat = 14
    @Environment(\.renderingPreview) private var rendering

    var body: some View {
        if rendering {
            Color(nsColor: .windowBackgroundColor)
        } else {
            Color.clear.glassEffect(.regular, in: .rect(cornerRadius: radius, style: .continuous))
        }
    }
}

struct FilterBar: View {
    @Bindable var model: PanelModel
    @AppStorage(Prefs.Key.showTypeFilters) private var showKinds = true

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    ForEach(Array(model.filters(kinds: showKinds).enumerated()), id: \.element) { i, filter in
                        Chip(title: filter.title,
                             count: model.showsCount(filter) ? model.countLabel : nil,
                             selected: model.filter == filter,
                             hint: model.controlHeld && i < 9 ? i + 1 : nil) { model.filter = filter }
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
    var hint: Int?
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
            .overlay(alignment: .leading) {
                if let hint {
                    Text("\(hint)")
                        .font(.system(size: 8, weight: .bold, design: .rounded))
                        .foregroundStyle(.secondary)
                        .padding(.leading, 2.5)
                }
            }
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
                    if let suggestion = model.suggestion {
                        SuggestionRow(suggestion: suggestion, selected: model.suggestionSelected)
                            .id("suggestion")
                            .onTapGesture { model.acceptSuggestion() }
                    }
                    ForEach(Array(model.items.enumerated()), id: \.element.id) { index, item in
                        ItemRow(item: item, index: index, selected: item.id == model.selectedID && !model.suggestionSelected,
                                pinned: model.app.pinnedIDs.contains(item.id), now: model.now)
                            .id(item.id)
                            .onTapGesture {
                                model.suggestionSelected = false
                                model.selectedID = item.id
                                if NSApp.currentEvent?.clickCount == 2 { model.perform(.choose(.standard), item) }
                            }
                            .contextMenu { RowMenu(model: model, item: item) }
                        if model.editing?.id == item.id {
                            InlineEditField(model: model).id("edit")
                        }
                    }
                }
                .padding(6)
            }
            .scrollIndicators(.automatic)
            .onChange(of: model.selectedID) { _, id in
                if let id { proxy.scrollTo(id) }
            }
            .onChange(of: model.suggestionSelected) { _, on in
                if on { proxy.scrollTo("suggestion") }
            }
            .onChange(of: model.editing?.id) { _, id in
                if id != nil { proxy.scrollTo("edit") }
            }
        }
        .overlay {
            if model.loaded, model.items.isEmpty, model.suggestion == nil {
                Text(model.emptyMessage)
                    .font(.system(size: 13))
                    .foregroundStyle(.secondary)
                    .padding(24)
                    .multilineTextAlignment(.center)
            }
        }
    }
}

/// The note or title field under the selected row: ⏎ saves, Esc cancels.
private struct InlineEditField: View {
    @Bindable var model: PanelModel
    @FocusState private var focused: Bool
    @AppStorage(Prefs.Key.density) private var density = Density.compact

    var body: some View {
        let note = model.editing?.field == .note
        HStack(spacing: 8) {
            Image(systemName: note ? "note.text" : "character.cursor.ibeam")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .frame(width: 18)
            TextField(note ? "Add a note…" : "Title", text: Binding(get: { model.editing?.text ?? "" },
                                                                    set: { model.editing?.text = $0 }))
                .textFieldStyle(.plain)
                .font(.system(size: 13))
                .focused($focused)
            Text("⏎ Save  Esc Cancel")
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
                .fixedSize()
        }
        .padding(.horizontal, 10)
        .frame(height: density.rowHeight)
        .background(RoundedRectangle(cornerRadius: 7, style: .continuous).strokeBorder(Color.accentColor.opacity(0.5)))
        .onAppear { focused = true }
    }
}

struct RowMenu: View {
    let model: PanelModel
    let item: Item

    var body: some View {
        Button("Paste") { model.perform(.choose(.standard), item) }
        Button("Paste as Plain Text") { model.perform(.choose(.plain), item) }
        Button("Copy") { model.perform(.choose(.copyOnly), item) }
        if !item.contacts.isEmpty {
            Divider()
            ForEach(item.contacts, id: \.self) { type in
                Button(Item.contactAction(type)) { model.perform(.openContact(type), item) }
            }
        }
        Divider()
        Button(model.app.pinnedIDs.contains(item.id) ? "Unpin" : "Pin") { model.perform(.togglePin, item) }
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
        Button("Rename") { model.perform(.rename, item) }
        if Features.notes {
            Button("Edit Note") { model.perform(.note, item) }
        }
        if item.kind == .file {
            Button("Reveal in Finder") { model.perform(.revealInFinder, item) }
        }
        Divider()
        Button("Delete", role: .destructive) { model.perform(.delete, item) }
    }
}
