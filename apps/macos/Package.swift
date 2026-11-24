// swift-tools-version: 5.9
import PackageDescription

let package = Package(
  name: "AirHoundMac",
  platforms: [.macOS(.v14)],
  products: [.executable(name: "AirHound", targets: ["AirHoundApp"])],
  targets: [
    .target(name: "AirHoundCore"),
    .executableTarget(
      name: "AirHoundApp", dependencies: ["AirHoundCore"],
      resources: [.copy("Resources/example.ndjson")]),
    .target(name: "AirHoundTestSupport", path: "Tests/AirHoundTestSupport"),
    .testTarget(name: "AirHoundCoreTests", dependencies: ["AirHoundCore", "AirHoundTestSupport"]),
    .testTarget(
      name: "AirHoundAppTests",
      dependencies: ["AirHoundApp", "AirHoundCore", "AirHoundTestSupport"]),
  ]
)
