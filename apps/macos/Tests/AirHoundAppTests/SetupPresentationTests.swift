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
