/// Display driver for M5StickC Plus2 (ST7789V2, 135x240, SPI).
///
/// Renders a status screen showing AirHound state, match counts, and
/// recent detections. Refreshes every 500ms via direct SPI writes (no
/// framebuffer — the 64KB required would exceed ESP32's heap).

use core::fmt::Write;
use core::sync::atomic::Ordering;

use embedded_graphics::mono_font::ascii::FONT_6X10;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::Text;
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

use embassy_time::{Duration, Instant, Timer};

use crate::board;
use crate::protocol::VERSION;

/// Landscape width after 90-degree rotation
const W: i32 = 240;

/// Colors
const BG: Rgb565 = Rgb565::BLACK;
const HEADER_BG: Rgb565 = Rgb565::new(2, 4, 12);
const TEXT: Rgb565 = Rgb565::WHITE;
const ACCENT: Rgb565 = Rgb565::new(0, 50, 0);
const DIM: Rgb565 = Rgb565::new(12, 24, 12);

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
    // Turn on backlight
    let _bl = Output::new(bl_pin, Level::High, OutputConfig::default());

    // Configure SPI bus (40 MHz, Mode 0)
    let spi_config = SpiConfig::default()
        .with_frequency(Rate::from_mhz(board::DISPLAY_SPI_FREQ_MHZ))
        .with_mode(Mode::_0);
    let spi = Spi::new(spi2, spi_config)
        .unwrap()
        .with_sck(clk)
        .with_mosi(mosi);

    // Wrap SpiBus + CS into SpiDevice
    let cs = Output::new(cs_pin, Level::High, OutputConfig::default());
    let spi_device = ExclusiveDevice::new_no_delay(spi, cs).unwrap();

    // Create mipidsi SPI interface (buffer on task stack)
    let dc = Output::new(dc_pin, Level::Low, OutputConfig::default());
    let mut buffer = [0u8; 512];
    let di = SpiInterface::new(spi_device, dc, &mut buffer);

    // Build display: ST7789V2, 135x240, landscape, inverted colors
    let rst = Output::new(rst_pin, Level::High, OutputConfig::default());
    let mut delay = Delay::new();
    let mut display = Builder::new(ST7789, di)
        .display_size(135, 240)
        .display_offset(52, 40)
        .invert_colors(ColorInversion::Inverted)
        .color_order(ColorOrder::Bgr)
        .orientation(Orientation::new().rotate(Rotation::Deg90))
        .reset_pin(rst)
        .init(&mut delay)
        .unwrap();

    log::info!("Display initialized (240x135 landscape)");

    // Splash screen
    draw_splash(&mut display);
    Timer::after(Duration::from_secs(2)).await;

    // Status loop — refresh every 500ms
    loop {
        draw_status(&mut display);
        Timer::after(Duration::from_millis(500)).await;
    }
}

fn draw_splash(display: &mut impl DrawTarget<Color = Rgb565>) {
    let _ = display.clear(BG);

    let style = MonoTextStyle::new(&FONT_6X10, TEXT);
    let accent = MonoTextStyle::new(&FONT_6X10, ACCENT);

    // Center "AIRHOUND" (8 chars × 6px = 48px)
    let _ = Text::new("AIRHOUND", Point::new((W - 48) / 2, 55), style).draw(display);

    // Version below
    let mut ver = heapless::String::<20>::new();
    let _ = write!(ver, "v{}", VERSION);
    let vw = ver.len() as i32 * 6;
    let _ = Text::new(&ver, Point::new((W - vw) / 2, 70), accent).draw(display);

    // Tagline
    let tag = "RF Companion";
    let tw = tag.len() as i32 * 6;
    let _ = Text::new(tag, Point::new((W - tw) / 2, 95), MonoTextStyle::new(&FONT_6X10, DIM)).draw(display);
}

fn draw_status(display: &mut impl DrawTarget<Color = Rgb565>) {
    let _ = display.clear(BG);

    let white = MonoTextStyle::new(&FONT_6X10, TEXT);
    let green = MonoTextStyle::new(&FONT_6X10, Rgb565::GREEN);
    let dim = MonoTextStyle::new(&FONT_6X10, DIM);

    // ── Header bar ──────────────────────────────────────────────────────
    let _ = Rectangle::new(Point::zero(), Size::new(W as u32, 14))
        .into_styled(PrimitiveStyle::with_fill(HEADER_BG))
        .draw(display);

    let mut header = heapless::String::<40>::new();
    let _ = write!(header, " AIRHOUND v{}", VERSION);
    let _ = Text::new(&header, Point::new(0, 10), white).draw(display);

    let scanning = crate::SCANNING.load(Ordering::Relaxed);
    let indicator = if scanning { "[SCAN]" } else { "[STOP]" };
    let indicator_style = if scanning { green } else { MonoTextStyle::new(&FONT_6X10, Rgb565::RED) };
    let _ = Text::new(indicator, Point::new(W - 6 * indicator.len() as i32 - 2, 10), indicator_style).draw(display);

    // ── Match counts ────────────────────────────────────────────────────
    let wifi_count = crate::WIFI_MATCH_COUNT.load(Ordering::Relaxed);
    let ble_count = crate::BLE_MATCH_COUNT.load(Ordering::Relaxed);

    let mut line = heapless::String::<40>::new();
    let _ = write!(line, " WiFi: {}    BLE: {}", wifi_count, ble_count);
    let _ = Text::new(&line, Point::new(0, 32), white).draw(display);

    // ── Last match ──────────────────────────────────────────────────────
    let last = critical_section::with(|cs| crate::LAST_MATCH.borrow(cs).borrow().clone());
    if !last.is_empty() {
        let mut last_line = heapless::String::<40>::new();
        let _ = write!(last_line, " Last: {}", last);
        let _ = Text::new(&last_line, Point::new(0, 48), green).draw(display);
    } else {
        let _ = Text::new(" Last: ---", Point::new(0, 48), dim).draw(display);
    }

    // ── Divider ─────────────────────────────────────────────────────────
    let _ = Rectangle::new(Point::new(0, 58), Size::new(W as u32, 1))
        .into_styled(PrimitiveStyle::with_fill(DIM))
        .draw(display);

    // ── Status info ─────────────────────────────────────────────────────
    let ble_clients = crate::BLE_CLIENTS.load(Ordering::Relaxed);
    let uptime_secs = (Instant::now().as_millis() / 1000) as u32;
    let hours = uptime_secs / 3600;
    let mins = (uptime_secs % 3600) / 60;
    let secs = uptime_secs % 60;

    let mut status1 = heapless::String::<40>::new();
    let _ = write!(status1, " BLE: {} client{}  Up: {:02}:{:02}:{:02}",
        ble_clients, if ble_clients == 1 { "" } else { "s" },
        hours, mins, secs);
    let _ = Text::new(&status1, Point::new(0, 76), dim).draw(display);

    let heap_free = esp_alloc::HEAP.free() as u32;
    let heap_k = heap_free / 1024;

    let mut status2 = heapless::String::<40>::new();
    let buzzer = if crate::BUZZER_ENABLED.load(Ordering::Relaxed) { "ON" } else { "OFF" };
    let _ = write!(status2, " Heap: {}K free  Buzzer: {}", heap_k, buzzer);
    let _ = Text::new(&status2, Point::new(0, 92), dim).draw(display);
}
