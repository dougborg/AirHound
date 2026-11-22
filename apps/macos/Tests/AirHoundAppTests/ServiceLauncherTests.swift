import AirHoundCore
import AppKit
import Darwin
import XCTest

@testable import AirHoundApp

@MainActor
final class ServiceLauncherTests: XCTestCase {
  func testServiceReadinessDoesNotWaitForAppKitLaunchCompletion() async throws {
    guard let path = ProcessInfo.processInfo.environment["AIRHOUND_TEST_SERVICE_EXECUTABLE"] else {
      throw XCTSkip("Set AIRHOUND_TEST_SERVICE_EXECUTABLE to test the signed service bundle.")
    }
    let executable = URL(fileURLWithPath: path)
    XCTAssertNotNil(ServiceLauncher.application(for: executable))
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(
      "ah-launch-" + UUID().uuidString.prefix(8))
    try LocalFiles.privateDirectory(root)
    defer { try? FileManager.default.removeItem(at: root) }
    let suite = "AirHound-launch-tests-" + UUID().uuidString
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let model = SessionModel(preferences: defaults)
    model.socketPath = root.appendingPathComponent("control.sock").path
    let previous = Set(
      NSRunningApplication.runningApplications(withBundleIdentifier: "tools.airhound.service").map(
        \.processIdentifier))
    model.startService(executable: executable)
    for _ in 0..<100 {
      if model.connected && !model.busy { break }
      try await Task.sleep(for: .milliseconds(50))
    }
    let running = try XCTUnwrap(
      NSRunningApplication.runningApplications(withBundleIdentifier: "tools.airhound.service").first
      { !previous.contains($0.processIdentifier) })
    let pid = running.processIdentifier
    defer {
      model.disconnect()
      kill(pid, SIGTERM)
    }
    let client = BackendClient(
      executable: executable, socket: URL(fileURLWithPath: model.socketPath))
    let status = try JSONDecoder().decode(JSONValue.self, from: await client.request(["status"]))
    let instance = try XCTUnwrap(status["instance_id"]?.string)
    XCTAssertEqual(Int32(instance.split(separator: "-")[0], radix: 16), pid)
    XCTAssertTrue(model.connected, model.error ?? model.message)
    XCTAssertNil(model.service, "An app bundle must use LaunchServices rather than a raw Process")
    XCTAssertFalse(model.isActive, "Starting the service must not open radios")
    XCTAssertEqual(running.bundleIdentifier, "tools.airhound.service")
    model.disconnect()
    XCTAssertFalse(running.isTerminated, "Presentation disconnect must preserve the service")
  }

  func testRecognizesOnlyMatchingServiceApplicationIncludingCLISymlink() throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    defer { try? FileManager.default.removeItem(at: root) }
    let application = root.appendingPathComponent("AirHound Service.app")
    let executable = application.appendingPathComponent("Contents/MacOS/airhound-hostd")
    try FileManager.default.createDirectory(
      at: executable.deletingLastPathComponent(), withIntermediateDirectories: true)
    try Data().write(to: executable)
    let plist: [String: Any] = [
      "CFBundleIdentifier": "tools.airhound.service", "CFBundleExecutable": "airhound-hostd",
      "CFBundlePackageType": "APPL",
    ]
    try PropertyListSerialization.data(fromPropertyList: plist, format: .xml, options: 0)
      .write(to: application.appendingPathComponent("Contents/Info.plist"))
    XCTAssertEqual(
      ServiceLauncher.application(for: executable), application.resolvingSymlinksInPath())
    let link = root.appendingPathComponent("backend")
    try FileManager.default.createSymbolicLink(at: link, withDestinationURL: executable)
    XCTAssertEqual(ServiceLauncher.application(for: link), application.resolvingSymlinksInPath())
    XCTAssertNil(ServiceLauncher.application(for: root.appendingPathComponent("airhound-hostd")))
    XCTAssertNil(
      ServiceLauncher.application(for: application.appendingPathComponent("Contents/MacOS/other")))
    XCTAssertNil(ServiceLauncher.application(for: Bundle.main.executableURL!))
  }
}
