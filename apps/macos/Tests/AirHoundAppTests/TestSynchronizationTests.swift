import AirHoundCore
import XCTest

@testable import AirHoundApp

@MainActor
final class TestSynchronizationTests: XCTestCase {
  func testModelPublicationCompletesWaitingPredicateAndAlreadyReadyState() async throws {
    let suite = "fixture-" + UUID().uuidString
    let preferences = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { preferences.removePersistentDomain(forName: suite) }
    let model = SessionModel(preferences: preferences)
    model.busy = true
    let waiting = expectation(description: "Predicate observed outstanding operation")
    var reported = false
    let task = Task {
      try await awaitModelState([model.objectWillChange]) {
        if model.busy && !reported {
          reported = true
          waiting.fulfill()
        }
        return !model.busy
      }
    }
    await fulfillment(of: [waiting], timeout: 30)
    model.busy = false
    try await task.value
    XCTAssertFalse(model.busy)
    try await awaitModelState([]) { true }
  }

  func testFileCreationUsesRegisteredFilesystemEventAndExistingFileFastPath() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let path = root.appendingPathComponent("ready")
    let watching = expectation(description: "Filesystem event source registered")
    let task = Task { try await awaitFixtureFile(path, onWatching: { watching.fulfill() }) }
    await fulfillment(of: [watching], timeout: 30)
    try Data("ready".utf8).write(to: path)
    try await task.value
    try await awaitFixtureFile(path)
    XCTAssertEqual(try Data(contentsOf: path), Data("ready".utf8))
  }
}
