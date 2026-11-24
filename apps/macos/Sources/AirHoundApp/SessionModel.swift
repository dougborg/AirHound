import AirHoundCore
import AppKit
import SwiftUI

@MainActor
final class SessionModel: ObservableObject {
  @Published var socketPath = FileManager.default.homeDirectoryForCurrentUser
    .appendingPathComponent("Library/Application Support/AirHound/control.sock").path
  @Published var connected = false {
    didSet { if !connected { gpsIdentifications = [:] } }
  }
  @Published var captureEnabled = false
  @Published var busy = false
  @Published var message = "Connect to the local service to set up a scan."
  @Published var inventory: Inventory? {
    didSet { gpsIdentifications = [:] }
  }
  @Published var gpsIdentifications: [GPSVersionTarget: JSONValue] = [:]
  @Published var selected = Set<String>()
  @Published var sourceOverrides: [String: JSONValue] = [:]
  @Published var configText = ""
  @Published var setups: [SavedSetup] = []
  @Published var selectedSetupID = ""
  @Published var setupReview: SetupReview?
  @Published var setupsAvailable = false
  @Published var setupError: String?
  @Published var locationProviders: [String] = []
  @Published var sessionSources: [String] = []
  @Published var rayhunterTransports: [String] = []
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
  private var connectionEpoch = UUID()
  private var pollTask: Task<Void, Never>?
  private(set) var startupTask: Task<Void, Never>?
  private var polling = false
  private var pendingUserAction: (() async throws -> Void)?
  @Published private(set) var userActionQueued = false
  private var requestID = UUID().uuidString
  @Published private(set) var pendingConfig: Data?
  private var pendingSocket: String?
  var hasPendingStart: Bool { pendingConfig != nil }

  private let preferences: UserDefaults
  private let readiness: ServiceReadiness
  private let clientFactory: (URL, URL) -> BackendClient
  init(
    preferences: UserDefaults = .standard, readiness: ServiceReadiness? = nil,
    clientFactory: @escaping (URL, URL) -> BackendClient = {
      BackendClient(executable: $0, socket: $1)
    }
  ) {
    self.preferences = preferences
    self.readiness = readiness ?? ServiceReadiness()
    self.clientFactory = clientFactory
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
  var isPerformingUserAction: Bool { busy && !polling }
  var receiverStatusIsCurrent: Bool { connected && isActive }
  var recordingStateLabel: String {
    guard connected else { return "Disconnected" }
    return isActive ? state.capitalized : "Idle"
  }
  var receiverStatusHeading: String {
    if receiverStatusIsCurrent { return "Receiver status" }
    return state == "idle" ? "Receiver status" : "Last recording · \(state.capitalized)"
  }

  func connect(executable: URL?) {
    guard let executable, !busy else { return }
    connectionEpoch = UUID()
    connected = false
    pollTask?.cancel()
    preferences.set(socketPath, forKey: "serviceSocket")
    client = clientFactory(executable, URL(fileURLWithPath: socketPath))
    perform {
      guard let client = self.client else { return }
      let status = try JSONDecoder().decode(JSONValue.self, from: await client.request(["status"]))
      self.captureEnabled = status["capture_control"] == .bool(true)
      self.backendBuild = status["build"]?["display_version"]?.string
      if case .array(let sources) = status["session_sources"] {
        self.sessionSources = sources.compactMap(\.string)
      } else {
        self.sessionSources = []
      }
      if case .array(let transports) = status["rayhunter_transports"] {
        self.rayhunterTransports = transports.compactMap(\.string)
      } else {
        self.rayhunterTransports = []
      }
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
      let deadline = readiness.deadline()
      let startupEpoch = UUID()
      connectionEpoch = startupEpoch
      startupTask = Task {
        do {
          try readiness.check(deadline)
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
          try await readiness.wait(
            client: clientFactory(executable, socket),
            deadline: deadline, running: running)
          guard connectionEpoch == startupEpoch else { throw CancellationError() }
          startupTask = nil
          busy = false
          connect(executable: executable)
        } catch {
          if connectionEpoch == startupEpoch {
            startupTask = nil
            busy = false
            self.error = error.localizedDescription
          }
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

  var canSubmitAction: Bool { connected && (!busy || polling) && !userActionQueued }

  var startUnavailableReason: String? {
    if !connected { return "Connect to the service to start a recording." }
    if !captureEnabled {
      return "This service provides recording history only. Connect to a capture-enabled service."
    }
    if isActive && !hasPendingStart {
      return "A recording is already running. Finish it before starting another."
    }
    if userActionQueued { return "Your action is waiting for the current status update." }
    if busy && !polling { return "Wait for the current operation to finish." }
    if configText.isEmpty { return "Load a saved setup or select receivers to create a scan plan." }
    return nil
  }

  var canEditPlan: Bool {
    !isActive && (!busy || polling) && !userActionQueued && !hasPendingStart
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
    if source["kind"] == .string("flipper_marauder"), !canAddFlipperBridge {
      error =
        "Connect to a backend with Flipper bridge support before adding this prototype receiver."
      return
    }
    editPlan {
      try PlanEditing.add(
        $0, id: "receiver-" + UUID().uuidString.prefix(8).lowercased(), source: source)
    }
  }
  var canAddFlipperBridge: Bool {
    connected && canEditPlan && sessionSources.contains("flipper_marauder")
  }
  func addFlipperBridge(port: String) {
    let endpoint = port.trimmingCharacters(in: .whitespacesAndNewlines)
    guard canAddFlipperBridge else {
      error =
        "Connect to a backend with Flipper bridge support and finish any active recording before editing the plan."
      return
    }
    guard !endpoint.isEmpty else {
      error = "Choose the Flipper USB serial port."
      return
    }
    addReceiver(source: .object(["kind": .string("flipper_marauder"), "port": .string(endpoint)]))
  }
  var canAddMarauderCapture: Bool {
    connected && canEditPlan && sessionSources.contains("marauder_capture")
  }
  func addMarauderCapture(url: URL, format: String) {
    guard canAddMarauderCapture else {
      error =
        "Connect to a backend with saved Marauder capture support and finish any active recording before editing the plan."
      return
    }
    guard url.isFileURL, ["pcap", "serial"].contains(format) else {
      error = "Choose a local file and an explicit PCAP or serial capture format."
      return
    }
    addReceiver(
      source: .object([
        "kind": .string("marauder_capture"), "path": .string(url.path),
        "format": .string(format),
      ]))
  }
  func removeReceiver(id: String) { editPlan { try PlanEditing.remove($0, id: id) } }
  func planSourceField(id: String, field: String, value: JSONValue) {
    editPlan { try PlanEditing.change($0, receiverID: id, field: field, value: value) }
  }
  var canManageRayhunterUSB: Bool {
    connected && canEditPlan && rayhunterTransports.contains("managed_adb_usb")
  }
  func setRayhunterConnection(id: String, managedUSB: Bool) {
    guard !managedUSB || canManageRayhunterUSB else {
      error = "Connect to a backend with managed USB support before selecting this connection."
      return
    }
    editPlan { try PlanEditing.rayhunterConnection($0, receiverID: id, managedUSB: managedUSB) }
  }
  func setRayhunterADBField(id: String, field: String, value: String) {
    guard canManageRayhunterUSB, ["executable", "serial"].contains(field),
      let receiver = plannedReceivers.first(where: { $0.id == id }),
      receiver.kind == "rayhunter", case .object(var adb) = receiver.source["adb"]
    else { return }
    adb[field] = .string(value)
    planSourceField(id: id, field: "adb", value: .object(adb))
  }
  var planLocation: JSONValue? {
    (try? JSONDecoder().decode(JSONValue.self, from: Data(configText.utf8)))?["location"]
  }
  func setLocation(_ value: JSONValue?) { editPlan { try PlanEditing.location($0, value: value) } }
  func selectLocationDevice(_ hint: JSONValue) {
    if let kind = planLocation?["kind"]?.string, ["gpsd", "managed_gpsd"].contains(kind),
      let device = hint["device"]
    {
      setLocationField("device", value: device)
    } else {
      setLocation(hint)
    }
  }
  func setManagedLocation(_ managed: Bool) {
    guard var location = planLocation, case .object(var fields) = location else { return }
    fields["kind"] = .string(managed ? "managed_gpsd" : "gpsd")
    if managed {
      fields["endpoint"] = .string("127.0.0.1:12947")
      fields["executable"] = .string("")
      fields["device"] = fields["device"] ?? .string("")
    } else {
      fields.removeValue(forKey: "executable")
    }
    location = .object(fields)
    setLocation(location)
  }
  func setLocationField(_ name: String, value: JSONValue) {
    guard case .object(var location) = planLocation else { return }
    location[name] = value
    setLocation(.object(location))
  }

  func refreshDevices() {
    guard let client else { return }
    let connection = connectionEpoch
    submitUserAction {
      try self.requireCurrent(client, epoch: connection)
      let inventory = try await client.inventory()
      try self.requireCurrent(client, epoch: connection)
      self.inventory = inventory
    }
  }

  func canReadGPSVersion(_ target: GPSVersionTarget) -> Bool {
    canSubmitAction && captureEnabled && !isActive && !hasPendingStart
      && target.isCurrent(in: inventory)
  }

  func gpsVersion(for target: GPSVersionTarget) -> JSONValue? {
    guard connected, target.isCurrent(in: inventory) else { return nil }
    return gpsIdentifications[target]
  }

  func readGPSVersion(_ target: GPSVersionTarget) {
    guard canReadGPSVersion(target), let client else { return }
    let connection = connectionEpoch
    submitUserAction {
      try self.requireCurrent(client, epoch: connection)
      self.gpsIdentifications = [:]
      self.inventory = try await client.inventory()
      try self.requireCurrent(client, epoch: connection)
      let capture = try await client.status()
      try self.requireCurrent(client, epoch: connection)
      guard self.captureEnabled, capture["active"] == .bool(false), !self.hasPendingStart,
        target.isCurrent(in: self.inventory)
      else {
        throw ModelError.invalid(
          "The GPS connection or recording changed. Refresh devices and try again when idle.")
      }
      let result = try await client.identifyDevice(config: target.request.data())
      try self.requireCurrent(client, epoch: connection)
      self.inventory = try await client.inventory()
      try self.requireCurrent(client, epoch: connection)
      guard target.isCurrent(in: self.inventory), target.matches(result) else {
        throw ModelError.invalid(
          "The GPS connection changed. Its previous version result was discarded.")
      }
      self.gpsIdentifications[target] = result
    }
  }

  func refreshSetups() {
    guard let client else { return }
    let connection = connectionEpoch
    submitUserAction {
      try self.requireCurrent(client, epoch: connection)
      let setups = try await client.setups()
      try self.requireCurrent(client, epoch: connection)
      self.setups = setups
      self.setupError = nil
    }
  }
  func reviewSetup() {
    guard canEditPlan, !selectedSetupID.isEmpty, let client else { return }
    let id = selectedSetupID
    let connection = connectionEpoch
    submitUserAction {
      try self.requireCurrent(client, epoch: connection)
      let review = try await client.loadSetup(id: id)
      try self.requireCurrent(client, epoch: connection)
      guard self.canEditPlanAfterDispatch else {
        throw ModelError.invalid("The recording changed. Review the setup again after it finishes.")
      }
      self.setupReview = review
    }
  }
  private var canEditPlanAfterDispatch: Bool { !isActive && !hasPendingStart }
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
  // Callers capture the submitted plan, backend and run ID before queuing.
  @discardableResult
  func submitSetupMutation(_ operation: @escaping () async throws -> Void) -> Bool {
    guard connected, canEditPlan else {
      error = "The setup cannot be changed right now. Wait for the current operation and try again."
      return false
    }
    return submitUserAction(operation)
  }

  @discardableResult
  private func submitUserAction(_ operation: @escaping () async throws -> Void) -> Bool {
    guard canSubmitAction else {
      error = "Wait for the current operation or reconnect, then try again."
      return false
    }
    if polling {
      pendingUserAction = operation
      userActionQueued = true
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
    let operation = pendingUserAction
    pendingUserAction = nil
    userActionQueued = false
    if let operation { perform(operation) }
  }

  func validate() {
    guard let client else { return }
    let config = Data(configText.utf8)
    let connection = connectionEpoch
    submitUserAction {
      try self.requireCurrent(client, epoch: connection)
      let value = try await client.validate(config: config)
      self.validation = String(decoding: try value.data(pretty: true), as: UTF8.self)
      self.message = "Plan validated. Receiver readiness is checked when capture starts."
    }
  }

  func start() {
    guard let client else { return }
    let config = Data(configText.utf8)
    let connection = connectionEpoch
    submitUserAction {
      try self.requireCurrent(client, epoch: connection)
      guard self.captureEnabled, !self.isActive || self.hasPendingStart else {
        throw ModelError.invalid("The recording state changed. Review it before starting.")
      }
      if let pending = self.pendingConfig,
        pending != config || self.pendingSocket != client.socket.path
      {
        throw ModelError.invalid(
          "A previous start has an uncertain outcome. Reconnect and check the active recording before changing the plan."
        )
      }
      if self.pendingConfig == nil { _ = try await client.validate(config: config) }
      try self.requireCurrent(client, epoch: connection)
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

  struct ControlTarget {
    fileprivate let runID: String
    fileprivate let client: BackendClient
    fileprivate let connection: UUID
  }

  func controlTarget() -> ControlTarget? {
    guard connected, isActive, let id = activeID, let client else { return nil }
    return ControlTarget(runID: id, client: client, connection: connectionEpoch)
  }

  func control(_ action: CaptureControl, target: ControlTarget? = nil) {
    guard let target = target ?? controlTarget() else { return }
    let id = target.runID
    let client = target.client
    let connection = target.connection
    submitUserAction {
      try self.requireCurrent(client, epoch: connection)
      guard self.isActive, self.activeID == id else {
        throw ModelError.invalid("The active recording changed. Review it before continuing.")
      }
      let result = try await client.control(action, runID: id)
      if result["state"]?.string == "failed" {
        self.error =
          result["error"]?.string ?? "Capture finished with a failure. Review receiver status."
      }
      self.runs = []
      try await self.update()
    }
  }

  private func requireCurrent(_ submitted: BackendClient, epoch: UUID) throws {
    guard connected, connectionEpoch == epoch, let current = client,
      current.socket == submitted.socket,
      current.executable == submitted.executable
    else {
      throw ModelError.invalid("The service changed. Reconnect and review before continuing.")
    }
  }

  func refreshHistory(more: Bool = false) {
    guard let client else { return }
    let connection = connectionEpoch
    let before = more ? nextRun : nil
    submitUserAction {
      try self.requireCurrent(client, epoch: connection)
      let page = try await client.recordings(before: before)
      try self.requireCurrent(client, epoch: connection)
      let existing = Set(self.runs.map(\.id))
      self.runs = more ? self.runs + page.runs.filter { !existing.contains($0.id) } : page.runs
      self.nextRun = page.hasMore ? page.nextBefore : nil
    }
  }

  func showCurrentRecordingIfNeeded(app: AppModel) {
    guard connected, !busy, !app.loadingRecording, browsingRun == nil, app.recording == nil,
      let id = activeID ?? capture?["last"]?["run_id"]?.string,
      let run = runs.first(where: { $0.id == id })
    else { return }
    open(run, app: app)
  }

  func open(_ run: StoredRun, app: AppModel) {
    guard canSubmitAction, let client else { return }
    let connection = connectionEpoch
    app.beginRecordingSelection()
    let selection = app.recordingSelection
    submitUserAction {
      try self.requireCurrent(client, epoch: connection)
      guard app.recordingSelection == selection else { return }
      self.browsingRun = run.id
      self.observationApp = app
      self.window = RecordWindow()
      self.recordingEpoch = UUID()
      self.hasPresentedWindow = false
      self.followLive = self.isActive && self.activeID == run.id
      app.selectedTab = "observations"
      app.recordingRevision = UUID()
      try await self.fetchRecords()
    }
  }

  func loadMoreRecords() { submitRecordRead(rewind: false) }
  func rewindRecords() { submitRecordRead(rewind: true) }

  private func submitRecordRead(rewind: Bool) {
    guard let client, let app = observationApp else { return }
    let connection = connectionEpoch
    let epoch = recordingEpoch
    let selection = app.recordingSelection
    submitUserAction {
      try self.requireCurrent(client, epoch: connection)
      guard self.recordingEpoch == epoch, app.recordingSelection == selection else { return }
      if rewind {
        self.window = RecordWindow()
        self.recordingEpoch = UUID()
        self.hasPresentedWindow = false
        self.followLive = false
        app.recordingRevision = UUID()
      }
      try await self.fetchRecords()
    }
  }

  private func fetchRecords() async throws {
    guard let client, let id = browsingRun, let app = observationApp else { return }
    let epoch = recordingEpoch
    let selection = app.recordingSelection
    let current = window
    do {
      let page = try await client.records(runID: id, after: current.cursor, limit: 500)
      guard recordingEpoch == epoch, browsingRun == id, app.recordingSelection == selection else {
        return
      }
      // Serialization, decoding and evidence formatting can take several frames.
      // Keep that work off the main actor while the native table is scrolling.
      let updated = try await Task.detached(priority: .userInitiated) {
        var updated = current
        try updated.append(page)
        return updated
      }.value
      try Task.checkCancellation()
      guard recordingEpoch == epoch, browsingRun == id, observationApp === app,
        app.recordingSelection == selection
      else { return }
      window = updated
      hasMoreRecords = page.hasMore
      // Polling an unchanged recording must not invalidate every table row.
      guard !page.records.isEmpty || !hasPresentedWindow else { return }
      app.recording = try window.observations()
      app.recordingLabel =
        "\(window.records.count) recent records · \(window.omitted) earlier records outside this view"
      app.isExample = false
      hasPresentedWindow = true
    } catch {
      guard recordingEpoch == epoch, browsingRun == id, observationApp === app,
        app.recordingSelection == selection
      else { return }
      throw error
    }
  }

  func leaveRecording() {
    browsingRun = nil
    followLive = false
    observationApp = nil
    window = RecordWindow()
    recordingEpoch = UUID()
    hasPresentedWindow = false
  }

  func export(to destination: URL? = nil) {
    guard canSubmitAction, let client else { return }
    let connection = connectionEpoch
    let url: URL
    if let destination {
      url = destination
    } else {
      let panel = NSSavePanel()
      panel.title = "Export recording archive"
      panel.nameFieldStringValue = "AirHound-\(Int(Date().timeIntervalSince1970)).sqlite"
      panel.message = "Exports a consistent snapshot of all recordings. Choose a new filename."
      guard panel.runModal() == .OK, let selected = panel.url else { return }
      url = selected
    }
    submitUserAction {
      try self.requireCurrent(client, epoch: connection)
      _ = try await client.export(to: url)
      try self.requireCurrent(client, epoch: connection)
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
    connectionEpoch = UUID()
    if let startup = startupTask {
      startupTask = nil
      startup.cancel()
      busy = false
    }
    recordingEpoch = UUID()
    pollTask?.cancel()
    pollTask = nil
    connected = false
  }
}
