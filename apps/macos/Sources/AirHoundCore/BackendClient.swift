import Foundation

/// Presentation adapter over the shared Rust client. Each command establishes a fresh
/// authenticated local connection; cancelling a client never stops service-owned capture.
public struct BackendClient {
  public let executable: URL
  public let socket: URL
  private let process: DiscoveryProcess

  public init(executable: URL, socket: URL) {
    self.executable = executable
    self.socket = socket
    process = DiscoveryProcess()
  }

  // Synthetic command fixtures can freeze the collector clock while retaining
  // the real executable, arguments, IO and owned-process cleanup.
  init(executable: URL, socket: URL, process: DiscoveryProcess) {
    self.executable = executable
    self.socket = socket
    self.process = process
  }

  public func request(_ command: [String]) async throws -> Data {
    try await process.run(
      executable: executable,
      arguments: ["client", "--socket", socket.path] + command,
      timeout: 25, outputLimit: 2 * 1024 * 1024)
  }

  /// Read-only, authenticated service startup handshake. Rust owns transport
  /// retry classification; Swift never interprets stderr as a retry signal.
  public func waitUntilReady(deadline: TimeInterval) async throws -> JSONValue {
    let remaining = deadline - ProcessInfo.processInfo.systemUptime
    guard remaining.isFinite, remaining >= 0.001, remaining <= 60 else {
      throw DiscoveryError.timedOut
    }
    let milliseconds = UInt64((remaining * 1000).rounded(.down))
    let response = try await process.runUntil(
      executable: executable,
      arguments: [
        "client", "--socket", socket.path, "wait-ready", "--timeout-ms", String(milliseconds),
      ], deadline: deadline, outputLimit: 2 * 1024 * 1024)
    return try JSONDecoder().decode(JSONValue.self, from: response)
  }

  public func status() async throws -> JSONValue {
    try JSONDecoder().decode(JSONValue.self, from: await request(["capture-status"]))
  }

  public func control(_ action: CaptureControl, runID: String) async throws -> JSONValue {
    try JSONDecoder().decode(
      JSONValue.self, from: await request([action.rawValue, "--run-id", runID]))
  }

  public func validate(config: Data) async throws -> JSONValue {
    try await withConfig(config, command: ["validate"])
  }

  public func identifyDevice(config: Data) async throws -> JSONValue {
    try await withConfig(config, command: ["identify"], configFlag: "--config")
  }

  /// The caller retains this request ID across uncertain outcomes. Never retry a start
  /// with a new ID until the service confirms whether the first request was accepted.
  public func start(config: Data, requestID: String) async throws -> JSONValue {
    try await withConfig(config, command: ["start", "--request-id", requestID])
  }

  public func export(to destination: URL) async throws -> JSONValue {
    try JSONDecoder().decode(
      JSONValue.self, from: await request(["export", "--output", destination.path]))
  }

  private func withConfig(
    _ config: Data, command: [String], configFlag: String = "--session-config"
  ) async throws -> JSONValue {
    // Private temporary directory, removed only after the bounded client process exits.
    // The Rust backend owns syntax, schema, resource and channel validation.
    let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(
      at: directory, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
    defer { try? FileManager.default.removeItem(at: directory) }
    let file = directory.appendingPathComponent("session.json")
    try config.write(to: file, options: .atomic)
    return try JSONDecoder().decode(
      JSONValue.self, from: await request(command + [configFlag, file.path]))
  }

  public func setups() async throws -> [SavedSetup] {
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    let result = try decoder.decode(SavedSetupList.self, from: await request(["profiles", "list"]))
    guard result.schemaVersion == 1 else {
      throw ModelError.invalid("Unsupported saved setup format.")
    }
    return result.profiles
  }

  public func loadSetup(id: String) async throws -> SetupReview {
    try SetupReview(
      JSONDecoder().decode(JSONValue.self, from: await request(["profiles", "load", "--id", id])))
  }

  public func saveSetup(id: String, name: String, config: Data, replace: Bool) async throws
    -> JSONValue
  {
    var command = ["profiles", "save", "--id", id, "--name", name]
    if replace { command.append("--replace") }
    return try await withConfig(config, command: command)
  }

  public func deleteSetup(id: String) async throws {
    _ = try await request(["profiles", "delete", "--id", id])
  }

  public func inventory() async throws -> Inventory {
    try Inventory.decode(await request(["devices"]))
  }

  public func recordings(before: String? = nil) async throws -> RunPage {
    var arguments = ["recordings", "--limit", "50"]
    if let before { arguments += ["--before", before] }
    return try JSONDecoder().decode(RunPage.self, from: await request(arguments))
  }

  public func records(runID: String, after: Int64 = 0, limit: Int = 100) async throws -> RecordPage
  {
    try JSONDecoder().decode(
      RecordPage.self,
      from: await request([
        "records", "--run-id", runID, "--after", String(after), "--limit", String(limit),
      ]))
  }
}

public enum CaptureControl: String { case pause, resume, finish }

/// Preserve structured evidence and integer counters, including unknown future fields.
public enum JSONValue: Codable, Equatable, Sendable {
  case object([String: JSONValue])
  case array([JSONValue])
  case string(String)
  case integer(Int64)
  case unsigned(UInt64)
  case number(Double)
  case bool(Bool)
  case null

  public init(from decoder: Decoder) throws {
    let value = try decoder.singleValueContainer()
    if value.decodeNil() {
      self = .null
    } else if let v = try? value.decode(Bool.self) {
      self = .bool(v)
    } else if let v = try? value.decode(Int64.self) {
      self = .integer(v)
    } else if let v = try? value.decode(UInt64.self) {
      self = .unsigned(v)
    } else if let v = try? value.decode(Double.self) {
      self = .number(v)
    } else if let v = try? value.decode(String.self) {
      self = .string(v)
    } else if let v = try? value.decode([JSONValue].self) {
      self = .array(v)
    } else {
      self = .object(try value.decode([String: JSONValue].self))
    }
  }
  public func encode(to encoder: Encoder) throws {
    var value = encoder.singleValueContainer()
    switch self {
    case .object(let v): try value.encode(v)
    case .array(let v): try value.encode(v)
    case .string(let v): try value.encode(v)
    case .integer(let v): try value.encode(v)
    case .unsigned(let v): try value.encode(v)
    case .number(let v): try value.encode(v)
    case .bool(let v): try value.encode(v)
    case .null: try value.encodeNil()
    }
  }
  public subscript(_ key: String) -> JSONValue? {
    guard case .object(let values) = self else { return nil }
    return values[key]
  }
  public var string: String? {
    guard case .string(let value) = self else { return nil }
    return value
  }
  public func data(pretty: Bool = false) throws -> Data {
    let encoder = JSONEncoder()
    encoder.outputFormatting = pretty ? [.prettyPrinted, .sortedKeys] : [.sortedKeys]
    return try encoder.encode(self)
  }
}

public struct StoredRun: Decodable, Identifiable {
  enum CodingKeys: String, CodingKey {
    case id, state, mode, config
    case startedAtUs = "started_at_us"
    case endedAtUs = "ended_at_us"
    case recordCount = "record_count"
  }

  public let id: String
  public let state: String
  public let mode: String
  public let startedAtUs: Int64
  public let endedAtUs: Int64?
  public let recordCount: Int64
  public let config: JSONValue
  public var startedAt: Date { Date(timeIntervalSince1970: Double(startedAtUs) / 1_000_000) }
}
public struct RunPage: Decodable {
  enum CodingKeys: String, CodingKey {
    case runs
    case nextBefore = "next_before"
    case hasMore = "has_more"
  }

  public let runs: [StoredRun]
  public let nextBefore: String?
  public let hasMore: Bool
}
public struct StoredRecord: Decodable, Identifiable, Sendable {
  enum CodingKeys: String, CodingKey {
    case cursor, kind, payload
    case sourceID = "source_id"
    case receivedAtUs = "received_at_us"
  }

  public let cursor: Int64
  public let sourceID: String
  public let kind: String
  public let receivedAtUs: Int64
  public let payload: JSONValue
  public var id: Int64 { cursor }
}
public struct RecordPage: Decodable, Sendable {
  enum CodingKeys: String, CodingKey {
    case records
    case nextAfter = "next_after"
    case hasMore = "has_more"
  }

  public let records: [StoredRecord]
  public let nextAfter: Int64
  public let hasMore: Bool

  public func observations() throws -> Recording {
    var stream = Data()
    for record in records {
      stream.append(try record.payload.data())
      stream.append(10)
    }
    return try Recording.decode(stream)
  }
}
