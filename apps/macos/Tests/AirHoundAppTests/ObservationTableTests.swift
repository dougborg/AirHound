import AirHoundCore
import AppKit
import XCTest

@testable import AirHoundApp

@MainActor
final class ObservationTableTests: XCTestCase {
  private func observations() throws -> [Observation] {
    try Recording.decode(
      Data(
        """
        {"type":"ble_observation","name":"Zulu","rssi":-80,"channel":37,"session_source":{"receiver_id":"cat"}}
        {"type":"wifi_observation","name":"Alpha","rssi":-30,"ch":6,"session_source":{"receiver_id":"wifi"}}
        {"type":"ble_observation","name":"Beta","channel":38,"session_source":{"receiver_id":"cat"}}
        """.utf8)
    ).observations
  }

  func testStableSelectionSurvivesResortAndEviction() throws {
    let coordinator = ObservationTableCoordinator()
    let scroll = coordinator.makeScrollView()
    let rows = try observations()
    var callbacks: [Int?] = []
    coordinator.selectionChanged = { callbacks.append($0) }
    coordinator.update(rows: rows, revision: UUID(), selection: rows[0].id, sortOrder: [])
    XCTAssertEqual(coordinator.table.selectedRow, 0)
    let sorted = rows.sorted(using: ObservationSort().comparator)
    coordinator.update(rows: sorted, revision: UUID(), selection: rows[0].id, sortOrder: [])
    XCTAssertEqual(coordinator.table.selectedRow, 2)
    XCTAssertTrue(callbacks.isEmpty, "Applying a refreshed snapshot must not emit user selection")
    coordinator.table.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)
    XCTAssertEqual(callbacks.last!, rows[2].id)
    coordinator.update(
      rows: Array(sorted.prefix(2)), revision: UUID(), selection: rows[0].id, sortOrder: [])
    XCTAssertEqual(coordinator.table.selectedRow, -1)
    XCTAssertEqual(scroll.documentView, coordinator.table)
  }

  func testNativeCellsAndHeaderSortPreservePresentation() throws {
    let coordinator = ObservationTableCoordinator()
    let scroll = coordinator.makeScrollView()
    let rows = try observations()
    var requested: [ObservationSort] = []
    coordinator.sortChanged = { requested = $0 }
    coordinator.update(rows: rows, revision: UUID(), selection: nil, sortOrder: [ObservationSort()])
    XCTAssertTrue(requested.isEmpty)
    coordinator.table.sortDescriptors = [NSSortDescriptor(key: "signal", ascending: false)]
    XCTAssertEqual(requested, [ObservationSort(column: .signal, ascending: false)])
    XCTAssertEqual(
      rows.sorted(using: requested.map(\.comparator)).map(\.name), ["Alpha", "Zulu", "Beta"])
    for (index, column) in coordinator.table.tableColumns.enumerated() {
      let cell = try XCTUnwrap(
        coordinator.tableView(coordinator.table, viewFor: column, row: 0) as? NSTableCellView)
      XCTAssertEqual(cell.textField?.stringValue, ObservationColumn.allCases[index].text(rows[0]))
      XCTAssertEqual(cell.toolTip, cell.textField?.stringValue)
    }
    XCTAssertNil(
      coordinator.tableView(
        coordinator.table, viewFor: coordinator.table.tableColumns.first, row: 3))
    XCTAssertFalse(coordinator.table.usesAutomaticRowHeights)
    XCTAssertEqual(coordinator.numberOfRows(in: coordinator.table), 3)
    XCTAssertNotNil(scroll.documentView)
  }
}
