import Darwin
import Foundation

public enum LocalFiles {
  public static func read(_ url: URL, limit: Int) throws -> Data {
    let descriptor = open(url.path, O_RDONLY | O_NONBLOCK | O_CLOEXEC)
    guard descriptor >= 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
    let handle = FileHandle(fileDescriptor: descriptor, closeOnDealloc: true)
    defer { try? handle.close() }
    var metadata = stat()
    guard fstat(descriptor, &metadata) == 0, metadata.st_mode & S_IFMT == S_IFREG else {
      throw ModelError.invalid("Choose a regular file, not a pipe, directory or device.")
    }
    guard metadata.st_size <= limit else {
      throw ModelError.invalid("File exceeds \(limit) bytes.")
    }
    let data = try handle.read(upToCount: limit + 1) ?? Data()
    guard data.count <= limit else { throw ModelError.invalid("File exceeds \(limit) bytes.") }
    return data
  }

  public static func privateDirectory(_ url: URL) throws {
    if !FileManager.default.fileExists(atPath: url.path) {
      try FileManager.default.createDirectory(
        at: url, withIntermediateDirectories: true,
        attributes: [.posixPermissions: 0o700])
    }
    var metadata = stat()
    guard lstat(url.path, &metadata) == 0,
      metadata.st_mode & S_IFMT == S_IFDIR, metadata.st_uid == getuid(),
      metadata.st_mode & 0o077 == 0
    else {
      throw ModelError.invalid(
        "Service directory must be owned by you, private (0700), and not a symlink.")
    }
  }
}
