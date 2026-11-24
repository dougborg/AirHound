import AirHoundCore
import AirHoundTestSupport
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

/// Shared with the core integration fixtures; callers retain this convenience name.
@MainActor
func awaitFixtureFile(_ path: URL, onWatching: @escaping () -> Void = {}) async throws {
  try await waitForFixtureFile(path, onWatching: onWatching)
}
