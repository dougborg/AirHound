import Foundation

@main struct Bench {
  static func ms(_ operation: () throws -> Void) rethrows -> Double {
    let start = DispatchTime.now().uptimeNanoseconds
    try operation()
    return Double(DispatchTime.now().uptimeNanoseconds - start) / 1_000_000
  }
  static func main() throws {
    let values: [[String: Any]] = (1...1000).map { i in
      [
        "cursor": i, "source_id": "cat", "kind": "ble_packet_observation",
        "received_at_us": 1_795_335_411_990_197,
        "payload": [
          "type": "ble_packet_observation", "name": "Sensor \(1000-i)",
          "address": "02:00:00:00:00:01", "rssi": -42, "channel": 37,
          "session_source": ["receiver_id": "cat"],
          "detection": [
            "sig": [12], "rules": [["id": 3, "basis": "manufacturer"]], "alert": "silent",
          ], "payload_hex": String(repeating: "00", count: 64),
        ],
      ]
    }
    let page = try JSONDecoder().decode(
      RecordPage.self,
      from: JSONSerialization.data(withJSONObject: [
        "records": values, "next_after": 1000, "has_more": true,
      ]))
    var window = RecordWindow()
    let initialFill = try ms { try window.append(page) }
    var sum = 0
    let rebuild = try ms { for _ in 0..<30 { sum += try window.observations().observations.count } }
    let rows = try window.observations().observations
    let order = [KeyPathComparator(\Observation.name)]
    let render = ms {
      for _ in 0..<30 {
        for _ in 0..<5 { sum += rows.filter { !$0.name.isEmpty }.sorted(using: order).count }
      }
    }
    print("initial_fill_ms=\(initialFill)")
    print("window_read_mean_ms=\(rebuild/30) five_filter_sort_mean_ms=\(render/30) checksum=\(sum)")
  }
}
