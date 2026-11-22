import AirHoundCore
import AppKit
import SwiftUI
import XCTest

@testable import AirHoundApp

@MainActor
final class SetupPresentationTests: XCTestCase {
  func testPlanEditorDoesNotReplaceSessionOptions() throws {
    let suite = "AirHound-setup-tests-" + UUID().uuidString
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let session = SessionModel(preferences: defaults)
    session.configText =
      #"{"version":1,"alert_policy":"silent","location":{"kind":"gpsd","endpoint":"127.0.0.1:2947"},"receivers":[{"id":"my-sensor","source":{"kind":"airhound_serial","port":"/dev/cu.fixture","baud":460800}}]}"#
    session.planSourceField(id: "my-sensor", field: "baud", value: .integer(115200))
    session.addReceiver(source: .object(["kind": .string("native_ble")]))
    let config = try JSONDecoder().decode(JSONValue.self, from: Data(session.configText.utf8))
    XCTAssertEqual(config["alert_policy"], .string("silent"))
    XCTAssertEqual(config["location"]?["kind"], .string("gpsd"))
    XCTAssertEqual(session.plannedReceivers[0].id, "my-sensor")
    session.busy = true
    let unchanged = session.configText
    session.removeReceiver(id: "my-sensor")
    XCTAssertEqual(session.configText, unchanged)
  }

  func testMarauderCaptureRequiresCapabilityAndPreservesPlan() throws {
    let suite = "marauder-plan-" + UUID().uuidString
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let session = SessionModel(preferences: defaults)
    session.configText =
      #"{"version":1,"alert_policy":"silent","location":{"kind":"gpsd","endpoint":"127.0.0.1:2947"},"receivers":[{"id":"sensor","source":{"kind":"native_ble"}}]}"#
    let original = session.configText
    let file = URL(fileURLWithPath: "/tmp/saved Marauder.pcap")
    session.connected = true
    session.addMarauderCapture(url: file, format: "pcap")
    XCTAssertEqual(
      session.configText, original, "An older backend cannot add an unsupported source")
    session.sessionSources = ["marauder_capture"]
    session.error = nil
    session.beginBackgroundRefresh()
    session.addMarauderCapture(url: file, format: "pcap")
    session.finishBackgroundRefresh()
    XCTAssertNil(session.error)
    XCTAssertEqual(session.plannedReceivers.count, 2)
    let added = try XCTUnwrap(session.plannedReceivers.last)
    XCTAssertEqual(added.title, "Saved Marauder capture")
    XCTAssertEqual(added.source["format"], .string("pcap"))
    XCTAssertEqual(added.source["path"], .string(file.path))
    let config = try JSONDecoder().decode(JSONValue.self, from: Data(session.configText.utf8))
    XCTAssertEqual(config["alert_policy"], .string("silent"))
    XCTAssertEqual(config["location"]?["kind"], .string("gpsd"))
    XCTAssertFalse(session.isActive, "Adding a file never starts a recording")
    let valid = session.configText
    session.addMarauderCapture(url: file, format: "guess")
    XCTAssertEqual(session.configText, valid)
    session.connected = false
    session.addMarauderCapture(url: file, format: "serial")
    XCTAssertEqual(
      session.configText, valid, "Cached capabilities cannot authorize a disconnected backend")
    session.connected = true
    session.capture = .object(["active": .bool(true)])
    session.addMarauderCapture(url: file, format: "serial")
    XCTAssertEqual(session.configText, valid, "The active plan cannot be edited")
  }

  func testStartExplanationMatchesConnectionPlanAndOperationGates() {
    let session = SessionModel()
    XCTAssertEqual(session.startUnavailableReason, "Connect to the service to start a recording.")
    session.connected = true
    XCTAssertTrue(session.startUnavailableReason?.contains("history only") == true)
    session.captureEnabled = true
    XCTAssertTrue(session.startUnavailableReason?.contains("scan plan") == true)
    session.configText = "{}"
    XCTAssertNil(session.startUnavailableReason)
    session.busy = true
    XCTAssertEqual(session.startUnavailableReason, "Wait for the current operation to finish.")
    session.busy = false
    session.beginBackgroundRefresh()
    XCTAssertNil(session.startUnavailableReason, "Polling must not disable Start")
    XCTAssertTrue(session.submitSetupMutation {})
    XCTAssertEqual(
      session.startUnavailableReason, "Your action is waiting for the current status update.")
    session.capture = .object(["active": .bool(true), "run_id": .string("active")])
    XCTAssertTrue(session.startUnavailableReason?.contains("already running") == true)
  }

  func testSetupIntentWaitsForPollingAndRunsOnce() async throws {
    let session = SessionModel()
    session.connected = true
    session.beginBackgroundRefresh()
    var executed: [String] = []
    XCTAssertTrue(session.submitSetupMutation { executed.append("save submitted plan") })
    XCTAssertTrue(session.userActionQueued)
    XCTAssertFalse(session.canEditPlan)
    XCTAssertFalse(session.submitSetupMutation { executed.append("duplicate") })
    XCTAssertTrue(executed.isEmpty)
    session.finishBackgroundRefresh()
    for _ in 0..<20 where session.busy { await Task.yield() }
    XCTAssertEqual(executed, ["save submitted plan"])
    XCTAssertFalse(session.userActionQueued)
    XCTAssertFalse(session.busy)
    session.beginBackgroundRefresh()
    XCTAssertTrue(session.submitSetupMutation { executed.append("delete submitted ID") })
    session.finishBackgroundRefresh()
    for _ in 0..<20 where session.busy { await Task.yield() }
    XCTAssertEqual(executed, ["save submitted plan", "delete submitted ID"])
    session.busy = true
    XCTAssertFalse(session.submitSetupMutation { executed.append("rejected") })
    XCTAssertNotNil(session.error)
  }

  func testReceiverIdentityRequiresMatchingActiveRunSource() throws {
    let session = SessionModel()
    session.connected = true
    session.configText =
      #"{"version":1,"receivers":[{"id":"same-id","source":{"kind":"sniffle","port":"/dev/cu.one","python":"/python","helper":"/helper"}}]}"#
    session.capture = try JSONDecoder().decode(
      JSONValue.self,
      from: Data(
        #"{"active":true,"run_id":"run-1","receiver_health":{"receivers":[{"id":"same-id","receiver":{"adapter_info":"actual firmware"}}]}}"#
          .utf8))
    func stored(_ source: String) throws -> StoredRun {
      try JSONDecoder().decode(
        StoredRun.self,
        from: Data(
          (#"{"id":"run-1","state":"recording","mode":"live","started_at_us":0,"record_count":0,"config":{"session":{"receivers":[{"id":"same-id","source":SOURCE}]}}}"#
            .replacingOccurrences(of: "SOURCE", with: source)).utf8))
    }
    XCTAssertNil(session.currentReceiverIdentity(id: "same-id"))
    session.runs = [try stored(#"{"kind":"native_ble"}"#)]
    XCTAssertNil(session.currentReceiverIdentity(id: "same-id"))
    session.runs = [
      try stored(
        #"{"kind":"sniffle","port":"/dev/cu.other","python":"/python","helper":"/helper","channel":38,"coded":false,"module_path":null}"#
      )
    ]
    XCTAssertNil(session.currentReceiverIdentity(id: "same-id"))
    session.runs = [
      try stored(
        #"{"kind":"sniffle","port":"/dev/cu.one","python":"/python","helper":"/helper","channel":38,"coded":false,"module_path":null}"#
      )
    ]
    XCTAssertEqual(
      session.currentReceiverIdentity(id: "same-id")?["adapter_info"], .string("actual firmware"))
    session.connected = false
    XCTAssertNil(session.currentReceiverIdentity(id: "same-id"))
    session.connected = true
    session.capture = .object(["active": .bool(false), "run_id": .string("run-1")])
    XCTAssertNil(session.currentReceiverIdentity(id: "same-id"))
  }

  func testDisconnectedServiceDoesNotPresentCachedGPSFixAsCurrent() {
    let cached = JSONValue.object([
      "state": .string("fix"), "daemon": .object(["version": .string("3.27.5")]),
    ])
    XCTAssertEqual(
      LocationProviderStatus(status: cached, connected: true).stateLabel, "Fix available")
    XCTAssertEqual(
      LocationProviderStatus(status: cached, connected: false).stateLabel,
      "Service disconnected; current fix unknown")
  }

  func testManagedGPSDeviceSelectionPreservesHelperAndOtherPlanFields() throws {
    let suite = "gps-edit-" + UUID().uuidString
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let session = SessionModel(preferences: defaults)
    session.configText =
      #"{"version":1,"alert_policy":"silent","location":{"kind":"gpsd","endpoint":"127.0.0.1:2947","device":"/dev/cu.old"},"receivers":[{"id":"ble","source":{"kind":"native_ble"}}]}"#
    session.setManagedLocation(true)
    session.setLocationField("executable", value: .string("/opt/homebrew/sbin/gpsd"))
    session.selectLocationDevice(
      .object(["kind": .string("gpsd"), "device": .string("/dev/cu.new")]))
    XCTAssertEqual(session.planLocation?["kind"], .string("managed_gpsd"))
    XCTAssertEqual(session.planLocation?["executable"], .string("/opt/homebrew/sbin/gpsd"))
    XCTAssertEqual(session.planLocation?["device"], .string("/dev/cu.new"))
    let config = try JSONDecoder().decode(JSONValue.self, from: Data(session.configText.utf8))
    XCTAssertEqual(config["alert_policy"], .string("silent"))
    XCTAssertEqual(session.plannedReceivers.first?.id, "ble")
    session.setManagedLocation(false)
    XCTAssertEqual(session.planLocation?["kind"], .string("gpsd"))
    XCTAssertNil(session.planLocation?["executable"])
    XCTAssertEqual(session.planLocation?["device"], .string("/dev/cu.new"))
  }

  func testSelectedBackendProfilesPreserveM5GPSPlanWithoutStartingCapture() async throws {
    guard let backend = ProcessInfo.processInfo.environment["AIRHOUND_TEST_BACKEND"] else {
      throw XCTSkip("Set AIRHOUND_TEST_BACKEND for combined setup lifecycle tests.")
    }
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(
      "ah-plan-" + UUID().uuidString.prefix(8))
    try LocalFiles.privateDirectory(root)
    defer { try? FileManager.default.removeItem(at: root) }
    let pcap = root.appendingPathComponent("empty.pcap")
    try Data([
      0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0, 0, 127, 0, 0, 0,
    ]).write(to: pcap)
    let database = root.appendingPathComponent("recordings.sqlite")
    let replay = Process()
    replay.executableURL = URL(fileURLWithPath: backend)
    replay.arguments = [
      "--wifi-pcap", pcap.path, "--database", database.path,
      "--output", root.appendingPathComponent("empty.csv").path,
    ]
    replay.standardOutput = FileHandle.nullDevice
    replay.standardError = FileHandle.nullDevice
    try replay.run()
    replay.waitUntilExit()
    XCTAssertEqual(replay.terminationStatus, 0)
    let socket = root.appendingPathComponent("control.sock")
    let server = Process()
    server.executableURL = URL(fileURLWithPath: backend)
    server.arguments = ["serve", "--socket", socket.path, "--database", database.path]
    server.standardOutput = FileHandle.nullDevice
    server.standardError = FileHandle.nullDevice
    try server.run()
    defer {
      if server.isRunning {
        server.terminate()
        server.waitUntilExit()
      }
    }
    for _ in 0..<100 {
      if FileManager.default.fileExists(atPath: socket.path) { break }
      try await Task.sleep(for: .milliseconds(50))
    }
    let wrapper = root.appendingPathComponent("selected-backend.sh")
    let log = root.appendingPathComponent("client-arguments")
    func quote(_ value: String) -> String {
      "'" + value.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }
    let script = """
      #!/bin/sh
      printf '%s\\n' "$@" >> \(quote(log.path))
      exec \(quote(backend)) "$@"
      """
    try Data(script.utf8).write(to: wrapper)
    try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: wrapper.path)
    let suite = "AirHound-setup-route-" + UUID().uuidString
    let preferences = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { preferences.removePersistentDomain(forName: suite) }
    let session = SessionModel(preferences: preferences)
    session.socketPath = socket.path
    session.connect(executable: wrapper)
    defer { session.disconnect() }
    func idle() async throws {
      for _ in 0..<100 {
        if !session.busy { return }
        try await Task.sleep(for: .milliseconds(50))
      }
      XCTFail("Setup operation did not finish")
    }
    try await idle()
    XCTAssertTrue(session.connected, session.error ?? session.message)
    XCTAssertFalse(session.captureEnabled, "This test service must never allow radio capture")
    XCTAssertTrue(session.setupsAvailable)
    XCTAssertEqual(session.locationProviders, ["gpsd", "managed_gpsd"])
    let plan =
      #"{"version":1,"alert_policy":"silent","location":{"kind":"gpsd","endpoint":"127.0.0.1:2947","device":"/dev/cu.fixture-gps"},"receivers":[{"id":"custom-m5","source":{"kind":"airhound_serial","port":"/dev/cu.fixture-m5","baud":460800}}]}"#
    session.configText = plan
    session.saveSetup(name: "M5 and GPS")
    try await idle()
    XCTAssertNil(session.error)
    XCTAssertEqual(session.setups.count, 1)
    session.configText = #"{"version":1,"receivers":[]}"#
    session.reviewSetup()
    try await idle()
    let review = try XCTUnwrap(session.setupReview)
    XCTAssertEqual(session.plannedReceivers.count, 0, "Review must not apply automatically")
    session.applySetup(review)
    XCTAssertEqual(
      try JSONDecoder().decode(JSONValue.self, from: Data(session.configText.utf8)),
      try JSONDecoder().decode(JSONValue.self, from: Data(plan.utf8)))
    XCTAssertFalse(session.isActive)
    session.deleteSelectedSetup()
    try await idle()
    XCTAssertTrue(session.setups.isEmpty)
    XCTAssertEqual(session.plannedReceivers.map(\.id), ["custom-m5"])
    let arguments = try String(contentsOf: log, encoding: .utf8).split(separator: "\n").map(
      String.init)
    XCTAssertTrue(arguments.contains("profiles"))
    for command in ["list", "save", "load", "delete"] { XCTAssertTrue(arguments.contains(command)) }
    XCTAssertFalse(arguments.contains("start"))
    XCTAssertFalse(arguments.contains("finish"))
  }

  /// Optional bounded screenshot fixture. Never connects to a service or opens radios.
  func testNativeSetupPresentationSnapshot() throws {
    guard let destination = ProcessInfo.processInfo.environment["AIRHOUND_UI_SNAPSHOT_DIR"] else {
      throw XCTSkip("Set AIRHOUND_UI_SNAPSHOT_DIR for the synthetic native presentation check.")
    }
    let app = AppModel()
    let session = app.session
    session.connected = true
    session.captureEnabled = true
    session.setupsAvailable = true
    session.locationProviders = ["gpsd"]
    session.backendBuild = "git fixture (synthetic preview)"
    session.configText =
      #"{"version":1,"alert_policy":"silent","location":{"kind":"gpsd","endpoint":"127.0.0.1:2947","device":"/dev/cu.example-gps"},"receivers":[{"id":"system-ble","source":{"kind":"native_ble"}},{"id":"m5","source":{"kind":"airhound_serial","port":"/dev/cu.example-m5","baud":460800}},{"id":"cat","source":{"kind":"sniffle","port":"/dev/cu.example-cat","python":"/example/python","helper":"/example/sniffle-source.py","channel":37}}]}"#
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    session.setups = try decoder.decode(
      SavedSetupList.self,
      from: Data(
        #"{"schema_version":1,"profiles":[{"id":"car","name":"Car kit","saved_at_ms":1,"receiver_count":3}]}"#
          .utf8)
    ).profiles
    session.selectedSetupID = "car"
    for (name, scheme, appearance) in [
      ("light", ColorScheme.light, NSAppearance.Name.aqua),
      ("dark", ColorScheme.dark, NSAppearance.Name.darkAqua),
    ] {
      let window = NSWindow(
        contentRect: NSRect(x: -3000, y: -3000, width: 1280, height: 820), styleMask: [.titled],
        backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.appearance = NSAppearance(named: appearance)
      let view = NSHostingView(
        rootView: SessionView(session: session, app: app).environment(\.colorScheme, scheme)
          .background(Color(nsColor: .windowBackgroundColor)))
      window.contentView = view
      window.orderBack(nil)
      for _ in 0..<10 {
        view.layoutSubtreeIfNeeded()
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.02))
      }
      let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
      view.cacheDisplay(in: view.bounds, to: bitmap)
      let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
      try png.write(
        to: URL(fileURLWithPath: destination).appendingPathComponent("setup-\(name).png"))
      XCTAssertGreaterThan(view.bounds.width, 1000)
      window.close()
    }
  }
}
