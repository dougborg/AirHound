//! CoreLocation GPS provider for macOS.
//!
//! Uses `CLLocationManager` with continuous updates to keep a fresh GPS fix.
//! `start()` must be called once at startup; `get_current_location()` reads
//! the cached position on each scan cycle.
//!
//! Requires running from within an `.app` bundle whose `Info.plist` includes
//! `NSLocationWhenInUseUsageDescription` — otherwise macOS won't show the
//! authorization prompt. See `hostd/macos/Info.plist` and `just bundle-hostd`.

use std::sync::{Mutex, Once};
use std::time::Duration;

use objc2::rc::autoreleasepool;
use objc2_core_location::{CLAuthorizationStatus, CLLocationAccuracy, CLLocationManager};
use objc2_foundation::{NSDate, NSRunLoop};

use crate::wigle::Location;

// kCLLocationAccuracyBest = -1.0 per Apple docs (request best available accuracy)
const ACCURACY_BEST: CLLocationAccuracy = -1.0;

// Only plain Rust snapshots cross thread boundaries. The Objective-C manager
// and its run loop remain on the thread that created them.
static LOCATION: Mutex<Option<Location>> = Mutex::new(None);
static START: Once = Once::new();

/// Start a location thread with the active run loop required by CoreLocation.
pub fn start() {
    START.call_once(|| {
        std::thread::Builder::new()
            .name("airhound-location".into())
            .spawn(location_loop)
            .expect("failed to start location thread");
    });
}

fn location_loop() {
    let mgr = unsafe { CLLocationManager::new() };

    // Check current authorization and request if needed
    let status = unsafe { mgr.authorizationStatus() };
    match status {
        CLAuthorizationStatus::NotDetermined => {
            log::info!("Requesting Location Services authorization...");
            unsafe { mgr.requestWhenInUseAuthorization() };
            // Authorization is delivered asynchronously through this
            // thread's run loop; the app bundle supplies usage descriptions.
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
    let run_loop = NSRunLoop::currentRunLoop();
    loop {
        autoreleasepool(|_| {
            run_loop.runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(1.0));
            let snapshot = read_location(&mgr);
            *LOCATION.lock().unwrap_or_else(|e| e.into_inner()) = snapshot;
        });
        // A run loop with no sources can return immediately (e.g. permission
        // denied); avoid a busy loop in that case.
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Read a thread-safe copy of the latest fix; no Objective-C objects escape.
pub fn get_current_location() -> Option<Location> {
    *LOCATION.lock().unwrap_or_else(|e| e.into_inner())
}

fn read_location(manager: &CLLocationManager) -> Option<Location> {
    let loc = unsafe { manager.location() }?;
    // CoreLocation may initially return a cached fix from an earlier session.
    if unsafe { loc.timestamp() }.timeIntervalSinceNow().abs() > 30.0 {
        return None;
    }
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
