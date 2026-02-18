//! Buzzer driver using ESP-IDF LEDC PWM.
//!
//! Drives a passive buzzer at the board-configured frequency.
//! Receives signals via mpsc channel and produces short beeps.

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

use esp_idf_svc::hal::gpio::OutputPin;
use esp_idf_svc::hal::ledc::{config::TimerConfig, LedcChannel, LedcDriver, LedcTimer, LedcTimerDriver, Resolution};
use esp_idf_svc::hal::peripheral::Peripheral;
use esp_idf_svc::hal::units::Hertz;

use airhound::board;

pub fn buzzer_thread<T, C>(
    buzzer_rx: mpsc::Receiver<()>,
    timer: impl Peripheral<P = T> + 'static,
    channel: impl Peripheral<P = C> + 'static,
    pin: impl Peripheral<P = impl OutputPin> + 'static,
)
where
    T: LedcTimer + 'static,
    C: LedcChannel<SpeedMode = T::SpeedMode> + 'static,
 {
    let timer_config = TimerConfig::new()
        .frequency(Hertz(board::BUZZER_FREQ_HZ))
        .resolution(Resolution::Bits8);

    let timer_driver = match LedcTimerDriver::new(timer, &timer_config) {
        Ok(t) => t,
        Err(e) => {
            log::error!("LEDC timer init failed: {:?}", e);
            return;
        }
    };

    let mut channel_driver = match LedcDriver::new(channel, &timer_driver, pin) {
        Ok(c) => c,
        Err(e) => {
            log::error!("LEDC channel init failed: {:?}", e);
            return;
        }
    };

    let max_duty = channel_driver.get_max_duty();
    log::info!("Buzzer ready on GPIO{}", board::BUZZER_PIN);

    // Boot beep
    std::thread::sleep(Duration::from_millis(50));
    channel_driver.set_duty(max_duty / 2).ok();
    std::thread::sleep(Duration::from_millis(200));
    channel_driver.set_duty(0).ok();

    loop {
        match buzzer_rx.recv() {
            Ok(()) => {}
            Err(_) => break,
        }

        if !crate::BUZZER_ENABLED.load(Ordering::Relaxed) {
            continue;
        }

        channel_driver.set_duty(max_duty / 2).ok();
        std::thread::sleep(Duration::from_millis(board::BUZZER_BEEP_MS));
        channel_driver.set_duty(0).ok();
    }
}
