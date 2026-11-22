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
      .disabled(!session.canEditPlan || !session.locationProviders.contains("gpsd"))
      if let location = session.planLocation, location != .null {
        if location["kind"]?.string == "gpsd" {
          TextField(
            "GPSD server (host:port)",
            text: Binding(
              get: { location["endpoint"]?.string ?? "" },
              set: { session.setLocationField("endpoint", value: .string($0)) })
          )
          .disabled(!session.canEditPlan)
          TextField(
            "Device path (optional)",
            text: Binding(
              get: { location["device"]?.string ?? "" },
              set: { session.setLocationField("device", value: $0.isEmpty ? .null : .string($0)) })
          )
          .disabled(!session.canEditPlan)
          Text(
            "Connect to an existing gpsd server. Choose a GPS under Available devices to fill its device path."
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
