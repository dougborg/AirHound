import XCTest

@testable import AirHoundCore

final class FlipperReceiverStatusTests: XCTestCase {
  private func capture(_ status: JSONValue?, run: String = "run-a") -> JSONValue {
    var receiver: [String: JSONValue] = ["backend": .string("flipper_marauder")]
    if let status { receiver["bridge_status"] = status }
    return .object([
      "run_id": .string(run),
      "receiver_health": .object([
        "receivers": .array([
          .object([
            "id": .string("flipper"), "receiver": .object(receiver),
            "scanners": .object([
              "wifi": .object([
                "state": .string("unavailable"), "reason": .string("receiver_task_failed"),
              ])
            ]),
          ])
        ])
      ]),
    ])
  }

  private func status(
    host: Int64, generation: Int64, uart: Int64 = 0,
    errors: Int64 = 0, rpc: Int64 = 0
  ) -> JSONValue {
    .object([
      "host_dropped_observations": .integer(host),
      "producer_last_report": .object([
        "capture_generation": .integer(generation), "state": .string("Failed"),
        "uart_dropped_bytes": .integer(uart), "uart_error_events": .integer(errors),
        "rpc_dropped_messages": .integer(rpc),
      ]),
      "radio_loss": .string("unknown"),
    ])
  }

  func testFailedReceiverShowsMeasuredHostDropsWithoutInventingProducerLoss() throws {
    // Sanitized shape of the active-host-loss result: eight host drops, no
    // reported UART/RPC loss. RF loss remains unknown despite those zeros.
    let row = try XCTUnwrap(
      ReceiverStatus.decode(
        capture(status(host: 8, generation: 1)), sourceEvents: [:]
      ).first)
    XCTAssertEqual(row.hostDrops, "8")
    XCTAssertEqual(row.phases, "Wi-Fi: unavailable")
    XCTAssertEqual(
      row.metricsDescription,
      "Host dropped observations: 8 · UART dropped bytes: 0 · UART errors: 0 · RPC dropped messages: 0 · RF loss: Unknown"
    )
    XCTAssertEqual(
      row.producerDrops, "Not reported",
      "Different producer units cannot be summed into packet drops")
  }

  func testAbsentAndPartialBridgeReportsRemainUnknownDespiteCachedEvents() throws {
    let stale = ["flipper": status(host: 99, generation: 1, uart: 12, errors: 3, rpc: 4)]
    for value in [nil, JSONValue.null, .object([:])] as [JSONValue?] {
      let row = try XCTUnwrap(ReceiverStatus.decode(capture(value), sourceEvents: stale).first)
      XCTAssertEqual(row.hostDrops, "Not reported")
      XCTAssertEqual(
        row.metricsDescription,
        "Host dropped observations: Not reported · UART dropped bytes: Not reported · UART errors: Not reported · RPC dropped messages: Not reported · RF loss: Unknown"
      )
    }
    let partial = try XCTUnwrap(
      ReceiverStatus.decode(
        capture(.object(["host_dropped_observations": .integer(0)])), sourceEvents: stale
      ).first)
    XCTAssertEqual(partial.hostDrops, "0")
    XCTAssertTrue(partial.metricsDescription.contains("UART dropped bytes: Not reported"))
  }

  func testNewGenerationUsesCurrentCountersWithoutResettingBackendTotals() throws {
    let old = status(host: 8, generation: 1, uart: 2, errors: 3, rpc: 4)
    let next = status(host: 8, generation: 2, uart: 5, errors: 6, rpc: 7)
    let row = try XCTUnwrap(
      ReceiverStatus.decode(capture(next), sourceEvents: ["flipper": old]).first)
    XCTAssertEqual(row.hostDrops, "8", "The backend owns cumulative counts across Resume")
    XCTAssertTrue(
      row.metricsDescription.contains(
        "UART dropped bytes: 5 · UART errors: 6 · RPC dropped messages: 7"))
    let freshRun = try XCTUnwrap(
      ReceiverStatus.decode(
        capture(status(host: 0, generation: 1), run: "run-b"), sourceEvents: ["flipper": old]
      ).first)
    XCTAssertEqual(
      freshRun.hostDrops, "0", "A reused receiver id must not import a prior run's counters")
    XCTAssertTrue(freshRun.metricsDescription.contains("UART dropped bytes: 0"))
  }

  func testOtherReceiverKeepsExistingReportedUnitsAndDoesNotUseBridgeShape() throws {
    let value: JSONValue = .object([
      "receiver_health": .object([
        "receivers": .array([
          .object([
            "id": .string("alfa"),
            "receiver": .object([
              "backend": .string("external_wifi"), "bridge_status": status(host: 99, generation: 1),
            ]),
          ])
        ])
      ])
    ])
    let row = try XCTUnwrap(
      ReceiverStatus.decode(
        value,
        sourceEvents: [
          "alfa": .object([
            "host_dropped_packets": .integer(2), "producer_dropped_observations": .integer(3),
            "reconnect_attempts": .integer(1),
          ])
        ]
      ).first)
    XCTAssertEqual(
      row.metricsDescription, "Recovery attempts: 1 · Host drops: 2 · Producer drops: 3")
  }
}
