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
}
