import XCTest

@testable import AirHoundCore

final class CellularObservationTests: XCTestCase {
  func testLiveAndSavedCellKeepPartialIdentityAndFullRawEvidence() throws {
    let lines = """
      {"type":"cellular_raw_chunk","hex":"0001","frame_complete":true}
      {"type":"cellular_observation","session_source":{"receiver_id":"orbic"},"observation":{"rat":"lte","kind":"cell_identity","earfcn":70000,"pci":42,"mcc":"001","mnc":"01","tac":123,"eci":456},"capture_time":null,"raw_reference":{"sequence":9,"message_index":1,"sha256":"fixture"}}
      {"type":"cellular_observation","session_source":{"receiver_id":"orbic"},"observation":{"rat":"lte","kind":"serving_measurement","earfcn":70000,"pci":42,"rsrp_dbm":-101.25},"capture_time":null}
      """
    let recording = try Recording.decode(Data(lines.utf8))
    XCTAssertEqual(recording.observations.count, 2)
    XCTAssertEqual(recording.ignoredRecords, 1)
    let identity = recording.observations[0]
    XCTAssertEqual(identity.name, "LTE 001-01")
    XCTAssertEqual(identity.medium, "Cellular")
    XCTAssertEqual(identity.channel, "EARFCN 70000")
    XCTAssertEqual(identity.address, "ECI 456")
    XCTAssertEqual(identity.match, "Not evaluated")
    XCTAssertTrue(identity.raw.contains("raw_reference"))
    let measurement = recording.observations[1]
    XCTAssertEqual(measurement.address, "PCI 42")
    XCTAssertFalse(measurement.name.contains("001"), "No cross-message identity join")
    XCTAssertNil(measurement.rssi, "RSRP must not be mislabeled RSSI")
    XCTAssertTrue(measurement.raw.contains("-101.25"))
  }

  func testCellularSetupAndHealthAreNotWifiOrBluetooth() throws {
    let plan = try JSONDecoder().decode(
      JSONValue.self,
      from: Data(
        """
        {"version":1,"receivers":[{"id":"orbic","source":{"kind":"rayhunter","endpoint":"ws://127.0.0.1:9999/raw-diag","decoder":"/tmp/decoder"}}]}
        """.utf8))
    let receiver = try XCTUnwrap(PlanEditing.receivers(in: plan).first)
    XCTAssertEqual(receiver.title, "Rayhunter / Orbic (experimental)")
    XCTAssertEqual(receiver.endpoint, "ws://127.0.0.1:9999/raw-diag")
  }

  func testMixedLiveAndSavedPagesKeepCellularCursorsAndUnknownIdentity() throws {
    let first = try JSONDecoder().decode(
      RecordPage.self,
      from: Data(
        #"{"records":[{"cursor":10,"source_id":"orbic","kind":"cellular_raw_chunk","received_at_us":1,"payload":{"type":"cellular_raw_chunk","hex":"0001","frame_complete":true}},{"cursor":20,"source_id":"orbic","kind":"cellular_observation","received_at_us":2,"payload":{"type":"cellular_observation","session_source":{"receiver_id":"orbic"},"identity_completeness":"earfcn_pci_only","receiver_location":null,"capture_time":null,"observation":{"rat":"lte","kind":"serving_measurement","earfcn":70000,"pci":42,"rsrp_dbm":-101.25},"raw_reference":{"sequence":9,"message_index":1,"sha256":"fixture"}}}],"next_after":20,"has_more":true}"#
          .utf8))
    let second = try JSONDecoder().decode(
      RecordPage.self,
      from: Data(
        #"{"records":[{"cursor":30,"source_id":"orbic","kind":"cellular_source_gap","received_at_us":3,"payload":{"type":"cellular_source_gap","reason":"subscription_pause_resume","lost_records":null}},{"cursor":40,"source_id":"native","kind":"wifi_observation","received_at_us":4,"payload":{"type":"wifi_observation","ssid":"fixture"}},{"cursor":50,"source_id":"orbic","kind":"cellular_observation","received_at_us":5,"payload":{"type":"cellular_observation","session_source":{"receiver_id":"orbic"},"observation":{"rat":"lte","kind":"neighbor_measurement","earfcn":70000,"pci":43},"capture_time":null}}],"next_after":50,"has_more":false}"#
          .utf8))
    var live = RecordWindow()
    try live.append(first)
    let selectedRaw = try XCTUnwrap(live.observations().observations.first?.raw)
    try live.append(second)
    let rows = try live.observations()
    XCTAssertEqual(live.cursor, 50)
    XCTAssertEqual(live.records.map(\.cursor), [10, 20, 30, 40, 50])
    XCTAssertEqual(rows.observations.map(\.id), [20, 40, 50])
    XCTAssertEqual(rows.ignoredRecords, 2)
    XCTAssertEqual(rows.observations.filter { $0.medium == "Cellular" }.map(\.id), [20, 50])
    XCTAssertEqual(rows.observations[0].raw, selectedRaw)
    XCTAssertEqual(rows.observations[0].name, "LTE PCI 42")
    XCTAssertNil(rows.observations[0].rssi, "RSRP is not RSSI")
    XCTAssertTrue(selectedRaw.contains("raw_reference"))
    XCTAssertTrue(selectedRaw.contains("earfcn_pci_only"))
    var saved = RecordWindow()
    try saved.append(first)
    try saved.append(second)
    XCTAssertEqual(try saved.observations().observations.map(\.raw), rows.observations.map(\.raw))
    XCTAssertThrowsError(try saved.append(first), "Saved browsing also rejects a stale cursor")
    XCTAssertEqual(saved.cursor, 50)
  }

  func testCellularHealthShowsRunningPausedAndFailedWithoutInventingDrops() throws {
    for (state, reason) in [
      ("running", "rayhunter_subscription"),
      ("paused", "rayhunter_subscription_paused"),
      ("failed", "rayhunter_decoder_failed"),
    ] {
      let capture = try JSONDecoder().decode(
        JSONValue.self,
        from: Data(
          """
          {"receiver_health":{"receivers":[{"id":"orbic","scanners":{"wifi":{"state":"disabled","reason":"not_assigned"},"ble":{"state":"disabled","reason":"not_assigned"},"cellular":{"state":"\(state)","reason":"\(reason)"}}}]}}
          """.utf8))
      let health = try XCTUnwrap(ReceiverStatus.decode(capture, sourceEvents: [:]).first)
      XCTAssertEqual(health.id, "orbic")
      XCTAssertEqual(health.phases, "Cellular: \(state)")
      XCTAssertTrue(health.detail.contains(reason.replacingOccurrences(of: "_", with: " ")))
      XCTAssertEqual(health.hostDrops, "Not reported")
      XCTAssertEqual(health.producerDrops, "Not reported")
      XCTAssertEqual(health.recovery, "Not reported")
    }
  }

}
