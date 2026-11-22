import Foundation

public struct ReceiverStatus: Identifiable {
  public let id: String
  public let phases: String
  public let detail: String
  public let recovery: String
  public let hostDrops: String
  public let producerDrops: String

  public static func decode(_ capture: JSONValue?, sourceEvents: [String: JSONValue]) -> [Self] {
    guard case .array(let receivers) = capture?["receiver_health"]?["receivers"] else { return [] }
    return receivers.compactMap { receiver in
      guard let id = receiver["id"]?.string else { return nil }
      let scanners = ["wifi", "ble"].compactMap { name -> String? in
        guard let state = receiver["scanners"]?[name]?["state"]?.string, state != "disabled" else {
          return nil
        }
        return
          "\(name == "wifi" ? "Wi-Fi" : "Bluetooth"): \(state.replacingOccurrences(of: "_", with: " "))"
      }
      let reasons = ["wifi", "ble"].compactMap { receiver["scanners"]?[$0]?["reason"]?.string }
      let event = sourceEvents[id]
      return Self(
        id: id, phases: scanners.joined(separator: " · "),
        detail: reasons.joined(separator: " · ").replacingOccurrences(of: "_", with: " "),
        recovery: metric(event?["reconnect_attempts"]),
        hostDrops: metric(event?["host_dropped_observations"] ?? event?["host_dropped_packets"]),
        producerDrops: metric(event?["producer_dropped_observations"]))
    }
  }
  private static func metric(_ value: JSONValue?) -> String {
    switch value {
    case .integer(let v): return String(v)
    case .unsigned(let v): return String(v)
    default: return "Not reported"
    }
  }
}
