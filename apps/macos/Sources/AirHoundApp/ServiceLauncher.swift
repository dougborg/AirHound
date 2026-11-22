import AppKit
import CoreServices
import Foundation

/// Give CoreLocation/Bluetooth a real application identity for the long-lived
/// service. Short-lived CLI requests still execute the same backend directly.
@MainActor
enum ServiceLauncher {
  static func application(for executable: URL) -> URL? {
    let resolved = executable.resolvingSymlinksInPath()
    let macOS = resolved.deletingLastPathComponent()
    let contents = macOS.deletingLastPathComponent()
    let application = contents.deletingLastPathComponent()
    guard macOS.lastPathComponent == "MacOS", contents.lastPathComponent == "Contents",
      application.pathExtension == "app", let bundle = Bundle(url: application),
      ["tools.airhound.service", "com.airhound.hostd"].contains(bundle.bundleIdentifier ?? ""),
      bundle.executableURL?.resolvingSymlinksInPath() == resolved
    else { return nil }
    return application
  }

  static func launch(application: URL, arguments: [String]) throws -> ServiceLaunch {
    let registration = LSRegisterURL(application as CFURL, true)
    guard registration == noErr else {
      throw NSError(domain: NSOSStatusErrorDomain, code: Int(registration))
    }
    let launch = ServiceLaunch()
    let configuration = NSWorkspace.OpenConfiguration()
    configuration.arguments = arguments
    configuration.activates = false
    configuration.addsToRecentItems = false
    // Never reuse a differently configured service or a same-ID development
    // bundle elsewhere. The Rust socket lock remains the final ownership guard.
    configuration.createsNewApplicationInstance = true
    configuration.allowsRunningApplicationSubstitution = false
    // A Rust daemon does not emit NSApplication.finishLaunching. LaunchServices
    // can start it successfully without completing this callback. The caller
    // waits for the authenticated service socket, never this AppKit notification.
    NSWorkspace.shared.openApplication(at: application, configuration: configuration) {
      app, error in
      Task { @MainActor in
        launch.application = app
        launch.error = error
      }
    }
    return launch
  }
}

@MainActor
final class ServiceLaunch {
  fileprivate var application: NSRunningApplication?
  fileprivate var error: Error?
  var isRunning: Bool { error == nil && application?.isTerminated != true }
}
