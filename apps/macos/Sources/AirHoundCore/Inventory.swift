import Foundation

public struct Inventory: Decodable {
  public let schemaVersion: Int
  public let mode: String
  public let platform: String
  public let devices: [Device]
  public let diagnostics: [Diagnostic]

  public var receiverChoices: [ReceiverChoice] {
    devices.flatMap { device in
      device.candidates.enumerated().map { index, candidate in
        ReceiverChoice(
          id: device.id + ":" + String(index), deviceName: device.name, candidate: candidate)
      }
    }
  }

  public static func decode(_ data: Data) throws -> Self {
    guard data.count <= 4 * 1024 * 1024 else { throw ModelError.invalid("Inventory exceeds 4 MiB") }
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    let inventory = try decoder.decode(Self.self, from: data)
    guard inventory.schemaVersion == 1,
      ["passive", "passive_with_active_metadata"].contains(inventory.mode)
    else {
      throw ModelError.invalid("Unsupported inventory schema or discovery mode")
    }
    guard Set(inventory.devices.map(\.id)).count == inventory.devices.count else {
      throw ModelError.invalid("Inventory contains duplicate device identities")
    }
    return inventory
  }
}

public struct Device: Decodable, Identifiable {
  public let id: String
  public let identityScope: String
  public let name: String
  public let endpoints: [Endpoint]
  public let candidates: [Candidate]
  public let usb: USBIdentity?
}
public struct USBIdentity: Decodable {
  public let vid: String
  public let pid: String
}
public struct Endpoint: Decodable {
  public let kind: String
  public let path: String
}
public struct Diagnostic: Decodable {
  public let backend: String
  public let message: String
}
public struct Candidate: Decodable {
  public let kind: String
  public let label: String
  public let support: String
  public let state: String
  public let evidence: [String]
  public let requirements: [Requirement]
  public let configurationHint: JSONValue?
  public var statusDescription: String {
    switch state {
    case "unsupported": return "Recognized — AirHound integration pending"
    case "needs_identification": return "Device or firmware identity not yet verified"
    case "needs_configuration": return "Setup required before capture"
    case "unverified": return "Detected; readiness not yet verified"
    default: return state.replacingOccurrences(of: "_", with: " ")
    }
  }
}
public struct Requirement: Decodable {
  public let name: String
  public let state: String
  public let detail: String
}
public enum ModelError: LocalizedError {
  case invalid(String)
  public var errorDescription: String? {
    if case .invalid(let message) = self { return message }
    return nil
  }
}

/// Globally unique row identities survive Form's flattening of nested content.
public struct ReceiverChoice: Identifiable {
  public let id: String
  public let deviceName: String
  public let candidate: Candidate
}
