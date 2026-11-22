import Foundation

/// Editing the displayed JSON is a presentation operation. The Rust backend still
/// validates all source, channel, resource and device-binding semantics.
public struct PlanReceiver: Identifiable {
  public let id: String
  public let source: JSONValue
  public var kind: String { source["kind"]?.string ?? "unknown" }
  public var title: String {
    switch kind {
    case "native_ble": return "System Bluetooth"
    case "native_wifi": return "System Wi-Fi"
    case "sniffle": return "Sniffle / CatSniffer"
    case "external_wifi": return "External Wi-Fi receiver"
    case "airhound_serial": return "AirHound sensor"
    case "linux_wifi": return "Linux monitor interface"
    case "marauder_capture": return "Saved Marauder capture"
    case "wifi_pcap": return "Wi-Fi recording"
    case "ble_pcap": return "Bluetooth recording"
    default: return kind
    }
  }
  public var endpoint: String {
    ["port", "usb_selector", "interface", "adapter", "path"].compactMap { source[$0]?.string }.first
      ?? "System managed"
  }
}

public enum PlanEditing {
  public static func receivers(in config: JSONValue) throws -> [PlanReceiver] {
    guard case .array(let values) = config["receivers"] else {
      throw ModelError.invalid("The plan has no receiver list. Check the advanced JSON editor.")
    }
    var ids = Set<String>()
    return try values.map { value in
      guard let id = value["id"]?.string, ids.insert(id).inserted,
        case .object = value["source"], let source = value["source"]
      else {
        throw ModelError.invalid("Receiver IDs and source objects are needed to edit this plan.")
      }
      return PlanReceiver(id: id, source: source)
    }
  }

  public static func change(
    _ config: JSONValue, receiverID: String, field: String, value: JSONValue
  ) throws -> JSONValue {
    try update(config) { receivers in
      guard let index = receivers.firstIndex(where: { $0["id"]?.string == receiverID }),
        case .object(var receiver) = receivers[index], case .object(var source) = receiver["source"]
      else { throw ModelError.invalid("This receiver is no longer in the plan.") }
      source[field] = value
      receiver["source"] = .object(source)
      receivers[index] = .object(receiver)
    }
  }

  public static func add(_ config: JSONValue, id: String, source: JSONValue) throws -> JSONValue {
    try update(config) { receivers in
      guard !receivers.contains(where: { $0["id"]?.string == id }) else {
        throw ModelError.invalid("This receiver ID is already in the plan.")
      }
      receivers.append(.object(["id": .string(id), "source": source]))
    }
  }

  public static func location(_ config: JSONValue, value: JSONValue?) throws -> JSONValue {
    guard case .object(var root) = config else {
      throw ModelError.invalid("The scan plan must be a JSON object.")
    }
    root["location"] = value
    return .object(root)
  }

  public static func remove(_ config: JSONValue, id: String) throws -> JSONValue {
    try update(config) { $0.removeAll { $0["id"]?.string == id } }
  }

  private static func update(_ config: JSONValue, mutate: (inout [JSONValue]) throws -> Void) throws
    -> JSONValue
  {
    _ = try receivers(in: config)
    guard case .object(var root) = config, case .array(var receivers) = root["receivers"] else {
      throw ModelError.invalid("The scan plan must be a JSON object.")
    }
    try mutate(&receivers)
    root["receivers"] = .array(receivers)
    return .object(root)
  }
}

public struct SavedSetup: Decodable, Identifiable {
  public let id: String
  public let name: String
  public let savedAtMs: UInt64
  public let receiverCount: Int
}
public struct SavedSetupList: Decodable {
  public let schemaVersion: Int
  public let profiles: [SavedSetup]
}

public struct SetupReview: Identifiable {
  public let id: String
  public let name: String
  public let config: JSONValue
  public let raw: JSONValue
  public init(_ value: JSONValue) throws {
    guard let id = value["profile"]?["id"]?.string,
      let name = value["profile"]?["name"]?.string,
      let config = value["profile"]?["config"], case .object = config
    else { throw ModelError.invalid("The service returned an incomplete saved setup.") }
    self.id = id
    self.name = name
    self.config = config
    self.raw = value
  }
  public var checks: [(label: String, detail: String)] {
    var result: [(String, String)] = []
    for group in ["bindings", "resources"] {
      if case .array(let checks) = raw[group]?["checks"] {
        for check in checks {
          let label = [check["receiver_id"]?.string, check["field"]?.string].compactMap { $0 }
            .joined(separator: " · ")
          let state = check["state"]?.string?.replacingOccurrences(of: "_", with: " ") ?? "Unknown"
          let path = check["path"]?.string ?? check["binding"]?["selector"]?.string
          result.append((label, [state, path].compactMap { $0 }.joined(separator: " — ")))
        }
      }
    }
    return result
  }
}
