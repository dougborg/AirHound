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
  @Published var setups: [SavedSetup] = []
  @Published var selectedSetupID = ""
  @Published var setupReview: SetupReview?
  @Published var setupsAvailable = false
  @Published var setupError: String?
  @Published var locationProviders: [String] = []
  @Published var backendBuild: String?
  @Published var appliedSetupName: String?
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
  private var recordingEpoch = UUID()
  private var hasPresentedWindow = false
  private weak var observationApp: AppModel?
  private var client: BackendClient?
  private var pollTask: Task<Void, Never>?
  private var polling = false
  private var pendingSetupMutation: (() async throws -> Void)?
  @Published private(set) var setupMutationQueued = false
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
      self.backendBuild = status["build"]?["display_version"]?.string
      if case .array(let providers) = status["location_providers"] {
        self.locationProviders = providers.compactMap(\.string)
      } else {
        self.locationProviders = []
      }
      if case .array(let methods) = status["methods"] {
        self.setupsAvailable = methods.contains(.string("profiles.list"))
      } else {
        self.setupsAvailable = false
      }
      self.setups = []
      self.selectedSetupID = ""
      self.setupReview = nil
      self.appliedSetupName = nil
      self.setupError = nil
      if self.setupsAvailable {
        do { self.setups = try await client.setups() } catch {
          self.setupError = error.localizedDescription
        }
      }
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

  var canEditPlan: Bool {
    !isActive && (!busy || polling) && !setupMutationQueued && !hasPendingStart
  }
  var plannedReceivers: [PlanReceiver] {
    guard let config = try? JSONDecoder().decode(JSONValue.self, from: Data(configText.utf8)) else {
      return []
    }
    return (try? PlanEditing.receivers(in: config)) ?? []
  }

  func currentReceiverIdentity(id: String) -> JSONValue? {
    guard connected, isActive, let activeID,
      let planSource = plannedReceivers.first(where: { $0.id == id })?.source,
      let run = runs.first(where: { $0.id == activeID }),
      case .array(let configured) = run.config["session"]?["receivers"],
      let activeSource = configured.first(where: { $0["id"]?.string == id })?["source"],
      identitySourcesMatch(planSource, activeSource),
      case .array(let receivers) = capture?["receiver_health"]?["receivers"]
    else { return nil }
    return receivers.first { $0["id"]?.string == id }?["receiver"]
  }

  private func identitySourcesMatch(_ planned: JSONValue, _ active: JSONValue) -> Bool {
    guard case .object(var expected) = planned, case .object(var actual) = active else {
      return false
    }
    expected = expected.filter { $0.value != .null }
    actual = actual.filter { $0.value != .null }
    // The service assigns an omitted Sniffle channel. All explicit source fields,
    // including helper/module paths and endpoint, must still match the stored run.
    if expected["kind"] == .string("sniffle") {
      if expected["channel"] == nil { actual.removeValue(forKey: "channel") }
      if expected["coded"] == nil { expected["coded"] = .bool(false) }
    }
    return expected == actual
  }

  private func editPlan(_ edit: (JSONValue) throws -> JSONValue) {
    guard canEditPlan else { return }
    do {
      let config =
        configText.isEmpty
        ? JSONValue.object(["version": .integer(1), "receivers": .array([])])
        : try JSONDecoder().decode(JSONValue.self, from: Data(configText.utf8))
      configText = String(decoding: try edit(config).data(pretty: true), as: UTF8.self)
      validation = nil
    } catch { self.error = error.localizedDescription }
  }
  func addReceiver(source: JSONValue) {
    editPlan {
      try PlanEditing.add(
        $0, id: "receiver-" + UUID().uuidString.prefix(8).lowercased(), source: source)
    }
  }
  func removeReceiver(id: String) { editPlan { try PlanEditing.remove($0, id: id) } }
  func planSourceField(id: String, field: String, value: JSONValue) {
    editPlan { try PlanEditing.change($0, receiverID: id, field: field, value: value) }
  }
  var planLocation: JSONValue? {
    (try? JSONDecoder().decode(JSONValue.self, from: Data(configText.utf8)))?["location"]
  }
  func setLocation(_ value: JSONValue?) { editPlan { try PlanEditing.location($0, value: value) } }
  func setLocationField(_ name: String, value: JSONValue) {
    guard case .object(var location) = planLocation else { return }
    location[name] = value
    setLocation(.object(location))
  }

  func refreshDevices() {
    perform {
      guard let client = self.client else { return }
      self.inventory = try await client.inventory()
    }
  }

  func refreshSetups() {
    perform {
      guard let client = self.client else { return }
      self.setups = try await client.setups()
      self.setupError = nil
    }
  }
  func reviewSetup() {
    guard canEditPlan, !selectedSetupID.isEmpty else { return }
    perform {
      guard let client = self.client else { return }
      self.setupReview = try await client.loadSetup(id: self.selectedSetupID)
    }
  }
  func applySetup(_ review: SetupReview) {
    guard canEditPlan else { return }
    do {
      configText = String(decoding: try review.config.data(pretty: true), as: UTF8.self)
      selected = []
      sourceOverrides = [:]
      appliedSetupName = review.name
      selectedSetupID = review.id
      setupReview = nil
      validation = nil
      message = "Setup loaded. Review the receivers before starting."
    } catch { self.error = error.localizedDescription }
  }
  @discardableResult
  func saveSetup(name: String, replacing id: String? = nil) -> Bool {
    guard let client else {
      error = "Connect before saving a setup."
      return false
    }
    let config = Data(configText.utf8)
    let selectedID = id ?? UUID().uuidString.lowercased()
    return submitSetupMutation {
      guard self.connected, self.client?.socket == client.socket, !self.isActive,
        !self.hasPendingStart
      else {
        throw ModelError.invalid(
          "The service or recording changed. Review the plan and save again.")
      }
      _ = try await client.saveSetup(id: selectedID, name: name, config: config, replace: id != nil)
      self.setups = try await client.setups()
      self.selectedSetupID = selectedID
      self.appliedSetupName = name
      self.message = "Setup saved on the backend."
    }
  }
  @discardableResult
  func deleteSelectedSetup() -> Bool {
    guard let client, !selectedSetupID.isEmpty else { return false }
    let id = selectedSetupID
    return submitSetupMutation {
      guard self.connected, self.client?.socket == client.socket, !self.isActive,
        !self.hasPendingStart
      else {
        throw ModelError.invalid(
          "The service or recording changed. Review the setup and delete again.")
      }
      try await client.deleteSetup(id: id)
      self.setups = try await client.setups()
      if self.selectedSetupID == id { self.selectedSetupID = "" }
      self.message = "Saved setup removed. The current plan is unchanged."
    }
  }

  // Keep one explicit user action while an in-flight status refresh completes.
  // The closure captures the submitted plan/ID, not later editor selection.
  @discardableResult
  func submitSetupMutation(_ operation: @escaping () async throws -> Void) -> Bool {
    guard connected, canEditPlan else {
      error = "The setup cannot be changed right now. Wait for the current operation and try again."
      return false
    }
    if polling {
      pendingSetupMutation = operation
      setupMutationQueued = true
    } else {
      perform(operation)
    }
    return true
  }

  func beginBackgroundRefresh() {
    polling = true
    busy = true
  }
  func finishBackgroundRefresh() {
    polling = false
    busy = false
    let operation = pendingSetupMutation
    pendingSetupMutation = nil
    setupMutationQueued = false
    if let operation { perform(operation) }
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
    recordingEpoch = UUID()
    hasPresentedWindow = false
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
    recordingEpoch = UUID()
    hasPresentedWindow = false
    followLive = false
    observationApp?.recordingRevision = UUID()
    loadMoreRecords()
  }

  private func fetchRecords() async throws {
    guard let client, let id = browsingRun, let app = observationApp else { return }
    let epoch = recordingEpoch
    let current = window
    let page = try await client.records(runID: id, after: current.cursor, limit: 500)
    guard recordingEpoch == epoch, browsingRun == id else { return }
    // Serialization, decoding and evidence formatting can take several frames.
    // Keep that work off the main actor while the native table is scrolling.
    let updated = try await Task.detached(priority: .userInitiated) {
      var updated = current
      try updated.append(page)
      return updated
    }.value
    try Task.checkCancellation()
    guard recordingEpoch == epoch, browsingRun == id, observationApp === app else { return }
    window = updated
    hasMoreRecords = page.hasMore
    // Polling an unchanged recording must not invalidate every table row.
    guard !page.records.isEmpty || !hasPresentedWindow else { return }
    app.recording = try window.observations()
    app.recordingLabel =
      "\(window.records.count) recent records · \(window.omitted) earlier records outside this view"
    app.isExample = false
    hasPresentedWindow = true
  }

  func leaveRecording() {
    browsingRun = nil
    followLive = false
    observationApp = nil
    window = RecordWindow()
    recordingEpoch = UUID()
    hasPresentedWindow = false
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
          self.beginBackgroundRefresh()
          do { try await self.update() } catch {
            defer { self.finishBackgroundRefresh() }
            if Task.isCancelled { return }
            self.connected = false
            self.message =
              "Disconnected. Capture may still be running. Reconnect to check its state."
            return
          }
          self.finishBackgroundRefresh()
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
    recordingEpoch = UUID()
    pollTask?.cancel()
    pollTask = nil
    connected = false
  }
}
