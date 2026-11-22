import AirHoundCore
import SwiftUI

struct DevicesView: View {
  @ObservedObject var model: AppModel
  @Environment(\.dismiss) private var dismiss
  var body: some View {
    VStack(alignment: .leading, spacing: 12) {
      Text("Devices").font(.title2)
      Text(
        "Passive inventory lists candidate backends and prerequisites. Presence does not establish permission, firmware compatibility or readiness to scan."
      )
      .foregroundStyle(.secondary)
      HStack {
        Text(model.executable?.path ?? "Choose an airhound-hostd executable").lineLimit(2)
          .textSelection(.enabled)
        Spacer()
        Button("Choose backend…") { model.chooseBackend() }.disabled(model.discovering)
        if model.discovering {
          ProgressView().controlSize(.small)
          Button("Cancel") { model.cancelDiscovery() }
        } else {
          Button("Refresh") { model.refresh() }.disabled(model.executable == nil)
        }
      }
      Divider()
      if let error = model.discoveryError {
        Text(error).foregroundStyle(.red).textSelection(.enabled)
      }
      ScrollView {
        VStack(alignment: .leading, spacing: 16) {
          if let inventory = model.inventory {
            Text("\(inventory.devices.count) candidates · \(inventory.platform) · Passive snapshot")
              .font(.headline)
            ForEach(Array(inventory.diagnostics.enumerated()), id: \.offset) { _, diagnostic in
              Label(
                diagnostic.backend + ": " + diagnostic.message,
                systemImage: "exclamationmark.triangle")
            }
            if inventory.devices.isEmpty {
              Text(
                "No candidates reported. Check diagnostics before concluding a device is absent.")
            }
            ForEach(inventory.devices) { device in
              DisclosureGroup {
                VStack(alignment: .leading, spacing: 8) {
                  Text(device.id).font(.system(.caption, design: .monospaced)).textSelection(
                    .enabled)
                  Text("Identity scope: " + device.identityScope)
                  ForEach(Array(device.endpoints.enumerated()), id: \.offset) { _, endpoint in
                    Text(endpoint.kind + ": " + endpoint.path).textSelection(.enabled)
                  }
                  ForEach(Array(device.candidates.enumerated()), id: \.offset) { _, candidate in
                    VStack(alignment: .leading, spacing: 6) {
                      Text(candidate.label).font(.headline)
                      Text(candidate.support + " · " + candidate.state)
                      ForEach(Array(candidate.evidence.enumerated()), id: \.offset) { _, evidence in
                        Text(evidence)
                      }
                      ForEach(Array(candidate.requirements.enumerated()), id: \.offset) {
                        _, requirement in
                        Text(
                          requirement.name + " (" + requirement.state + "): " + requirement.detail)
                      }
                    }.padding(.vertical, 4)
                  }
                }.font(.callout).padding(.top, 8)
              } label: {
                Text(device.name)
              }
              Divider()
            }
          } else if !model.discovering {
            Text("Click Refresh to enumerate devices. No radio or serial interface will be opened.")
              .foregroundStyle(.secondary)
          }
        }.frame(maxWidth: .infinity, alignment: .leading)
      }
      HStack {
        Spacer()
        Button("Done") { dismiss() }.keyboardShortcut(.defaultAction)
      }
    }.padding(20).frame(width: 720, height: 560)
  }
}
