import Foundation

/// A bounded presentation window; the complete recording remains in the Rust store.
public struct RecordWindow: Sendable {
  public private(set) var records: [StoredRecord] = []
  public private(set) var cursor: Int64 = 0
  public private(set) var omitted = 0
  private var sizes: [Int] = []
  private var bytes = 0
  private var decodedRows: [Observation] = []
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
    // Decode only new records, before changing the window. A rejected page must
    // leave its cursor and cached observations intact so the caller can retry.
    var stream = Data()
    var newSizes: [Int] = []
    for record in page.records {
      let data = try record.payload.data()
      newSizes.append(data.count + 1 + (try record.observationMetadata?.data().count ?? 0))
      stream.append(data)
      stream.append(10)
    }
    let decoded = try Recording.decode(
      stream,
      metadata: Dictionary(
        uniqueKeysWithValues: page.records.enumerated().compactMap { index, record in
          record.observationMetadata.map { (index, $0) }
        }))
    let newRows = decoded.observations.map { observation in
      var row = observation
      row.id = Int(page.records[observation.id].cursor)
      return row
    }
    records.append(contentsOf: page.records)
    sizes.append(contentsOf: newSizes)
    bytes += newSizes.reduce(0, +)
    decodedRows.append(contentsOf: newRows)
    var removed = 0
    while bytes > 4 * 1024 * 1024 || records.count - removed > 1000 {
      bytes -= sizes[removed]
      removed += 1
    }
    if removed > 0 {
      records.removeFirst(removed)
      sizes.removeFirst(removed)
      omitted += removed
      if let first = records.first {
        decodedRows.removeAll { $0.id < Int(first.cursor) }
      } else {
        decodedRows.removeAll()
      }
    }
    cursor = page.nextAfter
  }

  /// Cached immutable rows: reading a window does not parse or pretty-print JSON.
  public func observations() throws -> Recording {
    Recording(observations: decodedRows, ignoredRecords: records.count - decodedRows.count)
  }
}
