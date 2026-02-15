/// Buzzer driver using LEDC PWM.
///
/// Drives a passive buzzer at the board-configured frequency and GPIO pin.
/// The buzzer task waits for signals on `BUZZER_SIGNAL` and produces a short
/// beep when a surveillance device match is detected.
use core::sync::atomic::Ordering;

use embassy_time::{Duration, Timer};
use esp_hal::gpio::DriveMode;
use esp_hal::ledc::channel::{self, ChannelIFace};
use esp_hal::ledc::timer::{self, config::Duty, TimerIFace};
use esp_hal::ledc::{Ledc, LowSpeed};
use esp_hal::time::Rate;

use crate::board;

#[cfg(feature = "m5stickc")]
type BuzzerPin = esp_hal::peripherals::GPIO2<'static>;
#[cfg(feature = "xiao")]
type BuzzerPin = esp_hal::peripherals::GPIO3<'static>;

#[embassy_executor::task]
pub async fn buzzer_task(
    ledc_peripheral: esp_hal::peripherals::LEDC<'static>,
    buzzer_pin: BuzzerPin,
) {
    let ledc = Ledc::new(ledc_peripheral);

    let mut lstimer0 = ledc.timer::<LowSpeed>(timer::Number::Timer0);
    lstimer0
        .configure(timer::config::Config {
            duty: Duty::Duty8Bit,
            clock_source: timer::LSClockSource::APBClk,
            frequency: Rate::from_hz(board::BUZZER_FREQ_HZ),
        })
        .unwrap();

    let mut channel0 = ledc.channel(channel::Number::Channel0, buzzer_pin);
    channel0
        .configure(channel::config::Config {
            timer: &lstimer0,
            duty_pct: 0,
            drive_mode: DriveMode::PushPull,
        })
        .unwrap();

    log::info!("Buzzer ready on GPIO{}", board::BUZZER_PIN);

    let rx = crate::BUZZER_SIGNAL.receiver();

    loop {
        rx.receive().await;

        if !crate::BUZZER_ENABLED.load(Ordering::Relaxed) {
            continue;
        }

        // 50% duty = loudest for passive buzzer
        channel0.set_duty(50).unwrap();
        Timer::after(Duration::from_millis(board::BUZZER_BEEP_MS)).await;
        channel0.set_duty(0).unwrap();
    }
}
