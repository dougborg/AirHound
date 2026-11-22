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
  }
}
