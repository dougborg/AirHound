import AirHoundCore
import SwiftUI

/// Presentation binding only. Rust revalidates the attachment and owns the query.
struct GPSVersionTarget: Hashable {
  let deviceID: String
  let port: String

  static func ports(for choice: ReceiverChoice) -> [String] {
    guard choice.candidate.kind == "gps", choice.candidate.identificationProtocol == "ublox"
    else { return [] }
    return Array(Set(choice.device.endpoints.filter { $0.kind == "serial" }.map(\.path))).sorted()
  }

  func isCurrent(in inventory: Inventory?) -> Bool {
    inventory?.receiverChoices.contains {
      $0.device.id == deviceID && Self.ports(for: $0).contains(port)
    } == true
  }

  var request: JSONValue {
    .object(["protocol": .string("ublox"), "device_id": .string(deviceID), "port": .string(port)])
  }

  func matches(_ result: JSONValue) -> Bool {
    result["schema_version"] == .integer(1) && result["device_id"] == .string(deviceID)
      && result["endpoint"] == .string(port)
      && result["provenance"] == .string("explicit_ublox_mon_ver")
  }
}

struct GPSVersionDetails {
  let value: JSONValue
  var observedAt: Date? {
    let milliseconds: UInt64
    switch value["observed_at_ms"] {
    case .integer(let number) where number > 0: milliseconds = UInt64(number)
    case .unsigned(let number) where number > 0: milliseconds = number
    default: return nil
    }
    // Avoid overflowing or presenting dates outside the supported calendar range.
    guard milliseconds <= 253_402_300_799_999 else { return nil }
    return Date(timeIntervalSince1970: Double(milliseconds) / 1_000)
  }

  var lines: [String] {
    var result = [String]()
    if value["state"] == .string("identified") {
      result.append("Queried version snapshot")
      if let observedAt {
        result.append("Last queried \(observedAt.formatted(date: .abbreviated, time: .standard))")
      } else {
        result.append("Query time not reported.")
      }
    }
    for (key, label) in [
      ("firmware_version", "GPS firmware"), ("software_version", "GPS software"),
      ("hardware_version", "Hardware version"), ("protocol_version", "UBX protocol"),
    ] {
      result.append("\(label): \(value[key]?.string ?? "not reported")")
    }
    result.append("Module model: \(value["model"]?.string ?? "not reported")")
    if let message = value["message"]?.string { result.append(message) }
    result.append("GPS reception has not been tested by this version query.")
    return result
  }
}

struct GPSVersionAction: View {
  @ObservedObject var session: SessionModel
  let choice: ReceiverChoice
  @State private var selectedPort = ""

  var body: some View {
    let ports = GPSVersionTarget.ports(for: choice)
    let port = ports.count == 1 ? ports[0] : selectedPort
    let target = GPSVersionTarget(deviceID: choice.device.id, port: port)
    if !ports.isEmpty {
      if ports.count > 1 {
        Picker("GPS connection", selection: $selectedPort) {
          Text("Choose a connection").tag("")
          ForEach(ports, id: \.self) { Text($0).tag($0) }
        }
      }
      Button("Read GPS version") { session.readGPSVersion(target) }
        .disabled(!session.canReadGPSVersion(target))
      Text(
        "When idle, asks this u-blox receiver for its version without changing its configuration. This is separate from the gpsd host software version."
      )
      .font(.caption).foregroundStyle(.secondary)
      if let result = session.gpsVersion(for: target) {
        ForEach(Array(GPSVersionDetails(value: result).lines.enumerated()), id: \.offset) {
          _, line in
          Text(line).font(.callout).textSelection(.enabled)
        }
      }
    }
  }
}
