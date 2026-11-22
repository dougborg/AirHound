import AirHoundCore
import AppKit
import SwiftUI

/// AppKit recycles fixed-height cells; scrolling does not create SwiftUI hosts
/// or ask Auto Layout to measure every observation's text.
struct ObservationTable: NSViewRepresentable {
  let rows: [Observation]
  let revision: UUID
  @Binding var selection: Observation.ID?
  @Binding var sortOrder: [ObservationSort]

  func makeCoordinator() -> ObservationTableCoordinator { ObservationTableCoordinator() }

  func makeNSView(context: Context) -> NSScrollView {
    context.coordinator.makeScrollView()
  }

  func updateNSView(_ scrollView: NSScrollView, context: Context) {
    let coordinator = context.coordinator
    coordinator.selectionChanged = { selection = $0 }
    coordinator.sortChanged = { sortOrder = $0 }
    coordinator.update(rows: rows, revision: revision, selection: selection, sortOrder: sortOrder)
  }

  func sizeThatFits(_ proposal: ProposedViewSize, nsView: NSScrollView, context: Context) -> CGSize?
  {
    CGSize(width: proposal.width ?? 600, height: proposal.height ?? 400)
  }
}

struct ObservationSort: Equatable {
  var column: ObservationColumn = .name
  var ascending = true
  var comparator: KeyPathComparator<Observation> {
    let order: SortOrder = ascending ? .forward : .reverse
    switch column {
    case .name: return KeyPathComparator(\.name, order: order)
    case .medium: return KeyPathComparator(\.medium, order: order)
    case .signal: return KeyPathComparator(\.signalSort, order: order)
    case .channel: return KeyPathComparator(\.channel, order: order)
    case .receiver: return KeyPathComparator(\.receiver, order: order)
    case .match: return KeyPathComparator(\.match, order: order)
    }
  }
}

enum ObservationColumn: String, CaseIterable {
  case name, medium, signal, channel, receiver, match
  var title: String {
    switch self {
    case .name: return "Name"
    case .medium: return "Protocol"
    case .signal: return "dBm"
    case .channel: return "Channel"
    case .receiver: return "Receiver"
    case .match: return "Match"
    }
  }
  var width: CGFloat {
    switch self {
    case .name: return 250
    case .medium: return 85
    case .signal: return 60
    case .channel: return 70
    case .receiver: return 110
    case .match: return 90
    }
  }
  func text(_ row: Observation) -> String {
    switch self {
    case .name: return row.name
    case .medium: return row.medium
    case .signal: return row.signal
    case .channel: return row.channel
    case .receiver: return row.receiver
    case .match: return row.match
    }
  }
}

@MainActor
final class ObservationTableCoordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate {
  let table = NSTableView()
  private(set) var rows: [Observation] = []
  private var revision: UUID?
  private var applying = false
  var selectionChanged: (Observation.ID?) -> Void = { _ in }
  var sortChanged: ([ObservationSort]) -> Void = { _ in }

  func makeScrollView() -> NSScrollView {
    let scroll = NSScrollView()
    scroll.hasVerticalScroller = true
    scroll.hasHorizontalScroller = true
    scroll.autohidesScrollers = true
    scroll.borderType = .noBorder
    table.style = .inset
    table.rowHeight = 24
    table.usesAutomaticRowHeights = false
    table.intercellSpacing = NSSize(width: 8, height: 2)
    table.usesAlternatingRowBackgroundColors = true
    table.allowsEmptySelection = true
    table.allowsMultipleSelection = false
    table.allowsColumnReordering = true
    table.allowsColumnResizing = true
    table.setAccessibilityLabel("Observations")
    for field in ObservationColumn.allCases {
      let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(field.rawValue))
      column.title = field.title
      column.width = field.width
      column.minWidth = field == .name ? 100 : 45
      column.sortDescriptorPrototype = NSSortDescriptor(key: field.rawValue, ascending: true)
      table.addTableColumn(column)
    }
    table.delegate = self
    table.dataSource = self
    scroll.documentView = table
    return scroll
  }

  func update(
    rows newRows: [Observation], revision newRevision: UUID,
    selection: Observation.ID?, sortOrder: [ObservationSort]
  ) {
    applying = true
    defer { applying = false }
    let descriptors = sortOrder.map {
      NSSortDescriptor(key: $0.column.rawValue, ascending: $0.ascending)
    }
    if table.sortDescriptors != descriptors { table.sortDescriptors = descriptors }
    if revision != newRevision {
      // Retain the visible anchor during live-page insertion/eviction instead
      // of snapping the user back to the first row at every refresh.
      let clip = table.enclosingScrollView?.contentView
      let origin = clip?.bounds.origin ?? .zero
      let top = table.row(at: origin)
      let anchor = rows.indices.contains(top) ? rows[top].id : nil
      let offset = top >= 0 ? origin.y - table.rect(ofRow: top).minY : 0
      rows = newRows
      revision = newRevision
      table.reloadData()
      if let clip, let anchor, let row = rows.firstIndex(where: { $0.id == anchor }) {
        let maximum = max(0, table.bounds.height - clip.bounds.height)
        clip.scroll(
          to: NSPoint(x: origin.x, y: min(maximum, max(0, table.rect(ofRow: row).minY + offset))))
        table.enclosingScrollView?.reflectScrolledClipView(clip)
      }
    }
    let index = selection.flatMap { id in rows.firstIndex { $0.id == id } }
    if table.selectedRow != (index ?? -1) {
      table.selectRowIndexes(
        index.map { IndexSet(integer: $0) } ?? IndexSet(), byExtendingSelection: false)
    }
  }

  func numberOfRows(in tableView: NSTableView) -> Int { rows.count }

  func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView?
  {
    guard rows.indices.contains(row), let identifier = tableColumn?.identifier,
      let field = ObservationColumn(rawValue: identifier.rawValue)
    else { return nil }
    let cell: NSTableCellView
    if let reused = tableView.makeView(withIdentifier: identifier, owner: self) as? NSTableCellView
    {
      cell = reused
    } else {
      cell = NSTableCellView(frame: NSRect(x: 0, y: 0, width: field.width, height: 24))
      cell.identifier = identifier
      let label = NSTextField(labelWithString: "")
      label.frame = NSRect(x: 0, y: 3, width: field.width, height: 18)
      label.autoresizingMask = [.width]
      label.lineBreakMode = .byTruncatingTail
      label.maximumNumberOfLines = 1
      label.font =
        field == .signal
        ? .monospacedDigitSystemFont(ofSize: 13, weight: .regular) : .systemFont(ofSize: 13)
      cell.addSubview(label)
      cell.textField = label
    }
    let text = field.text(rows[row])
    cell.textField?.stringValue = text
    cell.toolTip = text
    return cell
  }

  func tableViewSelectionDidChange(_ notification: Notification) {
    guard !applying else { return }
    selectionChanged(rows.indices.contains(table.selectedRow) ? rows[table.selectedRow].id : nil)
  }

  func tableView(
    _ tableView: NSTableView, sortDescriptorsDidChange oldDescriptors: [NSSortDescriptor]
  ) {
    guard !applying else { return }
    sortChanged(
      tableView.sortDescriptors.compactMap { descriptor in
        guard let key = descriptor.key, let column = ObservationColumn(rawValue: key) else {
          return nil
        }
        return ObservationSort(column: column, ascending: descriptor.ascending)
      })
  }
}
