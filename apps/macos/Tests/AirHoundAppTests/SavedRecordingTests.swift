import XCTest

@testable import AirHoundApp
@testable import AirHoundCore

@MainActor
final class SavedRecordingTests: XCTestCase {
  private func run(_ id: String, total: Int = 900_001) throws -> StoredRun {
    try JSONDecoder().decode(
      StoredRun.self,
      from: JSONSerialization.data(withJSONObject: [
        "id": id, "state": "finished", "mode": "capture", "started_at_us": 1_700_000_000_000_000,
        "record_count": total, "config": [:],
      ]))
  }
  private func page(
    _ first: Int, count: Int, more: Bool, name: String = "fixture", statusFirst: Bool = false
  ) throws
    -> RecordPage
  {
    let rows: [[String: Any]] = (first..<(first + count)).map { cursor in
      [
        "cursor": cursor, "source_id": "synthetic", "kind": "ble_observation",
        "received_at_us": cursor,
        "payload": [
          "type": statusFirst && cursor == first ? "scanner_health" : "ble_observation",
          "name": name,
        ],
      ]
    }
    return try JSONDecoder().decode(
      RecordPage.self,
      from: JSONSerialization.data(withJSONObject: [
        "records": rows, "next_after": first + count - 1, "has_more": more,
      ]))
  }

  func testSavedPagesReplaceRowsNavigateBackAndRejectStaleSelectionResponse() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(
      "ah-saved-pages-" + UUID().uuidString)
    try LocalFiles.privateDirectory(root)
    defer { try? FileManager.default.removeItem(at: root) }
    let script = root.appendingPathComponent("fixture")
    try Data(
      #"""
      #!/bin/sh
      case "$4" in
        status) printf '%s\n' '{"capture_control":false,"methods":[]}' ;;
        devices) printf '%s\n' '{"schema_version":1,"mode":"passive","platform":"macos","devices":[],"diagnostics":[]}' ;;
        recordings) printf '%s\n' '{"runs":[],"has_more":false,"next_before":null}' ;;
        *) exit 9 ;;
      esac
      """#.utf8
    ).write(to: script)
    try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: script.path)
    let suite = "saved-pages-" + UUID().uuidString
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    var requests: [(String, Int64)] = []
    var failNextPage = false
    var delayed: CheckedContinuation<RecordPage, Error>?
    let waiting = expectation(description: "Exact delayed page request registered")
    let first = try page(1001, count: 257, more: true, statusFirst: true)
    let second = try page(1258, count: 500, more: true)
    let model = SessionModel(
      preferences: defaults,
      clientFactory: {
        BackendClient(
          executable: $0, socket: $1,
          process: DiscoveryProcess(scheduling: ProcessScheduling(now: { 0 })))
      },
      recordReader: { _, id, after, limit in
        XCTAssertEqual(limit, 500)
        requests.append((id, after))
        if id == "delayed" {
          return try await withCheckedThrowingContinuation { continuation in
            delayed = continuation
            waiting.fulfill()
          }
        }
        if failNextPage && after != 0 {
          failNextPage = false
          throw ModelError.invalid("Synthetic next-page failure")
        }
        if id == "empty" { return try self.page(1, count: 0, more: false) }
        if id == "failure" { throw ModelError.invalid("Synthetic page failure") }
        return after == 0 ? first : second
      })
    model.socketPath = root.appendingPathComponent("unused.sock").path
    model.connect(executable: script)
    defer {
      model.disconnect()
      delayed?.resume(throwing: CancellationError())
    }
    try await awaitModelState([model.objectWillChange], description: "Fixture connected") {
      model.connected && !model.busy
    }
    let app = AppModel()
    func complete() async throws {
      try await awaitModelState([model.objectWillChange], description: "Page request completed") {
        !model.busy
      }
      XCTAssertNil(model.error)
    }
    model.open(try run("large"), app: app)
    try await complete()
    XCTAssertEqual(model.browsingRun, "large")
    XCTAssertTrue(model.browsingTitle.contains("large"))
    XCTAssertEqual(model.browsingTotal, 900_001)
    XCTAssertEqual(app.recording?.observations.count, 256)
    XCTAssertEqual(
      model.savedPages?.count, 257,
      "Status records count toward the recording range, not observations")
    XCTAssertTrue(model.hasMoreRecords)
    XCTAssertFalse(model.followLive)
    XCTAssertFalse(app.recordingLabel.contains("recent"))
    var view = ObservationPresentation()
    view.receive(app.recording, label: app.recordingLabel, context: app.recordingRevision)
    view.select(1002, liveWindow: false)
    let firstContext = app.recordingRevision
    failNextPage = true
    model.loadMoreRecords()
    try await awaitModelState([model.objectWillChange], description: "Next page failed") {
      !model.busy
    }
    XCTAssertNotNil(model.recordReadError)
    XCTAssertEqual(model.savedPages?.firstRecord, 1)
    XCTAssertEqual(app.recording?.observations.first?.id, 1002)
    model.retryRecords()
    try await complete()
    XCTAssertEqual(
      app.recording?.observations.count, 500, "Saved page replaces, rather than appends")
    XCTAssertEqual(app.recording?.observations.first?.id, 1258)
    XCTAssertEqual(model.savedPages?.firstRecord, 258)
    XCTAssertNotEqual(firstContext, app.recordingRevision)
    view.receive(app.recording, label: app.recordingLabel, context: app.recordingRevision)
    XCTAssertNil(view.selection, "Explicit page navigation resets the old row selection")
    model.previousRecords()
    try await complete()
    XCTAssertEqual(app.recording?.observations.first?.id, 1002)
    XCTAssertEqual(model.savedPages?.firstRecord, 1)
    XCTAssertEqual(requests.map(\.1), [0, 1257, 1257, 0])
    model.loadMoreRecords()
    try await complete()
    model.rewindRecords()
    try await complete()
    XCTAssertEqual(model.savedPages?.firstRecord, 1)
    XCTAssertFalse(model.savedPages?.canGoBack ?? true)

    model.open(try run("delayed"), app: app)
    // Outer hang guard; continuation supplies readiness.
    await fulfillment(of: [waiting], timeout: 5)
    XCTAssertNil(app.recording, "Old recording must not be shown under the new run identity")
    XCTAssertTrue(model.readingRecords)
    model.leaveRecording()
    app.beginRecordingSelection()
    app.recording = try page(1, count: 1, more: false, name: "replacement").observations()
    let replacement = app.recording?.revision
    delayed?.resume(returning: first)
    delayed = nil
    try await complete()
    XCTAssertEqual(
      app.recording?.revision, replacement, "Late old-run page cannot replace new selection")
    XCTAssertNil(model.browsingRun)
    XCTAssertFalse(model.readingRecords)

    model.open(try run("failure"), app: app)
    try await awaitModelState([model.objectWillChange], description: "Failed page completed") {
      !model.busy
    }
    XCTAssertNil(app.recording)
    XCTAssertEqual(model.browsingRun, "failure")
    XCTAssertNotNil(model.recordReadError)
    XCTAssertEqual(app.recordingLabel, "Recording could not be loaded")
    model.open(try run("empty", total: 0), app: app)
    try await complete()
    XCTAssertEqual(app.recording?.observations.count, 0)
    XCTAssertFalse(model.hasMoreRecords)
    XCTAssertNil(model.recordReadError)
  }
}
