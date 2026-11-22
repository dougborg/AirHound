import AirHoundCore
import SwiftUI

struct ReceiverSettings: View {
  @ObservedObject var session: SessionModel
  let key: String
  let kind: String
  let source: JSONValue
  var body: some View {
    DisclosureGroup("Receiver settings") {
      if kind == "external_wifi" {
        field("USB selector", "usb_selector")
        field("Scanner helper", "executable", file: true)
        field("Firmware", "firmware", file: true)
        field("Channels (comma separated)", "channels", format: "array")
        field("Dwell (ms)", "dwell_ms", format: "integer")
        Text(
          "Use the helper's list-devices command for its exact USB selector. Firmware is supplied separately."
        )
        .font(.caption).foregroundStyle(.secondary)
      } else {
        field("Serial port", "port")
        field("Python executable", "python", file: true)
        field("Sniffle helper", "helper", file: true)
        field("Sniffle module directory", "module_path", file: true, directory: true)
        field("Advertising channel", "channel", format: "integer")
        Toggle(
          "Coded PHY",
          isOn: Binding(
            get: { session.sourceField(key, field: "coded", fallback: source) == .bool(true) },
            set: { session.setSourceField(key, field: "coded", value: .bool($0), fallback: source) }
          ))
        Text("Confirm this device is running compatible Sniffle firmware before starting.")
          .font(.caption).foregroundStyle(.secondary)
      }
    }
  }

  private func field(
    _ title: String, _ name: String, format: String = "string", file: Bool = false,
    directory: Bool = false
  ) -> some View {
    HStack {
      TextField(
        title,
        text: Binding(
          get: {
            let value = session.sourceField(key, field: name, fallback: source)
            if case .string(let text) = value { return text }
            if case .integer(let number) = value { return String(number) }
            if case .array(let entries) = value {
              return entries.compactMap {
                if case .integer(let number) = $0 { return String(number) }
                return nil
              }.joined(separator: ", ")
            }
            return ""
          },
          set: { text in
            let value: JSONValue
            if text.isEmpty {
              value = .null
            } else if format == "integer", let number = Int64(text) {
              value = .integer(number)
            } else if format == "array" {
              let parts = text.split(separator: ",", omittingEmptySubsequences: false)
              let numbers = parts.compactMap { Int64($0.trimmingCharacters(in: .whitespaces)) }
              value =
                numbers.count == parts.count
                ? .array(numbers.map(JSONValue.integer)) : .string(text)
            } else {
              value = .string(text)
            }
            session.setSourceField(key, field: name, value: value, fallback: source)
          }))
      if file {
        Button("Choose…") {
          let panel = NSOpenPanel()
          panel.title = title
          panel.canChooseFiles = !directory
          panel.canChooseDirectories = directory
          if panel.runModal() == .OK, let url = panel.url {
            session.setSourceField(key, field: name, value: .string(url.path), fallback: source)
          }
        }.accessibilityLabel("Choose \(title)")
      }
    }
  }
}
