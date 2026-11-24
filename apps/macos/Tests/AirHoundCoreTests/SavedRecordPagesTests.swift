import XCTest

@testable import AirHoundCore

final class SavedRecordPagesTests: XCTestCase {
  private func page(after: Int64, count: Int, more: Bool, padding: Int = 0) throws -> RecordPage {
    let payload: [String: Any] = [
      "type": "ble_observation", "name": "row",
      "padding": String(repeating: "x", count: padding),
    ]
    let records: [[String: Any]] = (0..<count).map { index -> [String: Any] in
      let cursor: Int64 = after + Int64(index + 1) * 3
      let receivedAt: Int64 = 1_700_000_000_000_000 + Int64(index)
      return [
        "cursor": cursor, "source_id": "fixture", "kind": "ble_observation",
        "received_at_us": receivedAt, "payload": payload,
      ]
    }
    let nextAfter: Int64 = after + Int64(count) * 3
    let envelope: [String: Any] = [
      "records": records, "next_after": nextAfter, "has_more": more,
    ]
    let data = try JSONSerialization.data(withJSONObject: envelope)
    return try JSONDecoder().decode(RecordPage.self, from: data)
  }

  func testByteLimitedFirstPageUsesRecordOrdinalsNotGlobalCursorsAndCanReturn() throws {
    // A backend byte-limited response is shorter than the requested 500 rows.
    let first = try page(after: 90_000, count: 257, more: true, padding: 1500)
    XCTAssertLessThan(try JSONEncoder().encode(first.records.map(\.payload)).count, 512 * 1024)
    var pages = SavedRecordPages()
    try pages.receive(first)
    XCTAssertEqual(pages.firstRecord, 1)
    XCTAssertEqual(pages.lastRecord, 257)
    XCTAssertTrue(pages.hasMore)
    XCTAssertTrue(pages.summary(total: 900_001).contains(899_744.formatted() + " remaining"))
    var next = try XCTUnwrap(pages.next())
    XCTAssertEqual(next.position.after, first.nextAfter)
    try next.receive(page(after: first.nextAfter, count: 500, more: true))
    XCTAssertEqual(next.firstRecord, 258)
    XCTAssertEqual(next.lastRecord, 757)
    var previous = try XCTUnwrap(next.back())
    XCTAssertEqual(previous.position.after, 0)
    try previous.receive(first)
    XCTAssertEqual(previous.count, 257)
    XCTAssertEqual(previous.position, pages.position)
    XCTAssertFalse(previous.canGoBack)
    XCTAssertNil(SavedRecordPages().back())
  }

  func testHistoryIsBoundedWithoutRetainingPayloadAndTerminalAndInvalidPages() throws {
    var pages = SavedRecordPages()
    for _ in 0..<(SavedRecordPages.historyLimit + 10) {
      try pages.receive(page(after: pages.position.after, count: 1, more: true))
      pages = try XCTUnwrap(pages.next())
    }
    XCTAssertEqual(pages.previous.count, SavedRecordPages.historyLimit)
    XCTAssertEqual(pages.count, 0)
    try pages.receive(page(after: pages.position.after, count: 2, more: false))
    XCTAssertNil(pages.next())
    let count = pages.count
    XCTAssertThrowsError(try pages.receive(page(after: 0, count: 1, more: false)))
    XCTAssertEqual(pages.count, count)
    var empty = SavedRecordPages()
    XCTAssertThrowsError(try empty.receive(page(after: 0, count: 0, more: true)))
    try empty.receive(page(after: 0, count: 0, more: false))
    XCTAssertEqual(empty.summary(total: 0), "No records on this page · 0 records in recording")
  }
}
