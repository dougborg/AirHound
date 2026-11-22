import XCTest

@testable import AirHoundApp

final class AppBuildInfoTests: XCTestCase {
  func testMissingMetadataNeverClaimsAReleaseOrCleanBuild() {
    let build = AppBuildInfo(info: [:])
    XCTAssertNil(build.packageVersion)
    XCTAssertNil(build.trackedSourceModified)
    XCTAssertEqual(build.sourceDescription, "Source revision unreported")
    XCTAssertEqual(build.displayVersion, "Version unreported · Source revision unreported")
  }

  func testPackageSourceTagAndDirtyStateRemainDistinct() {
    let build = AppBuildInfo(info: [
      "AirHoundPackageVersion": "0.2.0",
      "AirHoundSourceRevision": "0123456789abcdef0123456789abcdef01234567",
      "AirHoundSourceTag": "v0.2.0-preview.1",
      "AirHoundTrackedSourceModified": true,
    ])
    XCTAssertEqual(build.packageVersion, "0.2.0")
    XCTAssertEqual(
      build.sourceDescription,
      "Source 0123456789ab · tag v0.2.0-preview.1 · modified tracked source")
    XCTAssertTrue(build.displayVersion.hasPrefix("0.2.0 · Source "))
    let unknown = AppBuildInfo(info: ["AirHoundSourceRevision": "abcdef"])
    XCTAssertEqual(unknown.sourceDescription, "Source abcdef · source state unreported")
  }
}
