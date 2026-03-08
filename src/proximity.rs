/// RSSI-based proximity logic for distance estimation and buzzer cadence.
///
/// Pure math functions with no hardware or OS dependencies.

/// Minimum beep interval in milliseconds (closest proximity).
const MIN_INTERVAL: u16 = 50;

/// Maximum beep interval in milliseconds (farthest detectable proximity).
const MAX_INTERVAL: u16 = 2000;

/// Map an RSSI value to a beep interval in milliseconds.
///
/// `min_rssi` is the weakest signal to consider (e.g., -90 dBm).
/// `max_rssi` is the strongest signal expected (e.g., -30 dBm).
///
/// Returns `MIN_INTERVAL` (50ms) at `max_rssi` (closest),
/// `MAX_INTERVAL` (2000ms) at `min_rssi` (farthest).
///
/// If `min_rssi >= max_rssi` (degenerate range), returns `MAX_INTERVAL`.
/// Values outside the range are clamped.
pub fn rssi_to_beep_interval(rssi: i8, min_rssi: i8, max_rssi: i8) -> u16 {
    if min_rssi >= max_rssi {
        return MAX_INTERVAL;
    }

    let clamped = rssi.clamp(min_rssi, max_rssi);

    // Normalize to 0.0 (min_rssi) .. 1.0 (max_rssi)
    let t = (clamped - min_rssi) as f32 / (max_rssi - min_rssi) as f32;

    // Interpolate: 1.0 (closest) → MIN_INTERVAL, 0.0 (farthest) → MAX_INTERVAL
    let interval = MAX_INTERVAL as f32 - t * (MAX_INTERVAL - MIN_INTERVAL) as f32;
    interval as u16
}

/// Estimate distance in meters from RSSI using the log-distance path loss model.
///
/// Formula: d = 10^((tx_power - rssi) / (10 * n))
/// where n=2.0 (free-space path loss exponent).
///
/// `tx_power` is the expected RSSI at 1 meter (typically -40 to -60 dBm).
///
/// Returns estimated distance in meters. Minimum 0.1m.
pub fn rssi_to_distance_estimate(rssi: i8, tx_power: i8) -> f32 {
    let exponent = (tx_power as f32 - rssi as f32) / 20.0; // 10 * n where n=2.0
    let distance = libm::powf(10.0, exponent);
    if distance < 0.1 {
        0.1
    } else {
        distance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── rssi_to_beep_interval tests ─────────────────────────────────

    #[test]
    fn interval_at_max_rssi() {
        assert_eq!(rssi_to_beep_interval(-30, -90, -30), MIN_INTERVAL);
    }

    #[test]
    fn interval_at_min_rssi() {
        assert_eq!(rssi_to_beep_interval(-90, -90, -30), MAX_INTERVAL);
    }

    #[test]
    fn interval_at_midpoint() {
        let interval = rssi_to_beep_interval(-60, -90, -30);
        // Midpoint: t=0.5 → 2000 - 0.5*1950 = 1025
        assert_eq!(interval, 1025);
    }

    #[test]
    fn interval_clamps_above_max() {
        // RSSI stronger than max_rssi → clamped to max_rssi → MIN_INTERVAL
        assert_eq!(rssi_to_beep_interval(-20, -90, -30), MIN_INTERVAL);
    }

    #[test]
    fn interval_clamps_below_min() {
        // RSSI weaker than min_rssi → clamped to min_rssi → MAX_INTERVAL
        assert_eq!(rssi_to_beep_interval(-100, -90, -30), MAX_INTERVAL);
    }

    #[test]
    fn interval_degenerate_range_equal() {
        assert_eq!(rssi_to_beep_interval(-50, -50, -50), MAX_INTERVAL);
    }

    #[test]
    fn interval_degenerate_range_inverted() {
        assert_eq!(rssi_to_beep_interval(-50, -30, -90), MAX_INTERVAL);
    }

    // ── rssi_to_distance_estimate tests ─────────────────────────────

    #[test]
    fn distance_at_tx_power() {
        // rssi == tx_power → exponent=0 → 10^0 = 1.0m
        let d = rssi_to_distance_estimate(-40, -40);
        assert!((d - 1.0).abs() < 0.01, "expected ~1.0m, got {d}");
    }

    #[test]
    fn distance_weaker_signal() {
        // rssi weaker than tx_power → distance > 1.0m
        let d = rssi_to_distance_estimate(-60, -40);
        assert!(d > 1.0, "expected > 1.0m, got {d}");
        // 10^((−40 − −60)/20) = 10^1 = 10.0m
        assert!((d - 10.0).abs() < 0.1, "expected ~10.0m, got {d}");
    }

    #[test]
    fn distance_stronger_signal() {
        // rssi stronger than tx_power → distance < 1.0m
        let d = rssi_to_distance_estimate(-30, -40);
        assert!(d < 1.0, "expected < 1.0m, got {d}");
    }

    #[test]
    fn distance_minimum_clamped() {
        // Very strong signal → distance very small → clamped to 0.1
        let d = rssi_to_distance_estimate(-10, -40);
        assert!(d >= 0.1, "expected >= 0.1m, got {d}");
    }

    #[test]
    fn distance_very_weak_signal() {
        // Very weak signal → large distance
        let d = rssi_to_distance_estimate(-100, -40);
        assert!(d > 100.0, "expected > 100m for very weak signal, got {d}");
    }
}
