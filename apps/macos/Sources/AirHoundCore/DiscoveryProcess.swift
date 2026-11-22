import Darwin
import Foundation

public enum DiscoveryError: LocalizedError, Equatable {
  case invalidExecutable, timedOut, outputLimit, cancelled
  case failed(Int32, String)
  public var errorDescription: String? {
    switch self {
    case .invalidExecutable: return "Choose an executable airhound-hostd file."
    case .timedOut: return "Device discovery timed out. Check the backend and try again."
    case .outputLimit: return "Device discovery exceeded its output limit."
    case .cancelled: return "Device discovery cancelled."
    case .failed(let status, let message):
      return "Device discovery exited with status \(status). \(message)"
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

public struct DiscoveryProcess {
  public init() {}
  public func run(executable: URL, timeout: TimeInterval = 15, outputLimit: Int = 4 * 1024 * 1024)
    async throws -> Data
  {
    let cancellation = Cancellation()
    return try await withTaskCancellationHandler(
      operation: {
        try await withCheckedThrowingContinuation { continuation in
          DispatchQueue.global(qos: .userInitiated).async {
            continuation.resume(
              with: Result {
                try Self.collect(
                  executable, timeout: timeout, limit: outputLimit, cancellation: cancellation)
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
    _ executable: URL, timeout: TimeInterval, limit: Int, cancellation: Cancellation
  ) throws -> Data {
    guard executable.isFileURL, FileManager.default.isExecutableFile(atPath: executable.path),
      timeout > 0, limit > 0
    else { throw DiscoveryError.invalidExecutable }
    if cancellation.isCancelled { throw DiscoveryError.cancelled }
    let process = Process()
    process.executableURL = executable
    process.arguments = ["devices", "list", "--json"]
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
    try? stdout.fileHandleForWriting.close()
    try? stderr.fileHandleForWriting.close()
    var output = Data()
    var errors = Data()
    var failure: DiscoveryError?
    let start = ProcessInfo.processInfo.systemUptime
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
      let now = ProcessInfo.processInfo.systemUptime
      if cancellation.isCancelled { failure = failure ?? .cancelled }
      if now - start >= timeout { failure = failure ?? .timedOut }
      if failure != nil && process.isRunning {
        if stopTime == nil {
          process.terminate()
          stopTime = now
        } else if now - stopTime! >= 0.25 {
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
      Thread.sleep(forTimeInterval: 0.01)
    } while true
    process.waitUntilExit()
    if let failure { throw failure }
    guard process.terminationReason == .exit, process.terminationStatus == 0 else {
      throw DiscoveryError.failed(
        process.terminationStatus, String(decoding: errors.prefix(4096), as: UTF8.self))
    }
    return output
  }
}
