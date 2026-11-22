import Foundation

public struct Observation: Identifiable {
  public let id: Int
  public let name: String
  public let medium: String
  public let receiver: String
  public let address: String
  public let rssi: Int?
  public var signal: String { rssi.map(String.init) ?? "Unknown" }
  public let channel: String
  public let frequency: String
  public let band: String
  public let phy: String
  public var signalSort: Int { rssi ?? Int.min }
  public let match: String
  public let alert: String
  public let evidence: [String]
  public let raw: String
}

public struct Recording {
  public let observations: [Observation]
  public let ignoredRecords: Int
  public static let byteLimit = 8 * 1024 * 1024
  public static let rowLimit = 10_000

  /// Reads an exported hostd stream, preserving unknown/evolving payload fields in raw JSON.
  /// Runtime events are not mistaken for observations or applied as current live state.
  public static func decode(_ data: Data) throws -> Self {
    guard data.count <= byteLimit else { throw ModelError.invalid("Recording exceeds 8 MiB") }
    guard let string = String(data: data, encoding: .utf8) else {
      throw ModelError.invalid("Recording is not UTF-8")
    }
    var rows: [Observation] = []
    var ignored = 0
    for (lineNumber, line) in string.split(separator: "\n").enumerated() {
      guard line.utf8.count <= 256 * 1024 else {
        throw ModelError.invalid("Record \(lineNumber + 1) exceeds 256 KiB")
      }
      guard let object = try JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any],
        let type = object["type"] as? String
      else { throw ModelError.invalid("Record \(lineNumber + 1) has no event type") }
      guard
        [
          "wifi_observation", "ble_observation", "external_wifi_observation",
          "ble_packet_observation",
        ].contains(type)
      else {
        ignored += 1
        continue
      }
      guard rows.count < rowLimit else {
        throw ModelError.invalid("Recording exceeds 10,000 observations")
      }
      let detection = object["detection"] as? [String: Any] ?? [:]
      let source = object["session_source"] as? [String: Any] ?? [:]
      // Real hostd detection uses sig/rules, while the illustrative fixture uses evidence.
      // Keep the complete detection object rather than reducing structured proof to labels.
      let evidence: [Any] = detection.isEmpty ? [] : [detection]
      let matched =
        detection["matched"] as? Bool == true || !(detection["sig"] as? [Any] ?? []).isEmpty
        || !(detection["rules"] as? [Any] ?? []).isEmpty
        || !(object["match"] as? [Any] ?? []).isEmpty
      let advertisement = object["advertisement"] as? [String: Any] ?? [:]
      let isExternal = type == "external_wifi_observation"
      let signal =
        isExternal
        ? (object["rssi_validated"] as? Bool == true ? object["rssi_dbm"] as? Int : nil)
        : (advertisement["rssi_available"] as? Bool == false ? nil : object["rssi"] as? Int)
      let externalName = (object["ssid_bytes"] as? [UInt8]).flatMap {
        String(bytes: $0, encoding: .utf8)
      }
      let pretty = try JSONSerialization.data(
        withJSONObject: object, options: [.prettyPrinted, .sortedKeys])
      rows.append(
        Observation(
          id: lineNumber,
          name: object["ssid"] as? String ?? object["name"] as? String ?? externalName
            ?? "Unnamed observation", medium: type.hasPrefix("ble_") ? "Bluetooth" : "Wi-Fi",
          receiver: source["receiver_id"] as? String ?? object["source_id"] as? String
            ?? "Not reported",
          address: object["mac"] as? String ?? object["address"] as? String ?? object["bssid"]
            as? String ?? object["peripheral_id"] as? String ?? "Not reported", rssi: signal,
          channel: (object["ch"] as? Int ?? object["receive_channel"] as? Int ?? object["channel"]
            as? Int).map(String.init) ?? "Not reported",
          frequency: (object["frequency_mhz"] as? Int).map { "\($0) MHz" } ?? "Not reported",
          band: object["band"] as? String ?? "Not reported",
          phy: object["phy"] as? String ?? "Not reported",
          match: matched ? "Match" : (detection.isEmpty ? "Not evaluated" : "—"),
          alert: detection["alert"] as? String ?? "Not reported",
          evidence: try evidence.map { item in
            String(
              decoding: try JSONSerialization.data(
                withJSONObject: item, options: [.prettyPrinted, .sortedKeys, .fragmentsAllowed]),
              as: UTF8.self)
          }, raw: String(decoding: pretty, as: UTF8.self)))
    }
    return Recording(observations: rows, ignoredRecords: ignored)
  }

  public static func load(_ url: URL) throws -> Self {
    let handle = try FileHandle(forReadingFrom: url)
    defer { try? handle.close() }
    let bytes = try handle.read(upToCount: byteLimit + 1) ?? Data()
    return try decode(bytes)
  }
}
