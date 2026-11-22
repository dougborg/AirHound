import AirHoundCore
import SwiftUI

struct LocationSettings: View {
  @ObservedObject var session: SessionModel
  var body: some View {
    Section("Location") {
      Toggle(
        "Use a GPSD location provider",
        isOn: Binding(
          get: { session.planLocation != nil && session.planLocation != .null },
          set: { enabled in
            session.setLocation(
              enabled
                ? .object(["kind": .string("gpsd"), "endpoint": .string("127.0.0.1:2947")]) : nil)
          })
      )
      .disabled(
        !session.canEditPlan
          || (session.planLocation == nil || session.planLocation == .null)
            && !session.locationProviders.contains("gpsd")
      )
      if let location = session.planLocation, location != .null {
        if ["gpsd", "managed_gpsd"].contains(location["kind"]?.string ?? "") {
          let managed = location["kind"]?.string == "managed_gpsd"
          Toggle(
            "Start GPSD with this recording",
            isOn: Binding(
              get: { managed }, set: { session.setManagedLocation($0) })
          )
          .disabled(
            !session.canEditPlan
              || (!managed && !session.locationProviders.contains("managed_gpsd")))
          if managed {
            HStack {
              TextField(
                "GPSD executable",
                text: Binding(
                  get: { location["executable"]?.string ?? "" },
                  set: { session.setLocationField("executable", value: .string($0)) }))
              Button("Choose…") {
                let panel = NSOpenPanel()
                panel.title = "Choose installed gpsd executable"
                panel.canChooseDirectories = false
                panel.canChooseFiles = true
                panel.directoryURL = URL(fileURLWithPath: "/opt/homebrew/sbin")
                if panel.runModal() == .OK, let url = panel.url {
                  session.setLocationField("executable", value: .string(url.path))
                }
              }
            }.disabled(!session.canEditPlan)
          }
          TextField(
            "GPSD server (IP address:port)",
            text: Binding(
              get: { location["endpoint"]?.string ?? "" },
              set: { session.setLocationField("endpoint", value: .string($0)) })
          )
          .disabled(!session.canEditPlan)
          TextField(
            managed ? "GPS device path" : "Device path (optional)",
            text: Binding(
              get: { location["device"]?.string ?? "" },
              set: { session.setLocationField("device", value: $0.isEmpty ? .null : .string($0)) })
          )
          .disabled(!session.canEditPlan)
          Text(
            managed
              ? "AirHound starts the selected gpsd in read-only mode when recording begins and stops it when recording finishes. The local port must be unused."
              : "Connect to an existing gpsd server. Choose a GPS under Available devices to fill its device path."
          )
          .font(.caption).foregroundStyle(.secondary)
        } else {
          Text("Use Advanced JSON to edit this location provider.").font(.caption)
        }
      }
      if session.connected && !session.locationProviders.contains("gpsd") {
        Text("This backend does not advertise GPSD support.").font(.caption).foregroundStyle(
          .secondary)
      }
      if let status = session.capture?["receiver_health"]?["location"] {
        Text(
          "GPS: \(status["state"]?.string?.replacingOccurrences(of: "_", with: " ") ?? "unknown")"
        )
        .font(.caption).foregroundStyle(.secondary)
      }
    }
  }
}
