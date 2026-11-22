import AppKit
import Foundation

/// GUI provenance is independent of a connected service or receiver's version.
struct AppBuildInfo {
  let packageVersion: String?
  let revision: String?
  let tag: String?
  let trackedSourceModified: Bool?

  init(info: [String: Any]) {
    packageVersion = info["AirHoundPackageVersion"] as? String
    revision = info["AirHoundSourceRevision"] as? String
    tag = info["AirHoundSourceTag"] as? String
    trackedSourceModified = info["AirHoundTrackedSourceModified"] as? Bool
  }
  static var current: Self { Self(info: Bundle.main.infoDictionary ?? [:]) }
  var sourceDescription: String {
    guard let revision, !revision.isEmpty else { return "Source revision unreported" }
    var parts = ["Source " + String(revision.prefix(12))]
    if let tag, !tag.isEmpty { parts.append("tag " + tag) }
    if trackedSourceModified == true { parts.append("modified tracked source") }
    if trackedSourceModified == nil { parts.append("source state unreported") }
    return parts.joined(separator: " · ")
  }
  var displayVersion: String {
    "\(packageVersion ?? "Version unreported") · \(sourceDescription)"
  }
  @MainActor func showAbout() {
    NSApplication.shared.orderFrontStandardAboutPanel(options: [
      .applicationName: "AirHound",
      .applicationVersion: packageVersion ?? "Version unreported",
      .version: sourceDescription,
    ])
  }
}
