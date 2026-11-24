import AirHoundCore
import Foundation

/// One immutable, bounded display snapshot. Background reads may replace the
/// session's RecordWindow, but never replace the rows someone is inspecting.
struct ObservationPresentation {
  private(set) var recording: Recording?
  private(set) var label = "No recording open"
  private(set) var held = false
  var selection: Observation.ID?
  private var context: UUID?

  @discardableResult
  mutating func receive(_ recording: Recording?, label: String, context: UUID) -> Bool {
    if self.context != context {
      self.context = context
      held = false
      selection = nil
    }
    guard !held else { return false }
    guard self.recording?.revision != recording?.revision || self.label != label else {
      return false
    }
    self.recording = recording
    self.label = label
    return true
  }

  mutating func select(_ id: Observation.ID?, liveWindow: Bool) {
    selection = id
    if id != nil && liveWindow { held = true }
  }

  mutating func hold() { held = true }

  mutating func resume(_ recording: Recording?, label: String, context: UUID) {
    held = false
    selection = nil
    receive(recording, label: label, context: context)
  }
}
