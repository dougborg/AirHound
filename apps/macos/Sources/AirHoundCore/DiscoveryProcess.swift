import Darwin
import Foundation

public enum DiscoveryError: LocalizedError, Equatable {
  case invalidExecutable, timedOut, outputLimit, cancelled
  case failed(Int32, String)
  public var errorDescription: String? {
    switch self {
    case .invalidExecutable: return "Choose an executable airhound-hostd file."
    case .timedOut: return "Backend command timed out. Reconnect to check its current state."
    case .outputLimit: return "Backend command exceeded its output limit."
    case .cancelled: return "Backend command cancelled."
    case .failed(let status, let message):
      return "Backend command exited with status \(status). \(message)"
    }
  }
}

/// Cancellation is sampled by the process-owning worker; callers never signal a stale PID.
private final class Cancellation: @unchecked Sendable {
  private let lock = NSLock()
  private var flag = false
  func cancel() {
    lock.lock()
    flag = true
    lock.unlock()
  }
  var isCancelled: Bool {
    lock.lock()
    defer { lock.unlock() }
    return flag
  }
}

/// Injectable scheduling keeps deadline correctness independent of machine speed.
/// The public initializer retains the production monotonic clock and poll delay.
struct ProcessScheduling: Sendable {
  var now: @Sendable () -> TimeInterval = { ProcessInfo.processInfo.systemUptime }
  var wait: @Sendable (TimeInterval) -> Void = { Thread.sleep(forTimeInterval: $0) }
  var launched: @Sendable (Int32) -> Void = { _ in }
  var stdout: @Sendable (Data) -> Void = { _ in }
  var signalling: @Sendable (Int32, TimeInterval) -> Void = { _, _ in }
}

public struct DiscoveryProcess: Sendable {
  private let scheduling: ProcessScheduling
  public init() { scheduling = ProcessScheduling() }
  init(scheduling: ProcessScheduling) { self.scheduling = scheduling }
  public func run(
    executable: URL, arguments: [String] = ["devices", "list", "--json"],
    timeout: TimeInterval = 15, outputLimit: Int = 4 * 1024 * 1024
  )
    async throws -> Data
  {
    try await run(
      executable: executable, arguments: arguments, timeout: timeout,
      outputLimit: outputLimit, absoluteDeadline: nil)
  }

  /// Startup probes share their caller's monotonic deadline across dispatch,
  /// process launch and IO. Existing callers retain their ordinary timeout API.
  public func runUntil(
    executable: URL, arguments: [String], deadline: TimeInterval,
    outputLimit: Int = 4 * 1024 * 1024
  ) async throws -> Data {
    let remaining = deadline - scheduling.now()
    guard deadline.isFinite, remaining > 0 else { throw DiscoveryError.timedOut }
    return try await run(
      executable: executable, arguments: arguments, timeout: remaining,
      outputLimit: outputLimit, absoluteDeadline: deadline)
  }

  private func run(
    executable: URL, arguments: [String], timeout: TimeInterval,
    outputLimit: Int, absoluteDeadline: TimeInterval?
  ) async throws -> Data {
    let cancellation = Cancellation()
    return try await withTaskCancellationHandler(
      operation: {
        try await withCheckedThrowingContinuation { continuation in
          DispatchQueue.global(qos: .userInitiated).async {
            continuation.resume(
              with: Result {
                try Self.collect(
                  executable, arguments: arguments, timeout: timeout, limit: outputLimit,
                  cancellation: cancellation, scheduling: scheduling,
                  absoluteDeadline: absoluteDeadline)
              })
          }
        }
      }, onCancel: { cancellation.cancel() })
  }

  static func configureNonblocking(_ fd: Int32) throws {
    let flags = fcntl(fd, F_GETFL)
    guard flags >= 0, fcntl(fd, F_SETFL, flags | O_NONBLOCK) >= 0 else {
      throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno))
    }
  }

  private static func collect(
    _ executable: URL, arguments: [String] = ["devices", "list", "--json"], timeout: TimeInterval,
    limit: Int, cancellation: Cancellation, scheduling: ProcessScheduling,
    absoluteDeadline: TimeInterval?
  ) throws -> Data {
    guard executable.isFileURL, FileManager.default.isExecutableFile(atPath: executable.path),
      timeout > 0, limit > 0
    else { throw DiscoveryError.invalidExecutable }
    if cancellation.isCancelled { throw DiscoveryError.cancelled }
    if let absoluteDeadline, scheduling.now() >= absoluteDeadline { throw DiscoveryError.timedOut }
    let process = Process()
    process.executableURL = executable
    process.arguments = arguments
    process.standardInput = FileHandle.nullDevice
    let stdout = Pipe()
    let stderr = Pipe()
    process.standardOutput = stdout
    process.standardError = stderr
    // Only this worker owns/reaps the process. Never wait on unbounded pipe reads:
    // inherited descriptors may outlive the direct child.
    defer {
      try? stdout.fileHandleForWriting.close()
      try? stderr.fileHandleForWriting.close()
      try? stdout.fileHandleForReading.close()
      try? stderr.fileHandleForReading.close()
    }
    let outFD = stdout.fileHandleForReading.fileDescriptor
    let errFD = stderr.fileHandleForReading.fileDescriptor
    // Configure before spawning: a setup failure cannot leave an owned child running.
    try configureNonblocking(outFD)
    try configureNonblocking(errFD)
    try process.run()
    scheduling.launched(process.processIdentifier)
    try? stdout.fileHandleForWriting.close()
    try? stderr.fileHandleForWriting.close()
    var output = Data()
    var errors = Data()
    var failure: DiscoveryError?
    let start = scheduling.now()
    var stopTime: TimeInterval?
    func drain(_ fd: Int32, into data: inout Data, cap: Int) {
      var bytes = [UInt8](repeating: 0, count: 8192)
      // A bounded batch prevents a continuously noisy child starving the deadline.
      for _ in 0..<16 {
        let count = read(fd, &bytes, bytes.count)
        if count <= 0 { break }
        if data.count + count > cap {
          failure = failure ?? .outputLimit
          break
        }
        data.append(contentsOf: bytes.prefix(count))
      }
    }
    repeat {
      drain(outFD, into: &output, cap: limit)
      drain(errFD, into: &errors, cap: min(limit, 64 * 1024))
      scheduling.stdout(output)
      let now = scheduling.now()
      if cancellation.isCancelled { failure = failure ?? .cancelled }
      if absoluteDeadline.map({ now >= $0 }) ?? (now - start >= timeout) {
        failure = failure ?? .timedOut
      }
      if failure != nil && process.isRunning {
        if stopTime == nil {
          scheduling.signalling(SIGTERM, now)
          process.terminate()
          stopTime = now
        } else if now - stopTime! >= 0.25 {
          scheduling.signalling(SIGKILL, now)
          _ = kill(process.processIdentifier, SIGKILL)
        }
      }
      if !process.isRunning {
        // Pipes can still contain bytes after exit; drain a bounded remainder.
        for _ in 0..<(limit / 131072 + 2) {
          drain(outFD, into: &output, cap: limit)
          drain(errFD, into: &errors, cap: min(limit, 64 * 1024))
        }
        break
      }
      scheduling.wait(0.01)
    } while true
    process.waitUntilExit()
    if let absoluteDeadline, scheduling.now() >= absoluteDeadline {
      failure = failure ?? .timedOut
    }
    if let failure { throw failure }
    guard process.terminationReason == .exit, process.terminationStatus == 0 else {
      throw DiscoveryError.failed(
        process.terminationStatus, String(decoding: errors.prefix(4096), as: UTF8.self))
    }
    return output
  }
}
