import AirHoundCore
import XCTest

@testable import AirHoundApp

@MainActor
final class SessionTests: XCTestCase {
  func testServiceOwnedLifetimeAcrossPresentationDisconnect() async throws {
    guard let path = ProcessInfo.processInfo.environment["AIRHOUND_TEST_BACKEND"] else {
      throw XCTSkip("Set AIRHOUND_TEST_BACKEND for app/service lifecycle tests.")
    }
    let suite = "AirHound-tests-" + UUID().uuidString
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(
      "ah-ui-" + UUID().uuidString.prefix(8))
    try LocalFiles.privateDirectory(root)
    defer { try? FileManager.default.removeItem(at: root) }
    let model = SessionModel(preferences: defaults)
    model.socketPath = root.appendingPathComponent("control.sock").path
    model.startService(executable: URL(fileURLWithPath: path))
    defer {
      model.disconnect()
      if let process = model.service, process.isRunning {
        process.terminate()
        process.waitUntilExit()
      }
    }
    for _ in 0..<100 {
      if model.connected && !model.busy { break }
      try await Task.sleep(for: .milliseconds(50))
    }
    XCTAssertTrue(model.connected, model.error ?? model.message)
    XCTAssertFalse(
      model.configText.isEmpty,
      "Native discovery should offer a default plan without starting capture")
    XCTAssertFalse(model.isActive)
    let service = try XCTUnwrap(model.service)
    model.disconnect()  // The same path called by app shutdown.
    XCTAssertTrue(service.isRunning)
    let pcap = root.appendingPathComponent("empty.pcap")
    try Data([
      0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0, 0, 127, 0, 0, 0,
    ]).write(to: pcap)
    let config = try JSONValue.object([
      "version": .integer(1),
      "receivers": .array([
        .object([
          "id": .string("replay"),
          "source": .object(["kind": .string("wifi_pcap"), "path": .string(pcap.path)]),
        ])
      ]),
    ]).data()
    let socket = URL(fileURLWithPath: model.socketPath)
    let store = PendingStartStore(socket: socket)
    let pending = try store.prepare(socket: socket, config: config)
    let client = BackendClient(executable: URL(fileURLWithPath: path), socket: socket)
    let accepted = try await client.start(config: config, requestID: pending.requestID)
    // Simulate app loss after server acceptance but before clearing the durable request.
    let replacement = SessionModel(preferences: defaults)
    XCTAssertEqual(replacement.socketPath, model.socketPath)
    replacement.connect(executable: URL(fileURLWithPath: path))
    defer { replacement.disconnect() }
    for _ in 0..<100 {
      if replacement.connected && !replacement.busy { break }
      try await Task.sleep(for: .milliseconds(50))
    }
    XCTAssertTrue(replacement.connected, replacement.error ?? replacement.message)
    XCTAssertTrue(replacement.hasPendingStart)
    XCTAssertEqual(replacement.configText, String(decoding: config, as: UTF8.self))
    // Remove replay input: idempotent recovery must not revalidate an already accepted run.
    try FileManager.default.removeItem(at: pcap)
    replacement.start()
    for _ in 0..<100 {
      if !replacement.busy { break }
      try await Task.sleep(for: .milliseconds(50))
    }
    XCTAssertNil(replacement.error)
    XCTAssertFalse(replacement.hasPendingStart)
    XCTAssertNil(try store.load())
    let runs = try await client.recordings()
    XCTAssertEqual(runs.runs.count, 1)
    XCTAssertEqual(runs.runs.first?.id, accepted["run_id"]?.string)
    XCTAssertTrue(service.isRunning)

    // Synthetic receiver obeys the real helper protocol; it never opens hardware.
    let helper = root.appendingPathComponent("fake-receiver.sh")
    let script = #"""
      #!/bin/sh
      printf '%s\n' '{"type":"hello","protocol_version":1}' '{"type":"status","state":"running"}'
      while IFS= read -r command; do
      id=${command##*:}; id=${id%?}
      case "$command" in
      *pause*) printf '{"type":"status","state":"paused","cleanup_verified":true,"request_id":%s}\n' "$id";;
      *resume*) printf '{"type":"status","state":"running","request_id":%s}\n' "$id";;
      *stop*) printf '{"type":"stopped","cleanup_verified":true,"request_id":%s}\n' "$id"; exit 0;;
      esac
      done
      exit 3
      """#
    try Data(script.utf8).write(to: helper)
    try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: helper.path)
    let firmware = root.appendingPathComponent("unused-test-firmware")
    try Data().write(to: firmware)
    let liveConfig = try JSONValue.object([
      "version": .integer(1),
      "receivers": .array([
        .object([
          "id": .string("synthetic"),
          "source": .object([
            "kind": .string("external_wifi"), "executable": .string(helper.path),
            "firmware": .string(firmware.path), "channels": .array([.integer(1)]),
            "dwell_ms": .integer(500),
          ]),
        ])
      ]),
    ]).data()
    replacement.configText = String(decoding: liveConfig, as: UTF8.self)
    replacement.start()
    try await waitIdle(replacement)
    XCTAssertNil(replacement.error)
    XCTAssertTrue(replacement.isActive)
    let activeRun = try XCTUnwrap(replacement.activeID)
    replacement.disconnect()
    let afterDisconnect = try await client.status()
    XCTAssertEqual(afterDisconnect["run_id"]?.string, activeRun)
    XCTAssertEqual(afterDisconnect["active"], .bool(true))
    replacement.connect(executable: URL(fileURLWithPath: path))
    try await waitIdle(replacement)
    XCTAssertEqual(replacement.activeID, activeRun)
    let presentation = AppModel()
    replacement.showCurrentRecordingIfNeeded(app: presentation)
    try await waitIdle(replacement)
    XCTAssertEqual(replacement.browsingRun, activeRun)
    XCTAssertTrue(replacement.followLive)
    XCTAssertNotNil(presentation.recording)
    // Repeated automatic selection must not reset a user's stopped follow state.
    replacement.followLive = false
    replacement.showCurrentRecordingIfNeeded(app: presentation)
    XCTAssertFalse(replacement.followLive)
    replacement.control(.pause)
    try await waitIdle(replacement)
    XCTAssertNil(replacement.error)
    XCTAssertEqual(replacement.state, "paused")
    replacement.control(.resume)
    try await waitIdle(replacement)
    XCTAssertNil(replacement.error)
    XCTAssertEqual(replacement.state, "recording")
    replacement.control(.finish)
    try await waitIdle(replacement)
    XCTAssertNil(replacement.error)
    XCTAssertFalse(replacement.isActive)
    XCTAssertEqual(replacement.state, "finished")
    replacement.leaveRecording()
    let completedPresentation = AppModel()
    replacement.showCurrentRecordingIfNeeded(app: completedPresentation)
    try await waitIdle(replacement)
    XCTAssertEqual(replacement.browsingRun, activeRun)
    XCTAssertFalse(replacement.followLive)
    XCTAssertNotNil(completedPresentation.recording)
    let finishedRuns = try await client.recordings()
    XCTAssertEqual(finishedRuns.runs.first?.id, activeRun)
    XCTAssertEqual(finishedRuns.runs.first?.state, "finished")
    let unchangedRevision = try XCTUnwrap(completedPresentation.recording?.revision)
    replacement.loadMoreRecords()
    try await waitIdle(replacement)
    XCTAssertEqual(
      completedPresentation.recording?.revision, unchangedRevision,
      "An empty page must not publish a new observation table")
    // Block an actual records subprocess, not merely the task queued by open().
    let wrapper = root.appendingPathComponent("delayed-client.sh")
    let entered = root.appendingPathComponent("entered")
    let release = root.appendingPathComponent("release")
    let armed = root.appendingPathComponent("armed")
    func quote(_ value: String) -> String {
      "'" + value.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }
    let delayedScript = """
      #!/bin/sh
      for argument in "$@"; do
        if [ "$argument" = records ] && [ -f \(quote(armed.path)) ]; then
          : > \(quote(entered.path))
          attempts=0
          while [ ! -f \(quote(release.path)) ]; do
            attempts=$((attempts + 1))
            [ "$attempts" -lt 200 ] || exit 70
            sleep 0.05
          done
        fi
      done
      exec \(quote(path)) "$@"
      """
    try Data(delayedScript.utf8).write(to: wrapper)
    try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: wrapper.path)
    let delayed = SessionModel(preferences: defaults)
    delayed.connect(executable: wrapper)
    defer {
      try? Data().write(to: release)
      delayed.disconnect()
    }
    try await waitIdle(delayed)
    XCTAssertTrue(delayed.connected)
    try Data().write(to: armed)
    let latePresentation = AppModel()
    delayed.open(try XCTUnwrap(finishedRuns.runs.first), app: latePresentation)
    for _ in 0..<100 {
      if FileManager.default.fileExists(atPath: entered.path) { break }
      try await Task.sleep(for: .milliseconds(50))
    }
    XCTAssertTrue(
      FileManager.default.fileExists(atPath: entered.path), "Records request must be in flight")
    delayed.leaveRecording()
    try Data().write(to: release)
    try await waitIdle(delayed)
    XCTAssertNil(delayed.browsingRun)
    XCTAssertNil(latePresentation.recording, "An in-flight page must not publish after leaving")

  }
  func testQueuedSetupActionsPersistSubmittedPlanAndDeleteSubmittedID() async throws {
    guard let path = ProcessInfo.processInfo.environment["AIRHOUND_TEST_BACKEND"] else {
      throw XCTSkip("Set AIRHOUND_TEST_BACKEND for private setup persistence tests.")
    }
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(
      "ah-setup-" + UUID().uuidString.prefix(8))
    try LocalFiles.privateDirectory(root)
    defer { try? FileManager.default.removeItem(at: root) }
    let socket = root.appendingPathComponent("control.sock")
    let pcap = root.appendingPathComponent("empty.pcap")
    try Data([
      0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0, 0, 127, 0, 0, 0,
    ]).write(to: pcap)
    let replay = Process()
    replay.executableURL = URL(fileURLWithPath: path)
    replay.arguments = [
      "--wifi-pcap", pcap.path, "--database", root.appendingPathComponent("runs.sqlite").path,
      "--output", root.appendingPathComponent("empty.csv").path,
    ]
    replay.standardOutput = FileHandle.nullDevice
    replay.standardError = FileHandle.nullDevice
    try replay.run()
    replay.waitUntilExit()
    XCTAssertEqual(replay.terminationStatus, 0)
    let service = Process()
    service.executableURL = URL(fileURLWithPath: path)
    service.arguments = [
      "serve", "--socket", socket.path, "--database",
      root.appendingPathComponent("runs.sqlite").path,
    ]
    service.standardOutput = FileHandle.nullDevice
    service.standardError = FileHandle.nullDevice
    try service.run()
    defer {
      if service.isRunning {
        service.terminate()
        service.waitUntilExit()
      }
    }
    for _ in 0..<100 where !FileManager.default.fileExists(atPath: socket.path) {
      try await Task.sleep(for: .milliseconds(20))
    }
    let suite = "AirHound-queued-setup-" + UUID().uuidString
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let model = SessionModel(preferences: defaults)
    model.socketPath = socket.path
    model.connect(executable: URL(fileURLWithPath: path))
    defer { model.disconnect() }
    try await waitIdle(model)
    XCTAssertTrue(model.connected, model.error ?? model.message)
    model.configText =
      #"{"version":1,"receivers":[{"id":"submitted","source":{"kind":"native_ble"}}]}"#
    model.beginBackgroundRefresh()
    XCTAssertTrue(model.saveSetup(name: "Submitted kit"))
    model.configText = #"{"version":1,"receivers":[]}"#
    model.finishBackgroundRefresh()
    try await waitIdle(model)
    XCTAssertNil(model.error)
    let id = model.selectedSetupID
    XCTAssertFalse(id.isEmpty)
    let client = BackendClient(executable: URL(fileURLWithPath: path), socket: socket)
    let loaded = try await client.loadSetup(id: id)
    XCTAssertEqual(try PlanEditing.receivers(in: loaded.config).first?.id, "submitted")
    model.beginBackgroundRefresh()
    XCTAssertTrue(model.deleteSelectedSetup())
    model.selectedSetupID = "later-selection"
    model.finishBackgroundRefresh()
    try await waitIdle(model)
    XCTAssertNil(model.error)
    let remaining = try await client.setups()
    XCTAssertTrue(remaining.isEmpty)
    XCTAssertEqual(model.selectedSetupID, "later-selection")
  }

  private func waitIdle(_ model: SessionModel) async throws {
    for _ in 0..<200 {
      if !model.busy { return }
      try await Task.sleep(for: .milliseconds(50))
    }
    XCTFail("Presentation operation did not finish in time")
  }

}
