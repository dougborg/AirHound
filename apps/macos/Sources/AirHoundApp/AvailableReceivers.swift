import AirHoundCore
import SwiftUI

private struct SerialSetup: Identifiable {
  let id: String
  let ports: [String]
}

struct AvailableReceivers: View {
  @ObservedObject var session: SessionModel
  @State private var serialSetup: SerialSetup?
  @State private var port = ""
  @State private var baud: Int64 = 115200
  var body: some View {
    DisclosureGroup("Available devices") {
      Button("Add Rayhunter / Orbic (experimental)") {
        session.addReceiver(
          source: .object([
            "kind": .string("rayhunter"), "endpoint": .string(""), "decoder": .string(""),
          ]))
      }.disabled(!session.canEditPlan || !session.sessionSources.contains("rayhunter"))
      Text("Requires a prepared local raw-feed forward and the separate Rayhunter decoder.").font(
        .caption
      ).foregroundStyle(.secondary)
      if let inventory = session.inventory {
        ForEach(inventory.receiverChoices) { choice in
          let device = choice.device
          let candidate = choice.candidate
          DisclosureGroup {
            if GPSVersionTarget.ports(for: choice).isEmpty {
              if let identity = candidate.identification {
                Text(
                  "\(identity["application"]?.string ?? "Device") \(identity["firmware_version"]?.string ?? "version unreported")"
                )
                .font(.callout)
                if let board = identity["board"]?.string {
                  Text(board).font(.caption).foregroundStyle(.secondary)
                }
              } else {
                Text("Firmware / software version not reported.").font(.caption).foregroundStyle(
                  .secondary)
              }
            }
            GPSVersionAction(session: session, choice: choice)
            Text(candidate.evidence.joined(separator: "\n")).font(.caption)
              .foregroundStyle(.secondary).textSelection(.enabled)
            ForEach(Array(candidate.requirements.enumerated()), id: \.offset) { _, requirement in
              Text(requirement.detail).font(.caption).textSelection(.enabled)
            }
            if let issue = candidate.supportIssue, ["https", "http"].contains(issue.scheme) {
              Link("Track device support", destination: issue)
            }
            if let location = candidate.locationConfigurationHint {
              Button("Use for Location") { session.selectLocationDevice(location) }
                .disabled(!session.canEditPlan || !session.locationProviders.contains("gpsd"))
            }
            if let source = candidate.configurationHint {
              Button("Add Receiver") { session.addReceiver(source: source) }
                .disabled(
                  !session.canEditPlan
                    || (source["kind"] == .string("flipper_marauder")
                      && !session.canAddFlipperBridge)
                )
            } else if candidate.kind == "serial_unknown" {
              let ports = device.endpoints.filter { $0.kind == "serial" }.map(\.path)
              if !ports.isEmpty {
                Button("Use AirHound Firmware…") {
                  port = ports.first(where: { $0.hasPrefix("/dev/cu.") }) ?? ports[0]
                  baud = 115200
                  serialSetup = SerialSetup(id: device.id, ports: ports)
                }.disabled(!session.canEditPlan)
              }
            }
          } label: {
            VStack(alignment: .leading, spacing: 3) {
              Text(candidate.label)
              Text("\(device.name) · \(candidate.statusDescription)")
                .font(.caption).foregroundStyle(.secondary)
            }
          }
        }
        ForEach(Array(inventory.diagnostics.enumerated()), id: \.offset) { _, diagnostic in
          Text(diagnostic.message).font(.caption).foregroundStyle(.secondary)
        }
        Button("Refresh Devices") { session.refreshDevices() }
          .disabled(!session.canSubmitAction)
        Text("Discovery lists candidates. Adding one does not verify reception or open its radio.")
          .font(.caption).foregroundStyle(.secondary)
      } else {
        Text("Connect to discover attached devices.").foregroundStyle(.secondary)
      }
    }
    .sheet(item: $serialSetup) { setup in
      VStack(alignment: .leading, spacing: 16) {
        Text("AirHound Sensor").font(.title2)
        Text("Use this connection only if the attached board runs AirHound firmware.")
        Picker("Serial port", selection: $port) {
          ForEach(setup.ports, id: \.self) { Text($0).tag($0) }
        }
        Picker("Baud rate", selection: $baud) {
          Text("115200").tag(Int64(115200))
          Text("460800 · M5 no_std firmware").tag(Int64(460800))
          Text("921600").tag(Int64(921600))
        }
        Text("Starting capture opens the port and may reset the board. Adding it here does not.")
          .font(.caption).foregroundStyle(.secondary)
        HStack {
          Spacer()
          Button("Cancel") { serialSetup = nil }.keyboardShortcut(.cancelAction)
          Button("Add Sensor") {
            session.addReceiver(
              source: .object([
                "kind": .string("airhound_serial"), "port": .string(port), "baud": .integer(baud),
              ]))
            serialSetup = nil
          }.keyboardShortcut(.defaultAction).disabled(!session.canEditPlan)
        }
      }.padding(24).frame(width: 500)
    }
  }
}
