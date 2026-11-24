import AirHoundCore
import SwiftUI

struct ReceiverSettings: View {
  @ObservedObject var session: SessionModel
  let receiver: PlanReceiver
  var body: some View {
    switch receiver.kind {
    case "rayhunter":
      Picker(
        "Connection",
        selection: Binding(
          get: { receiver.source["adb"] != nil && receiver.source["adb"] != .null },
          set: { session.setRayhunterConnection(id: receiver.id, managedUSB: $0) })
      ) {
        Text("Prepared local endpoint").tag(false)
        Text("USB device").tag(true).disabled(!session.canManageRayhunterUSB)
      }
      if case .object = receiver.source["adb"] {
        adbField("USB serial", "serial")
        adbField("ADB executable", "executable", file: true)
        if !session.canManageRayhunterUSB {
          Text("Connect to a backend with managed USB support to edit this connection.")
            .font(.caption).foregroundStyle(.secondary)
        }
      } else {
        field("Forwarded raw endpoint", "endpoint")
      }
      field("Rayhunter decoder", "decoder", file: true)
      Text(
        "Experimental cellular subscription. USB mode uses the selected authorized device and manages its local forward. Pause and Finish release the subscription and owned forward; the modem keeps operating. Capture time and transmitter location are unverified."
      )
      .font(.caption).foregroundStyle(.secondary)
    case "external_wifi":
      field("USB selector", "usb_selector")
      field("Scanner helper", "executable", file: true)
      field("Firmware", "firmware", file: true)
      field("Channels", "channels", format: "array")
      field("Dwell (ms)", "dwell_ms", format: "integer")
    case "sniffle":
      field("Serial port", "port")
      field("Python executable", "python", file: true)
      field("Sniffle helper", "helper", file: true)
      field("Sniffle module directory", "module_path", file: true, directory: true)
      field("Advertising channel", "channel", format: "integer")
      Toggle(
        "Coded PHY",
        isOn: Binding(
          get: { receiver.source["coded"] == .bool(true) },
          set: { session.planSourceField(id: receiver.id, field: "coded", value: .bool($0)) }))
    case "airhound_serial":
      field("Serial port", "port")
      Picker(
        "Baud rate",
        selection: Binding(
          get: {
            if case .integer(let value) = receiver.source["baud"] { return value }
            return 0
          },
          set: { session.planSourceField(id: receiver.id, field: "baud", value: .integer($0)) })
      ) {
        Text("Choose baud rate").tag(Int64(0))
        Text("115200").tag(Int64(115200))
        Text("460800 · M5 no_std firmware").tag(Int64(460800))
        Text("921600").tag(Int64(921600))
      }
      Text(
        "Opening the port may reset the board. Pause affects host ingestion; the device keeps scanning."
      )
      .font(.caption).foregroundStyle(.secondary)
    case "flipper_marauder":
      field("Flipper USB serial port", "port")
      FlipperBridgeRequirements()
    case "marauder_capture":
      field("Saved capture", "path", file: true)
      Picker(
        "File format",
        selection: Binding(
          get: { receiver.source["format"]?.string ?? "" },
          set: { session.planSourceField(id: receiver.id, field: "format", value: .string($0)) })
      ) {
        Text("Choose format").tag("")
        Text("PCAP").tag("pcap")
        Text("Serial capture").tag("serial")
      }
      Text(
        "Replays saved packets. Missing signal strength, channel and location stay unknown; device-relative timestamps are not wall-clock times."
      )
      .font(.caption).foregroundStyle(.secondary)
    case "linux_wifi": field("Monitor interface", "interface")
    case "ble_pcap", "wifi_pcap": field("Recording file", "path", file: true)
    case "native_ble":
      Text("macOS chooses the Bluetooth controller.").font(.caption).foregroundStyle(.secondary)
    case "native_wifi":
      Text("Wi-Fi addresses require Location permission for the running service.").font(.caption)
        .foregroundStyle(.secondary)
    default:
      Text("Use Advanced JSON to edit this receiver.").font(.caption).foregroundStyle(.secondary)
    }
  }

  private func adbField(_ title: String, _ name: String, file: Bool = false) -> some View {
    HStack {
      TextField(
        title,
        text: Binding(
          get: { receiver.source["adb"]?[name]?.string ?? "" },
          set: { session.setRayhunterADBField(id: receiver.id, field: name, value: $0) }))
      if file {
        Button("Choose…") {
          let panel = NSOpenPanel()
          panel.title = title
          panel.canChooseDirectories = false
          if panel.runModal() == .OK, let url = panel.url {
            session.setRayhunterADBField(id: receiver.id, field: name, value: url.path)
          }
        }.accessibilityLabel("Choose \(title)")
      }
    }.disabled(!session.canManageRayhunterUSB)
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
            let value = receiver.source[name]
            if case .string(let text) = value { return text }
            if case .integer(let number) = value { return String(number) }
            if case .array(let entries) = value {
              return entries.compactMap {
                if case .integer(let n) = $0 { return String(n) }
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
            session.planSourceField(id: receiver.id, field: name, value: value)
          }))
      if file {
        Button("Choose…") {
          let panel = NSOpenPanel()
          panel.title = title
          panel.canChooseFiles = !directory
          panel.canChooseDirectories = directory
          if panel.runModal() == .OK, let url = panel.url {
            session.planSourceField(id: receiver.id, field: name, value: .string(url.path))
          }
        }.accessibilityLabel("Choose \(title)")
      }
    }
  }
}
