import AirHoundCore
import AppKit
import SwiftUI

private struct HostedTable: View {
  let rows: [Observation]
  @State private var selection: Int?
  @State private var order = [KeyPathComparator(\Observation.name)]
  var body: some View {
    Table(rows, selection: $selection, sortOrder: $order) {
      TableColumn("Name", value: \.name).width(min: 180, ideal: 250)
      TableColumn("Protocol", value: \.medium).width(min: 70, ideal: 85)
      TableColumn("dBm", value: \.signalSort) { Text($0.signal).monospacedDigit() }.width(50)
      TableColumn("Channel", value: \.channel).width(58)
      TableColumn("Receiver", value: \.receiver).width(min: 90, ideal: 110)
      TableColumn("Match", value: \.match).width(52)
    }
  }
}

@main struct TableBenchmark {
  @MainActor static func main() throws {
    NSApplication.shared.setActivationPolicy(.prohibited)
    let lines = (0..<1000).map {
      "{\"type\":\"ble_observation\",\"name\":\"Sensor \($0)\",\"rssi\":-42,\"channel\":37}"
    }.joined(separator: "\n")
    let rows = try Recording.decode(Data(lines.utf8)).observations
    let native = ObservationTableCoordinator()
    let nativeView = native.makeScrollView()
    native.update(rows: rows, revision: UUID(), selection: nil, sortOrder: [ObservationSort()])
    let hosted = NSHostingView(rootView: HostedTable(rows: rows))
    try run("swiftui", view: hosted)
    try run("appkit", view: nativeView)
  }

  @MainActor private static func run(_ name: String, view: NSView) throws {
    let window = NSWindow(
      contentRect: NSRect(x: -3000, y: -3000, width: 1000, height: 600),
      styleMask: [.borderless], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = view
    window.orderBack(nil)
    defer { window.close() }
    for _ in 0..<10 {
      window.contentView?.layoutSubtreeIfNeeded()
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.01))
    }
    guard let scroll = findScroll(view) else { throw NSError(domain: "No scroll view", code: 1) }
    let start = ContinuousClock.now
    for index in 0..<300 {
      // Exercise recycled rows in both directions, including layout and display.
      let step = index < 150 ? index : 299 - index
      scroll.contentView.scroll(to: NSPoint(x: 0, y: step * 80))
      scroll.reflectScrolledClipView(scroll.contentView)
      window.contentView?.layoutSubtreeIfNeeded()
      window.contentView?.displayIfNeeded()
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.001))
    }
    let elapsed = start.duration(to: .now)
    print(
      "\(name) 300 scroll/layout/display steps: \(elapsed); instantiated views: \(viewCount(view))")
  }
  @MainActor private static func findScroll(_ view: NSView) -> NSScrollView? {
    (view as? NSScrollView) ?? view.subviews.lazy.compactMap { findScroll($0) }.first
  }
  @MainActor private static func viewCount(_ view: NSView) -> Int {
    1 + view.subviews.reduce(0) { $0 + viewCount($1) }
  }
}
