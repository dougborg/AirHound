import XCTest

@testable import AirHoundCore

final class ObservationMetadataTests: XCTestCase {
  func page(_ records: [[String: Any]]) throws -> RecordPage {
    try JSONDecoder().decode(
      RecordPage.self,
      from: JSONSerialization.data(withJSONObject: [
        "records": records, "next_after": records.last?["cursor"] ?? 0, "has_more": false,
      ]))
  }
  func row(_ cursor: Int, metadata: [String: Any]? = nil) -> [String: Any] {
    var row: [String: Any] = [
      "cursor": cursor, "source_id": "fixture", "kind": "wifi_observation", "received_at_us": 1,
      "payload": [
        "type": "wifi_observation", "ssid": "fixture", "ch": 36, "security": "[WPA2-PSK]",
      ],
    ]
    if let metadata { row["observation_metadata"] = metadata }
    return row
  }
  func testProjectionIsConsistentAcrossPagesAndDoesNotReplaceOriginalRecord() throws {
    let metadata: [String: Any] = [
      "schema_version": 1, "frequency_mhz": 5180, "band": "5GHz",
      "frequency_basis": "derived_from_fixture_context",
      "wifi": ["advertised_phy_modes": ["802.11ac"]],
    ]
    let page = try page([row(42, metadata: metadata)])
    let saved = try XCTUnwrap(page.observations().observations.first)
    var live = RecordWindow()
    try live.append(page)
    let current = try XCTUnwrap(live.observations().observations.first)
    XCTAssertEqual(current.id, 42)
    XCTAssertEqual(saved.frequency, "5180 MHz (derived)")
    XCTAssertEqual(saved.frequency, current.frequency)
    XCTAssertEqual(current.phy, "Not reported", "Advertised capability is not receive PHY")
    XCTAssertEqual(current.advertisedPHY, "802.11ac")
    XCTAssertEqual(current.security, "[WPA2-PSK]")
    XCTAssertFalse(current.raw.contains("5180"), "Original payload stays unchanged")
    XCTAssertTrue(current.metadata?.contains("derived_from_fixture_context") == true)
  }
  func testOldBackendAndUnknownMetadataRemainReadable() throws {
    let old = try XCTUnwrap(page([row(1)]).observations().observations.first)
    XCTAssertEqual(old.frequency, "Not reported")
    let future = try XCTUnwrap(
      page([row(1, metadata: ["new_future_field": "kept"])]).observations().observations.first)
    XCTAssertEqual(future.frequency, "Not reported")
    XCTAssertTrue(future.metadata?.contains("new_future_field") == true)
  }
  func testProjectionBytesCountTowardBoundedPresentationWindow() throws {
    let records = (1...150).map {
      row($0, metadata: ["extra": String(repeating: "x", count: 32_000)])
    }
    var window = RecordWindow()
    try window.append(page(records))
    XCTAssertGreaterThan(window.omitted, 0)
    XCTAssertLessThan(window.records.count, 150)
    XCTAssertEqual(window.cursor, 150)
    XCTAssertEqual(try window.observations().observations.last?.id, 150)
  }
}
