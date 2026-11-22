import AirHoundCore
import SwiftUI

/// Presentation of the current owner's reports. No discovery guesses or freshness timers.
struct LocationProviderStatus: View {
  let status: JSONValue
  var body: some View {
    VStack(alignment: .leading, spacing: 4) {
      Text("GPS location").font(.headline)
      Text(stateLabel)
      if let device = status["device"]?.string {
        Text(device).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
      }
      if let daemon = status["daemon"], let version = daemon["version"]?.string {
        Text("gpsd \(version) · host software").font(.caption).textSelection(.enabled)
      }
      Text("Receiver/rig position · GNSS receiver firmware unreported")
        .font(.caption).foregroundStyle(.secondary)
    }
  }
  private var stateLabel: String {
    switch status["state"]?.string {
    case "fix": return "Fix available"
    case "connecting": return "Connecting to GPSD"
    case "waiting_for_fix", "no_fix": return "Waiting for a satellite fix"
    case "stale": return "Fix expired"
    case "disconnected", "device_disconnected": return "GPS disconnected"
    case "managed_starting": return "Starting GPSD"
    case "managed_restarting": return "Restarting GPSD"
    case "managed_device_busy": return "GPS device is in use"
    case "managed_endpoint_busy": return "GPS service address is in use"
    case "managed_configuration_invalid": return "GPS helper configuration is invalid"
    case "managed_spawn_failed", "managed_start_failed": return "GPSD could not start"
    case "managed_connection_lost", "managed_child_exited": return "GPSD connection lost"
    case "managed_restart_limit_reached": return "GPSD stopped after repeated failures"
    case "stopped": return "GPSD stopped"
    default:
      return status["state"]?.string?.replacingOccurrences(of: "_", with: " ").capitalized
        ?? "Location status unreported"
    }
  }
}

struct CompanionIdentityDetails: View {
  let identity: JSONValue
  var body: some View {
    DisclosureGroup("Scanner software and firmware") {
      if let helper = identity["helper_build"] {
        Text("Helper package: \(helper["package_version"]?.string ?? "unreported")")
        Text(helper["display_version"]?.string ?? "Helper source version unreported")
          .foregroundStyle(.secondary)
      }
      if let firmware = identity["firmware"], firmware != .null {
        if let supplied = firmware["supplied_image"] {
          imageDetails("Supplied image file", supplied)
        }
        if firmware["load_state"]?.string == "uploaded_this_initialization",
          let uploaded = firmware["uploaded_image"], uploaded != .null
        {
          imageDetails("Image uploaded this initialization", uploaded)
        } else {
          Text("Running firmware image unverified").foregroundStyle(.secondary)
        }
      } else {
        Text("Current firmware image unreported").foregroundStyle(.secondary)
      }
    }.font(.caption).textSelection(.enabled)
  }
  private func imageDetails(_ title: String, _ value: JSONValue) -> some View {
    VStack(alignment: .leading, spacing: 3) {
      Text(title).fontWeight(.medium)
      Text("Header version: \(number(value["version"])) · build: \(number(value["build_version"]))")
      if let time = value["build_time"]?.string, !time.isEmpty { Text(time) }
      if let hash = value["sha256"]?.string {
        Text("SHA-256: \(hash)").font(.system(.caption, design: .monospaced))
      }
    }
  }
  private func number(_ value: JSONValue?) -> String {
    switch value {
    case .integer(let number): return String(number)
    case .unsigned(let number): return String(number)
    default: return "unreported"
    }
  }
}
