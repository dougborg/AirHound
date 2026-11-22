import AirHoundCore
import SwiftUI

struct SessionView: View {
  @ObservedObject var session: SessionModel
  @ObservedObject var app: AppModel
  @State private var advanced = false
  @State private var confirmFinish = false
  @State private var finishTarget: SessionModel.ControlTarget?
  var body: some View {
    VStack(spacing: 0) {
      captureControls
      Divider()
      HSplitView {
        Form {
          SavedSetupView(session: session)
          Section("Receivers") {
            ForEach(session.plannedReceivers) { receiver in
              DisclosureGroup {
                if let identity = session.currentReceiverIdentity(id: receiver.id) {
                  Text(identity["adapter_info"]?.string ?? "Identity unreported").font(.caption)
                    .foregroundStyle(.secondary)
                  if let companion = identity["external_identity"], companion != .null {
                    CompanionIdentityDetails(identity: companion)
                  }
                  if let bridge = identity["bridge_identity"], bridge != .null {
                    FlipperBridgeIdentityDetails(identity: bridge)
                  }
                  if let producer = identity["producer_identity"] {
                    Text(
                      "\(producer["application"]?.string ?? "Firmware") \(producer["version"]?.string ?? "version unreported") · \(producer["board"]?.string ?? "board unreported")"
                    )
                    .font(.caption).textSelection(.enabled)
                  }
                }
                ReceiverSettings(session: session, receiver: receiver)
                  .disabled(!session.canEditPlan)
                HStack {
                  Text(receiver.id).font(.caption).foregroundStyle(.secondary).textSelection(
                    .enabled)
                  Spacer()
                  Button("Remove", role: .destructive) { session.removeReceiver(id: receiver.id) }
                    .disabled(!session.canEditPlan)
                }
              } label: {
                VStack(alignment: .leading, spacing: 3) {
                  Text(receiver.title)
                  Text(receiver.endpoint).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
              }
            }
            if session.plannedReceivers.isEmpty {
              Text(
                session.configText.isEmpty
                  ? "Add receivers to build a scan plan."
                  : "No receivers to display. Add a device or check Advanced JSON."
              )
              .foregroundStyle(.secondary)
            }
            AvailableReceivers(session: session)
            if session.connected && session.sessionSources.contains("flipper_marauder") {
              FlipperBridgeSetup(session: session)
            }
            if session.connected && session.sessionSources.contains("marauder_capture") {
              Menu("Add Saved Marauder Capture") {
                Button("PCAP…") { addMarauderCapture(format: "pcap") }
                Button("Serial capture…") { addMarauderCapture(format: "serial") }
              }.disabled(!session.canAddMarauderCapture)
            }
          }
          LocationSettings(session: session)
          Section("Scan plan") {
            Text(
              "Expand a receiver to configure it. Validate the plan before starting."
            )
            .font(.callout).foregroundStyle(.secondary)
            DisclosureGroup("Advanced JSON", isExpanded: $advanced) {
              TextEditor(text: $session.configText).font(.system(.body, design: .monospaced))
                .frame(minHeight: 180).accessibilityLabel("Session configuration JSON")
                .disabled(!session.canEditPlan)
              Button("Import Plan…") { importPlan() }.disabled(!session.canEditPlan)
            }
            if let validation = session.validation {
              DisclosureGroup("Validation details") {
                Text(validation).font(.system(.caption, design: .monospaced)).textSelection(
                  .enabled)
              }
            }
          }
        }.formStyle(.grouped).frame(minWidth: 390, idealWidth: 490)
        VSplitView {
          ScrollView {
            VStack(alignment: .leading, spacing: 16) {
              Text(session.receiverStatusHeading).font(.title2)
              if !session.receiverStatusIsCurrent {
                Text(
                  session.connected
                    ? "Historical receiver reports from the last recording. No recording is running."
                    : "Last reported receiver status. Connect to check the current recording."
                ).font(.callout).foregroundStyle(.secondary)
              }
              if let failure = session.captureFailure {
                Label(failure, systemImage: "exclamationmark.triangle").foregroundStyle(.red)
                  .textSelection(.enabled)
              }
              ForEach(session.receiverStatuses) { receiver in
                VStack(alignment: .leading, spacing: 4) {
                  Text(receiver.id).font(.headline)
                  Text(
                    session.receiverStatusIsCurrent
                      ? receiver.phases : "Last reported: \(receiver.phases)")
                  Text(receiver.detail).font(.caption).foregroundStyle(.secondary)
                  Text(
                    "Recovery attempts: \(receiver.recovery) · Host drops: \(receiver.hostDrops) · Producer drops: \(receiver.producerDrops)"
                  )
                  .font(.caption).monospacedDigit()
                }
              }
              if session.isActive, let location = session.capture?["receiver_health"]?["location"],
                location != .null
              {
                LocationProviderStatus(status: location, connected: session.connected)
              }
              if session.healthCatchingUp {
                Text(
                  session.receiverStatusIsCurrent
                    ? "Catching up on receiver metrics; counters may be behind capture."
                    : "Loading historical receiver reports…"
                ).font(.caption)
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
            }.frame(maxWidth: .infinity, alignment: .leading).padding(20)
          }.frame(minHeight: 120, idealHeight: 280)
          VStack(alignment: .leading, spacing: 16) {
            HStack {
              Text("Recordings").font(.headline)
              Spacer()
              Button("Refresh") { session.refreshHistory() }
              Button("Export Archive…") { session.export() }
            }.disabled(!session.canSubmitAction)
            List(session.runs) { run in
              HStack {
                VStack(alignment: .leading, spacing: 4) {
                  Text(run.startedAt, style: .date) + Text(" at ")
                    + Text(run.startedAt, style: .time)
                  Text("\(run.state.capitalized) · \(run.recordCount) records")
                    .font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Button("Open") { session.open(run, app: app) }
                  .disabled(!session.canSubmitAction)
              }.padding(.vertical, 4)
            }.overlay {
              if session.runs.isEmpty {
                ContentUnavailableView(
                  "No recordings", systemImage: "waveform",
                  description: Text(
                    session.connected
                      ? "Start a recording to save observations here."
                      : "Connect to view saved recordings."))
              }
            }
            if session.nextRun != nil {
              Button("Older Recordings") { session.refreshHistory(more: true) }.disabled(
                !session.canSubmitAction)
            }
          }.padding(20).frame(minHeight: 180)
        }.frame(minWidth: 430)
      }
    }
    .confirmationDialog(
      "Finish this recording?", isPresented: $confirmFinish, presenting: finishTarget
    ) { target in
      Button("Finish Recording") { session.control(.finish, target: target) }
    } message: { _ in
      Text(
        "The service will close its receivers and save the remaining observations. Standalone devices may continue scanning."
      )
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
  private var captureControls: some View {
    VStack(alignment: .leading, spacing: 6) {
      HStack {
        Text(session.recordingStateLabel).font(.headline)
        if session.isPerformingUserAction {
          ProgressView().controlSize(.small).accessibilityLabel("Working")
        }
        Spacer()
        Button("Validate Plan") { session.validate() }
          .disabled(
            !session.canSubmitAction || !session.captureEnabled || session.configText.isEmpty)
        Button(
          session.hasPendingStart ? "Resolve Pending Start" : "Start Recording",
          systemImage: "record.circle"
        ) { session.start() }
        .disabled(session.startUnavailableReason != nil)
        Button(
          session.state == "paused" ? "Resume" : "Pause",
          systemImage: session.state == "paused" ? "play.fill" : "pause.fill"
        ) { session.control(session.state == "paused" ? .resume : .pause) }
        .disabled(!session.canSubmitAction || !session.isActive)
        Button("Finish Recording", systemImage: "stop.fill") {
          finishTarget = session.controlTarget()
          confirmFinish = finishTarget != nil
        }.disabled(!session.canSubmitAction || !session.isActive)
      }
      if !session.isActive, let reason = session.startUnavailableReason {
        Text(reason).font(.callout).foregroundStyle(.secondary)
      } else if session.isActive {
        Text(
          "Closing this window leaves capture running. Finish Recording saves and closes the host run."
        )
        .font(.callout).foregroundStyle(.secondary)
      }
    }.padding(.horizontal, 20).padding(.vertical, 12)
      .accessibilityIdentifier("capture-controls")
  }
  private func addMarauderCapture(format: String) {
    let panel = NSOpenPanel()
    panel.title = format == "pcap" ? "Choose Marauder PCAP" : "Choose saved Marauder serial capture"
    panel.message = "Adds a saved capture to the plan. No device connection or recording starts."
    panel.canChooseDirectories = false
    panel.canChooseFiles = true
    panel.allowsMultipleSelection = false
    guard panel.runModal() == .OK, let url = panel.url else { return }
    session.addMarauderCapture(url: url, format: format)
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
      session.appliedSetupName = nil
      session.selectedSetupID = ""
      session.validation = nil
      advanced = true
    } catch { session.error = error.localizedDescription }
  }
}
