//! Allocation-free, active-low button debouncing shared with host tests.
//! One event per stable press; holding a button never repeats an action.
pub struct Button {
    raw_pressed: bool,
    stable_pressed: bool,
    changed_at_ms: u64,
    armed: bool,
}
impl Button {
    /// A button held during boot must be released before it can trigger.
    pub const fn new(pressed: bool, now_ms: u64) -> Self {
        Self {
            raw_pressed: pressed,
            stable_pressed: pressed,
            changed_at_ms: now_ms,
            armed: !pressed,
        }
    }
    pub fn update(&mut self, pressed: bool, now_ms: u64) -> bool {
        if pressed != self.raw_pressed {
            self.raw_pressed = pressed;
            self.changed_at_ms = now_ms;
        }
        if now_ms.saturating_sub(self.changed_at_ms) < 40 || pressed == self.stable_pressed {
            return false;
        }
        self.stable_pressed = pressed;
        if !pressed {
            self.armed = true;
            return false;
        }
        let event = self.armed;
        self.armed = false;
        event
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    #[test]
    fn bounce_hold_and_release_produce_one_event_per_press() {
        let mut b = Button::new(false, 0);
        for (pressed, ms) in [(true, 10), (false, 20), (true, 30), (true, 69)] {
            assert!(!b.update(pressed, ms));
        }
        assert!(b.update(true, 70));
        assert!(!b.update(true, 10_000));
        assert!(!b.update(false, 10_020));
        assert!(!b.update(true, 10_030)); // release bounce cannot re-arm
        assert!(!b.update(true, 10_070));
        assert!(!b.update(false, 10_080));
        assert!(!b.update(false, 10_120));
        assert!(!b.update(true, 10_140));
        assert!(b.update(true, 10_180));
    }
    #[test]
    fn boot_held_and_short_pulses_are_ignored() {
        let mut b = Button::new(true, 0);
        assert!(!b.update(true, 2000));
        assert!(!b.update(false, 2100));
        assert!(!b.update(false, 2140));
        assert!(!b.update(true, 2200));
        assert!(!b.update(false, 2220));
        assert!(!b.update(false, 2260));
        assert!(!b.update(true, 2300));
        assert!(b.update(true, 2340));
    }
    #[test]
    fn simultaneous_buttons_are_independent() {
        let mut a = Button::new(false, 0);
        let mut b = Button::new(false, 0);
        assert!(!a.update(true, 10));
        assert!(!b.update(true, 10));
        assert!(a.update(true, 50));
        assert!(b.update(true, 50));
    }
}
