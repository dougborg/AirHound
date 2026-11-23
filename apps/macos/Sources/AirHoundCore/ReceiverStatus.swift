import Foundation

public struct ReceiverStatus: Identifiable {
  public let id: String
  public let phases: String
  public let detail: String
  public let recovery: String
  public let hostDrops: String
  public let producerDrops: String
  public let metricsDescription: String

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
      // Flipper health carries the latest backend-owned cumulative counters,
      // even when its bounded observation queue cannot publish a status event.
      // Do not combine this snapshot with a cached event from an older generation.
      let isFlipper = receiver["receiver"]?["backend"]?.string == "flipper_marauder"
      let event = isFlipper ? receiver["receiver"]?["bridge_status"] : sourceEvents[id]
      let hostDrops = metric(event?["host_dropped_observations"] ?? event?["host_dropped_packets"])
      let recovery = metric(event?["reconnect_attempts"])
      let producerDrops = metric(event?["producer_dropped_observations"])
      let metrics: String
      if isFlipper {
        let producer = event?["producer_last_report"]
        metrics =
          "Host dropped observations: \(hostDrops)"
          + " · UART dropped bytes: \(metric(producer?["uart_dropped_bytes"]))"
          + " · UART errors: \(metric(producer?["uart_error_events"]))"
          + " · RPC dropped messages: \(metric(producer?["rpc_dropped_messages"]))"
          + " · RF loss: Unknown"
      } else {
        metrics =
          "Recovery attempts: \(recovery) · Host drops: \(hostDrops) · Producer drops: \(producerDrops)"
      }
      return Self(
        id: id, phases: scanners.joined(separator: " · "),
        detail: reasons.joined(separator: " · ").replacingOccurrences(of: "_", with: " "),
        recovery: recovery, hostDrops: hostDrops, producerDrops: producerDrops,
        metricsDescription: metrics)
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
