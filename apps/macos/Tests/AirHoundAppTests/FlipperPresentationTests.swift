import AirHoundCore
import AppKit
import SwiftUI
import XCTest

@testable import AirHoundApp

@MainActor
final class FlipperPresentationTests: XCTestCase {
  func testRealBackendFlipperSetupRoundtripAndMissingBindingWithoutCapture() async throws {
    guard let backend = ProcessInfo.processInfo.environment["AIRHOUND_TEST_BACKEND"] else {
      throw XCTSkip("Set AIRHOUND_TEST_BACKEND for private Flipper setup integration")
    }
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(
      "ah-flipper-ui-" + UUID().uuidString.prefix(8))
    try LocalFiles.privateDirectory(root)
    defer { try? FileManager.default.removeItem(at: root) }
    let pcap = root.appendingPathComponent("empty.pcap")
    try Data([
      0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0, 0, 127, 0, 0, 0,
    ]).write(to: pcap)
    let database = root.appendingPathComponent("records.sqlite")
    let replay = Process()
    replay.executableURL = URL(fileURLWithPath: backend)
    replay.arguments = [
      "--wifi-pcap", pcap.path, "--database", database.path,
      "--output", root.appendingPathComponent("empty.csv").path,
    ]
    replay.standardOutput = FileHandle.nullDevice
    replay.standardError = FileHandle.nullDevice
    try replay.run()
    replay.waitUntilExit()
    XCTAssertEqual(replay.terminationStatus, 0)
    let socket = root.appendingPathComponent("control.sock")
    let server = Process()
    server.executableURL = URL(fileURLWithPath: backend)
    // History-only service: even an accidental Start cannot open a receiver.
    server.arguments = ["serve", "--socket", socket.path, "--database", database.path]
    server.standardOutput = FileHandle.nullDevice
    server.standardError = FileHandle.nullDevice
    try server.run()
    defer {
      if server.isRunning {
        server.terminate()
        server.waitUntilExit()
      }
    }
    for _ in 0..<100 where !FileManager.default.fileExists(atPath: socket.path) {
      try await Task.sleep(for: .milliseconds(20))
    }
    let suite = "flipper-integration-" + UUID().uuidString
    let preferences = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { preferences.removePersistentDomain(forName: suite) }
    let model = SessionModel(preferences: preferences)
    model.socketPath = socket.path
    model.connect(executable: URL(fileURLWithPath: backend))
    defer { model.disconnect() }
    func idle() async throws {
      for _ in 0..<100 {
        if !model.busy { return }
        try await Task.sleep(for: .milliseconds(50))
      }
      XCTFail("Private backend operation did not complete")
    }
    try await idle()
    XCTAssertTrue(model.connected, model.error ?? model.message)
    XCTAssertFalse(model.captureEnabled)
    XCTAssertTrue(model.canAddFlipperBridge, "Capability must come from the actual backend")
    model.configText = #"{"version":1,"alert_policy":"silent","receivers":[]}"#
    let missingPort = "/dev/cu.airhound-missing-" + UUID().uuidString
    model.addFlipperBridge(port: missingPort)
    let original = try JSONDecoder().decode(JSONValue.self, from: Data(model.configText.utf8))
    let receiver = try XCTUnwrap(model.plannedReceivers.first)
    XCTAssertTrue(model.saveSetup(name: "Private Flipper fixture"))
    try await idle()
    XCTAssertNil(model.error)
    model.configText = #"{"version":1,"receivers":[]}"#
    model.reviewSetup()
    try await idle()
    let review = try XCTUnwrap(model.setupReview)
    XCTAssertEqual(review.config, original)
    XCTAssertTrue(
      review.checks.contains { $0.label == receiver.id && $0.detail.contains("missing") })
    XCTAssertEqual(review.raw["applied"], .bool(false))
    XCTAssertTrue(model.plannedReceivers.isEmpty)
    model.applySetup(review)
    model.planSourceField(id: receiver.id, field: "port", value: .string(missingPort + "-edited"))
    XCTAssertEqual(model.plannedReceivers.first?.source["port"], .string(missingPort + "-edited"))
    XCTAssertTrue(model.saveSetup(name: "Edited Flipper fixture", replacing: review.id))
    try await idle()
    model.reviewSetup()
    try await idle()
    let editedReview = try XCTUnwrap(model.setupReview)
    XCTAssertEqual(
      try PlanEditing.receivers(in: editedReview.config).first?.source["port"],
      .string(missingPort + "-edited"))
    model.planSourceField(id: receiver.id, field: "port", value: .string("not-a-local-device"))
    model.validate()
    try await idle()
    XCTAssertTrue(
      model.error?.contains("explicit local /dev serial endpoint") == true,
      model.error ?? "Missing backend validation error")
    XCTAssertFalse(model.isActive)
    XCTAssertEqual(model.runs.count, 1, "Only the empty replay fixture may exist")
  }

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

  func testBoundFlipperIdentityPresentationRejectsAnotherPortAndHistoricalRun() throws {
    let suite = "flipper-identity-" + UUID().uuidString
    let preferences = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { preferences.removePersistentDomain(forName: suite) }
    let model = SessionModel(preferences: preferences)
    model.connected = true
    model.configText =
      #"{"version":1,"receivers":[{"id":"bridge","source":{"kind":"flipper_marauder","port":"/dev/cu.fixture"}}]}"#
    model.runs = [
      try JSONDecoder().decode(
        StoredRun.self,
        from: Data(
          #"{"id":"fixture-run","state":"recording","mode":"live","started_at_us":0,"record_count":0,"config":{"session":{"receivers":[{"id":"bridge","source":{"kind":"flipper_marauder","port":"/dev/cu.fixture"}}]}}}"#
            .utf8))
    ]
    model.capture = .object([
      "active": .bool(true), "run_id": .string("fixture-run"),
      "receiver_health": .object([
        "receivers": .array([
          .object([
            "id": .string("bridge"),
            "receiver": .object([
              "backend": .string("flipper_marauder"), "bridge_identity": identity,
            ]),
          ])
        ])
      ]),
    ])
    let actual = try XCTUnwrap(model.currentReceiverIdentity(id: "bridge")?["bridge_identity"])
    let lines = FlipperBridgeIdentityPresentation(identity: actual).lines
    XCTAssertTrue(lines.contains("Flipper firmware: mntm-test"))
    XCTAssertTrue(lines.contains("AirHound Live Bridge: 0.1.0"))
    XCTAssertTrue(lines.contains("AWOK radio firmware unverified"))
    model.configText = model.configText.replacingOccurrences(
      of: "/dev/cu.fixture", with: "/dev/cu.other")
    XCTAssertNil(model.currentReceiverIdentity(id: "bridge"))
    model.configText = model.configText.replacingOccurrences(
      of: "/dev/cu.other", with: "/dev/cu.fixture")
    model.connected = false
    XCTAssertNil(model.currentReceiverIdentity(id: "bridge"))
    model.connected = true
    model.capture = .object(["active": .bool(false), "run_id": .string("fixture-run")])
    XCTAssertNil(model.currentReceiverIdentity(id: "bridge"))
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
