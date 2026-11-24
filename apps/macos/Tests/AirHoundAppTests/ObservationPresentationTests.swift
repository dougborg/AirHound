import AirHoundCore
import AppKit
import XCTest

@testable import AirHoundApp

@MainActor
final class ObservationPresentationTests: XCTestCase {
  private func page(_ range: ClosedRange<Int>, padding: Int = 0) throws -> RecordPage {
    let records: [[String: Any]] = range.map { cursor in
      [
        "cursor": cursor, "source_id": "fixture", "kind": "ble_observation", "received_at_us": 1,
        "payload": [
          "type": "ble_observation", "name": String(format: "%06d", cursor),
          "padding": String(repeating: "x", count: padding),
        ],
      ]
    }
    return try JSONDecoder().decode(
      RecordPage.self,
      from: JSONSerialization.data(withJSONObject: [
        "records": records, "next_after": range.upperBound, "has_more": true,
      ]))
  }

  func testSelectionKeepsEntireBoundedViewAcrossRollingPagesUntilExplicitFollow() throws {
    var window = RecordWindow()
    try window.append(page(1...500))
    let context = UUID()
    var view = ObservationPresentation()
    view.receive(try window.observations(), label: "500 recent", context: context)
    let original = try XCTUnwrap(view.recording)
    view.select(20, liveWindow: true)
    for first in stride(from: 501, through: 2501, by: 500) {
      try window.append(page(first...(first + 499)))
      XCTAssertLessThanOrEqual(window.records.count, 1000)
      XCTAssertFalse(view.receive(try window.observations(), label: "new window", context: context))
      XCTAssertEqual(view.recording?.revision, original.revision)
      XCTAssertEqual(view.recording?.observations.map(\.id), Array(1...500))
      XCTAssertEqual(view.selection, 20)
      XCTAssertEqual(view.label, "500 recent")
    }
    // This is the original failure trigger: the live window evicts cursor 20.
    let latest = try window.observations()
    XCTAssertFalse(latest.observations.contains { $0.id == 20 })
    view.resume(latest, label: "1000 recent", context: context)
    XCTAssertFalse(view.held)
    XCTAssertNil(view.selection)
    XCTAssertEqual(view.recording?.revision, latest.revision)
    XCTAssertEqual(view.recording?.observations.count, 1000)
    try window.append(page(3001...3500))
    XCTAssertTrue(view.receive(try window.observations(), label: "1000 recent", context: context))
    XCTAssertEqual(view.recording?.observations.last?.id, 3500)
  }

  func testByteLimitEvictionKeepsOnlyOneHeldSnapshotAndRecordingChangeResetsIdentity() throws {
    var window = RecordWindow()
    try window.append(page(1...20, padding: 210_000))
    XCTAssertLessThan(window.records.count, 20, "Exercise 4 MiB rather than the row cap")
    var view = ObservationPresentation()
    let context = UUID()
    view.receive(try window.observations(), label: "original", context: context)
    let oldIDs = try XCTUnwrap(view.recording).observations.map(\.id)
    view.select(oldIDs[0], liveWindow: true)
    try window.append(page(21...40, padding: 210_000))
    XCTAssertFalse(view.receive(try window.observations(), label: "latest", context: context))
    XCTAssertEqual(view.recording?.observations.map(\.id), oldIDs)
    XCTAssertFalse(try window.observations().observations.contains { $0.id == oldIDs[0] })
    var other = RecordWindow()
    try other.append(page(1...20))  // IDs overlap, but belong to another recording.
    XCTAssertTrue(view.receive(try other.observations(), label: "other", context: UUID()))
    XCTAssertFalse(view.held)
    XCTAssertNil(view.selection)
    XCTAssertEqual(view.label, "other")
    XCTAssertEqual(view.recording?.observations.count, 20)
  }

  func testRealTableUserSelectionAndScrollHoldButProgrammaticUpdatesDoNot() throws {
    var window = RecordWindow()
    try window.append(page(1...500))
    var view = ObservationPresentation()
    let context = UUID()
    view.receive(try window.observations(), label: "fixture", context: context)
    let coordinator = ObservationTableCoordinator()
    let scroll = coordinator.makeScrollView()
    scroll.frame = NSRect(x: 0, y: 0, width: 800, height: 300)
    var callbackCount = 0
    coordinator.selectionChanged = { id in
      callbackCount += 1
      view.select(id, liveWindow: true)
    }
    coordinator.userScrolled = { view.hold() }
    func update() throws {
      let recording = try XCTUnwrap(view.recording)
      coordinator.update(
        rows: recording.observations, revision: recording.revision,
        selection: view.selection, sortOrder: [], context: context)
      scroll.layoutSubtreeIfNeeded()
    }
    try update()
    XCTAssertFalse(view.held)
    XCTAssertEqual(callbackCount, 0)
    scroll.contentView.scroll(to: NSPoint(x: 0, y: 500))
    scroll.reflectScrolledClipView(scroll.contentView)
    try window.append(page(501...1000))
    view.receive(try window.observations(), label: "larger", context: context)
    try update()
    XCTAssertFalse(view.held, "Programmatic anchoring/reload is not user scrolling")
    XCTAssertEqual(callbackCount, 0)
    coordinator.table.selectRowIndexes(IndexSet(integer: 30), byExtendingSelection: false)
    XCTAssertEqual(view.selection, 31)
    XCTAssertTrue(view.held)
    let origin = scroll.contentView.bounds.origin
    let revision = view.recording?.revision
    try window.append(page(1001...1500))
    XCTAssertFalse(view.receive(try window.observations(), label: "evicted", context: context))
    try update()
    XCTAssertEqual(view.recording?.revision, revision)
    XCTAssertEqual(coordinator.table.selectedRow, 30)
    XCTAssertEqual(scroll.contentView.bounds.origin, origin)
    XCTAssertEqual(callbackCount, 1)
    view.resume(try window.observations(), label: "latest", context: context)
    try update()
    XCTAssertFalse(view.held)
    XCTAssertNil(view.selection)
    XCTAssertEqual(callbackCount, 1, "Programmatic deselection must not hold the view again")
    NotificationCenter.default.post(
      name: NSScrollView.willStartLiveScrollNotification, object: scroll)
    XCTAssertTrue(view.held, "Dragging the scroller is a user action")
  }
  func testViewportAnchorSurvivesInsertionSortAndExplicitFilterThenResetsForNewRecording() throws {
    var window = RecordWindow()
    try window.append(page(1...500))
    let initial = try window.observations().observations
    let coordinator = ObservationTableCoordinator()
    let scroll = coordinator.makeScrollView()
    scroll.frame = NSRect(x: 0, y: 0, width: 800, height: 300)
    let context = UUID()
    var callbacks: [Int?] = []
    coordinator.selectionChanged = { callbacks.append($0) }
    coordinator.update(
      rows: initial, revision: UUID(), selection: 120, sortOrder: [], context: context)
    scroll.layoutSubtreeIfNeeded()
    let offset: CGFloat = 5
    scroll.contentView.scroll(
      to: NSPoint(x: 0, y: coordinator.table.rect(ofRow: 100).minY + offset))
    scroll.reflectScrolledClipView(scroll.contentView)
    let anchor = initial[100].id
    try window.append(page(501...1000))
    let reversed = Array(try window.observations().observations.reversed())
    coordinator.update(
      rows: reversed, revision: UUID(), selection: 120, sortOrder: [], context: context)
    let anchorIndex = try XCTUnwrap(reversed.firstIndex { $0.id == anchor })
    XCTAssertEqual(
      scroll.contentView.bounds.minY,
      coordinator.table.rect(ofRow: anchorIndex).minY + offset, accuracy: 1)
    XCTAssertEqual(coordinator.rows[coordinator.table.selectedRow].id, 120)
    XCTAssertTrue(callbacks.isEmpty)
    // A deliberate filter clears excluded selection; it must not select the
    // unrelated row that now occupies its old index.
    let filtered = reversed.filter { $0.id != 120 }
    coordinator.update(
      rows: filtered, revision: UUID(), selection: nil, sortOrder: [], context: context)
    XCTAssertEqual(coordinator.table.selectedRow, -1)
    XCTAssertTrue(callbacks.isEmpty)
    coordinator.update(
      rows: initial, revision: UUID(), selection: nil, sortOrder: [], context: UUID())
    XCTAssertEqual(scroll.contentView.bounds.minY, 0, accuracy: 1)
    XCTAssertEqual(coordinator.table.selectedRow, -1)
  }

}
