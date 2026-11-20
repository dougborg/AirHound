//! CoreLocation GPS provider for macOS.
//!
//! Uses `CLLocationManager` with continuous updates to keep a fresh GPS fix.
//! `start()` must be called once at startup; `get_current_location()` reads
//! the cached position on each scan cycle.
//!
//! Requires running from within an `.app` bundle whose `Info.plist` includes
//! `NSLocationWhenInUseUsageDescription` — otherwise macOS won't show the
//! authorization prompt. See `hostd/macos/Info.plist` and `just bundle-hostd`.

use std::sync::atomic::{AtomicU8, Ordering};
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
static AUTHORIZATION: AtomicU8 = AtomicU8::new(0);

/// Read only the thread-owned manager's current authorization snapshot.
pub fn permission_blocked() -> bool {
    AUTHORIZATION.load(Ordering::Relaxed) == 2
}
fn authorization_state(status: CLAuthorizationStatus, services_enabled: bool) -> u8 {
    let allowed = services_enabled
        && matches!(
            status,
            CLAuthorizationStatus::AuthorizedAlways | CLAuthorizationStatus::AuthorizedWhenInUse
        );
    let blocked = !services_enabled
        || matches!(
            status,
            CLAuthorizationStatus::Denied | CLAuthorizationStatus::Restricted
        );
    if allowed {
        1
    } else if blocked {
        2
    } else {
        0
    }
}
fn refresh_authorization(status: CLAuthorizationStatus, services_enabled: bool) -> bool {
    let state = authorization_state(status, services_enabled);
    AUTHORIZATION.store(state, Ordering::Relaxed);
    state == 1
}

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
            let allowed = refresh_authorization(unsafe { mgr.authorizationStatus() }, unsafe {
                CLLocationManager::locationServicesEnabled_class()
            });
            let snapshot = if allowed {
                unsafe { mgr.location() }.as_deref().and_then(read_location)
            } else {
                None
            };
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

fn read_location(loc: &objc2_core_location::CLLocation) -> Option<Location> {
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
        accuracy: Some(accuracy),
        altitude: (unsafe { loc.verticalAccuracy() } >= 0.0).then(|| unsafe { loc.altitude() }),
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use objc2::AllocAnyThread;
    use objc2_core_location::{CLLocation, CLLocationCoordinate2D};

    #[test]
    fn authorization_distinguishes_pending_denied_and_granted_without_cached_fixes() {
        for (status, expected) in [
            (CLAuthorizationStatus::NotDetermined, 0),
            (CLAuthorizationStatus::Denied, 2),
            (CLAuthorizationStatus::Restricted, 2),
            (CLAuthorizationStatus::AuthorizedAlways, 1),
            (CLAuthorizationStatus::AuthorizedWhenInUse, 1),
        ] {
            assert_eq!(authorization_state(status, true), expected);
            assert_eq!(authorization_state(status, false), 2);
        }
    }

    #[test]
    fn fresh_fixes_preserve_coordinates_and_stale_or_invalid_fixes_are_rejected() {
        for (age, accuracy, valid) in [
            (0., 5., true),
            (-60., 5., false),
            (60., 5., false),
            (0., -1., false),
        ] {
            let fix = unsafe {
                CLLocation::initWithCoordinate_altitude_horizontalAccuracy_verticalAccuracy_timestamp(
                    CLLocation::alloc(), CLLocationCoordinate2D { latitude: 40., longitude: -105. },
                    1600., accuracy, 10., &NSDate::dateWithTimeIntervalSinceNow(age))
            };
            let result = read_location(&fix);
            assert_eq!(result.is_some(), valid);
            if let Some(location) = result {
                assert_eq!(location.latitude, 40.);
                assert_eq!(location.longitude, -105.);
                assert_eq!(location.altitude, Some(1600.));
                assert_eq!(location.accuracy, Some(5.));
            }
        }
    }
}
