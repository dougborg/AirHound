import AirHoundCore
import AppKit
import SwiftUI

@MainActor
final class AppModel: ObservableObject {
  @Published var recording: Recording?
  @Published var loadingRecording = false
  @Published var recordingLabel = "No recording open"
  @Published var isExample = false
  @Published var recordingRevision = UUID()
  @Published var inventory: Inventory?
  @Published var discovering = false
  @Published var discoveryError: String?
  @Published var error: String?
  @Published var showDevices = false
  @Published var executable: URL?
  private var discoveryTask: Task<Void, Never>?

  init() {
    let bundled = Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/airhound-hostd")
    let sibling = Bundle.main.bundleURL.deletingLastPathComponent().appendingPathComponent(
      "airhound-hostd")
    executable = [bundled, sibling].first { FileManager.default.isExecutableFile(atPath: $0.path) }
  }

  func chooseBackend() {
    let panel = NSOpenPanel()
    panel.title = "Choose airhound-hostd"
    panel.message =
      "Choose a trusted AirHound backend executable. It will be run only when you click Refresh."
    panel.canChooseDirectories = false
    panel.allowsMultipleSelection = false
    if panel.runModal() == .OK {
      executable = panel.url
      inventory = nil
      discoveryError = nil
    }
  }

  func refresh() {
    guard !discovering, let executable else { return }
    discovering = true
    inventory = nil
    discoveryError = nil
    discoveryTask = Task {
      do {
        let data = try await DiscoveryProcess().run(executable: executable)
        try Task.checkCancellation()
        inventory = try Inventory.decode(data)
      } catch { discoveryError = error.localizedDescription }
      discovering = false
    }
  }
  func cancelDiscovery() { discoveryTask?.cancel() }
  func shutdown() async {
    discoveryTask?.cancel()
    await discoveryTask?.value
  }

  func loadExample() {
    guard !loadingRecording else { return }
    do {
      guard
        let url = Bundle.main.url(forResource: "example", withExtension: "ndjson")
          ?? Bundle.module.url(forResource: "example", withExtension: "ndjson")
      else {
        throw ModelError.invalid("Bundled example is missing")
      }
      recording = try Recording.load(url)
      recordingLabel = "Example recording"
      isExample = true
      recordingRevision = UUID()
    } catch { self.error = error.localizedDescription }
  }
  func openRecording() {
    guard !loadingRecording else { return }
    let panel = NSOpenPanel()
    panel.title = "Open hostd recording"
    panel.message =
      "Open an NDJSON recording (up to 8 MiB / 10,000 observations). This does not start a scanner."
    panel.canChooseDirectories = false
    panel.allowsMultipleSelection = false
    guard panel.runModal() == .OK, let url = panel.url else { return }
    loadingRecording = true
    Task {
      defer { loadingRecording = false }
      do {
        let loaded = try await Task.detached(priority: .userInitiated) { try Recording.load(url) }
          .value
        recording = loaded
        recordingLabel = "Recording: " + url.lastPathComponent
        isExample = false
        recordingRevision = UUID()
      } catch { self.error = error.localizedDescription }
    }
  }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
  var model: AppModel?
  func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
  func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
    guard let model else { return .terminateNow }
    Task {
      await model.shutdown()
      sender.reply(toApplicationShouldTerminate: true)
    }
    return .terminateLater
  }
}

@main
struct AirHoundApp: App {
  @NSApplicationDelegateAdaptor(AppDelegate.self) var delegate
  @StateObject private var model = AppModel()
  var body: some Scene {
    Window("AirHound", id: "main") {
      MainView(model: model).onAppear { delegate.model = model }
    }
    .defaultSize(width: 1280, height: 820)
    .commands {
      CommandGroup(replacing: .newItem) {
        Button("Open Recording…") { model.openRecording() }.keyboardShortcut("o")
          .disabled(model.loadingRecording)
        Button("Open Example Recording") { model.loadExample() }
          .disabled(model.loadingRecording)
        Divider()
        Button("Devices…") { model.showDevices = true }.keyboardShortcut(
          "d", modifiers: [.command, .shift])
      }
      SidebarCommands()
    }
  }
}
