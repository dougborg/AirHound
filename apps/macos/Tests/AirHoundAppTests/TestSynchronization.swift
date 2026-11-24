import AirHoundCore
import Combine
import Darwin
import XCTest

@testable import AirHoundApp

/// Completion is driven by the actual publishers, never by a scheduling delay.
/// The timeout only terminates a hung test; no correctness claim uses its value.
@MainActor
func awaitModelState(
  _ publishers: [ObservableObjectPublisher],
  description: String = "Model state transition", condition: @escaping () -> Bool
) async throws {
  if condition() { return }
  let completion = XCTestExpectation(description: description)
  var completed = false
  func check() {
    if !completed && condition() {
      completed = true
      completion.fulfill()
    }
  }
  let subscriptions = publishers.map { publisher in
    publisher.sink { Task { @MainActor in check() } }
  }
  defer { for subscription in subscriptions { subscription.cancel() } }
  check()  // Subscribe-before-recheck closes the publication race.
  let result = await XCTWaiter.fulfillment(of: [completion], timeout: 30)
  guard result == .completed else { throw ModelError.invalid(description + " did not complete") }
}

/// Observe the containing directory so creation/rename of the expected socket
/// or handshake file is causal. Subscribe before the final existence check.
@MainActor
func awaitFixtureFile(_ path: URL, onWatching: @escaping () -> Void = {}) async throws {
  if FileManager.default.fileExists(atPath: path.path) { return }
  let fd = open(path.deletingLastPathComponent().path, O_EVTONLY)
  guard fd >= 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
  let source = DispatchSource.makeFileSystemObjectSource(
    fileDescriptor: fd, eventMask: .write, queue: .main)
  let completion = XCTestExpectation(description: "Fixture created " + path.lastPathComponent)
  var completed = false
  func check() {
    if !completed && FileManager.default.fileExists(atPath: path.path) {
      completed = true
      completion.fulfill()
    }
  }
  source.setRegistrationHandler {
    check()
    onWatching()
  }
  source.setEventHandler { check() }
  source.setCancelHandler { close(fd) }
  source.resume()
  defer { source.cancel() }
  check()
  let result = await XCTWaiter.fulfillment(of: [completion], timeout: 30)
  guard result == .completed else { throw ModelError.invalid("Fixture readiness was not reported") }
}
