import AirHoundCore
import AppKit
import SwiftUI
import XCTest

@testable import AirHoundApp

@MainActor
final class FlipperPresentationTests: XCTestCase {
  func testFlipperSetupGatesBackendAndPreservesPlanWithoutStarting() throws {
    let suite = "flipper-setup-" + UUID().uuidString
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let session = SessionModel(preferences: defaults)
    session.configText =
      #"{"version":1,"future_options":{"keep":true},"alert_policy":"silent","location":{"kind":"gpsd","endpoint":"127.0.0.1:2947"},"receivers":[{"id":"existing","source":{"kind":"native_ble"}}]}"#
    let original = session.configText
    session.connected = true
    session.addFlipperBridge(port: "/dev/cu.fixture")
    XCTAssertEqual(session.configText, original)
    session.sessionSources = ["flipper_marauder"]
    session.addFlipperBridge(port: "  ")
    XCTAssertEqual(session.configText, original)
    session.beginBackgroundRefresh()
    XCTAssertTrue(session.canAddFlipperBridge)
    session.addFlipperBridge(port: " /dev/cu.fixture ")
    session.finishBackgroundRefresh()
    let receiver = try XCTUnwrap(session.plannedReceivers.last)
    XCTAssertEqual(receiver.title, "Flipper / AWOK")
    XCTAssertEqual(
      receiver.source,
      .object(["kind": .string("flipper_marauder"), "port": .string("/dev/cu.fixture")]))
    let config = try JSONDecoder().decode(JSONValue.self, from: Data(session.configText.utf8))
    XCTAssertEqual(config["alert_policy"], .string("silent"))
    XCTAssertEqual(config["future_options"], .object(["keep": .bool(true)]))
    XCTAssertEqual(config["location"]?["kind"], .string("gpsd"))
    XCTAssertEqual(session.plannedReceivers.first?.id, "existing")
    XCTAssertFalse(session.isActive)
    let unchanged = session.configText
    session.connected = false
    session.addReceiver(source: receiver.source)
    XCTAssertEqual(
      session.configText, unchanged, "Inventory hints cannot bypass a disconnected capability gate")
    session.connected = true
    session.capture = .object(["active": .bool(true)])
    session.addFlipperBridge(port: "/dev/cu.other")
    XCTAssertEqual(session.configText, unchanged)
    session.capture = .object(["active": .bool(false)])
    session.busy = true
    XCTAssertFalse(session.canAddFlipperBridge)
  }

  private var identity: JSONValue {
    .object([
      "flipper_firmware": .object([
        "evidence": .string("usb_cli_device_info"),
        "fields": .object([
          "firmware_version": .string("mntm-test"), "firmware_commit": .string("abc123"),
        ]),
      ]),
      "application": .object([
        "evidence": .string("ahfb_hello"), "version": .string("0.1.0"),
        "source_revision": .string(String(repeating: "a", count: 40)),
        "source_modified": .integer(1), "compiled_firmware_abi": .string("87.1"),
      ]),
      "app_instance": .unsigned(42), "awok_firmware": .null,
    ])
  }

  func testBridgeBuildDoesNotBecomeRadioFirmwareOrBootIdentity() {
    let lines = FlipperBridgeIdentityPresentation(identity: identity).lines
    XCTAssertTrue(lines.contains("Flipper firmware: mntm-test"))
    XCTAssertTrue(lines.contains("AirHound Live Bridge: 0.1.0"))
    XCTAssertTrue(lines.contains("Bridge source: local changes reported"))
    XCTAssertTrue(lines.contains("Bridge compiled firmware API: 87.1"))
    XCTAssertTrue(lines.contains("AWOK radio firmware unverified"))
    XCTAssertFalse(lines.contains { $0.contains("boot") || $0.contains("42") })
    let missing = FlipperBridgeIdentityPresentation(
      identity: .object(["application": .object(["version": .string("guessed")])])
    ).lines
    XCTAssertTrue(missing.contains("AirHound Live Bridge identity unreported"))
    XCTAssertFalse(missing.contains { $0.contains("guessed") })
  }

  func testRenderFlipperPrototypeRequirementsAndProvenance() throws {
    guard let destination = ProcessInfo.processInfo.environment["AIRHOUND_RENDER_FLIPPER_DIR"]
    else {
      throw XCTSkip("Set AIRHOUND_RENDER_FLIPPER_DIR for bounded native visual inspection")
    }
    try FileManager.default.createDirectory(atPath: destination, withIntermediateDirectories: true)
    for (name, scheme) in [("light", ColorScheme.light), ("dark", .dark)] {
      let content = VStack(alignment: .leading, spacing: 16) {
        Text("Flipper / AWOK").font(.title2)
        TextField("Flipper USB serial port", text: .constant("/dev/cu.fixture"))
        FlipperBridgeRequirements()
        Divider()
        Text("Flipper and bridge software").font(.headline)
        ForEach(
          Array(FlipperBridgeIdentityPresentation(identity: identity).lines.enumerated()),
          id: \.offset
        ) { _, line in
          Text(line).font(.caption).textSelection(.enabled)
        }
      }.padding(24).frame(width: 500).background(Color(nsColor: .windowBackgroundColor))
        .environment(\.colorScheme, scheme)
      let window = NSWindow(
        contentRect: NSRect(x: -3000, y: -3000, width: 500, height: 520),
        styleMask: [.titled], backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.appearance = NSAppearance(named: scheme == .dark ? .darkAqua : .aqua)
      let view = NSHostingView(rootView: content)
      window.contentView = view
      window.orderBack(nil)
      for _ in 0..<10 {
        view.layoutSubtreeIfNeeded()
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.02))
      }
      let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
      view.cacheDisplay(in: view.bounds, to: bitmap)
      try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(
        to: URL(fileURLWithPath: destination).appendingPathComponent("flipper-\(name).png"))
      window.close()
    }
  }
}
