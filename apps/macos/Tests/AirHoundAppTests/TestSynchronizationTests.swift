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

  func testServiceWitnessRejectsCreatedSocketUntilExplicitPostListenDiagnostic() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let release = root.appendingPathComponent("release")
    let barrier = try FixtureReleasePipe(release)
    let started = root.appendingPathComponent("started")
    let socket = root.appendingPathComponent("control.sock")
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/bin/sh")
    process.arguments = [
      "-c",
      """
      exec 3<>"$1"
      : > "$2"
      printf 'socket created, not listening yet\n' >&2
      : > "$3"
      IFS= read -r released <&3 || exit 2
      printf 'DEBUG AirHound control socket listening' >&2
      IFS= read -r newline <&3 || exit 3
      printf '\n' >&2
      IFS= read -r cleanup <&3
      """, "fixture", release.path, socket.path, started.path,
    ]
    let logPath = root.appendingPathComponent("stderr")
    let log = try FixtureServiceLog(process: process, path: logPath)
    try process.run()
    defer { XCTAssertTrue(stopOwnedProcess(process)) }
    try await awaitFixtureFile(started)
    XCTAssertTrue(FileManager.default.fileExists(atPath: socket.path))
    let notReady = expectation(description: "Socket creation does not satisfy listen witness")
    let partial = expectation(description: "Unterminated diagnostic does not satisfy witness")
    var checked = false
    var checkedPartial = false
    var completed = false
    let task = Task {
      try await log.wait(for: process) { ready in
        if !checked {
          checked = true
          XCTAssertFalse(ready)
          notReady.fulfill()
        }
        if !checkedPartial,
          (try? String(contentsOf: logPath, encoding: .utf8))?.hasSuffix(
            "AirHound control socket listening") == true
        {
          checkedPartial = true
          XCTAssertFalse(ready)
          partial.fulfill()
        }
      }
      completed = true
    }
    await fulfillment(of: [notReady], timeout: 30)
    XCTAssertFalse(completed)
    try barrier.release()
    await fulfillment(of: [partial], timeout: 30)
    XCTAssertFalse(completed)
    try barrier.release()
    try await task.value
    XCTAssertTrue(completed)
  }

  func testServiceExitWithoutAnotherLogWriteWakesAndFailsReadiness() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let release = root.appendingPathComponent("release")
    let barrier = try FixtureReleasePipe(release)
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/bin/sh")
    process.arguments = [
      "-c", "exec 3<>\"$1\"; IFS= read -r release <&3; exit 7", "fixture", release.path,
    ]
    let log = try FixtureServiceLog(process: process, path: root.appendingPathComponent("stderr"))
    try process.run()
    defer { XCTAssertTrue(stopOwnedProcess(process)) }
    let observing = expectation(description: "Readiness is observing a still-running child")
    var first = true
    let task = Task {
      try await log.wait(for: process) { ready in
        if first {
          first = false
          XCTAssertFalse(ready)
          observing.fulfill()
        }
      }
    }
    await fulfillment(of: [observing], timeout: 30)
    try barrier.release()
    do {
      try await task.value
      XCTFail("A child that exits before listen must not be ready")
    } catch {
      XCTAssertEqual((error as NSError).code, 4)
    }
    XCTAssertNil(process.terminationHandler, "Readiness must release its exit observer")
    XCTAssertEqual(process.terminationStatus, 7)
  }

  func testStorageWitnessRefusesCompetingExitObserverWithoutReplacingIt() async throws {
    let process = Process()
    let original = expectation(description: "Original exit observer retained")
    process.terminationHandler = { _ in original.fulfill() }
    do {
      try await waitForFixtureStorage([], ownedProcess: process) {
        XCTFail("Competing exit ownership must reject before querying")
        return true
      }
      XCTFail("Competing exit ownership must be rejected")
    } catch {
      XCTAssertEqual((error as NSError).code, 5)
    }
    process.terminationHandler?(process)
    await fulfillment(of: [original], timeout: 30)
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
