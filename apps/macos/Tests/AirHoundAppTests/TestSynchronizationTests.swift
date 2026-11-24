import AirHoundCore
import AirHoundTestSupport
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

  func testStorageWritesDuringOutstandingQueryAreRetainedAndNewWALIsWatched() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let wal = root.appendingPathComponent("history.sqlite-wal")
    let querying = expectation(description: "Initial query is suspended")
    var release: CheckedContinuation<Void, Never>?
    var queries = 0
    let task = Task {
      try await waitForFixtureStorage([wal]) {
        queries += 1
        if queries == 1 {
          await withCheckedContinuation { continuation in
            release = continuation
            querying.fulfill()
          }
          return false  // The query began before the durable change.
        }
        return (try? Data(contentsOf: wal)) == Data("finished".utf8)
      }
    }
    await fulfillment(of: [querying], timeout: 30)
    try Data("finished".utf8).write(to: wal)
    release?.resume()
    try await task.value
    XCTAssertGreaterThanOrEqual(queries, 2)
    // The already-terminal path must also succeed without another file write.
    try await waitForFixtureStorage([wal]) { true }
  }

  func testReplacedWALIsRegisteredBeforeSubsequentInPlaceWrite() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let wal = root.appendingPathComponent("history.sqlite-wal")
    try Data("pending!".utf8).write(to: wal)
    let first = expectation(description: "Original WAL registered")
    let replacement = expectation(description: "Replacement WAL registered")
    var registrations = 0
    let task = Task {
      try await waitForFixtureStorage(
        [wal],
        onWatching: { path in
          guard path == wal else { return }
          registrations += 1
          if registrations == 1 { first.fulfill() }
          if registrations == 2 { replacement.fulfill() }
        }
      ) {
        (try? Data(contentsOf: wal)) == Data("finished".utf8)
      }
    }
    await fulfillment(of: [first], timeout: 30)
    try Data("pending!".utf8).write(to: wal, options: .atomic)
    await fulfillment(of: [replacement], timeout: 30)
    let handle = try FileHandle(forWritingTo: wal)
    try handle.write(contentsOf: Data("finished".utf8))
    try handle.close()
    try await task.value
    XCTAssertEqual(registrations, 2)
  }

  func testStorageWitnessCancellationJoinsItsPendingQueryWithoutAnotherWrite() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let queried = expectation(description: "Durable state is still pending")
    var first = true
    let task = Task {
      try await waitForFixtureStorage([root.appendingPathComponent("history.sqlite")]) {
        if first {
          first = false
          queried.fulfill()
        }
        return false
      }
    }
    await fulfillment(of: [queried], timeout: 30)
    task.cancel()
    do {
      try await task.value
      XCTFail("Cancelled storage witness must not report success")
    } catch is CancellationError {
    }
  }
}
