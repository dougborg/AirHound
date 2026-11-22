import Darwin
import XCTest

@testable import AirHoundCore

final class CoreTests: XCTestCase {
  private let inventory =
    #"{"schema_version":1,"mode":"passive","platform":"macos","devices":[{"id":"usb:example","identity_scope":"attachment_topology","name":"Example bridge","endpoints":[{"kind":"serial","path":"/dev/example"}],"candidates":[{"kind":"future_backend","label":"Future role","support":"candidate","state":"needs_identification","evidence":["Generic bridge"],"requirements":[{"name":"firmware","state":"unknown","detail":"Inspect manually"}],"capabilities":{"scope":"unknown"}}]}],"diagnostics":[{"backend":"ioreg","message":"Partial inventory"}]}"#

  func testInventoryKeepsUncertaintyAndUnknownCandidates() throws {
    let value = try Inventory.decode(Data(inventory.utf8))
    XCTAssertEqual(value.devices[0].candidates[0].state, "needs_identification")
    XCTAssertEqual(value.devices[0].candidates[0].kind, "future_backend")
    XCTAssertEqual(value.diagnostics[0].message, "Partial inventory")
    XCTAssertEqual(value.devices[0].identityScope, "attachment_topology")
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
    let start = Date()
    XCTAssertThrowsError(try Recording.load(fifo))
    XCTAssertThrowsError(try Recording.load(root))
    XCTAssertLessThan(Date().timeIntervalSince(start), 1)
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
    let executable = try script("trap '' TERM\nwhile :; do :; done\n")
    let start = Date()
    do {
      _ = try await DiscoveryProcess().run(executable: executable, timeout: 0.1)
      XCTFail("Expected timeout")
    } catch { XCTAssertEqual(error as? DiscoveryError, .timedOut) }
    XCTAssertLessThan(Date().timeIntervalSince(start), 3)
  }
  func testCancellationWaitsForOwnedProcess() async throws {
    let executable = try script("trap '' TERM\nwhile :; do :; done\n")
    let task = Task { try await DiscoveryProcess().run(executable: executable) }
    try await Task.sleep(nanoseconds: 50_000_000)
    task.cancel()
    do {
      _ = try await task.value
      XCTFail("Expected cancellation")
    } catch { XCTAssertEqual(error as? DiscoveryError, .cancelled) }
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
  func testMissingExecutable() async {
    do {
      _ = try await DiscoveryProcess().run(executable: URL(fileURLWithPath: "/does-not-exist"))
      XCTFail("Expected failure")
    } catch { XCTAssertEqual(error as? DiscoveryError, .invalidExecutable) }
  }
}
