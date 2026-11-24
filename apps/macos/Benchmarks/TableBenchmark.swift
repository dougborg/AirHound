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
  @MainActor static func main() async throws {
    NSApplication.shared.setActivationPolicy(.prohibited)
    let lines = (0..<1000).map {
      "{\"type\":\"ble_observation\",\"name\":\"Sensor \($0)\",\"rssi\":-42,\"channel\":37}"
    }.joined(separator: "\n")
    let rows = try Recording.decode(Data(lines.utf8)).observations
    let native = ObservationTableCoordinator()
    let nativeView = native.makeScrollView()
    native.update(rows: rows, revision: UUID(), selection: nil, sortOrder: [ObservationSort()])
    let ready = RenderReady()
    let hosted = NSHostingView(rootView: HostedTable(rows: rows).onAppear { ready.signal() })
    try await run("swiftui", view: hosted, ready: ready)
    try await run("appkit", view: nativeView)
  }

  @MainActor private static func run(_ name: String, view: NSView, ready: RenderReady? = nil)
    async throws
  {
    let window = NSWindow(
      contentRect: NSRect(x: -3000, y: -3000, width: 1000, height: 600),
      styleMask: [.borderless], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = view
    window.orderBack(nil)
    defer { window.close() }
    window.contentView?.layoutSubtreeIfNeeded()
    if let ready { try await ready.wait() }
    window.contentView?.layoutSubtreeIfNeeded()
    window.contentView?.displayIfNeeded()
    guard let scroll = findScroll(view) else { throw NSError(domain: "No scroll view", code: 1) }
    measure(name, view: view, window: window, scroll: scroll)
  }

  @MainActor private static func measure(
    _ name: String, view: NSView, window: NSWindow, scroll: NSScrollView
  ) {
    let start = ContinuousClock.now
    for index in 0..<300 {
      // Measured workload includes the explicit 1 ms run-loop service below;
      // it is not a correctness gate or a pure renderer-throughput measurement.
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

/// Actual SwiftUI mounting is the prerequisite. The wall-clock timer only
/// terminates a hung benchmark and never supplies a successful readiness result.
@MainActor
private final class RenderReady {
  private var appeared = false
  private var pending: CheckedContinuation<Void, Error>?
  private var watchdog: DispatchWorkItem?

  func signal() {
    appeared = true
    finish(.success(()))
  }

  func wait() async throws {
    if appeared { return }
    try await withCheckedThrowingContinuation { continuation in
      pending = continuation
      let guardWork = DispatchWorkItem {
        self.finish(.failure(NSError(domain: "Benchmark render readiness", code: 1)))
      }
      watchdog = guardWork
      DispatchQueue.main.asyncAfter(deadline: .now() + 30, execute: guardWork)
    }
  }

  private func finish(_ result: Result<Void, Error>) {
    watchdog?.cancel()
    watchdog = nil
    let continuation = pending
    pending = nil
    continuation?.resume(with: result)
  }
}
