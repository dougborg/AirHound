import AirHoundCore
import SwiftUI

struct MainView: View {
  @ObservedObject var model: AppModel
  @State private var presentation = ObservationPresentation()
  var observations: [Observation] { presentation.recording?.observations ?? [] }
  @State private var receiver: String? = "all"
  @State private var search = ""
  @State private var medium = "All types"
  @State private var matchesOnly = false
  @State private var showInspector = true
  @State private var sortOrder = [ObservationSort()]
  @State private var tableRevision = UUID()
  // Rebuild only when the data or query changes, never while rendering a row,
  // scrolling, selecting an observation, or opening the inspector.
  @State private var filtered: [Observation] = []
  @State private var receiverIDs: [String] = []
  var selected: Observation? { filtered.first { $0.id == presentation.selection } }
  var body: some View {
    NavigationSplitView {
      List(selection: $receiver) {
        Label("All observations", systemImage: "list.bullet").tag("all")
        Section(model.isExample ? "Example receivers" : "Recorded receivers") {
          ForEach(receiverIDs, id: \.self) { id in
            receiverLabel(
              id, id: id, icon: "antenna.radiowaves.left.and.right",
              assignment: "Source of retained observations")
          }
        }
      }
      .navigationSplitViewColumnWidth(min: 210, ideal: 235, max: 320)
      .safeAreaInset(edge: .bottom) {
        VStack(alignment: .leading, spacing: 6) {
          Text(presentation.label).font(.headline)
          Text("Capture controls are in the Capture tab.").font(.caption).foregroundStyle(
            .secondary)
        }.frame(maxWidth: .infinity, alignment: .leading).padding()
      }
    } detail: {
      VStack(spacing: 0) {
        RecordNavigation(
          session: model.session, held: presentation.held,
          hold: { presentation.hold() }, resume: resumeUpdates)
        HStack {
          Picker("Protocol", selection: $medium) {
            Text("All types").tag("All types")
            Text("Wi-Fi").tag("Wi-Fi")
            Text("Bluetooth").tag("Bluetooth")
          }.frame(width: 180)
          Toggle("Matches only", isOn: $matchesOnly).toggleStyle(.checkbox)
          Spacer()
        }.padding(12)
        Divider()
        ObservationTable(
          rows: filtered, revision: tableRevision, context: model.recordingRevision,
          selection: Binding(
            get: { presentation.selection },
            set: { presentation.select($0, liveWindow: model.session.browsingRun != nil) }),
          sortOrder: $sortOrder,
          userScrolled: {
            if model.session.browsingRun != nil { presentation.hold() }
          }
        )
        .overlay {
          if observations.isEmpty {
            ContentUnavailableView(
              "Open a recording", systemImage: "doc.text",
              description: Text(
                "Open a saved recording from Capture, an NDJSON file, or the example recording."
              ))
          } else if filtered.isEmpty {
            ContentUnavailableView.search(text: search)
          }
        }
        Divider()
        HStack {
          Text("\(filtered.count) of \(observations.count) observations")
          Spacer()
          Text(presentation.label)
        }.font(.caption).foregroundStyle(.secondary).padding(10)
      }
      .navigationTitle("AirHound")
      .navigationSubtitle(presentation.label)
      .searchable(text: $search, placement: .toolbar, prompt: "Search observations")
      .inspector(isPresented: $showInspector) {
        InspectorView(observation: selected).inspectorColumnWidth(min: 260, ideal: 300, max: 420)
      }
      .toolbar {
        ToolbarItemGroup(placement: .primaryAction) {
          Button {
            model.showDevices = true
          } label: {
            Label("Devices", systemImage: "externaldrive.connected.to.line.below")
          }
          Button {
            model.openRecording()
          } label: {
            Label("Open recording", systemImage: "folder")
          }
          .disabled(model.loadingRecording)
          Button {
            model.loadExample()
          } label: {
            Label("Example", systemImage: "doc.text")
          }
          .disabled(model.loadingRecording)
          if model.loadingRecording { ProgressView("Loading recording…").controlSize(.small) }
          Button {
            showInspector.toggle()
          } label: {
            Label("Inspector", systemImage: "sidebar.right")
          }
          .keyboardShortcut("i", modifiers: [.command, .option])
          .help("Show or hide observation details")
        }
      }
    }
    .frame(minWidth: 1020, minHeight: 600)
    .sheet(isPresented: $model.showDevices) { DevicesView(model: model) }
    .alert(
      "AirHound",
      isPresented: Binding(get: { model.error != nil }, set: { if !$0 { model.error = nil } })
    ) {
      Button("OK") { model.error = nil }
    } message: {
      Text(model.error ?? "")
    }
    .onChange(of: model.recordingRevision) { _, _ in
      receiver = "all"
      receiveUpdates()
    }
    .onChange(of: model.recording?.revision, initial: true) { _, _ in receiveUpdates() }
    .onChange(of: receiver) { _, _ in refreshRows() }
    .onChange(of: medium) { _, _ in refreshRows() }
    .onChange(of: matchesOnly) { _, _ in refreshRows() }
    .onChange(of: search) { _, _ in refreshRows() }
    .onChange(of: sortOrder) { _, _ in refreshRows() }
  }
  private func receiveUpdates() {
    if presentation.receive(
      model.recording, label: model.recordingLabel, context: model.recordingRevision)
    {
      receiverIDs = Array(Set(observations.map(\.receiver))).sorted()
      refreshRows()
    }
  }
  private func resumeUpdates() {
    presentation.resume(
      model.recording, label: model.recordingLabel, context: model.recordingRevision)
    receiverIDs = Array(Set(observations.map(\.receiver))).sorted()
    refreshRows()
  }
  private func refreshRows() {
    filtered = observations.filter { row in
      (receiver == nil || receiver == "all" || row.receiver == receiver)
        && (medium == "All types" || row.medium == medium) && (!matchesOnly || row.match == "Match")
        && (search.isEmpty
          || [row.name, row.address, row.receiver, row.medium].contains {
            $0.localizedCaseInsensitiveContains(search)
          })
    }.sorted(using: sortOrder.map(\.comparator))
    tableRevision = UUID()
    if let selection = presentation.selection, !filtered.contains(where: { $0.id == selection }) {
      presentation.selection = nil
    }
  }
  private func receiverLabel(_ name: String, id: String, icon: String, assignment: String)
    -> some View
  {
    VStack(alignment: .leading, spacing: 4) {
      Label(name, systemImage: icon)
      Text("Recorded source").font(.caption).foregroundStyle(.secondary)
      Text(assignment).font(.caption).foregroundStyle(.secondary)
    }.padding(.vertical, 4).tag(id)
  }
}

struct InspectorView: View {
  let observation: Observation?
  var body: some View {
    if let row = observation {
      ScrollView {
        VStack(alignment: .leading, spacing: 16) {
          Text("Details").font(.headline)
          Text(row.name).font(.title3).textSelection(.enabled).fixedSize(
            horizontal: false, vertical: true)
          Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 10) {
            field("Protocol", row.medium)
            field("Receiver", row.receiver)
            field("Address / ID", row.address)
            field("Signal", row.signalDescription)
            field("Channel", row.channel)
            field("Frequency", row.frequency)
            field("Band", row.band)
            field("PHY", row.phy)
            field("Match", row.match == "—" ? "No match reported" : row.match)
            field("Alert", row.alert)
          }
          Divider()
          DisclosureGroup("Signature evidence") {
            VStack(alignment: .leading, spacing: 8) {
              if row.evidence.isEmpty { Text("No signature evidence attached.") }
              ForEach(row.evidence, id: \.self) { Text($0).textSelection(.enabled) }
            }.font(.callout).frame(maxWidth: .infinity, alignment: .leading).padding(.top, 8)
          }
          DisclosureGroup("Original record") {
            Text(row.raw).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
              .frame(maxWidth: .infinity, alignment: .leading).padding(.top, 8)
          }
        }.padding(16)
      }.id(row.id)
    } else {
      ContentUnavailableView(
        "Select an observation", systemImage: "sidebar.right",
        description: Text("View its source, signature evidence and original record."))
    }
  }
  private func field(_ name: String, _ value: String) -> some View {
    GridRow(alignment: .top) {
      Text(name).foregroundStyle(.secondary)
      Text(value).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
    }.font(.callout)
  }
}

struct RecordNavigation: View {
  @ObservedObject var session: SessionModel
  let held: Bool
  let hold: () -> Void
  let resume: () -> Void
  var body: some View {
    if session.browsingRun != nil {
      VStack(alignment: .leading, spacing: 6) {
        HStack {
          Text("Stored recording").font(.headline)
          Toggle(
            "Follow live",
            isOn: Binding(
              get: { session.followLive && !held },
              set: { follow in
                session.followLive = follow
                if follow { resume() } else { hold() }
              })
          ).toggleStyle(.checkbox)
          Spacer()
          Button("From Beginning") {
            resume()
            session.rewindRecords()
          }.disabled(!session.canSubmitAction)
          Button(session.hasMoreRecords ? "Load Next Page" : "Check for New Records") {
            resume()
            session.loadMoreRecords()
          }.disabled(!session.canSubmitAction)
        }
        if held {
          Text("View held for inspection. Holding this view does not pause capture.")
            .font(.caption).foregroundStyle(.secondary)
        }
      }.padding(12)
      Divider()
    }
  }
}
