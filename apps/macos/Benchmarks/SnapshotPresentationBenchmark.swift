import AirHoundCore
import AppKit
import Foundation

@main struct SnapshotPresentationBenchmark {
  static func ms<T>(_ body: () throws -> T) rethrows -> (T, Double) {
    let start = DispatchTime.now().uptimeNanoseconds
    let value = try body()
    return (value, Double(DispatchTime.now().uptimeNanoseconds - start) / 1_000_000)
  }

  @MainActor static func main() throws {
    NSApplication.shared.setActivationPolicy(.prohibited)
    guard CommandLine.arguments.count == 2 else {
      throw NSError(domain: "Supply the private Reader benchmark output directory", code: 1)
    }
    let root = URL(fileURLWithPath: CommandLine.arguments[1])
    let files = try FileManager.default.contentsOfDirectory(
      at: root.appendingPathComponent("pages"), includingPropertiesForKeys: nil
    ).filter { $0.lastPathComponent.hasPrefix("0-") && $0.pathExtension == "json" }.sorted {
      $0.lastPathComponent < $1.lastPathComponent
    }
    guard !files.isEmpty && files.count <= 8 else {
      throw NSError(domain: "Expected one to eight first-region pages", code: 1)
    }
    var background = RecordWindow()
    var display = ObservationPresentation()
    let context = UUID()
    let coordinator = ObservationTableCoordinator()
    let scroll = coordinator.makeScrollView()
    let window = NSWindow(
      contentRect: NSRect(x: -5000, y: -5000, width: 1000, height: 600), styleMask: [.borderless],
      backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = scroll
    window.orderBack(nil)
    defer { window.close() }
    var samples: [[String: Any]] = []
    var heldIDs: [Int] = []
    var selected: Int?
    var anchor = NSPoint.zero
    var total = 0
    for (index, file) in files.enumerated() {
      let size = try file.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? Int.max
      guard size <= 512 * 1024 else { throw NSError(domain: "Page exceeds byte limit", code: 1) }
      let data = try Data(contentsOf: file)
      let (page, decodeMS) = try ms { try JSONDecoder().decode(RecordPage.self, from: data) }
      let (_, appendMS) = try ms { try background.append(page) }
      total += page.records.count
      let (current, cachedWindowReadMS) = try ms { try background.observations() }
      let (_, updateMS) = ms {
        display.receive(current, label: "Private snapshot replay", context: context)
        let held = display.recording!
        coordinator.update(
          rows: held.observations, revision: held.revision, selection: display.selection,
          sortOrder: [ObservationSort()], context: context)
        window.contentView?.layoutSubtreeIfNeeded()
        window.contentView?.displayIfNeeded()
      }
      if index == 0 {
        heldIDs = display.recording!.observations.map(\.id)
        guard !heldIDs.isEmpty else {
          throw NSError(domain: "First page contains no displayable observations", code: 1)
        }
        selected = heldIDs[min(20, heldIDs.count - 1)]
        display.select(selected, liveWindow: true)
        coordinator.update(
          rows: display.recording!.observations, revision: display.recording!.revision,
          selection: selected, sortOrder: [ObservationSort()], context: context)
        scroll.contentView.scroll(to: NSPoint(x: 0, y: 350))
        scroll.reflectScrolledClipView(scroll.contentView)
        anchor = scroll.contentView.bounds.origin
      } else {
        precondition(display.held && display.selection == selected)
        precondition(display.recording!.observations.map(\.id) == heldIDs)
        precondition(scroll.contentView.bounds.origin == anchor)
        precondition(coordinator.rows[coordinator.table.selectedRow].id == selected)
      }
      precondition(background.records.count <= 1000)
      samples.append([
        "page": index, "bytes": data.count, "rows": page.records.count, "decode_ms": decodeMS,
        "append_and_normalize_ms": appendMS, "cached_window_read_ms": cachedWindowReadMS,
        "held_table_update_ms": updateMS,
      ])
    }
    // Small snapshots still measure layout, but do not claim eviction coverage.
    let current = try background.observations()
    let selectedEvicted = !current.observations.contains { $0.id == selected }
    display.resume(current, label: "Private snapshot replay", context: context)
    precondition(!display.held && display.selection == nil)
    let (_, resumeMS) = ms {
      coordinator.update(
        rows: current.observations, revision: current.revision, selection: nil,
        sortOrder: [ObservationSort()], context: context)
      window.contentView?.layoutSubtreeIfNeeded()
      window.contentView?.displayIfNeeded()
    }
    precondition(coordinator.rows.map(\.id) == current.observations.map(\.id))
    let (_, scrollMS) = ms {
      // Physical benchmark workload, not a readiness delay or correctness clock.
      for i in 0..<300 {
        let step = i < 150 ? i : 299 - i
        scroll.contentView.scroll(to: NSPoint(x: 0, y: step * 80))
        scroll.reflectScrolledClipView(scroll.contentView)
        window.contentView?.layoutSubtreeIfNeeded()
        window.contentView?.displayIfNeeded()
      }
    }
    var checksum = 0
    let (_, filterMS) = ms {
      for _ in 0..<30 {
        for medium in ["Wi-Fi", "Bluetooth"] {
          checksum +=
            current.observations.filter { $0.medium == medium }.sorted(
              using: KeyPathComparator(\Observation.signalSort, order: .reverse)
            ).count
        }
      }
    }
    let (_, groupMS) = ms {
      for _ in 0..<30 {
        checksum +=
          Dictionary(grouping: current.observations) {
            $0.medium + "|" + $0.receiver + "|" + $0.address
          }.count
      }
    }
    let result: [String: Any] = [
      "scope":
        "Real private stored pages replayed into exact production RecordWindow/ObservationPresentation/AppKit coordinator; no installed UI or live capture",
      "timing_role": "Informational optimized-build benchmark; no elapsed pass criterion",
      "identity_grouping_scope":
        "Offline presentation experiment, protocol + receiver + reported address; not persistent physical-device identity or shipped grouping UI",
      "input_rows": total, "background_rows": background.records.count,
      "omitted_rows": background.omitted,
      "held_rows": heldIDs.count, "held_selection_and_viewport_preserved": true,
      "eviction_exercised": background.omitted > 0,
      "selected_row_evicted_from_background": selectedEvicted, "follow_live_resumed": true,
      "resume_ms": resumeMS, "scroll_300_layout_display_ms": scrollMS,
      "filter_sort_mean_ms": filterMS / 30, "group_mean_ms": groupMS / 30,
      "checksum": checksum, "pages": samples,
    ]
    try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
      .write(
        to: root.appendingPathComponent("presentation-results.json"), options: .withoutOverwriting)
  }
}
