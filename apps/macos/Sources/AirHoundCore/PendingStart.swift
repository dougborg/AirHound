import Darwin
import Foundation

public struct PendingStart: Codable, Equatable {
  public let requestID: String
  public let socket: String
  public let config: Data
}

/// Persist before dispatch; publish without replacing another client's pending start.
/// This stores client intent only. The Rust service owns idempotency and capture state.
public struct PendingStartStore {
  private let directory: URL
  private let file: URL
  public init(socket: URL) {
    directory = socket.deletingLastPathComponent()
    file = directory.appendingPathComponent(socket.lastPathComponent + ".pending-start.json")
  }
  public func load() throws -> PendingStart? {
    var metadata = stat()
    if lstat(file.path, &metadata) != 0 {
      if errno == ENOENT { return nil }
      throw posixError()
    }
    guard metadata.st_mode & S_IFMT == S_IFREG, metadata.st_uid == getuid(),
      metadata.st_mode & 0o077 == 0
    else {
      throw ModelError.invalid("Pending start must be a private regular file owned by you.")
    }
    return try JSONDecoder().decode(
      PendingStart.self, from: LocalFiles.read(file, limit: 128 * 1024))
  }
  public func prepare(socket: URL, config: Data) throws -> PendingStart {
    guard config.count <= 65536 else {
      throw ModelError.invalid("Session plans must be at most 64 KiB.")
    }
    try LocalFiles.privateDirectory(directory)
    if let prior = try load() { return try matching(prior, socket: socket, config: config) }
    let pending = PendingStart(requestID: UUID().uuidString, socket: socket.path, config: config)
    let temporary = directory.appendingPathComponent(".pending-" + UUID().uuidString)
    let fd = open(temporary.path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0o600)
    guard fd >= 0 else { throw posixError() }
    let handle = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
    defer {
      try? handle.close()
      try? FileManager.default.removeItem(at: temporary)
    }
    try handle.write(contentsOf: JSONEncoder().encode(pending))
    guard fsync(fd) == 0 else { throw posixError() }
    if link(temporary.path, file.path) != 0 {
      guard errno == EEXIST, let prior = try load() else { throw posixError() }
      return try matching(prior, socket: socket, config: config)
    }
    try syncDirectory()
    return pending
  }
  public func clear(_ pending: PendingStart) throws {
    guard try load() == pending else {
      throw ModelError.invalid(
        "Pending start changed; reconnect before starting another recording.")
    }
    guard unlink(file.path) == 0 else { throw posixError() }
    try syncDirectory()
  }
  private func matching(_ pending: PendingStart, socket: URL, config: Data) throws -> PendingStart {
    guard pending.socket == socket.path, pending.config == config else {
      throw ModelError.invalid(
        "A different start is pending. Reconnect and resolve that request before changing the plan."
      )
    }
    return pending
  }
  private func syncDirectory() throws {
    let fd = open(directory.path, O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW)
    guard fd >= 0 else { throw posixError() }
    defer { close(fd) }
    guard fsync(fd) == 0 else { throw posixError() }
  }
  private func posixError() -> NSError { NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
}
