import AirHoundCore
import AppKit
import SwiftUI

@MainActor
final class SessionModel: ObservableObject {
  @Published var socketPath = FileManager.default.homeDirectoryForCurrentUser
    .appendingPathComponent("Library/Application Support/AirHound/control.sock").path
  @Published var connected = false
  @Published var captureEnabled = false
  @Published var busy = false
  @Published var message = "Connect to the local service to set up a scan."
  @Published var inventory: Inventory?
  @Published var selected = Set<String>()
  @Published var sourceOverrides: [String: JSONValue] = [:]
  @Published var configText = ""
  @Published var validation: String?
  @Published var capture: JSONValue?
  @Published var runs: [StoredRun] = []
  @Published var nextRun: String?
  @Published var receiverStatuses: [ReceiverStatus] = []
  @Published var healthCatchingUp = false
  private var healthRun: String?
  private var healthCursor: Int64 = 0
  private var lastReportedHealth: JSONValue?
  private var sourceEvents: [String: JSONValue] = [:]
  @Published var healthText = "No receiver status reported."
  @Published var error: String?
  @Published var captureFailure: String?
  @Published var browsingRun: String?
  @Published var followLive = false
  @Published var hasMoreRecords = false
  private var window = RecordWindow()
  private weak var observationApp: AppModel?
  private var client: BackendClient?
  private var pollTask: Task<Void, Never>?
  private var requestID = UUID().uuidString
  @Published private(set) var pendingConfig: Data?
  private var pendingSocket: String?
  var hasPendingStart: Bool { pendingConfig != nil }

  private let preferences: UserDefaults
  init(preferences: UserDefaults = .standard) {
    self.preferences = preferences
    if let path = preferences.string(forKey: "serviceSocket") { socketPath = path }
  }

  private func restorePendingStart() throws {
    if let pending = try PendingStartStore(socket: URL(fileURLWithPath: socketPath)).load() {
      pendingConfig = pending.config
      pendingSocket = pending.socket
      requestID = pending.requestID
      configText = String(decoding: pending.config, as: UTF8.self)
    } else {
      pendingConfig = nil
      pendingSocket = nil
    }
  }
  private(set) var service: Process?
  var activeID: String? { capture?["run_id"]?.string }
  var state: String { capture?["state"]?.string ?? capture?["last"]?["state"]?.string ?? "idle" }
  var isActive: Bool { capture?["active"] == .bool(true) }

  func connect(executable: URL?) {
    guard let executable, !busy else { return }
    connected = false
    pollTask?.cancel()
    preferences.set(socketPath, forKey: "serviceSocket")
    client = BackendClient(executable: executable, socket: URL(fileURLWithPath: socketPath))
    perform {
      guard let client = self.client else { return }
      let status = try JSONDecoder().decode(JSONValue.self, from: await client.request(["status"]))
      self.captureEnabled = status["capture_control"] == .bool(true)
      self.leaveRecording()
      self.runs = []
      self.nextRun = nil
      self.healthRun = nil
      self.healthCursor = 0
      self.lastReportedHealth = nil
      self.sourceEvents = [:]
      self.capture = nil
      try self.restorePendingStart()
      self.inventory = try await client.inventory()
      if self.configText.isEmpty, let inventory = self.inventory {
        for device in inventory.devices {
          for (index, candidate) in device.candidates.enumerated()
          where ["native_wifi", "native_ble"].contains(candidate.kind) {
            self.selected.insert(device.id + ":" + String(index))
          }
        }
        self.useSelectedReceivers()
      }
      try await self.update()
      self.connected = true
      self.message = "Connected. Closing AirHound leaves capture running."
      self.beginPolling()
    }
  }

  func startService(executable: URL?) {
    guard let executable, !busy else { return }
    do {
      let socket = URL(fileURLWithPath: socketPath)
      let directory = socket.deletingLastPathComponent()
      try LocalFiles.privateDirectory(directory)
      // The Rust server validates ownership, permissions and symlinks before binding.
      // Never remove an existing socket: it may belong to an active capture service.
      guard !FileManager.default.fileExists(atPath: socket.path) else {
        throw ModelError.invalid("A socket already exists. Connect to the service instead.")
      }
      let arguments = [
        "serve", "--socket", socket.path, "--database",
        directory.appendingPathComponent("recordings.sqlite").path, "--capture",
      ]
      busy = true
      message = "Starting service…"
      Task {
        do {
          let running: () -> Bool
          if let application = ServiceLauncher.application(for: executable) {
            let launched = try ServiceLauncher.launch(
              application: application, arguments: arguments)
            running = { launched.isRunning }
          } else {
            // Explicit standalone developer backends remain usable, but their
            // native radio permissions depend on their own launch identity.
            let process = Process()
            process.executableURL = executable
            process.arguments = arguments
            process.standardInput = FileHandle.nullDevice
            process.standardOutput = FileHandle.nullDevice
            process.standardError = FileHandle.nullDevice
            try process.run()
            service = process
            running = { process.isRunning }
          }
          for _ in 0..<50 {
            if FileManager.default.fileExists(atPath: socket.path) {
              busy = false
              connect(executable: executable)
              return
            }
            guard running() else {
              throw ModelError.invalid(
                "Service exited before becoming ready. Check the directory permissions and database ownership."
              )
            }
            try await Task.sleep(for: .milliseconds(100))
          }
          throw ModelError.invalid(
            "Service has not become ready. Try Connect; no socket has been removed.")
        } catch {
          busy = false
          self.error = error.localizedDescription
        }
      }
    } catch { self.error = error.localizedDescription }
  }

  func useSelectedReceivers() {
    guard let inventory else { return }
    var receivers: [JSONValue] = []
    for device in inventory.devices {
      for (index, candidate) in device.candidates.enumerated() {
        guard selected.contains(device.id + ":" + String(index)),
          let source = sourceOverrides[device.id + ":" + String(index)]
            ?? candidate.configurationHint
        else { continue }
        receivers.append(
          .object(["id": .string("receiver-\(receivers.count + 1)"), "source": source]))
      }
    }
    let config = JSONValue.object(["version": .integer(1), "receivers": .array(receivers)])
    configText = (try? String(decoding: config.data(pretty: true), as: UTF8.self)) ?? ""
    validation = nil
  }

  func sourceField(_ key: String, field: String, fallback: JSONValue) -> JSONValue? {
    (sourceOverrides[key] ?? fallback)[field]
  }

  func setSourceField(_ key: String, field: String, value: JSONValue, fallback: JSONValue) {
    guard case .object(var source) = sourceOverrides[key] ?? fallback else { return }
    source[field] = value
    sourceOverrides[key] = .object(source)
    useSelectedReceivers()
  }

  func validate() {
    perform {
      guard let client = self.client else { return }
      let value = try await client.validate(config: Data(self.configText.utf8))
      self.validation = String(decoding: try value.data(pretty: true), as: UTF8.self)
      self.message = "Plan validated. Receiver readiness is checked when capture starts."
    }
  }

  func start() {
    perform {
      guard let client = self.client else { return }
      let config = Data(self.configText.utf8)
      if let pending = self.pendingConfig,
        pending != config || self.pendingSocket != client.socket.path
      {
        throw ModelError.invalid(
          "A previous start has an uncertain outcome. Reconnect and check the active recording before changing the plan."
        )
      }
      if self.pendingConfig == nil { _ = try await client.validate(config: config) }
      let store = PendingStartStore(socket: client.socket)
      let pending = try store.prepare(socket: client.socket, config: config)
      self.pendingConfig = pending.config
      self.pendingSocket = pending.socket
      self.requestID = pending.requestID
      self.capture = try await client.start(config: config, requestID: pending.requestID)
      try store.clear(pending)
      self.pendingConfig = nil
      self.pendingSocket = nil
      self.requestID = UUID().uuidString
      self.runs = []
      try await self.update()
    }
  }

  func control(_ action: CaptureControl) {
    guard let id = activeID else { return }
    perform {
      guard let client = self.client else { return }
      let result = try await client.control(action, runID: id)
      if result["state"]?.string == "failed" {
        self.error =
          result["error"]?.string ?? "Capture finished with a failure. Review receiver status."
      }
      self.runs = []
      try await self.update()
    }
  }

  func refreshHistory(more: Bool = false) {
    perform {
      guard let client = self.client else { return }
      let page = try await client.recordings(before: more ? self.nextRun : nil)
      self.runs = more ? self.runs + page.runs : page.runs
      self.nextRun = page.hasMore ? page.nextBefore : nil
    }
  }

  func showCurrentRecordingIfNeeded(app: AppModel) {
    guard connected, !busy, browsingRun == nil, app.recording == nil,
      let id = activeID ?? capture?["last"]?["run_id"]?.string,
      let run = runs.first(where: { $0.id == id })
    else { return }
    open(run, app: app)
  }

  func open(_ run: StoredRun, app: AppModel) {
    guard !busy else { return }
    browsingRun = run.id
    observationApp = app
    window = RecordWindow()
    followLive = isActive && activeID == run.id
    app.selectedTab = "observations"
    app.recordingRevision = UUID()
    loadMoreRecords()
  }

  func loadMoreRecords() {
    perform { try await self.fetchRecords() }
  }

  func rewindRecords() {
    guard !busy else { return }
    window = RecordWindow()
    followLive = false
    observationApp?.recordingRevision = UUID()
    loadMoreRecords()
  }

  private func fetchRecords() async throws {
    guard let client, let id = browsingRun, let app = observationApp else { return }
    let page = try await client.records(runID: id, after: window.cursor, limit: 500)
    try window.append(page)
    hasMoreRecords = page.hasMore
    app.recording = try window.observations()
    app.recordingLabel =
      "\(window.records.count) recent records · \(window.omitted) earlier records outside this view"
    app.isExample = false
  }

  func leaveRecording() {
    browsingRun = nil
    followLive = false
    observationApp = nil
    window = RecordWindow()
  }

  func export() {
    let panel = NSSavePanel()
    panel.title = "Export recording archive"
    panel.nameFieldStringValue = "AirHound-\(Int(Date().timeIntervalSince1970)).sqlite"
    panel.message = "Exports a consistent snapshot of all recordings. Choose a new filename."
    guard panel.runModal() == .OK, let url = panel.url else { return }
    perform {
      guard let client = self.client else { return }
      _ = try await client.export(to: url)
      self.message = "Archive exported to \(url.path)"
    }
  }

  private func update() async throws {
    guard let client else { return }
    capture = captureEnabled ? try await client.status() : nil
    captureFailure =
      capture?["last"]?["state"]?.string == "failed"
      ? (capture?["last"]?["error"]?.string
        ?? "The recording ended with a receiver or storage failure. Inspect its retained status records.")
      : nil
    if let id = activeID ?? capture?["last"]?["run_id"]?.string {
      if healthRun != id {
        healthRun = id
        healthCursor = 0
        sourceEvents = [:]
        lastReportedHealth = nil
      }
      let events = try await client.records(runID: id, after: healthCursor, limit: 500)
      for record in events.records {
        if ["external_scanner_status", "ble_source_status"].contains(record.kind) {
          sourceEvents[record.sourceID] = record.payload
        } else if record.kind == "scanner_health" {
          lastReportedHealth = record.payload
        }
      }
      healthCursor = events.nextAfter
      healthCatchingUp = events.hasMore
    }
    let health = capture?["receiver_health"] ?? lastReportedHealth
    healthText =
      try health.map { String(decoding: try $0.data(pretty: true), as: UTF8.self) }
      ?? "No receiver status reported."
    receiverStatuses = ReceiverStatus.decode(
      .object(["receiver_health": health ?? .null]), sourceEvents: sourceEvents)
    let page = try await client.recordings()
    let freshIDs = Set(page.runs.map(\.id))
    let retained = runs.filter { !freshIDs.contains($0.id) }
    if retained.isEmpty { nextRun = page.hasMore ? page.nextBefore : nil }
    runs = page.runs + retained
    if followLive { try await fetchRecords() }
  }

  private func beginPolling() {
    pollTask?.cancel()
    pollTask = Task { [weak self] in
      while !Task.isCancelled {
        do { try await Task.sleep(for: .seconds(2)) } catch { return }
        guard let self else { return }
        if !self.busy {
          self.busy = true
          do { try await self.update() } catch {
            self.busy = false
            if Task.isCancelled { return }
            self.connected = false
            self.message =
              "Disconnected. Capture may still be running. Reconnect to check its state."
            return
          }
          self.busy = false
        }
      }
    }
  }
  private func perform(_ operation: @escaping () async throws -> Void) {
    guard !busy else { return }
    busy = true
    error = nil
    Task {
      defer { busy = false }
      do { try await operation() } catch { self.error = error.localizedDescription }
    }
  }
  func disconnect() {
    pollTask?.cancel()
    pollTask = nil
    connected = false
  }
}
