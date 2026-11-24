import Darwin
import XCTest

@testable import AirHoundCore

final class CoreTests: XCTestCase {
  private let inventory =
    #"{"schema_version":1,"mode":"passive","platform":"macos","devices":[{"id":"usb:example","identity_scope":"attachment_topology","name":"Example bridge","endpoints":[{"kind":"serial","path":"/dev/example"}],"candidates":[{"kind":"future_backend","label":"Future role","support":"candidate","state":"needs_identification","evidence":["Generic bridge"],"requirements":[{"name":"firmware","state":"unknown","detail":"Inspect manually"}],"capabilities":{"scope":"unknown"}}]}],"diagnostics":[{"backend":"ioreg","message":"Partial inventory"}]}"#

  func testMarauderReplayObservationPreservesUnknownReceiveFields() throws {
    let input =
      #"{"type":"wifi_packet_observation","mac":"02:01:02:03:04:05","ssid":"fixture","replay":true,"timestamp_micros32":4294967295,"timestamp_clock":"device_boot_micros32_wrap_or_reset_unknown","rssi_dbm":null,"receive_channel":null,"gps":null,"session_source":{"receiver_id":"awok-file"}}"#
    let row = try XCTUnwrap(Recording.decode(Data(input.utf8)).observations.first)
    XCTAssertEqual(row.name, "fixture")
    XCTAssertEqual(row.address, "02:01:02:03:04:05")
    XCTAssertEqual(row.receiver, "awok-file")
    XCTAssertNil(row.rssi)
    XCTAssertEqual(row.channel, "Not reported")
    XCTAssertTrue(row.raw.contains("device_boot_micros32_wrap_or_reset_unknown"))
  }

  func testUnnamedObservationsUseAvailableIdentity() throws {
    let input = #"{"type":"ble_observation","name":"","mac":"02:00:00:00:00:01"}"#
    let row = try XCTUnwrap(Recording.decode(Data(input.utf8)).observations.first)
    XCTAssertEqual(row.name, "02:00:00:00:00:01")
    let named = input.replacingOccurrences(of: "\"name\":\"\"", with: "\"name\":\"Sensor\"")
    XCTAssertEqual(try Recording.decode(Data(named.utf8)).observations.first?.name, "Sensor")
  }

  func testInventoryKeepsUncertaintyAndUnknownCandidates() throws {
    let value = try Inventory.decode(Data(inventory.utf8))
    XCTAssertEqual(value.devices[0].candidates[0].state, "needs_identification")
    XCTAssertEqual(value.devices[0].candidates[0].kind, "future_backend")
    XCTAssertEqual(value.diagnostics[0].message, "Partial inventory")
    XCTAssertEqual(value.devices[0].identityScope, "attachment_topology")
  }
  func testInventoryAcceptsOnlyKnownPassiveModes() throws {
    let active = inventory.replacingOccurrences(
      of: "\"mode\":\"passive\"", with: "\"mode\":\"passive_with_active_metadata\"")
    XCTAssertEqual(try Inventory.decode(Data(active.utf8)).mode, "passive_with_active_metadata")
    let unsupported = inventory.replacingOccurrences(
      of: "\"mode\":\"passive\"", with: "\"mode\":\"active_probe\"")
    XCTAssertThrowsError(try Inventory.decode(Data(unsupported.utf8)))
  }

  func testInventoryRejectsIncompatibleAndMalformedOutput() {
    for value in [
      inventory.replacingOccurrences(of: "\"schema_version\":1", with: "\"schema_version\":2"),
      inventory.replacingOccurrences(of: "\"passive\"", with: "\"active\""), "{}", "not JSON",
    ] {
      XCTAssertThrowsError(try Inventory.decode(Data(value.utf8)))
    }
  }
  func testDuplicateInventoryIdentitiesRejected() {
    let value =
      #"{"schema_version":1,"mode":"passive","platform":"macos","devices":[{"id":"x","name":"A","identity_scope":"system_backend","endpoints":[],"candidates":[]},{"id":"x","name":"B","identity_scope":"system_backend","endpoints":[],"candidates":[]}],"diagnostics":[]}"#
    XCTAssertThrowsError(try Inventory.decode(Data(value.utf8)))
  }
  func testObservationIdentityEvidenceAndRuntimeRecords() throws {
    let value =
      #"{"type":"scanner_health","scan_requested":true}"# + "\n"
      + #"{"type":"ble_observation","name":"Example","peripheral_id":"platform-only","detection":{"matched":true,"alert":"suppressed","evidence":[{"description":"Test","confidence":"low","future_field":42}]},"session_source":{"receiver_id":"sniffle-A"}}"#
    let recording = try Recording.decode(Data(value.utf8))
    XCTAssertEqual(recording.ignoredRecords, 1)
    XCTAssertEqual(recording.observations.count, 1)
    let row = recording.observations[0]
    XCTAssertEqual(row.address, "platform-only")
    XCTAssertEqual(row.receiver, "sniffle-A")
    XCTAssertNil(row.rssi)
    XCTAssertEqual(row.signal, "Unknown")
    XCTAssertEqual(row.alert, "suppressed")
    XCTAssertTrue(row.evidence[0].contains("future_field"))
    XCTAssertTrue(row.raw.contains("platform-only"))
  }
  func testRecordingRejectsTruncationInvalidUTF8AndBounds() {
    for data in [
      Data([255]), Data("{\"type\":".utf8), Data(repeating: 65, count: Recording.byteLimit + 1),
      Data(repeating: 65, count: 256 * 1024 + 1),
    ] {
      XCTAssertThrowsError(try Recording.decode(data))
    }
  }
  func testRecordingNeverDuplicatesLegacyMatchRecords() throws {
    let value =
      #"{"type":"wifi_observation","mac":"02:00:00:00:00:01","rssi":-42,"detection":{"matched":true}}"#
      + "\n" + #"{"type":"wifi","mac":"02:00:00:00:00:01"}"# + "\n"
      + #"{"type":"detection","id":"02:00:00:00:00:01"}"#
    let recording = try Recording.decode(Data(value.utf8))
    XCTAssertEqual(recording.observations.count, 1)
    XCTAssertEqual(recording.ignoredRecords, 2)
  }
  func testRealDetectionContractAndUnvalidatedSignal() throws {
    let value =
      #"{"type":"external_wifi_observation","bssid":"02:00:00:00:00:02","ssid_bytes":[65,80],"receive_channel":36,"rssi_dbm":null,"estimated_rssi_dbm":-42,"rssi_validated":false,"detection":{"engine":"evidence-1","sig":[12],"rules":[{"id":3,"sig":[12],"more":false}],"alert":"suppressed"}}"#
    let row = try XCTUnwrap(Recording.decode(Data(value.utf8)).observations.first)
    XCTAssertEqual(row.name, "AP")
    XCTAssertEqual(row.match, "Match")
    XCTAssertEqual(row.channel, "36")
    XCTAssertEqual(row.address, "02:00:00:00:00:02")
    XCTAssertNil(row.rssi, "Estimated external signal must not become measured RSSI")
    XCTAssertTrue(row.evidence[0].contains("evidence-1"))
  }
  func testBLEMissingSignalDoesNotBecomeNumericSentinel() throws {
    let value =
      #"{"type":"ble_observation","address":"02:00:00:00:00:03","peripheral_id":"opaque","rssi":-128,"advertisement":{"rssi_available":false}}"#
    let row = try XCTUnwrap(Recording.decode(Data(value.utf8)).observations.first)
    XCTAssertNil(row.rssi)
    XCTAssertEqual(row.address, "02:00:00:00:00:03")
  }
  func testPacketObservationRetainsSourceAndUnknownDetection() throws {
    let value =
      #"{"type":"ble_packet_observation","source_id":"catsniffer-A","address":null,"name":"","rssi":null,"channel":38,"phy":"coded_s8","detection":null}"#
    let row = try XCTUnwrap(Recording.decode(Data(value.utf8)).observations.first)
    XCTAssertEqual(row.medium, "Bluetooth")
    XCTAssertEqual(row.receiver, "catsniffer-A")
    XCTAssertEqual(row.match, "Not evaluated")
    XCTAssertEqual(row.address, "Not reported")
    XCTAssertEqual(row.channel, "38")
    XCTAssertEqual(row.phy, "coded_s8")
    XCTAssertEqual(
      row.frequency, "Not reported", "Do not derive frequency from an ambiguous channel")
  }
  func testPipeConfigurationFailsClosed() {
    XCTAssertThrowsError(try DiscoveryProcess.configureNonblocking(-1))
  }
  func testUnknownNativeChannelIsNotBLEChannelZero() throws {
    let wifi = try Recording.decode(Data(#"{"type":"wifi_observation","ch":0}"#.utf8))
    let ble = try Recording.decode(Data(#"{"type":"ble_packet_observation","channel":0}"#.utf8))
    XCTAssertEqual(wifi.observations[0].channel, "Not reported")
    XCTAssertEqual(ble.observations[0].channel, "0")
    XCTAssertEqual(wifi.observations[0].signalDescription, "Unknown")
  }
  func testRecordingFileRejectsFIFOAndDirectoryWithoutBlocking() throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let fifo = root.appendingPathComponent("pipe")
    XCTAssertEqual(mkfifo(fifo.path, 0o600), 0)
    XCTAssertThrowsError(try Recording.load(fifo))
    XCTAssertThrowsError(try Recording.load(root))
    let file = root.appendingPathComponent("regular.ndjson")
    try Data(#"{"type":"wifi_observation","ch":6}"#.utf8).write(to: file)
    XCTAssertEqual(try Recording.load(file).observations[0].channel, "6")
  }
  func testBackendWatchlistAndSilentMatchesWithoutSignatureProof() throws {
    for alert in ["watchlist", "suppressed"] {
      let value =
        "{\"type\":\"wifi_observation\",\"detection\":{\"sig\":[],\"rules\":[],\"alert\":\"\(alert)\"}}"
      let row = try XCTUnwrap(Recording.decode(Data(value.utf8)).observations.first)
      XCTAssertEqual(row.match, "Match")
      XCTAssertEqual(row.alert, alert, "Present the backend decision without reapplying policy")
    }
  }

  private func script(_ body: String) throws -> URL {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    addTeardownBlock { try? FileManager.default.removeItem(at: root) }
    let executable = root.appendingPathComponent("backend with spaces")
    try ("#!/bin/sh\n" + body).write(to: executable, atomically: true, encoding: .utf8)
    try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: executable.path)
    return executable
  }
  func testAbsoluteDeadlineExpiresDuringDispatchWithoutLaunchingChild() async throws {
    for expired in [5.0, 5.001] {
      let time = DispatchDeadlineTime(expired: expired)
      let scheduling = ProcessScheduling(
        now: { time.read() }, launched: { time.launched($0) })
      do {
        _ = try await DiscoveryProcess(scheduling: scheduling).runUntil(
          executable: URL(fileURLWithPath: "/bin/echo"), arguments: ["not launched"], deadline: 5)
        XCTFail("Dispatch must not renew the startup budget")
      } catch { XCTAssertEqual(error as? DiscoveryError, .timedOut) }
      XCTAssertEqual(time.pid, 0)
      XCTAssertEqual(time.reads, 2, "Entry check then actual collector check")
    }
  }

  func testAbsoluteDeadlineIncludesLaunchAndReapsOwnedChild() async throws {
    for expired in [5.0, 5.001] {
      let time = ControlledProcessTime()
      let scheduling = ProcessScheduling(
        now: { time.now },
        launched: { pid in
          time.launched(pid)
          time.advance(expired)
        })
      do {
        _ = try await DiscoveryProcess(scheduling: scheduling).runUntil(
          executable: URL(fileURLWithPath: "/bin/cat"), arguments: [], deadline: 5)
        XCTFail("Launch time cannot start a fresh relative timeout")
      } catch { XCTAssertEqual(error as? DiscoveryError, .timedOut) }
      XCTAssertGreaterThan(time.pid, 0)
      XCTAssertEqual(kill(time.pid, 0), -1)
      XCTAssertEqual(errno, ESRCH, "Only the readiness child was reaped")
    }
  }

  func testProcessForwardsOnlyPassiveArguments() async throws {
    let executable = try script(
      "[ \"$#\" = 3 ] && [ \"$1\" = devices ] && [ \"$2\" = list ] && [ \"$3\" = --json ] || exit 9\nprintf '%s' '"
        + inventory + "'\n")
    let data = try await DiscoveryProcess().run(executable: executable)
    XCTAssertEqual(try Inventory.decode(data).devices.count, 1)
  }
  func testNonzeroExitAndStderr() async throws {
    let executable = try script("printf 'backend unavailable' >&2\nexit 7\n")
    do {
      _ = try await DiscoveryProcess().run(executable: executable)
      XCTFail("Expected failure")
    } catch { XCTAssertEqual(error as? DiscoveryError, .failed(7, "backend unavailable")) }
  }
  func testTimeoutKillsAndReapsUncooperativeChild() async throws {
    let executable = try script("trap '' TERM\nprintf READY\nkill -STOP $$\nexit 9\n")
    let time = ControlledProcessTime()
    let scheduling = ProcessScheduling(
      now: { time.now },
      launched: { time.launched($0) },
      stdout: { bytes in
        // Advance only after the child actually installed its TERM handler.
        if bytes.starts(with: Data("READY".utf8)) { time.stepDeadlineBoundary() }
      }, signalling: { signal, now in time.signalled(signal, now) })
    do {
      _ = try await DiscoveryProcess(scheduling: scheduling).run(
        executable: executable, timeout: 15)
      XCTFail("Expected timeout")
    } catch { XCTAssertEqual(error as? DiscoveryError, .timedOut) }
    XCTAssertEqual(time.signals.first?.0, SIGTERM)
    XCTAssertEqual(time.signals.first?.1, 15)
    XCTAssertTrue(time.signals.contains { $0.0 == SIGKILL })
    XCTAssertTrue(time.signals.filter { $0.0 == SIGKILL }.allSatisfy { $0.1 >= 15.25 })
    XCTAssertTrue(time.visitedTimes.contains(14.999))
    XCTAssertTrue(time.visitedTimes.contains(15.249))
    XCTAssertEqual(kill(time.pid, 0), -1)
    XCTAssertEqual(errno, ESRCH, "Owned process was reaped")
  }
  func testCancellationWaitsForOwnedProcess() async throws {
    let executable = try script("trap '' TERM\nprintf READY\nkill -STOP $$\nexit 9\n")
    let ready = expectation(description: "Child installed termination handler")
    let time = ControlledProcessTime()
    let scheduling = ProcessScheduling(
      now: { time.now },
      launched: { time.launched($0) },
      stdout: { bytes in
        if bytes.starts(with: Data("READY".utf8)), time.markReady() { ready.fulfill() }
        if time.isCancelled { time.advance(1) }
      })
    let task = Task {
      try await DiscoveryProcess(scheduling: scheduling).run(
        executable: executable, timeout: 100_000)
    }
    await fulfillment(of: [ready], timeout: 30)  // Outer hang guard, not a timing assertion.
    task.cancel()
    time.cancel()
    do {
      _ = try await task.value
      XCTFail("Expected cancellation")
    } catch { XCTAssertEqual(error as? DiscoveryError, .cancelled) }
    XCTAssertEqual(kill(time.pid, 0), -1)
    XCTAssertEqual(errno, ESRCH)
  }
  func testOutputFloodIsBounded() async throws {
    for redirect in ["", " >&2"] {
      let executable = try script(
        "while :; do printf 'abcdefghijklmnopqrstuvwxyz'" + redirect + "; done\n")
      do {
        _ = try await DiscoveryProcess().run(executable: executable, outputLimit: 1024)
        XCTFail("Expected overflow")
      } catch { XCTAssertEqual(error as? DiscoveryError, .outputLimit) }
    }
  }
  func testSharedClientArgumentsDoNotInterpretShellCharacters() async throws {
    let executable = try script(
      "[ \"$#\" = 4 ] && [ \"$1\" = client ] && [ \"$2\" = --socket ] && [ \"$3\" = '/tmp/socket with $characters' ] && [ \"$4\" = status ] || exit 9\nprintf '{}'\n"
    )
    let client = BackendClient(
      executable: executable, socket: URL(fileURLWithPath: "/tmp/socket with $characters"))
    let data = try await client.request(["status"])
    XCTAssertEqual(String(decoding: data, as: UTF8.self), "{}")
  }

  func testHistoryPagePreservesEvidenceAndLargeIntegerCursors() throws {
    let data = Data(
      #"{"records":[{"cursor":9007199254740993,"source_id":"cat","kind":"ble_packet_observation","received_at_us":1795335411990197,"payload":{"type":"ble_packet_observation","session_source":{"receiver_id":"cat"},"detection":{"sig":[12],"future":{"counter":9007199254740993}}}}],"next_after":9007199254740993,"has_more":true}"#
        .utf8)
    let page = try JSONDecoder().decode(RecordPage.self, from: data)
    XCTAssertEqual(page.nextAfter, 9_007_199_254_740_993)
    XCTAssertTrue(page.hasMore)
    let observations = try page.observations()
    XCTAssertEqual(observations.observations.first?.receiver, "cat")
    XCTAssertTrue(observations.observations.first?.raw.contains("9007199254740993") == true)
    let value = try JSONDecoder().decode(JSONValue.self, from: data)
    XCTAssertEqual(try JSONDecoder().decode(JSONValue.self, from: value.data()), value)
  }

  func testRecordWindowEvictsPresentationOnlyAndRejectsCursorRegression() throws {
    var window = RecordWindow()
    for cursor in 1...1005 {
      let json =
        "{\"records\":[{\"cursor\":\(cursor),\"source_id\":\"test\",\"kind\":\"wifi_observation\",\"received_at_us\":1,\"payload\":{\"type\":\"wifi_observation\"}}],\"next_after\":\(cursor),\"has_more\":true}"
      try window.append(JSONDecoder().decode(RecordPage.self, from: Data(json.utf8)))
    }
    XCTAssertEqual(window.records.count, 1000)
    XCTAssertEqual(window.omitted, 5)
    XCTAssertEqual(window.cursor, 1005)
    XCTAssertEqual(try window.observations().observations.count, 1000)
    XCTAssertEqual(try window.observations().observations.first?.id, 6)
    XCTAssertEqual(try window.observations().observations.last?.id, 1005)
    let invalid = try JSONDecoder().decode(
      RecordPage.self, from: Data(#"{"records":[],"next_after":3,"has_more":false}"#.utf8))
    XCTAssertThrowsError(try window.append(invalid))
    XCTAssertEqual(window.cursor, 1005)
  }

  func testCachedWindowPreservesEvidenceIgnoredRecordsAndRejectsPartialPage() throws {
    func page(_ records: [[String: Any]], after: Int) throws -> RecordPage {
      try JSONDecoder().decode(
        RecordPage.self,
        from: JSONSerialization.data(withJSONObject: [
          "records": records, "next_after": after, "has_more": false,
        ]))
    }
    func record(_ cursor: Int, _ payload: [String: Any]) -> [String: Any] {
      [
        "cursor": cursor, "source_id": "test", "kind": payload["type"] ?? "invalid",
        "received_at_us": 1, "payload": payload,
      ]
    }
    var window = RecordWindow()
    try window.append(
      page(
        [
          record(10, ["type": "scanner_health"]),
          record(
            20,
            [
              "type": "ble_packet_observation", "address": "02:00:00:00:00:01",
              "detection": ["sig": [42], "alert": "silent"],
            ]),
        ], after: 20))
    let first = try window.observations()
    XCTAssertEqual(first.ignoredRecords, 1)
    XCTAssertEqual(first.observations.map(\.id), [20])
    XCTAssertTrue(first.observations[0].raw.contains("42"))
    XCTAssertFalse(first.observations[0].evidence.isEmpty)
    try window.append(page([], after: 20))
    XCTAssertEqual(try window.observations().observations[0].raw, first.observations[0].raw)
    // Decoding the second record must fail without committing the first record.
    XCTAssertThrowsError(
      try window.append(
        page(
          [
            record(30, ["type": "wifi_observation", "ssid": "new"]),
            record(40, ["missing_type": true]),
          ], after: 40)))
    XCTAssertEqual(window.cursor, 20)
    XCTAssertEqual(window.records.count, 2)
    XCTAssertEqual(try window.observations().observations.map(\.id), [20])
    XCTAssertThrowsError(
      try window.append(page([record(20, ["type": "scanner_health"])], after: 20)))
    XCTAssertEqual(window.cursor, 20)
  }

  func testCachedWindowEvictsRowsAtByteLimit() throws {
    let records: [[String: Any]] = (1...24).map { cursor in
      [
        "cursor": cursor, "source_id": "test", "kind": "wifi_observation", "received_at_us": 1,
        "payload": [
          "type": "wifi_observation", "ssid": "test",
          "future_data": String(repeating: "x", count: 200_000),
        ],
      ]
    }
    let page = try JSONDecoder().decode(
      RecordPage.self,
      from: JSONSerialization.data(withJSONObject: [
        "records": records, "next_after": 24, "has_more": false,
      ]))
    var window = RecordWindow()
    try window.append(page)
    XCTAssertLessThan(window.records.count, 24)
    XCTAssertEqual(window.omitted + window.records.count, 24)
    XCTAssertEqual(
      try window.observations().observations.map(\.id), window.records.map { Int($0.cursor) })
    XCTAssertLessThanOrEqual(
      try window.records.reduce(0) { try $0 + $1.payload.data().count + 1 }, 4 * 1024 * 1024)
    XCTAssertEqual(try window.observations().ignoredRecords, 0)
  }

  func testPlanReadRejectsNonregularAndOversizedFiles() throws {
    let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try LocalFiles.privateDirectory(directory)
    defer { try? FileManager.default.removeItem(at: directory) }
    let fifo = directory.appendingPathComponent("fifo")
    XCTAssertEqual(mkfifo(fifo.path, 0o600), 0)
    XCTAssertThrowsError(try LocalFiles.read(fifo, limit: 64))
    XCTAssertThrowsError(try LocalFiles.read(directory, limit: 64))
    let file = directory.appendingPathComponent("plan")
    try Data(repeating: 65, count: 65).write(to: file)
    XCTAssertThrowsError(try LocalFiles.read(file, limit: 64))
    XCTAssertEqual(try LocalFiles.read(file, limit: 65).count, 65)
    let link = directory.appendingPathComponent("link")
    try FileManager.default.createSymbolicLink(at: link, withDestinationURL: directory)
    XCTAssertThrowsError(try LocalFiles.privateDirectory(link))
    try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: directory.path)
    XCTAssertThrowsError(try LocalFiles.privateDirectory(directory))
  }

  func testConfigurationHintPreservesBackendFieldNames() throws {
    let json = inventory.replacingOccurrences(
      of: "\"capabilities\":{",
      with:
        "\"configuration_hint\":{\"kind\":\"sniffle\",\"module_path\":\"/example\",\"usb_selector\":null},\"capabilities\":{"
    )
    let hint = try Inventory.decode(Data(json.utf8)).devices[0].candidates[0].configurationHint
    XCTAssertEqual(hint?["module_path"], .string("/example"))
    XCTAssertEqual(hint?["usb_selector"], .null)
  }

  func testReceiverMetricsPreserveUnknownVersusZero() throws {
    let data = Data(
      #"{"receiver_health":{"receivers":[{"id":"alfa","scanners":{"wifi":{"state":"unavailable","reason":"receiver_task_failed"},"ble":{"state":"disabled","reason":"not_assigned"}}}]}}"#
        .utf8)
    let capture = try JSONDecoder().decode(JSONValue.self, from: data)
    let missing = try XCTUnwrap(ReceiverStatus.decode(capture, sourceEvents: [:]).first)
    XCTAssertEqual(missing.hostDrops, "Not reported")
    XCTAssertEqual(missing.recovery, "Not reported")
    XCTAssertEqual(missing.phases, "Wi-Fi: unavailable")
    let reported = try XCTUnwrap(
      ReceiverStatus.decode(
        capture,
        sourceEvents: [
          "alfa": .object([
            "host_dropped_observations": .integer(0), "producer_dropped_observations": .integer(5),
            "reconnect_attempts": .integer(2),
          ])
        ]
      ).first)
    XCTAssertEqual(reported.hostDrops, "0")
    XCTAssertEqual(reported.producerDrops, "5")
    XCTAssertEqual(reported.recovery, "2")
  }

  func testPendingStartSurvivesRecreationAndRefusesReplacement() throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try LocalFiles.privateDirectory(root)
    defer { try? FileManager.default.removeItem(at: root) }
    let socket = root.appendingPathComponent("service.sock")
    let store = PendingStartStore(socket: socket)
    XCTAssertNil(try store.load())
    let config = Data("{\"version\":1}".utf8)
    let pending = try store.prepare(socket: socket, config: config)
    let reopened = PendingStartStore(socket: socket)
    XCTAssertEqual(try reopened.load(), pending)
    XCTAssertEqual(
      try reopened.prepare(socket: socket, config: config).requestID, pending.requestID)
    XCTAssertThrowsError(try reopened.prepare(socket: socket, config: Data("{}".utf8)))
    XCTAssertEqual(try reopened.load(), pending)
    try reopened.clear(pending)
    XCTAssertNil(try store.load())
    XCTAssertNotEqual(
      try store.prepare(socket: socket, config: config).requestID, pending.requestID)
    XCTAssertThrowsError(try store.clear(pending))
    let file = root.appendingPathComponent("service.sock.pending-start.json")
    try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: file.path)
    XCTAssertThrowsError(try store.load())
  }

  func testReceiverRowsKeepDistinctDevicesWithCandidateIndexZero() throws {
    let data = Data(
      #"{"schema_version":1,"mode":"passive","platform":"macos","devices":[{"id":"system:ble","identity_scope":"system_backend","name":"Bluetooth","endpoints":[],"candidates":[{"kind":"native_ble","label":"Bluetooth receiver","support":"available","state":"unverified","evidence":[],"requirements":[]}]},{"id":"system:wifi","identity_scope":"system_backend","name":"Wi-Fi","endpoints":[],"candidates":[{"kind":"native_wifi","label":"Wi-Fi receiver","support":"available","state":"unverified","evidence":[],"requirements":[]}]}],"diagnostics":[]}"#
        .utf8)
    let rows = try Inventory.decode(data).receiverChoices
    XCTAssertEqual(rows.map(\.id), ["system:ble:0", "system:wifi:0"])
    XCTAssertEqual(rows.map { $0.candidate.label }, ["Bluetooth receiver", "Wi-Fi receiver"])
    XCTAssertEqual(Set(rows.map(\.id)).count, rows.count)
  }

  func testMissingExecutable() async {
    do {
      _ = try await DiscoveryProcess().run(executable: URL(fileURLWithPath: "/does-not-exist"))
      XCTFail("Expected failure")
    } catch { XCTAssertEqual(error as? DiscoveryError, .invalidExecutable) }
  }
}

/// Thread-safe manual time for the actual subprocess collector. No wall time
/// advances correctness; real OS readiness and terminal state are still checked.
private final class ControlledProcessTime: @unchecked Sendable {
  private let lock = NSLock()
  private var value: TimeInterval = 0
  private var child: Int32 = 0
  private var ready = false
  private var cancelled = false
  private var boundary = 0
  private var signalEvents: [(Int32, TimeInterval)] = []
  private var visited: [TimeInterval] = []
  var signals: [(Int32, TimeInterval)] {
    lock.lock()
    defer { lock.unlock() }
    return signalEvents
  }
  var visitedTimes: [TimeInterval] {
    lock.lock()
    defer { lock.unlock() }
    return visited
  }
  func signalled(_ signal: Int32, _ now: TimeInterval) {
    lock.lock()
    signalEvents.append((signal, now))
    lock.unlock()
  }
  func stepDeadlineBoundary() {
    lock.lock()
    defer { lock.unlock() }
    let steps: [TimeInterval] = [14.999, 15, 15.249, 15.25]
    if boundary < steps.count {
      value = steps[boundary]
      visited.append(value)
      boundary += 1
    }
  }
  var now: TimeInterval {
    lock.lock()
    defer { lock.unlock() }
    return value
  }
  var pid: Int32 {
    lock.lock()
    defer { lock.unlock() }
    return child
  }
  var isCancelled: Bool {
    lock.lock()
    defer { lock.unlock() }
    return cancelled
  }
  func launched(_ pid: Int32) {
    lock.lock()
    child = pid
    lock.unlock()
  }
  func advance(_ amount: TimeInterval) {
    lock.lock()
    value += amount
    lock.unlock()
  }
  func markReady() -> Bool {
    lock.lock()
    defer { lock.unlock() }
    if ready { return false }
    ready = true
    return true
  }
  func cancel() {
    lock.lock()
    cancelled = true
    lock.unlock()
  }
}

/// Simulates time consumed before the queued collector starts, without a sleep
/// or assumptions about how quickly its dispatch queue runs.
private final class DispatchDeadlineTime: @unchecked Sendable {
  private let lock = NSLock()
  private let expired: Double
  private var count = 0
  private var child: Int32 = 0
  init(expired: Double) { self.expired = expired }
  func read() -> Double {
    lock.lock()
    defer { lock.unlock() }
    count += 1
    return count == 1 ? 0 : expired
  }
  func launched(_ pid: Int32) {
    lock.lock()
    child = pid
    lock.unlock()
  }
  var pid: Int32 {
    lock.lock()
    defer { lock.unlock() }
    return child
  }
  var reads: Int {
    lock.lock()
    defer { lock.unlock() }
    return count
  }
}
