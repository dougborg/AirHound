import AirHoundCore
import AirHoundTestSupport
import XCTest

@testable import AirHoundApp

@MainActor
final class ServiceReadinessTests: XCTestCase {
  private let client = BackendClient(
    executable: URL(fileURLWithPath: "/unused-fixture"),
    socket: URL(fileURLWithPath: "/unused.sock"))

  func testLaunchAndProbeShareOneDeadlineWithExactBoundaryChecks() async throws {
    for before in [4.999, 5.0, 5.001] {
      var now = 0.0
      var probes: [Double] = []
      let readiness = ServiceReadiness(
        now: { now }, probe: { _, deadline in probes.append(deadline) })
      let deadline = readiness.deadline()
      now = before
      do {
        try await readiness.wait(client: client, deadline: deadline, running: { true })
        XCTAssertLessThan(before, 5)
      } catch {
        XCTAssertEqual(error as? DiscoveryError, .timedOut)
        XCTAssertGreaterThanOrEqual(before, 5)
      }
      XCTAssertEqual(probes, before < 5 ? [5] : [])
    }
    for completion in [4.999, 5.0, 5.001] {
      var now = 0.0
      let readiness = ServiceReadiness(
        now: { now },
        probe: { _, deadline in
          XCTAssertEqual(deadline, 5)
          now = completion
        })
      do {
        try await readiness.wait(client: client, deadline: 5, running: { true })
        XCTAssertLessThan(completion, 5)
      } catch {
        XCTAssertEqual(error as? DiscoveryError, .timedOut)
        XCTAssertGreaterThanOrEqual(completion, 5)
      }
    }
  }

  func testProbeErrorsKeepTheirIdentityAndAreNotRetried() async throws {
    for message in ["permission denied", "incompatible hello"] {
      var calls = 0
      let expected = DiscoveryError.failed(7, message)
      let readiness = ServiceReadiness(
        now: { 0 },
        probe: { _, _ in
          calls += 1
          throw expected
        })
      do {
        try await readiness.wait(client: client, deadline: 5, running: { true })
        XCTFail("Failed readiness must not connect")
      } catch {
        XCTAssertEqual(error as? DiscoveryError, expected)
      }
      XCTAssertEqual(calls, 1)
    }
  }

  func testCancellationAfterProbeStartsDoesNotAcceptItsLateSuccess() async throws {
    let entered = expectation(description: "Readiness probe started")
    var release: CheckedContinuation<Void, Never>?
    let readiness = ServiceReadiness(
      now: { 0 },
      probe: { _, _ in
        await withCheckedContinuation { continuation in
          release = continuation
          entered.fulfill()
        }
      })
    let task = Task { try await readiness.wait(client: client, deadline: 5, running: { true }) }
    await fulfillment(of: [entered], timeout: 30)
    task.cancel()
    release?.resume()
    do {
      try await task.value
      XCTFail("A cancelled startup must not accept a late successful probe")
    } catch is CancellationError {
    }
  }

  func testOldCancelledStartupCannotClearNewTaskBusyStateOrError() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try LocalFiles.privateDirectory(root)
    defer { try? FileManager.default.removeItem(at: root) }
    let executable = root.appendingPathComponent("fixture")
    let barrier = try FixtureReleasePipe(executable.appendingPathExtension("gate"))
    defer { withExtendedLifetime(barrier) {} }
    try Data("#!/bin/sh\nexec 3<>\"$0.gate\"\nIFS= read -r release <&3\n".utf8).write(
      to: executable)
    try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: executable.path)
    let first = expectation(description: "First startup probe entered")
    let second = expectation(description: "Second startup probe entered")
    var releases: [CheckedContinuation<Void, Error>?] = []
    defer { for release in releases { release?.resume(throwing: CancellationError()) } }
    let readiness = ServiceReadiness(
      now: { 0 },
      probe: { _, _ in
        try await withCheckedThrowingContinuation { continuation in
          releases.append(continuation)
          (releases.count == 1 ? first : second).fulfill()
        }
      })
    let suite = "readiness-" + UUID().uuidString
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let model = SessionModel(preferences: defaults, readiness: readiness)
    var owned: [Process] = []
    defer {
      model.disconnect()
      if let process = model.service { XCTAssertTrue(stopOwnedProcess(process)) }
      for process in owned { XCTAssertTrue(stopOwnedProcess(process)) }
    }
    model.socketPath = root.appendingPathComponent("first.sock").path
    model.startService(executable: executable)
    let oldTask = try XCTUnwrap(model.startupTask)
    await fulfillment(of: [first], timeout: 30)
    owned.append(try XCTUnwrap(model.service))
    model.disconnect()
    XCTAssertFalse(model.busy)
    XCTAssertNil(model.startupTask)
    model.socketPath = root.appendingPathComponent("second.sock").path
    model.startService(executable: executable)
    let newTask = try XCTUnwrap(model.startupTask)
    await fulfillment(of: [second], timeout: 30)
    releases[0]?.resume(throwing: DiscoveryError.failed(7, "stale failure"))
    releases[0] = nil
    await oldTask.value
    XCTAssertTrue(model.busy)
    XCTAssertNotNil(model.startupTask)
    XCTAssertNil(model.error)
    releases[1]?.resume(throwing: DiscoveryError.failed(8, "current failure"))
    releases[1] = nil
    await newTask.value
    XCTAssertFalse(model.busy)
    XCTAssertNil(model.startupTask)
    XCTAssertTrue(model.error?.contains("current failure") == true)
    XCTAssertTrue(owned[0].isRunning, "Cancelling presentation startup must not stop its service")
  }
}
