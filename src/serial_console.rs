//! M5 runtime UART ownership. No runtime log call waits on the UART.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use airhound::serial_output::{self, Fifo, Step};
use esp_hal::{uart::UartTx, Blocking};

use crate::{OUTPUT_CHANNEL, OUTPUT_DROPS, SERIALIZE_ERRORS};

static RUNTIME: AtomicBool = AtomicBool::new(false);
pub static PANICKED: AtomicBool = AtomicBool::new(false);
pub static LOG_DROPS: AtomicU32 = AtomicU32::new(0);
pub static FORMAT_ERRORS: AtomicU32 = AtomicU32::new(0);
pub static WRITE_ERRORS: AtomicU32 = AtomicU32::new(0);

struct Logger;

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) || PANICKED.load(Ordering::Relaxed) {
            return;
        }
        if !RUNTIME.load(Ordering::Acquire) {
            let printed = critical_section::with(|_| {
                if RUNTIME.load(Ordering::Relaxed) {
                    return false;
                }
                // Before the UART task takes ownership and before radio starts.
                esp_println::println!("{} - {}", record.level(), record.args());
                true
            });
            if printed {
                return;
            }
        }
        match serial_output::enqueue_diagnostic(record.level(), *record.args(), |line| {
            OUTPUT_CHANNEL.try_send(line).is_ok()
        }) {
            serial_output::Enqueue::Queued => {}
            serial_output::Enqueue::Full => {
                LOG_DROPS.fetch_add(1, Ordering::Relaxed);
                OUTPUT_DROPS.fetch_add(1, Ordering::Relaxed);
            }
            serial_output::Enqueue::FormatError => {
                FORMAT_ERRORS.fetch_add(1, Ordering::Relaxed);
                SERIALIZE_ERRORS.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn flush(&self) {}
}

pub fn init() {
    // Called once before the RTOS and radio tasks. Matches ESP_LOG=info in the
    // checked-in firmware configuration; no dynamically allocated filters.
    unsafe {
        log::set_logger_racy(&Logger).unwrap();
        log::set_max_level_racy(log::LevelFilter::Info);
    }
}

pub fn take_runtime_ownership() {
    // The application starts all runtime tasks on this executor. Startup waits
    // for OUTPUT_READY before starting either radio. No boot logger is in flight
    // when this task first runs.
    critical_section::with(|_| RUNTIME.store(true, Ordering::Release));
}

struct HardwareFifo<'a>(&'a mut UartTx<'static, Blocking>);

impl Fifo for HardwareFifo<'_> {
    type Error = esp_hal::uart::TxError;

    fn ready(&mut self) -> bool {
        self.0.write_ready()
    }

    fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
        self.0.write(bytes)
    }
}

pub fn step(
    uart: &mut UartTx<'static, Blocking>,
    line: &[u8],
    offset: &mut usize,
) -> Result<Step, ()> {
    critical_section::with(|_| {
        if PANICKED.load(Ordering::Relaxed) {
            return Err(());
        }
        // Public pinned HAL contract: write_ready=true means write will not
        // block. The sole writer and this CS exclude another writer/panic
        // handoff between readiness and the <=16-byte FIFO copy. No wait,
        // formatting, logging, flush, or await occurs under this lock.
        serial_output::step(&mut HardwareFifo(uart), line, offset).map_err(|_| ())
    })
}

/// Fatal ownership transfer used by esp-backtrace before it prints a newline.
/// A partial ordinary line is abandoned, never resumed after the backtrace.
#[no_mangle]
pub extern "Rust" fn custom_pre_backtrace() {
    // Deliberately never release: the panic handler never returns. This also
    // prevents a runtime writer on another core from continuing after handoff.
    let _token = unsafe { critical_section::acquire() };
    PANICKED.store(true, Ordering::Relaxed);
}
