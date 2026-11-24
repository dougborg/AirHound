import AirHoundCore
import Darwin
import XCTest

@testable import AirHoundApp

@MainActor
final class GPSVersionTests: XCTestCase {
  private func inventory(
    identificationProtocol: String? = "ublox", kind: String = "gps",
    port: String = "/dev/cu.test-gps",
    id: String = "attachment-test"
  ) throws -> Inventory {
    let value: JSONValue = .object([
      "schema_version": .integer(1), "mode": .string("passive"), "platform": .string("macos"),
      "diagnostics": .array([]),
      "devices": .array([
        .object([
          "id": .string(id), "identity_scope": .string("attachment"),
          "name": .string("Test candidate"),
          "usb": .object(["vid": .string("1546"), "pid": .string("01a8")]),
          "endpoints": .array([.object(["kind": .string("serial"), "path": .string(port)])]),
          "candidates": .array([
            .object([
              "kind": .string(kind),
              "identification_protocol": identificationProtocol.map(JSONValue.string) ?? .null,
              "label": .string("Fixture GPS"),
              "support": .string("available"),
              "state": .string("needs_configuration"), "evidence": .array([]),
              "requirements": .array([]),
            ])
          ]),
        ])
      ]),
    ])
    return try Inventory.decode(value.data())
  }

  private var target: GPSVersionTarget {
    GPSVersionTarget(deviceID: "attachment-test", port: "/dev/cu.test-gps")
  }
  private var response: JSONValue {
    .object([
      "schema_version": .integer(1), "device_id": .string(target.deviceID),
      "endpoint": .string(target.port),
      "provenance": .string("explicit_ublox_mon_ver"), "state": .string("identified"),
      "firmware_version": .string("TEST FW"), "software_version": .string("TEST ROM"),
      "hardware_version": .string("TEST HW"), "protocol_version": .string("TEST PROTOCOL"),
      "model": .null, "compatible": .null, "reception": .string("not_tested"),
    ])
  }

  func testExplicitRequestAndCandidateGating() throws {
    XCTAssertEqual(
      target.request,
      .object([
        "protocol": .string("ublox"), "device_id": .string("attachment-test"),
        "port": .string("/dev/cu.test-gps"),
      ]))
    XCTAssertTrue(target.isCurrent(in: try inventory()))
    XCTAssertFalse(target.isCurrent(in: try inventory(identificationProtocol: nil)))
    XCTAssertFalse(target.isCurrent(in: try inventory(kind: "serial_unknown")))
    XCTAssertFalse(target.isCurrent(in: try inventory(port: "/dev/cu.replaced")))
    XCTAssertFalse(target.isCurrent(in: try inventory(id: "new-attachment")))
    XCTAssertFalse(
      GPSVersionTarget(deviceID: target.deviceID, port: "").isCurrent(in: try inventory()))
    let session = SessionModel()
    session.inventory = try inventory()
    XCTAssertFalse(session.canReadGPSVersion(target))
    session.connected = true
    session.captureEnabled = true
    XCTAssertTrue(session.canReadGPSVersion(target))
    session.beginBackgroundRefresh()
    XCTAssertTrue(session.canReadGPSVersion(target), "Polling must not flash the explicit action")
    session.finishBackgroundRefresh()
    session.capture = .object(["active": .bool(true), "state": .string("paused")])
    XCTAssertFalse(session.canReadGPSVersion(target), "Paused capture still owns resources")
    session.capture = .object(["active": .bool(false)])
    session.captureEnabled = false
    XCTAssertFalse(session.canReadGPSVersion(target), "History-only service cannot query hardware")
  }

  func testVersionEvidenceIsBoundAndInvalidated() throws {
    XCTAssertTrue(target.matches(response))
    guard case .object(var fields) = response else { return XCTFail() }
    fields["endpoint"] = .string("/dev/cu.other")
    XCTAssertFalse(target.matches(.object(fields)))
    fields["endpoint"] = .string(target.port)
    fields["provenance"] = .string("host_gpsd_version")
    XCTAssertFalse(target.matches(.object(fields)))
    let session = SessionModel()
    session.connected = true
    session.inventory = try inventory()
    session.gpsIdentifications[target] = response
    XCTAssertEqual(session.gpsVersion(for: target), response)
    session.inventory = try inventory()
    XCTAssertNil(
      session.gpsVersion(for: target),
      "Even a refresh of the same endpoint invalidates historical evidence")
    session.gpsIdentifications[target] = response
    session.disconnect()
    XCTAssertNil(session.gpsVersion(for: target))
    session.connected = true
    XCTAssertNil(session.gpsVersion(for: target), "Reconnection must not revive prior evidence")
  }

  func testPresentationDoesNotInferModelOrReception() {
    let lines = GPSVersionDetails(value: response).lines
    XCTAssertTrue(lines.contains("GPS firmware: TEST FW"))
    XCTAssertTrue(lines.contains("GPS software: TEST ROM"))
    XCTAssertTrue(lines.contains("Hardware version: TEST HW"))
    XCTAssertTrue(lines.contains("UBX protocol: TEST PROTOCOL"))
    XCTAssertTrue(lines.contains("Module model: not reported"))
    XCTAssertTrue(lines.contains("GPS reception has not been tested by this version query."))
    XCTAssertEqual(
      GPSVersionDetails(value: .object(["message": .string("Device is busy")])).lines[5],
      "Device is busy")
  }

  func testSharedClientUsesExplicitIdentifyConfigAndRemovesTemporaryFile() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try LocalFiles.privateDirectory(root)
    defer { try? FileManager.default.removeItem(at: root) }
    let executable = root.appendingPathComponent("fixture")
    try Data("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\n/bin/cat \"$6\"\n".utf8).write(
      to: executable)
    try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: executable.path)
    let socket = root.appendingPathComponent("private.sock")
    let client = BackendClient(executable: executable, socket: socket)
    let value = try await client.identifyDevice(config: target.request.data())
    XCTAssertEqual(value, target.request)
    let arguments = try String(
      contentsOf: executable.appendingPathExtension("args"), encoding: .utf8
    ).split(separator: "\n").map(String.init)
    XCTAssertEqual(
      Array(arguments.prefix(5)), ["client", "--socket", socket.path, "identify", "--config"])
    XCTAssertEqual(arguments.count, 6)
    XCTAssertFalse(FileManager.default.fileExists(atPath: arguments[5]))
  }

  func testSnapshotUsesOnlyValidBackendQueryTime() {
    guard case .object(var fields) = response else { return XCTFail() }
    fields["observed_at_ms"] = .integer(1_700_000_000_123)
    let details = GPSVersionDetails(value: .object(fields))
    XCTAssertEqual(details.observedAt, Date(timeIntervalSince1970: 1_700_000_000.123))
    XCTAssertEqual(details.lines.first, "Queried version snapshot")
    XCTAssertTrue(details.lines[1].hasPrefix("Last queried "))
    for invalid: JSONValue in [
      .null, .string("today"), .integer(-1), .integer(0), .unsigned(UInt64.max), .number(.infinity),
    ] {
      fields["observed_at_ms"] = invalid
      let missing = GPSVersionDetails(value: .object(fields))
      XCTAssertNil(missing.observedAt)
      XCTAssertEqual(missing.lines[1], "Query time not reported.")
    }
    fields["state"] = .string("busy")
    fields["observed_at_ms"] = .integer(1_700_000_000_123)
    XCTAssertFalse(
      GPSVersionDetails(value: .object(fields)).lines.contains { $0.hasPrefix("Last queried ") },
      "A busy response must not imply a firmware query occurred")
  }

  func testLateResponseAndUnsupportedBackendDoNotPublishIdentity() async throws {
    for scenario in ["endpoint-changed", "disconnected", "unsupported"] {
      let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
      try LocalFiles.privateDirectory(root)
      defer { try? FileManager.default.removeItem(at: root) }
      let devices = root.appendingPathComponent("devices.json")
      let initial =
        #"{"schema_version":1,"mode":"passive","platform":"macos","diagnostics":[],"devices":[{"id":"attachment-test","identity_scope":"attachment","name":"Fixture","endpoints":[{"kind":"serial","path":"/dev/cu.test-gps"}],"candidates":[{"kind":"gps","identification_protocol":"ublox","label":"Fixture","support":"candidate","state":"unverified","evidence":[],"requirements":[]}]}]}"#
      try Data(initial.utf8).write(to: devices)
      try response.data().write(to: root.appendingPathComponent("response.json"))
      let release = root.appendingPathComponent("release")
      guard mkfifo(release.path, 0o600) == 0 else {
        throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno))
      }
      func releaseResponse() throws {
        let fd = open(release.path, O_RDWR | O_NONBLOCK)
        guard fd >= 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        defer { close(fd) }
        guard "go\n".withCString({ write(fd, $0, 3) }) == 3 else {
          throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno))
        }
      }
      let executable = root.appendingPathComponent("fixture")
      // A bounded, private CLI fixture. It has no hardware or service access.
      let script = #"""
        #!/bin/sh
        cd "$(/usr/bin/dirname "$0")" || exit 2
        case "$4" in
          status) printf '%s\n' '{"capture_control":true,"methods":[]}' ;;
          capture-status) printf '%s\n' '{"active":false}' ;;
          devices) /bin/cat devices.json ;;
          recordings) printf '%s\n' '{"runs":[],"has_more":false,"next_before":null}' ;;
          identify)
            /bin/cat "$6" > request.json
            exec 3<>release
            /usr/bin/touch started
            IFS= read -r response_release <&3 || exit 3
            if [ -e unsupported ]; then printf '%s\n' 'unsupported identity protocol' >&2; exit 4; fi
            /bin/cat response.json ;;
          *) exit 5 ;;
        esac
        """#
      try Data(script.utf8).write(to: executable)
      try FileManager.default.setAttributes(
        [.posixPermissions: 0o700], ofItemAtPath: executable.path)
      let suite = "GPS-fixture-" + UUID().uuidString
      let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
      defer { defaults.removePersistentDomain(forName: suite) }
      let session = SessionModel(preferences: defaults)
      session.socketPath = root.appendingPathComponent("private.sock").path
      session.connect(executable: executable)
      defer { session.disconnect() }
      func wait(_ predicate: @escaping () -> Bool) async throws {
        try await awaitModelState([session.objectWillChange], condition: predicate)
      }
      try await wait { !session.busy }
      XCTAssertTrue(session.connected, session.error ?? "Not connected")
      XCTAssertFalse(
        FileManager.default.fileExists(atPath: root.appendingPathComponent("started").path),
        "Connection and refresh must never identify automatically")
      session.readGPSVersion(target)
      try await awaitFixtureFile(root.appendingPathComponent("started"))
      // Release even if a subsequent assertion/setup throws. The normal backend
      // process deadline remains only an outer guard for a hung fixture.
      defer { try? releaseResponse() }
      XCTAssertTrue(session.busy, "The response must still be held at the explicit release barrier")
      XCTAssertNil(session.error)
      if scenario == "endpoint-changed" {
        try Data(
          initial.replacingOccurrences(of: "/dev/cu.test-gps", with: "/dev/cu.replaced").utf8
        ).write(to: devices)
      } else if scenario == "disconnected" {
        session.disconnect()
      } else {
        try Data().write(to: root.appendingPathComponent("unsupported"))
      }
      try releaseResponse()
      try await wait { !session.busy }
      XCTAssertNil(session.gpsVersion(for: target), scenario)
      XCTAssertTrue(session.gpsIdentifications.isEmpty, scenario)
      XCTAssertNotNil(session.error, scenario)
      if scenario == "unsupported" {
        XCTAssertTrue(session.error?.contains("unsupported identity protocol") == true)
      }
      let sent = try JSONDecoder().decode(
        JSONValue.self, from: Data(contentsOf: root.appendingPathComponent("request.json")))
      XCTAssertEqual(sent, target.request)
    }
  }
}
