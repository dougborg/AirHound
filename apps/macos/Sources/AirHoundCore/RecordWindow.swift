import Foundation

/// A bounded presentation window; the complete recording remains in the Rust store.
public struct RecordWindow {
  public private(set) var records: [StoredRecord] = []
  public private(set) var cursor: Int64 = 0
  public private(set) var omitted = 0
  private var sizes: [Int] = []
  private var bytes = 0
  public init() {}

  public mutating func append(_ page: RecordPage) throws {
    guard page.nextAfter >= cursor else {
      throw ModelError.invalid("Recording cursor moved backwards.")
    }
    var previous = cursor
    for record in page.records {
      guard record.cursor > previous, record.cursor <= page.nextAfter else {
        throw ModelError.invalid("Recording page contains an invalid or unordered cursor.")
      }
      previous = record.cursor
    }
    for record in page.records {
      let size = try record.payload.data().count + 1
      records.append(record)
      sizes.append(size)
      bytes += size
      cursor = record.cursor
      while bytes > 4 * 1024 * 1024 || records.count > 1000 {
        records.removeFirst()
        bytes -= sizes.removeFirst()
        omitted += 1
      }
    }
    cursor = page.nextAfter
  }
  public func observations() throws -> Recording {
    var stream = Data()
    for record in records {
      stream.append(try record.payload.data())
      stream.append(10)
    }
    return try Recording.decode(stream)
  }
}
