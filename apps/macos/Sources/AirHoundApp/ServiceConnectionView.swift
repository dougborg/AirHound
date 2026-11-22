import SwiftUI

struct ServiceConnectionView: View {
  @ObservedObject var session: SessionModel
  @ObservedObject var app: AppModel
  @State private var connectionSettings = false
  var body: some View {
    VStack(spacing: 0) {
      Divider()
      HStack {
        Button {
          connectionSettings.toggle()
        } label: {
          HStack(spacing: 6) {
            Circle().fill(connectionColor).frame(width: 8, height: 8)
              .accessibilityHidden(true)
            Text(connectionStatus)
          }
          .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help("Configure the backend service connection")
        .accessibilityLabel("Service: \(connectionStatus). Connection settings")
        .accessibilityIdentifier("serviceConnectionStatus")
        .popover(isPresented: $connectionSettings, arrowEdge: .bottom) {
          serviceConfiguration
        }
        Spacer()
      }
      .font(.callout)
      .padding(.horizontal, 16)
      .padding(.vertical, 8)
      .background(.bar)
    }
  }
  private var connectionStatus: String {
    return session.connected ? "Service connected" : "Service disconnected"
  }
  private var connectionColor: Color {
    session.connected ? .green : .red
  }
  private var serviceConfiguration: some View {
    VStack(alignment: .leading, spacing: 12) {
      Text("Service connection").font(.headline)
      Text(
        session.connected
          ? "Backend: \(session.backendBuild ?? "Version unreported")" : "Backend not connected"
      )
      .font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
      TextField("Socket", text: $session.socketPath)
        .disabled(session.connected || session.busy)
      Text(app.executable?.path ?? "No backend selected")
        .font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
      HStack {
        Button("Connect") { session.connect(executable: app.executable) }
          .disabled(session.connected)
        Button("Start Local Service") { session.startService(executable: app.executable) }
          .disabled(session.connected)
      }.disabled(session.isPerformingUserAction || session.userActionQueued)
      HStack {
        Button("Choose Backend…") { app.chooseBackend() }
        if session.connected {
          Button("Disconnect Client") { session.disconnect() }
        }
      }.disabled(session.isPerformingUserAction || session.userActionQueued)
      Text(session.message).font(.callout).foregroundStyle(.secondary)
      if let error = session.error {
        Text(error).font(.callout).foregroundStyle(.red).textSelection(.enabled)
      }
    }
    .padding(20)
    .frame(width: 440)
  }
}
