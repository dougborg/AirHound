import AirHoundCore
import SwiftUI

struct SavedSetupView: View {
  @ObservedObject var session: SessionModel
  @State private var saving = false
  @State private var name = ""
  @State private var replace = false
  @State private var deleting = false
  var body: some View {
    Section("Saved setups") {
      if session.setupsAvailable {
        HStack {
          Picker("Setup", selection: $session.selectedSetupID) {
            Text("Choose a saved setup").tag("")
            ForEach(session.setups) { setup in Text(setup.name).tag(setup.id) }
          }
          Button("Load") { session.reviewSetup() }.disabled(session.selectedSetupID.isEmpty)
        }
        HStack {
          Button("Save Setup…") {
            name = session.appliedSetupName ?? ""
            replace = false
            saving = true
          }.disabled(session.configText.isEmpty)
          Menu("Manage") {
            Button("Refresh List") { session.refreshSetups() }
            Button("Delete Selected Setup…", role: .destructive) { deleting = true }
              .disabled(session.selectedSetupID.isEmpty)
          }
          Spacer()
        }
        if let error = session.setupError {
          Text(error).font(.caption).foregroundStyle(.red).textSelection(.enabled)
        }
      } else {
        Text(
          session.connected
            ? "This backend does not support saved setups." : "Connect to load or save a setup."
        )
        .foregroundStyle(.secondary)
      }
    }
    .disabled(!session.connected || !session.canEditPlan)
    .sheet(isPresented: $saving) {
      VStack(alignment: .leading, spacing: 16) {
        Text("Save Setup").font(.title2)
        TextField("Name", text: $name).onSubmit { save() }
        if !session.selectedSetupID.isEmpty {
          Toggle("Replace the selected saved setup", isOn: $replace)
        }
        Text("Stored on the connected backend. Saving does not start capture.")
          .font(.caption).foregroundStyle(.secondary)
        HStack {
          Spacer()
          Button("Cancel") { saving = false }.keyboardShortcut(.cancelAction)
          Button("Save") { save() }.keyboardShortcut(.defaultAction)
            .disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }
      }.padding(24).frame(width: 420)
    }
    .sheet(item: $session.setupReview) { review in
      VStack(alignment: .leading, spacing: 16) {
        Text(review.name).font(.title2)
        Text("Review this setup before applying it to the plan.").foregroundStyle(.secondary)
        if let origin = review.raw["origin_check"]?["state"]?.string {
          Text(
            "Saved setup origin: \(origin.replacingOccurrences(of: "_", with: " ")). Physical device identity is not verified."
          )
          .font(.caption).foregroundStyle(.secondary)
        }
        ScrollView {
          VStack(alignment: .leading, spacing: 12) {
            ForEach(Array(review.checks.enumerated()), id: \.offset) { _, check in
              VStack(alignment: .leading, spacing: 3) {
                Text(check.label).font(.headline)
                Text(check.detail).font(.callout).foregroundStyle(.secondary).textSelection(
                  .enabled)
              }
            }
            if let message = review.raw["bindings"]?["message"]?.string {
              Text(message).font(.caption).foregroundStyle(.secondary)
            }
            if review.raw["validation"]?["valid"] == .bool(false) {
              Text(
                "The backend found configuration problems. Apply the setup to edit it, then validate again."
              )
              .foregroundStyle(.red)
            }
            DisclosureGroup("Backend details") {
              Text((try? String(decoding: review.raw.data(pretty: true), as: UTF8.self)) ?? "")
                .font(.system(.caption, design: .monospaced)).textSelection(.enabled)
            }
          }.frame(maxWidth: .infinity, alignment: .leading)
        }.frame(maxHeight: 340)
        HStack {
          Text("Loading does not start capture.").font(.caption).foregroundStyle(.secondary)
          Spacer()
          Button("Cancel") { session.setupReview = nil }.keyboardShortcut(.cancelAction)
          Button("Use Setup") { session.applySetup(review) }.keyboardShortcut(.defaultAction)
            .disabled(!session.canEditPlan)
        }
      }.padding(24).frame(width: 590)
    }
    .confirmationDialog("Delete this saved setup?", isPresented: $deleting) {
      Button("Delete Setup", role: .destructive) { session.deleteSelectedSetup() }
    } message: {
      Text("The current plan and recordings are kept.")
    }
  }
  private func save() {
    guard !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
    saving = false
    session.saveSetup(name: name, replacing: replace ? session.selectedSetupID : nil)
  }
}
