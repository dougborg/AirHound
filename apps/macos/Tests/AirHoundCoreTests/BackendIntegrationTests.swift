import Foundation
import XCTest

@testable import AirHoundCore

final class BackendIntegrationTests: XCTestCase {
  func testReplaySurvivesClientReconnectAndExportsHistory() async throws {
    guard let path = ProcessInfo.processInfo.environment["AIRHOUND_TEST_BACKEND"] else {
      throw XCTSkip("Set AIRHOUND_TEST_BACKEND to exercise the real Rust service without radios.")
    }
    let directory = FileManager.default.temporaryDirectory.appendingPathComponent(
      "ah-" + UUID().uuidString.prefix(8))
    try LocalFiles.privateDirectory(directory)
    defer { try? FileManager.default.removeItem(at: directory) }
    let executable = URL(fileURLWithPath: path)
    let socket = directory.appendingPathComponent("control.sock")
    let process = Process()
    process.executableURL = executable
    process.arguments = [
      "serve", "--socket", socket.path, "--database",
      directory.appendingPathComponent("history.sqlite").path, "--capture",
    ]
    process.standardOutput = FileHandle.nullDevice
    process.standardError = FileHandle.nullDevice
    try process.run()
    defer {
      if process.isRunning {
        process.terminate()
        process.waitUntilExit()
      }
    }
    for _ in 0..<100 {
      if FileManager.default.fileExists(atPath: socket.path) { break }
      try await Task.sleep(for: .milliseconds(50))
    }
    let pcap = directory.appendingPathComponent("empty.pcap")
    // Little-endian PCAP v2.4, linktype radiotap (127), no packets.
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
    let firstClient = BackendClient(executable: executable, socket: socket)
    _ = try await firstClient.validate(config: config)
    let accepted = try await firstClient.start(config: config, requestID: "swift-reconnect-test")
    let runID = try XCTUnwrap(accepted["run_id"]?.string)
    // A fresh client uses new subprocesses/connections, with no in-memory session state.
    let reconnected = BackendClient(executable: executable, socket: socket)
    let retried = try await reconnected.start(config: config, requestID: "swift-reconnect-test")
    XCTAssertEqual(retried["run_id"]?.string, runID)
    var status = try await reconnected.status()
    for _ in 0..<100 where status["active"] == .bool(true) {
      try await Task.sleep(for: .milliseconds(50))
      status = try await reconnected.status()
    }
    let runs = try await reconnected.recordings()
    XCTAssertEqual(runs.runs.count, 1)
    XCTAssertEqual(runs.runs[0].id, runID)
    XCTAssertEqual(runs.runs[0].state, "finished")
    let records = try await reconnected.records(runID: runID)
    XCTAssertFalse(records.records.isEmpty)
    let snapshot = directory.appendingPathComponent("export.sqlite")
    let exported = try await reconnected.export(to: snapshot)
    XCTAssertEqual(exported["consistent_snapshot"], .bool(true))
    XCTAssertTrue(FileManager.default.fileExists(atPath: snapshot.path))
    XCTAssertTrue(process.isRunning, "Disconnecting clients must not terminate the service")
  }
}
