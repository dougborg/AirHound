import Darwin
import Foundation
import XCTest

/// A test-owned response barrier. The parent holds the FIFO open, so release
/// before the child's read is retained rather than depending on scheduling.
public final class FixtureReleasePipe {
  private let fd: Int32
  public init(_ path: URL) throws {
    guard mkfifo(path.path, 0o600) == 0 else {
      throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno))
    }
    fd = open(path.path, O_RDWR | O_NONBLOCK)
    guard fd >= 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
  }
  deinit { close(fd) }
  public func release() throws {
    guard "go\n".withCString({ write(fd, $0, 3) }) == 3 else {
      throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno))
    }
  }
}

/// A real exit notification, with an outer hang guard. Callers own this Process
/// and must not install a competing termination handler.
public func waitForOwnedProcess(_ process: Process, seconds: TimeInterval = 30) -> Bool {
  let exited = DispatchSemaphore(value: 0)
  process.terminationHandler = { _ in exited.signal() }
  defer { process.terminationHandler = nil }
  // Installing before checking closes the already-exited/registration race.
  if !process.isRunning { return true }
  guard exited.wait(timeout: .now() + seconds) == .success else { return !process.isRunning }
  return !process.isRunning
}

/// Cleanup owns only this Process. Tests of the escalation branch inject the
/// grace-expiration result; they do not wait for a short real timeout to win.
public func stopOwnedProcess(
  _ process: Process, grace: TimeInterval = 2,
  wait: (Process, TimeInterval) -> Bool = waitForOwnedProcess
) -> Bool {
  guard process.isRunning else { return true }
  process.terminate()
  if wait(process, grace) { return true }
  if process.isRunning { _ = Darwin.kill(process.processIdentifier, SIGKILL) }
  return wait(process, 30)
}

/// Watch an actual file creation rather than polling its existence. Both the
/// registration callback and the post-registration check close creation races.
@MainActor
public func waitForFixtureFile(_ path: URL, onWatching: @escaping () -> Void = {}) async throws {
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
  guard await XCTWaiter.fulfillment(of: [completion], timeout: 30) == .completed else {
    throw NSError(
      domain: "AirHoundTestSupport", code: 1,
      userInfo: [NSLocalizedDescriptionKey: "Fixture readiness was not reported"])
  }
}

/// Recheck durable state only after an actual file write/lifecycle event. The
/// parent-directory watch covers creation of a WAL after registration. Subscribe
/// before the initial query; bounded coalescing retains writes during that query.
@MainActor
public func waitForFixtureStorage(
  _ files: [URL], onWatching: @escaping (URL) -> Void = { _ in },
  condition: @escaping @MainActor () async throws -> Bool
) async throws {
  let events = AsyncStream<Void>.makeStream(bufferingPolicy: .bufferingNewest(1))
  var sources: [String: DispatchSourceFileSystemObject] = [:]
  let completion = XCTestExpectation(description: "Fixture durable state completed")
  var failure: Error?
  func watch(_ path: URL, directory: Bool) {
    guard sources[path.path] == nil else { return }
    let fd = open(path.path, O_EVTONLY)
    guard fd >= 0 else { return }  // A later parent event discovers newly created WALs.
    let source = DispatchSource.makeFileSystemObjectSource(
      fileDescriptor: fd, eventMask: [.write, .extend, .rename, .delete], queue: .main)
    source.setCancelHandler { close(fd) }
    source.setRegistrationHandler {
      onWatching(path)
      events.continuation.yield(())
    }
    source.setEventHandler {
      if !source.data.intersection([.rename, .delete]).isEmpty {
        sources.removeValue(forKey: path.path)
        source.setEventHandler(handler: nil)
        source.setRegistrationHandler(handler: nil)
        source.cancel()
        // The parent event may already have run while this old inode occupied
        // the path entry. Re-arm here too, so event ordering cannot lose writes
        // to its replacement. Registration then triggers a fresh state query.
        watch(path, directory: directory)
      }
      if directory {
        for file in files { watch(file, directory: false) }
      }
      events.continuation.yield(())
    }
    sources[path.path] = source
    source.resume()
  }
  for file in files {
    watch(file.deletingLastPathComponent(), directory: true)
    watch(file, directory: false)
  }
  let task = Task { @MainActor in
    do {
      for await _ in events.stream {
        try Task.checkCancellation()
        if try await condition() {
          completion.fulfill()
          return
        }
      }
      throw CancellationError()
    } catch {
      failure = error
      completion.fulfill()
    }
  }
  events.continuation.yield(())
  let result = await withTaskCancellationHandler {
    await XCTWaiter.fulfillment(of: [completion], timeout: 30)
  } onCancel: {
    events.continuation.finish()
  }
  task.cancel()
  events.continuation.finish()
  await task.value
  for source in sources.values {
    source.setEventHandler(handler: nil)
    source.setRegistrationHandler(handler: nil)
    source.cancel()
  }
  sources.removeAll()
  if let failure { throw failure }
  guard result == .completed else {
    throw NSError(
      domain: "AirHoundTestSupport", code: 2,
      userInfo: [NSLocalizedDescriptionKey: "Fixture durable state did not complete"])
  }
}
