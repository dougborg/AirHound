import Darwin
import XCTest

@testable import AirHoundCore

final class SetupTests: XCTestCase {
  func testReceiverEditsPreservePlanOptionsIDsAndUnknownFields() throws {
    let original = try JSONDecoder().decode(
      JSONValue.self,
      from: Data(
        #"{"version":1,"alert_policy":"silent","location":{"kind":"gpsd","endpoint":"127.0.0.1:2947"},"future_option":{"keep":true},"receivers":[{"id":"my-custom-id","metadata":"keep","source":{"kind":"sniffle","port":"/dev/cu.fixture","channel":37,"future_source":true}},{"id":"second","source":{"kind":"native_ble"}}]}"#
          .utf8))
    let edited = try PlanEditing.change(
      original, receiverID: "my-custom-id", field: "channel", value: .integer(38))
    let added = try PlanEditing.add(
      edited, id: "m5",
      source: .object([
        "kind": .string("airhound_serial"), "port": .string("/dev/cu.sensor"),
        "baud": .integer(460800),
      ]))
    let removed = try PlanEditing.remove(added, id: "second")
    for key in ["version", "alert_policy", "location", "future_option"] {
      XCTAssertEqual(removed[key], original[key])
    }
    let receivers = try PlanEditing.receivers(in: removed)
    XCTAssertEqual(receivers.map(\.id), ["my-custom-id", "m5"])
    XCTAssertEqual(receivers[0].source["channel"], .integer(38))
    XCTAssertEqual(receivers[0].source["future_source"], .bool(true))
    if case .array(let values) = removed["receivers"] {
      XCTAssertEqual(values[0]["metadata"], .string("keep"))
    }
    XCTAssertEqual(try PlanEditing.receivers(in: original)[0].source["channel"], .integer(37))
    let disabledLocation = try PlanEditing.location(removed, value: nil)
    XCTAssertNil(disabledLocation["location"])
    XCTAssertEqual(disabledLocation["receivers"], removed["receivers"])
    XCTAssertEqual(disabledLocation["alert_policy"], .string("silent"))
    let restoredLocation = try PlanEditing.location(disabledLocation, value: original["location"])
    XCTAssertEqual(restoredLocation, removed)
    XCTAssertThrowsError(try PlanEditing.add(original, id: "second", source: .object([:])))
    XCTAssertThrowsError(
      try PlanEditing.change(original, receiverID: "missing", field: "port", value: .null))
  }

  func testSavedSetupReviewPreservesBackendReadinessAndConfiguration() throws {
    let response = try JSONDecoder().decode(
      JSONValue.self,
      from: Data(
        #"{"profile":{"id":"kit","name":"Car kit","config":{"version":1,"receivers":[]}},"applied":false,"reception":"not_tested","validation":{"valid":true},"bindings":{"checks":[{"receiver_id":"cat","state":"missing","binding":{"selector":"/dev/cu.old"}}]},"resources":{"valid":false,"checks":[{"receiver_id":"cat","field":"helper","path":"/missing/helper","state":"missing"}]}}"#
          .utf8))
    let review = try SetupReview(response)
    XCTAssertEqual(review.raw, response)
    XCTAssertEqual(review.config, response["profile"]?["config"])
    XCTAssertEqual(review.checks.count, 2)
    XCTAssertTrue(review.checks[0].detail.contains("missing"))
    XCTAssertTrue(review.checks[1].detail.contains("/missing/helper"))
    XCTAssertThrowsError(try SetupReview(.object([:])))
  }

  func testDiscoveryPresentationKeepsUniqueRowsAndOptionalMetadata() throws {
    let inventory = try Inventory.decode(
      Data(
        #"{"schema_version":1,"mode":"passive_with_active_metadata","platform":"macos","diagnostics":[],"devices":[{"id":"usb-a","name":"Sensor","identity_scope":"attachment","endpoints":[{"kind":"serial","path":"/dev/cu.sensor"}],"candidates":[{"kind":"airhound_serial","label":"AirHound","support":"prototype","state":"in_use","evidence":[],"requirements":[],"identification":{"application":"AirHound","firmware_version":"0.1.1","board":"m5stickc_plus2"}}]},{"id":"usb-b","name":"GPS","identity_scope":"attachment","endpoints":[],"candidates":[{"kind":"gps","label":"GPS","support":"prototype","state":"needs_configuration","evidence":[],"requirements":[],"support_issue":"https://github.com/dougborg/AirHound/issues/269","location_configuration_hint":{"kind":"gpsd","endpoint":"127.0.0.1:2947","device":"/dev/cu.gps"}}]}]}"#
          .utf8))
    XCTAssertEqual(inventory.receiverChoices.map(\.id), ["usb-a:0", "usb-b:0"])
    XCTAssertEqual(inventory.receiverChoices[0].device.endpoints[0].path, "/dev/cu.sensor")
    XCTAssertEqual(
      inventory.receiverChoices[0].candidate.identification?["firmware_version"], .string("0.1.1"))
    XCTAssertEqual(
      inventory.receiverChoices[1].candidate.locationConfigurationHint?["device"],
      .string("/dev/cu.gps"))
    XCTAssertNotNil(inventory.receiverChoices[1].candidate.supportIssue)
    XCTAssertNil(inventory.receiverChoices[0].candidate.supportIssue)
  }

  private func waitForExit(_ process: Process, seconds: TimeInterval) -> Bool {
    let deadline = ProcessInfo.processInfo.systemUptime + seconds
    while process.isRunning && ProcessInfo.processInfo.systemUptime < deadline {
      Thread.sleep(forTimeInterval: 0.01)
    }
    return !process.isRunning
  }

  private func stopOwned(_ process: Process, grace: TimeInterval = 2) -> Bool {
    guard process.isRunning else { return true }
    process.terminate()
    // Foundation waitUntilExit can remain in its run loop after the child has
    // exited on a Swift async test worker. Poll its state with a fixed deadline.
    if waitForExit(process, seconds: grace) { return true }
    if process.isRunning { _ = Darwin.kill(process.processIdentifier, SIGKILL) }
    return waitForExit(process, seconds: 2)
  }

  func testProfileFixtureCleanupHandlesAlreadyExitedAndUncooperativeChildren() async throws {
    for ignoresTermination in [false, true] {
      let ready = FileManager.default.temporaryDirectory.appendingPathComponent(
        "ah-profile-cleanup-" + UUID().uuidString)
      defer { try? FileManager.default.removeItem(at: ready) }
      let process = Process()
      process.executableURL = URL(fileURLWithPath: "/bin/sh")
      process.arguments = [
        "-c",
        ignoresTermination
          ? "trap '' TERM; : > \"$1\"; while :; do :; done"
          : "exit 0",
        "fixture", ready.path,
      ]
      process.standardOutput = FileHandle.nullDevice
      process.standardError = FileHandle.nullDevice
      try process.run()
      defer { XCTAssertTrue(stopOwned(process, grace: 0.05)) }
      if ignoresTermination {
        let deadline = ProcessInfo.processInfo.systemUptime + 2
        while !FileManager.default.fileExists(atPath: ready.path)
          && ProcessInfo.processInfo.systemUptime < deadline
        { try await Task.sleep(for: .milliseconds(10)) }
        XCTAssertTrue(FileManager.default.fileExists(atPath: ready.path))
        XCTAssertFalse(waitForExit(process, seconds: 0.05))
      } else {
        XCTAssertTrue(waitForExit(process, seconds: 2))
      }
      let started = ProcessInfo.processInfo.systemUptime
      XCTAssertTrue(stopOwned(process, grace: 0.05))
      XCTAssertLessThan(ProcessInfo.processInfo.systemUptime - started, 2.5)
      XCTAssertEqual(process.terminationStatus, ignoresTermination ? SIGKILL : 0)
      XCTAssertEqual(Darwin.kill(process.processIdentifier, 0), -1)
      XCTAssertEqual(errno, ESRCH, "Owned child must be reaped")
    }
  }

  func testProfilesUseSharedBackendWithoutStartingCapture() async throws {
    guard let path = ProcessInfo.processInfo.environment["AIRHOUND_TEST_BACKEND"] else {
      throw XCTSkip("Set AIRHOUND_TEST_BACKEND for profile integration.")
    }
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(
      "ah-setup-" + UUID().uuidString.prefix(8))
    try LocalFiles.privateDirectory(root)
    defer { try? FileManager.default.removeItem(at: root) }
    let socket = root.appendingPathComponent("control.sock")
    let server = Process()
    server.executableURL = URL(fileURLWithPath: path)
    // Passive-control mode prevents this test from opening capture devices.
    server.arguments = ["serve", "--socket", socket.path]
    server.standardOutput = FileHandle.nullDevice
    server.standardError = FileHandle.nullDevice
    try server.run()
    defer { XCTAssertTrue(stopOwned(server), "Private profile service did not exit after cleanup") }
    for _ in 0..<100 {
      if FileManager.default.fileExists(atPath: socket.path) { break }
      try await Task.sleep(for: .milliseconds(50))
    }
    let client = BackendClient(executable: URL(fileURLWithPath: path), socket: socket)
    let config = Data(
      #"{"version":1,"alert_policy":"silent","receivers":[{"id":"my-ble","source":{"kind":"native_ble"}}]}"#
        .utf8)
    let saved = try await client.saveSetup(
      id: "kit", name: "Night drive; $(ignored)", config: config, replace: false)
    XCTAssertEqual(saved["profile"]?["name"], .string("Night drive; $(ignored)"))
    let setups = try await client.setups()
    XCTAssertEqual(setups.map(\.id), ["kit"])
    let review = try await client.loadSetup(id: "kit")
    XCTAssertEqual(review.raw["applied"], .bool(false))
    XCTAssertEqual(review.raw["reception"], .string("not_tested"))
    XCTAssertEqual(try PlanEditing.receivers(in: review.config).map(\.id), ["my-ble"])
    do {
      _ = try await client.saveSetup(id: "kit", name: "Replacement", config: config, replace: false)
      XCTFail("Replacement requires explicit intent")
    } catch {}
    _ = try await client.saveSetup(id: "kit", name: "Replacement", config: config, replace: true)
    let replaced = try await client.loadSetup(id: "kit")
    XCTAssertEqual(replaced.name, "Replacement")
    let status = try JSONDecoder().decode(JSONValue.self, from: await client.request(["status"]))
    XCTAssertEqual(status["capture_control"], .bool(false))
    try await client.deleteSetup(id: "kit")
    let empty = try await client.setups()
    XCTAssertTrue(empty.isEmpty)
  }
}
