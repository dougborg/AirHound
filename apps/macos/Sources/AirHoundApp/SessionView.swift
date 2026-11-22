import AirHoundCore
import SwiftUI

struct SessionView: View {
  @ObservedObject var session: SessionModel
  @ObservedObject var app: AppModel
  @State private var advanced = false
  @State private var confirmFinish = false
  var body: some View {
    HSplitView {
      ScrollView {
        Form {
          Section("Service") {
            LabeledContent("Connection", value: session.connected ? "Connected" : "Disconnected")
            HStack {
              Button("Connect") { session.connect(executable: app.executable) }
              Button("Start Local Service") { session.startService(executable: app.executable) }
              Button("Choose Backend…") { app.chooseBackend() }
            }.disabled(session.busy)
            DisclosureGroup("Connection settings") {
              TextField("Socket", text: $session.socketPath).disabled(
                session.connected || session.busy)
              if session.connected {
                Button("Disconnect Client") { session.disconnect() }.disabled(session.busy)
              }
              Text(app.executable?.path ?? "No backend selected").font(.caption).textSelection(
                .enabled)
            }
            Text(session.message).foregroundStyle(.secondary)
          }
          Section("Receivers") {
            if let inventory = session.inventory {
              ForEach(inventory.devices) { device in
                ForEach(Array(device.candidates.enumerated()), id: \.offset) { index, candidate in
                  let key = device.id + ":" + String(index)
                  Toggle(
                    isOn: Binding(
                      get: { session.selected.contains(key) },
                      set: {
                        if $0 { session.selected.insert(key) } else { session.selected.remove(key) }
                        session.useSelectedReceivers()
                      })
                  ) {
                    VStack(alignment: .leading) {
                      Text(candidate.label)
                      Text(
                        "\(device.name) · \(candidate.state.replacingOccurrences(of: "_", with: " "))"
                      )
                      .font(.caption).foregroundStyle(.secondary)
                    }
                  }.disabled(candidate.configurationHint == nil || session.isActive || session.busy)
                  if session.selected.contains(key), let source = candidate.configurationHint,
                    ["sniffle", "external_wifi"].contains(candidate.kind)
                  {
                    ReceiverSettings(
                      session: session, key: key, kind: candidate.kind, source: source
                    )
                    .disabled(session.isActive || session.busy)
                  }
                }
              }
              ForEach(Array(inventory.diagnostics.enumerated()), id: \.offset) { _, diagnostic in
                Text(diagnostic.message).font(.caption).foregroundStyle(.secondary)
              }
            } else {
              Text("Connect to discover attached receivers.").foregroundStyle(.secondary)
            }
          }
          Section("Scan plan") {
            Text(
              "Select receivers and configure any external helpers. Validation checks the plan without opening radios."
            )
            .font(.callout).foregroundStyle(.secondary)
            DisclosureGroup("Advanced receiver and channel settings", isExpanded: $advanced) {
              TextEditor(text: $session.configText).font(.system(.body, design: .monospaced))
                .frame(minHeight: 180).accessibilityLabel("Session configuration JSON")
                .disabled(session.isActive)
              Button("Import Plan…") { importPlan() }
            }
            HStack {
              Button("Validate Plan") { session.validate() }
              Button(
                session.hasPendingStart ? "Resolve Pending Start" : "Start Recording",
                systemImage: "record.circle"
              ) { session.start() }
              .disabled(session.isActive && !session.hasPendingStart)
            }.disabled(
              !session.connected || !session.captureEnabled || session.busy
                || session.configText.isEmpty)
            if let validation = session.validation {
              DisclosureGroup("Validation details") {
                Text(validation).font(.system(.caption, design: .monospaced)).textSelection(
                  .enabled)
              }
            }
          }
        }.formStyle(.grouped)
      }.frame(minWidth: 390, idealWidth: 490)
      VStack(alignment: .leading, spacing: 16) {
        HStack {
          Text("Recording").font(.title2)
          Spacer()
          Text(session.state.capitalized).foregroundStyle(.secondary)
          if session.busy { ProgressView().controlSize(.small) }
        }
        HStack {
          Button(
            session.state == "paused" ? "Resume" : "Pause",
            systemImage: session.state == "paused" ? "play.fill" : "pause.fill"
          ) {
            session.control(session.state == "paused" ? .resume : .pause)
          }
          Button("Finish Recording", systemImage: "stop.fill") { confirmFinish = true }
        }.disabled(!session.connected || !session.isActive || session.busy)
        Text(
          "Closing this window leaves capture running. Finish Recording stops and saves the run."
        )
        .font(.callout).foregroundStyle(.secondary)
        if let failure = session.captureFailure {
          Label(failure, systemImage: "exclamationmark.triangle").foregroundStyle(.red)
            .textSelection(.enabled)
        }
        ForEach(session.receiverStatuses) { receiver in
          VStack(alignment: .leading, spacing: 4) {
            Text(receiver.id).font(.headline)
            Text(receiver.phases)
            Text(receiver.detail).font(.caption).foregroundStyle(.secondary)
            Text(
              "Recovery attempts: \(receiver.recovery) · Host drops: \(receiver.hostDrops) · Producer drops: \(receiver.producerDrops)"
            )
            .font(.caption).monospacedDigit()
          }
        }
        if session.healthCatchingUp {
          Text("Catching up on receiver metrics; counters may be behind capture.").font(.caption)
            .foregroundStyle(.secondary)
        }
        Text("Radio loss is unknown unless the receiver reports it.").font(.caption)
          .foregroundStyle(.secondary)
        DisclosureGroup("Raw receiver status") {
          ScrollView {
            Text(session.healthText).font(.system(.caption, design: .monospaced))
              .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
          }.frame(maxHeight: 200)
        }
        Divider()
        HStack {
          Text("Recordings").font(.headline)
          Spacer()
          Button("Refresh") { session.refreshHistory() }
          Button("Export Archive…") { session.export() }
        }.disabled(!session.connected || session.busy)
        List(session.runs) { run in
          HStack {
            VStack(alignment: .leading, spacing: 4) {
              Text(run.startedAt, style: .date) + Text(" at ") + Text(run.startedAt, style: .time)
              Text("\(run.state.capitalized) · \(run.recordCount) records")
                .font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            Button("Open") { session.open(run, app: app) }
              .disabled(session.busy)
          }.padding(.vertical, 4)
        }.overlay {
          if session.runs.isEmpty {
            ContentUnavailableView(
              "No recordings", systemImage: "waveform",
              description: Text("Saved recordings appear here after the service is connected."))
          }
        }
        if session.nextRun != nil {
          Button("Older Recordings") { session.refreshHistory(more: true) }.disabled(session.busy)
        }
      }.padding(20).frame(minWidth: 430)
    }
    .confirmationDialog("Finish this recording?", isPresented: $confirmFinish) {
      Button("Finish Recording") { session.control(.finish) }
    } message: {
      Text("The service will stop the receivers and save the remaining observations.")
    }
    .alert(
      "AirHound Service",
      isPresented: Binding(get: { session.error != nil }, set: { if !$0 { session.error = nil } })
    ) {
      Button("OK") { session.error = nil }
    } message: {
      Text(session.error ?? "")
    }
  }
  private func importPlan() {
    let panel = NSOpenPanel()
    panel.title = "Import session plan"
    panel.canChooseDirectories = false
    guard panel.runModal() == .OK, let url = panel.url else { return }
    do {
      let data = try LocalFiles.read(url, limit: 65536)
      guard data.count <= 65536 else {
        throw ModelError.invalid("Session plans must be at most 64 KiB.")
      }
      session.configText = String(decoding: data, as: UTF8.self)
      session.validation = nil
      advanced = true
    } catch { session.error = error.localizedDescription }
  }
}
