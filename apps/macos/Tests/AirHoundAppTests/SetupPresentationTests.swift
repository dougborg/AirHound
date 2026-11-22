import AirHoundCore
import AppKit
import SwiftUI
import XCTest

@testable import AirHoundApp

@MainActor
final class SetupPresentationTests: XCTestCase {
  func testPlanEditorDoesNotReplaceSessionOptions() throws {
    let suite = "AirHound-setup-tests-" + UUID().uuidString
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let session = SessionModel(preferences: defaults)
    session.configText =
      #"{"version":1,"alert_policy":"silent","location":{"kind":"gpsd","endpoint":"127.0.0.1:2947"},"receivers":[{"id":"my-sensor","source":{"kind":"airhound_serial","port":"/dev/cu.fixture","baud":460800}}]}"#
    session.planSourceField(id: "my-sensor", field: "baud", value: .integer(115200))
    session.addReceiver(source: .object(["kind": .string("native_ble")]))
    let config = try JSONDecoder().decode(JSONValue.self, from: Data(session.configText.utf8))
    XCTAssertEqual(config["alert_policy"], .string("silent"))
    XCTAssertEqual(config["location"]?["kind"], .string("gpsd"))
    XCTAssertEqual(session.plannedReceivers[0].id, "my-sensor")
    session.busy = true
    let unchanged = session.configText
    session.removeReceiver(id: "my-sensor")
    XCTAssertEqual(session.configText, unchanged)
  }

  func testSetupIntentWaitsForPollingAndRunsOnce() async throws {
    let session = SessionModel()
    session.connected = true
    session.beginBackgroundRefresh()
    var executed: [String] = []
    XCTAssertTrue(session.submitSetupMutation { executed.append("save submitted plan") })
    XCTAssertTrue(session.userActionQueued)
    XCTAssertFalse(session.canEditPlan)
    XCTAssertFalse(session.submitSetupMutation { executed.append("duplicate") })
    XCTAssertTrue(executed.isEmpty)
    session.finishBackgroundRefresh()
    for _ in 0..<20 where session.busy { await Task.yield() }
    XCTAssertEqual(executed, ["save submitted plan"])
    XCTAssertFalse(session.userActionQueued)
    XCTAssertFalse(session.busy)
    session.beginBackgroundRefresh()
    XCTAssertTrue(session.submitSetupMutation { executed.append("delete submitted ID") })
    session.finishBackgroundRefresh()
    for _ in 0..<20 where session.busy { await Task.yield() }
    XCTAssertEqual(executed, ["save submitted plan", "delete submitted ID"])
    session.busy = true
    XCTAssertFalse(session.submitSetupMutation { executed.append("rejected") })
    XCTAssertNotNil(session.error)
  }

  func testReceiverIdentityRequiresMatchingActiveRunSource() throws {
    let session = SessionModel()
    session.connected = true
    session.configText =
      #"{"version":1,"receivers":[{"id":"same-id","source":{"kind":"sniffle","port":"/dev/cu.one","python":"/python","helper":"/helper"}}]}"#
    session.capture = try JSONDecoder().decode(
      JSONValue.self,
      from: Data(
        #"{"active":true,"run_id":"run-1","receiver_health":{"receivers":[{"id":"same-id","receiver":{"adapter_info":"actual firmware"}}]}}"#
          .utf8))
    func stored(_ source: String) throws -> StoredRun {
      try JSONDecoder().decode(
        StoredRun.self,
        from: Data(
          (#"{"id":"run-1","state":"recording","mode":"live","started_at_us":0,"record_count":0,"config":{"session":{"receivers":[{"id":"same-id","source":SOURCE}]}}}"#
            .replacingOccurrences(of: "SOURCE", with: source)).utf8))
    }
    XCTAssertNil(session.currentReceiverIdentity(id: "same-id"))
    session.runs = [try stored(#"{"kind":"native_ble"}"#)]
    XCTAssertNil(session.currentReceiverIdentity(id: "same-id"))
    session.runs = [
      try stored(
        #"{"kind":"sniffle","port":"/dev/cu.other","python":"/python","helper":"/helper","channel":38,"coded":false,"module_path":null}"#
      )
    ]
    XCTAssertNil(session.currentReceiverIdentity(id: "same-id"))
    session.runs = [
      try stored(
        #"{"kind":"sniffle","port":"/dev/cu.one","python":"/python","helper":"/helper","channel":38,"coded":false,"module_path":null}"#
      )
    ]
    XCTAssertEqual(
      session.currentReceiverIdentity(id: "same-id")?["adapter_info"], .string("actual firmware"))
    session.connected = false
    XCTAssertNil(session.currentReceiverIdentity(id: "same-id"))
    session.connected = true
    session.capture = .object(["active": .bool(false), "run_id": .string("run-1")])
    XCTAssertNil(session.currentReceiverIdentity(id: "same-id"))
  }

  /// Optional bounded screenshot fixture. Never connects to a service or opens radios.
  func testNativeSetupPresentationSnapshot() throws {
    guard let destination = ProcessInfo.processInfo.environment["AIRHOUND_UI_SNAPSHOT_DIR"] else {
      throw XCTSkip("Set AIRHOUND_UI_SNAPSHOT_DIR for the synthetic native presentation check.")
    }
    let app = AppModel()
    let session = app.session
    session.connected = true
    session.captureEnabled = true
    session.setupsAvailable = true
    session.locationProviders = ["gpsd"]
    session.backendBuild = "git fixture (synthetic preview)"
    session.configText =
      #"{"version":1,"alert_policy":"silent","location":{"kind":"gpsd","endpoint":"127.0.0.1:2947","device":"/dev/cu.example-gps"},"receivers":[{"id":"system-ble","source":{"kind":"native_ble"}},{"id":"m5","source":{"kind":"airhound_serial","port":"/dev/cu.example-m5","baud":460800}},{"id":"cat","source":{"kind":"sniffle","port":"/dev/cu.example-cat","python":"/example/python","helper":"/example/sniffle-source.py","channel":37}}]}"#
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    session.setups = try decoder.decode(
      SavedSetupList.self,
      from: Data(
        #"{"schema_version":1,"profiles":[{"id":"car","name":"Car kit","saved_at_ms":1,"receiver_count":3}]}"#
          .utf8)
    ).profiles
    session.selectedSetupID = "car"
    for (name, scheme, appearance) in [
      ("light", ColorScheme.light, NSAppearance.Name.aqua),
      ("dark", ColorScheme.dark, NSAppearance.Name.darkAqua),
    ] {
      let window = NSWindow(
        contentRect: NSRect(x: -3000, y: -3000, width: 1280, height: 820), styleMask: [.titled],
        backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.appearance = NSAppearance(named: appearance)
      let view = NSHostingView(
        rootView: SessionView(session: session, app: app).environment(\.colorScheme, scheme)
          .background(Color(nsColor: .windowBackgroundColor)))
      window.contentView = view
      window.orderBack(nil)
      for _ in 0..<10 {
        view.layoutSubtreeIfNeeded()
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.02))
      }
      let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
      view.cacheDisplay(in: view.bounds, to: bitmap)
      let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
      try png.write(
        to: URL(fileURLWithPath: destination).appendingPathComponent("setup-\(name).png"))
      XCTAssertGreaterThan(view.bounds.width, 1000)
      window.close()
    }
  }
}
