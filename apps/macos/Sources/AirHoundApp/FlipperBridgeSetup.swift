import AirHoundCore
import SwiftUI

struct FlipperBridgeRequirements: View {
  var body: some View {
    VStack(alignment: .leading, spacing: 6) {
      Text("Prototype · passive Wi-Fi beacons").fontWeight(.medium)
      Text("Requires Flipper firmware API 87.1 and AirHound Live Bridge installed at:")
      Text("/ext/apps/GPIO/airhound_bridge.fap")
        .font(.system(.caption, design: .monospaced)).textSelection(.enabled)
      Text(
        "Starting checks the bridge identity. Adding this receiver does not install software or open its radio."
      )
      Text(
        "Pause and Finish request a radio stop; check the receiver's reported result. Packets missed over the air remain unknown."
      )
    }.font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
  }
}

struct FlipperBridgeSetup: View {
  @ObservedObject var session: SessionModel
  @State private var showingSetup = false
  @State private var port = ""
  var body: some View {
    Button("Add Flipper / AWOK…") {
      port = ""
      showingSetup = true
    }.disabled(!session.canAddFlipperBridge)
      .sheet(isPresented: $showingSetup) {
        VStack(alignment: .leading, spacing: 16) {
          Text("Flipper / AWOK").font(.title2)
          TextField("Flipper USB serial port", text: $port)
          if let inventory = session.inventory {
            let devices = inventory.devices.filter {
              $0.candidates.contains { $0.kind == "flipper" }
                && $0.endpoints.contains { $0.kind == "serial" }
            }
            if !devices.isEmpty {
              Menu("Choose a Flipper candidate port") {
                ForEach(devices) { device in
                  ForEach(device.endpoints.filter { $0.kind == "serial" }, id: \.path) { endpoint in
                    Button("\(device.name) · \(endpoint.path)") { port = endpoint.path }
                  }
                }
              }
            }
          }
          Text(
            "USB candidates do not verify the attached radio. Enter a port manually if its descriptor is not recognized."
          )
          .font(.caption).foregroundStyle(.secondary)
          FlipperBridgeRequirements()
          HStack {
            Spacer()
            Button("Cancel") { showingSetup = false }.keyboardShortcut(.cancelAction)
            Button("Add Receiver") {
              session.addFlipperBridge(port: port)
              showingSetup = false
            }.keyboardShortcut(.defaultAction)
              .disabled(
                !session.canAddFlipperBridge
                  || port.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
          }
        }.padding(24).frame(width: 500)
      }
  }
}

/// These labels keep the USB firmware report, producer build and radio firmware distinct.
struct FlipperBridgeIdentityPresentation {
  let identity: JSONValue
  var lines: [String] {
    var result: [String] = []
    if let firmware = identity["flipper_firmware"],
      firmware["evidence"]?.string == "usb_cli_device_info", let fields = firmware["fields"]
    {
      result.append(
        "Flipper firmware: \(fields["firmware_version"]?.string ?? "version unreported")")
      if let commit = fields["firmware_commit"]?.string {
        result.append("Flipper firmware source: \(commit)")
      }
    } else {
      result.append("Flipper firmware unreported")
    }
    if let app = identity["application"], app["evidence"]?.string == "ahfb_hello" {
      result.append("AirHound Live Bridge: \(app["version"]?.string ?? "version unreported")")
      if let revision = app["source_revision"]?.string, !revision.isEmpty {
        result.append("Bridge source: \(revision)")
      }
      switch app["source_modified"] {
      case .integer(0), .unsigned(0): result.append("Bridge source: no tracked changes reported")
      case .integer(1), .unsigned(1): result.append("Bridge source: local changes reported")
      default: result.append("Bridge source changes unreported")
      }
      if let abi = app["compiled_firmware_abi"]?.string {
        result.append("Bridge compiled firmware API: \(abi)")
      }
    } else {
      result.append("AirHound Live Bridge identity unreported")
    }
    result.append("AWOK radio firmware unverified")
    return result
  }
}

struct FlipperBridgeIdentityDetails: View {
  let identity: JSONValue
  var body: some View {
    DisclosureGroup("Flipper and bridge software") {
      VStack(alignment: .leading, spacing: 4) {
        ForEach(
          Array(FlipperBridgeIdentityPresentation(identity: identity).lines.enumerated()),
          id: \.offset
        ) { _, line in
          Text(line).fixedSize(horizontal: false, vertical: true)
        }
      }.textSelection(.enabled)
    }.font(.caption)
  }
}
