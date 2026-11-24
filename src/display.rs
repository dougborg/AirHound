/// Display driver for M5StickC Plus2 (ST7789V2, 135x240, SPI).
///
/// Renders screens via direct SPI writes (no framebuffer — the 64KB
/// required would exceed ESP32's heap). Uses the shared pure renderer
/// to lay out text rows flicker-free: each row is padded to full display
/// width and drawn with an explicit `background_color`, so every pixel
/// is overwritten in a single pass with no intermediate blank frame.
use core::sync::atomic::Ordering;

use airhound::display_render::{self, Status, H, W};
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::spi::Mode;
use esp_hal::time::Rate;
use mipidsi::interface::SpiInterface;
use mipidsi::models::ST7789;
use mipidsi::options::{ColorInversion, ColorOrder, Orientation, Rotation};
use mipidsi::Builder;

use static_cell::StaticCell;

use embassy_time::{Duration, Instant, Timer};

use crate::board;

fn status_snapshot() -> Status {
    let details = crate::UI_DETAILS.load(Ordering::Relaxed);
    Status {
        scanning: crate::SCANNING.load(Ordering::Relaxed),
        details,
        counts: crate::memory::counts(),
        internal_free: if details {
            crate::memory::internal_free()
        } else {
            0
        },
        external_free: if details {
            crate::memory::external_free()
        } else {
            0
        },
        wifi_packets: crate::WIFI_RX_COUNT.load(Ordering::Relaxed),
        wifi_drops: crate::WIFI_QUEUE_DROPS.load(Ordering::Relaxed),
        last_match: if details {
            heapless::String::new()
        } else {
            critical_section::with(|cs| crate::LAST_MATCH.borrow(cs).borrow().clone())
        },
        clients: u32::from(crate::BLE_CLIENTS.load(Ordering::Relaxed)),
        uptime_seconds: (Instant::now().as_millis() / 1000) as u32,
        sound: crate::BUZZER_ENABLED.load(Ordering::Relaxed),
    }
}

// ── Display task (hardware init + render loop) ────────────────────────

#[embassy_executor::task]
pub async fn display_task(
    spi2: esp_hal::peripherals::SPI2<'static>,
    mosi: esp_hal::peripherals::GPIO15<'static>,
    clk: esp_hal::peripherals::GPIO13<'static>,
    cs_pin: esp_hal::peripherals::GPIO5<'static>,
    dc_pin: esp_hal::peripherals::GPIO14<'static>,
    rst_pin: esp_hal::peripherals::GPIO12<'static>,
    bl_pin: esp_hal::peripherals::GPIO27<'static>,
) {
    log::info!("Display task starting");

    // Manual hardware reset before anything else
    let mut rst_out = Output::new(rst_pin, Level::High, OutputConfig::default());
    let delay = Delay::new();
    rst_out.set_low();
    delay.delay_millis(20);
    rst_out.set_high();
    delay.delay_millis(120);
    log::info!("Display RST toggled");

    // Configure SPI bus (40 MHz, Mode 0)
    let spi_config = SpiConfig::default()
        .with_frequency(Rate::from_mhz(board::DISPLAY_SPI_FREQ_MHZ))
        .with_mode(Mode::_0);
    let spi = match Spi::new(spi2, spi_config) {
        Ok(spi) => spi.with_sck(clk).with_mosi(mosi),
        Err(e) => {
            log::error!("SPI init failed: {:?}", e);
            crate::DISPLAY_READY.signal(false);
            return;
        }
    };
    log::info!("SPI bus configured");

    // Wrap SpiBus + CS into SpiDevice
    let cs = Output::new(cs_pin, Level::High, OutputConfig::default());
    let spi_device = ExclusiveDevice::new_no_delay(spi, cs).unwrap();

    // Create mipidsi SPI interface (buffer in static to avoid stack overflow)
    let dc = Output::new(dc_pin, Level::Low, OutputConfig::default());
    static SPI_BUF: StaticCell<[u8; 512]> = StaticCell::new();
    let buffer = SPI_BUF.init([0u8; 512]);
    let di = SpiInterface::new(spi_device, dc, buffer);

    // Build display: ST7789V2, 135x240, landscape, inverted colors.
    // Hardware reset was done manually above, so no reset_pin here.
    let mut delay2 = Delay::new();
    let mut display = match Builder::new(ST7789, di)
        .display_size(135, 240)
        .display_offset(52, 40)
        .invert_colors(ColorInversion::Inverted)
        .color_order(ColorOrder::Bgr)
        .orientation(Orientation::new().rotate(Rotation::Deg90))
        .init(&mut delay2)
    {
        Ok(d) => d,
        Err(e) => {
            log::error!("Display init failed: {:?}", e);
            crate::DISPLAY_READY.signal(false);
            return;
        }
    };
    log::info!("Display initialized ({}x{} landscape)", W, H);

    // Turn on backlight AFTER display init (active high on M5StickC Plus2)
    let _bl = Output::new(bl_pin, Level::High, OutputConfig::default());
    log::info!("Backlight on");

    // Splash screen
    display_render::draw_splash(&mut display);
    Timer::after(Duration::from_secs(2)).await;

    // Prepare for status loop: clear splash, paint header bg once.
    // The header text draw covers the middle 10px each frame via
    // background_color, but the 4px row-edge gap needs a one-time fill.
    display_render::prepare_status(&mut display);

    display_render::draw_status(&mut display, &status_snapshot());
    crate::DISPLAY_READY.signal(true);

    loop {
        Timer::after(Duration::from_millis(500)).await;
        // Snapshot/locks and each complete SPI row return before every yield.
        display_render::draw_status_cooperative(&mut display, &status_snapshot()).await;
    }
}
