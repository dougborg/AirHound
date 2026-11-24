import AirHoundCore
import Combine
import Darwin
import XCTest

@testable import AirHoundApp

@MainActor
final class RepeatRecordingTests: XCTestCase {
  private var changes: [ObservableObjectPublisher] = []
  func testSamePlanStartsAgainAfterFinishAndKeepsBothObservationHistories() async throws {
    guard let backend = ProcessInfo.processInfo.environment["AIRHOUND_TEST_BACKEND"] else {
      throw XCTSkip("Set AIRHOUND_TEST_BACKEND for private repeat-recording integration")
    }
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(
      "ah-repeat-" + UUID().uuidString.prefix(8))
    try LocalFiles.privateDirectory(root)
    defer { try? FileManager.default.removeItem(at: root) }
    let socket = root.appendingPathComponent("control.sock")
    let helper = root.appendingPathComponent("receiver.sh")
    // Shell builtins only: no radio access, subprocess fan-out or timer producer.
    // Each invocation emits R1/R2 so stale presentation is observable.
    try script(
      #"""
      #!/bin/sh
      prior=0
      if [ -f "$0.count" ]; then read -r prior < "$0.count"; fi
      ordinal=$((prior+1)); printf '%s\n' "$ordinal" > "$0.count"
      digit=$((48+ordinal))
      emit() {
        printf '{"schema_version":1,"type":"beacon","source":"mt7610u-lab","session_id":"fixture-%s","sequence":0,"bssid":"02:00:00:00:00:01","ssid_bytes":[82,%s],"advertised_channel":1,"receive_channel":1,"elapsed_ms":1,"observed_at_unix_ms":1700000000001,"dwell_elapsed_ms":1,"channel_epoch":0,"rssi_dbm":null,"estimated_rssi_dbm":null,"rssi_validated":false,"rssi_estimate_basis":"mt76_eeprom_formula","raw_rssi":[0,0,0,0],"full_calibration":false}\n' "$ordinal" "$digit"
      }
      printf '%s\n' '{"type":"hello","protocol_version":1}' '{"type":"status","state":"running"}'
      emit
      while IFS= read -r command; do
        id=${command##*:}; id=${id%?}
        case "$command" in
          *pause*) printf '{"type":"status","state":"paused","cleanup_verified":true,"request_id":%s}\n' "$id";;
          *resume*) printf '{"type":"status","state":"running","request_id":%s}\n' "$id"; emit;;
          *stop*) printf '{"type":"stopped","cleanup_verified":true,"request_id":%s}\n' "$id"; exit 0;;
        esac
      done
      exit 3
      """#, to: helper)
    let firmware = root.appendingPathComponent("unused-firmware")
    try Data().write(to: firmware)
    let requests = root.appendingPathComponent("requests.txt")
    let wrapper = root.appendingPathComponent("client.sh")
    try script(
      """
      #!/bin/sh
      previous=''
      for argument in "$@"; do
        if [ "$previous" = '--request-id' ]; then printf '%s\\n' "$argument" >> \(quote(requests.path)); fi
        previous="$argument"
      done
      exec \(quote(backend)) "$@"
      """, to: wrapper)
    let log = root.appendingPathComponent("service.log")
    FileManager.default.createFile(atPath: log.path, contents: nil)
    let output = try FileHandle(forWritingTo: log)
    defer { try? output.close() }
    let server = Process()
    server.executableURL = URL(fileURLWithPath: backend)
    server.arguments = [
      "serve", "--capture", "--socket", socket.path, "--database",
      root.appendingPathComponent("recordings.sqlite").path,
    ]
    server.standardOutput = output
    server.standardError = output
    try server.run()
    defer { XCTAssertTrue(stopOwned(server), "Private backend did not terminate/reap") }
    try await awaitFixtureFile(socket)
    let suite = "AirHound-repeat-" + UUID().uuidString
    let preferences = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { preferences.removePersistentDomain(forName: suite) }
    let model = SessionModel(preferences: preferences)
    changes = [model.objectWillChange]
    model.socketPath = socket.path
    model.connect(executable: wrapper)
    defer { model.disconnect() }
    try await eventually("Connected model", log: log) { model.connected && !model.busy }
    let client = BackendClient(executable: URL(fileURLWithPath: backend), socket: socket)
    let serviceIdentity = try await client.request(["status"])
    let plan = try JSONValue.object([
      "version": .integer(1), "alert_policy": .string("silent"),
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
    model.configText = String(decoding: plan, as: UTF8.self)
    let presentation = AppModel()
    changes.append(presentation.objectWillChange)
    var runs: [String] = []
    for ordinal in 1...2 {
      XCTAssertNil(model.startUnavailableReason)
      XCTAssertTrue(model.canSubmitAction)
      XCTAssertFalse(model.hasPendingStart)
      // The second click arrives while polling, as ordinary users can do.
      if ordinal == 2 { model.beginBackgroundRefresh() }
      model.start()
      if ordinal == 2 {
        XCTAssertTrue(model.userActionQueued)
        XCTAssertFalse(model.canSubmitAction)
        model.finishBackgroundRefresh()
      }
      try await eventually("Recording \(ordinal) started", log: log) {
        !model.busy && model.isActive
      }
      XCTAssertNil(model.error)
      let id = try XCTUnwrap(model.activeID)
      XCTAssertFalse(runs.contains(id), "New Start must not replay the previous run")
      runs.append(id)
      XCTAssertNil(try PendingStartStore(socket: socket).load())
      let run = try XCTUnwrap(model.runs.first { $0.id == id })
      model.open(run, app: presentation)
      try await eventually("Live observations \(ordinal)", log: log) {
        !model.busy
          && presentation.recording?.observations.contains { $0.name == "R\(ordinal)" } == true
      }
      XCTAssertEqual(model.browsingRun, id)
      XCTAssertTrue(model.followLive)
      XCTAssertTrue(
        presentation.recording!.observations.allSatisfy {
          $0.name == "R\(ordinal)" && $0.receiver == "synthetic"
        })
      if ordinal == 1 {
        model.control(.pause)
        try await eventually("Paused first recording", log: log) {
          !model.busy && model.state == "paused"
        }
        model.control(.resume)
        try await eventually("Resumed first recording", log: log) {
          !model.busy && model.state == "recording"
            && (presentation.recording?.observations.count ?? 0) >= 2
        }
      }
      model.control(.finish)
      try await eventually("Finished recording \(ordinal)", log: log) {
        !model.busy && !model.isActive && model.state == "finished"
      }
      XCTAssertNil(model.error)
      XCTAssertNil(model.startUnavailableReason, "Finish must re-enable Start without reconnecting")
      XCTAssertEqual(model.configText, String(decoding: plan, as: UTF8.self))
      XCTAssertFalse(model.hasPendingStart)
      XCTAssertNil(try PendingStartStore(socket: socket).load())
      XCTAssertTrue(server.isRunning)
    }
    let requestIDs = try String(contentsOf: requests, encoding: .utf8).split(separator: "\n")
    XCTAssertEqual(requestIDs.count, 2)
    XCTAssertEqual(Set(requestIDs).count, 2, "Each ordinary recording needs a fresh start request")
    let history = try await client.recordings()
    XCTAssertEqual(history.runs.count, 2)
    XCTAssertTrue(history.runs.allSatisfy { $0.state == "finished" })
    for (offset, id) in runs.enumerated() {
      let run = try XCTUnwrap(history.runs.first { $0.id == id })
      let page = try await client.records(runID: id, limit: 500)
      let rows = page.records.filter { $0.kind == "external_wifi_observation" }
      XCTAssertEqual(rows.count, offset == 0 ? 2 : 1)
      XCTAssertTrue(rows.allSatisfy { $0.sourceID == "synthetic" })
      model.open(run, app: presentation)
      try await eventually("Saved observations \(offset + 1)", log: log) {
        !model.busy && model.browsingRun == id
          && presentation.recording?.observations.count == rows.count
      }
      XCTAssertFalse(model.followLive)
      XCTAssertTrue(presentation.recording!.observations.allSatisfy { $0.name == "R\(offset + 1)" })
    }
    let finalIdentity = try await client.request(["status"])
    let initial = try JSONDecoder().decode(JSONValue.self, from: serviceIdentity)
    let final = try JSONDecoder().decode(JSONValue.self, from: finalIdentity)
    XCTAssertEqual(initial["instance_id"], final["instance_id"])
  }

  private func script(_ text: String, to url: URL) throws {
    try Data((text + "\n").utf8).write(to: url)
    try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: url.path)
  }
  private func quote(_ value: String) -> String {
    "'" + value.replacingOccurrences(of: "'", with: "'\\''") + "'"
  }
  private func eventually(_ label: String, log: URL, condition: @escaping () -> Bool) async throws {
    do { try await awaitModelState(changes, description: label, condition: condition) } catch {
      let output = (try? String(contentsOf: log, encoding: .utf8)) ?? "No service log"
      XCTFail("\(label) did not complete: \(output.suffix(4000))")
      throw error
    }
  }
  private func stopOwned(_ process: Process) -> Bool {
    guard process.isRunning else { return true }
    process.terminate()
    func wait(_ seconds: TimeInterval) -> Bool {
      let deadline = ProcessInfo.processInfo.systemUptime + seconds
      while process.isRunning && ProcessInfo.processInfo.systemUptime < deadline {
        Thread.sleep(forTimeInterval: 0.01)
      }
      return !process.isRunning
    }
    if wait(3) { return true }
    if process.isRunning { _ = Darwin.kill(process.processIdentifier, SIGKILL) }
    return wait(2)
  }
}
