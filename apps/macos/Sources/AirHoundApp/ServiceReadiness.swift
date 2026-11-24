import AirHoundCore
import Foundation

/// A single startup budget, including launch and authenticated Hello/Status.
/// The probe performs only read-only readiness; cancelling it never stops service
/// capture. Clock/probe injection keeps boundary tests independent of scheduling.
@MainActor
struct ServiceReadiness {
  static let budget: TimeInterval = 5
  var now: () -> TimeInterval = { ProcessInfo.processInfo.systemUptime }
  var probe: (BackendClient, TimeInterval) async throws -> Void = { client, deadline in
    _ = try await client.waitUntilReady(deadline: deadline)
  }

  func deadline() -> TimeInterval { now() + Self.budget }

  func check(_ deadline: TimeInterval) throws {
    try Task.checkCancellation()
    guard now() < deadline else { throw DiscoveryError.timedOut }
  }

  func wait(client: BackendClient, deadline: TimeInterval, running: () -> Bool) async throws {
    try check(deadline)
    guard running() else {
      throw ModelError.invalid(
        "Service exited before becoming ready. Check the directory permissions and database ownership."
      )
    }
    try await probe(client, deadline)
    try check(deadline)
  }
}
