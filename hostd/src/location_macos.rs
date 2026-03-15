//! CoreLocation GPS provider for macOS.
//!
//! Uses `CLLocationManager` with continuous updates to keep a fresh GPS fix.
//! `start()` must be called once at startup; `get_current_location()` reads
//! the cached position on each scan cycle.
//!
//! Requires running from within an `.app` bundle whose `Info.plist` includes
//! `NSLocationWhenInUseUsageDescription` — otherwise macOS won't show the
//! authorization prompt. See `hostd/macos/Info.plist` and `just bundle-hostd`.

use std::sync::{Mutex, OnceLock};

use objc2::rc::Retained;
use objc2_core_location::{CLAuthorizationStatus, CLLocationAccuracy, CLLocationManager};

use crate::wigle::Location;

// kCLLocationAccuracyBest = -1.0 per Apple docs (request best available accuracy)
const ACCURACY_BEST: CLLocationAccuracy = -1.0;

/// Wrapper to allow `Retained<CLLocationManager>` in a static.
/// Safety: CLLocationManager is only accessed from the main thread or via
/// `location()` which reads a continuously-updated cached property.
struct ManagerWrapper(Retained<CLLocationManager>);
unsafe impl Send for ManagerWrapper {}
unsafe impl Sync for ManagerWrapper {}

/// Shared CLLocationManager — must persist to keep receiving updates.
static MANAGER: OnceLock<Mutex<ManagerWrapper>> = OnceLock::new();

/// Initialize CoreLocation and start continuous location updates.
/// Call once at startup from the main thread.
pub fn start() {
    MANAGER.get_or_init(|| {
        let mgr = unsafe { CLLocationManager::new() };

        // Check current authorization and request if needed
        let status = unsafe { mgr.authorizationStatus() };
        match status {
            CLAuthorizationStatus::NotDetermined => {
                log::info!("Requesting Location Services authorization...");
                unsafe { mgr.requestWhenInUseAuthorization() };
                // macOS delivers the auth prompt asynchronously via the app
                // bundle's Info.plist. No run loop spin needed.
            }
            CLAuthorizationStatus::Denied | CLAuthorizationStatus::Restricted => {
                log::warn!(
                    "Location Services denied/restricted — enable for AirHound in \
                     System Settings → Privacy & Security → Location Services"
                );
            }
            _ => {
                log::info!("Location Services authorized");
            }
        }

        unsafe { mgr.setDesiredAccuracy(ACCURACY_BEST) };
        unsafe { mgr.startUpdatingLocation() };
        log::info!("CoreLocation started (continuous updates)");
        Mutex::new(ManagerWrapper(mgr))
    });
}

/// Get the current location from the continuously-updated cache.
/// Returns None if GPS hasn't resolved yet.
pub fn get_current_location() -> Option<Location> {
    let mutex = MANAGER.get()?;
    let guard = mutex.lock().ok()?;
    let loc = unsafe { guard.0.location() }?;
    let coord = unsafe { loc.coordinate() };
    let accuracy = unsafe { loc.horizontalAccuracy() };
    if accuracy < 0.0 {
        return None; // negative accuracy = invalid
    }
    Some(Location {
        latitude: coord.latitude,
        longitude: coord.longitude,
        accuracy,
        altitude: unsafe { loc.altitude() },
    })
}
