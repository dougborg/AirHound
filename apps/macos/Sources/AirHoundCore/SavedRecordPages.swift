import Foundation

/// Cursor-only navigation for a saved recording. Payloads are never cached here.
/// A single page replaces the displayed page; the live RecordWindow is separate.
public struct SavedRecordPages: Sendable {
  public struct Position: Equatable, Sendable {
    public let after: Int64
    public let offset: Int64
  }
  public static let historyLimit = 256
  public private(set) var position = Position(after: 0, offset: 0)
  public private(set) var previous: [Position] = []
  public private(set) var count = 0
  public private(set) var nextAfter: Int64 = 0
  public private(set) var hasMore = false
  public private(set) var receivedRange: ClosedRange<Int64>?
  public init() {}

  public var canGoBack: Bool { !previous.isEmpty }
  public var firstRecord: Int64 { count == 0 ? 0 : position.offset + 1 }
  public var lastRecord: Int64 { position.offset + Int64(count) }

  public func next() -> Self? {
    guard hasMore, count > 0 else { return nil }
    var result = self
    result.previous.append(position)
    if result.previous.count > Self.historyLimit { result.previous.removeFirst() }
    result.position = Position(after: nextAfter, offset: lastRecord)
    result.clearPage()
    return result
  }

  public func back() -> Self? {
    var result = self
    guard let prior = result.previous.popLast() else { return nil }
    result.position = prior
    result.clearPage()
    return result
  }

  /// Commit only a validated response. On request/decode failure, the caller
  /// keeps the previous navigation state and displayed page intact.
  public mutating func receive(_ page: RecordPage) throws {
    guard page.nextAfter >= position.after,
      page.records.allSatisfy({ $0.cursor > position.after }),
      !page.hasMore || !page.records.isEmpty
    else { throw ModelError.invalid("Saved recording page did not advance its cursor.") }
    var cursor = position.after
    for record in page.records {
      guard record.cursor > cursor, record.cursor <= page.nextAfter else {
        throw ModelError.invalid("Saved recording page contains unordered cursors.")
      }
      cursor = record.cursor
    }
    count = page.records.count
    nextAfter = page.nextAfter
    hasMore = page.hasMore
    let times = page.records.map(\.receivedAtUs)
    receivedRange = times.min().flatMap { first in times.max().map { first...$0 } }
  }

  public func summary(total: Int64) -> String {
    let remaining = max(0, total - lastRecord)
    if count == 0 { return "No records on this page · \(total.formatted()) records in recording" }
    return
      "Records \(firstRecord.formatted())–\(lastRecord.formatted()) of \(total.formatted()) · \(remaining.formatted()) remaining"
  }

  private mutating func clearPage() {
    count = 0
    nextAfter = position.after
    hasMore = false
    receivedRange = nil
  }
}
