//! Display driver for M5StickC Plus2 (ST7789V2, 135x240, SPI).
//!
//! ESP-IDF std version — uses esp-idf-svc SPI driver + mipidsi.
//! No StaticCell needed for SPI buffer (normal stack allocation is fine
//! since we're on a FreeRTOS thread with its own stack).

use std::sync::atomic::Ordering;
use std::time::Duration;

use embedded_graphics::mono_font::ascii::FONT_6X10;
use embedded_graphics::mono_font::{MonoTextStyle, MonoTextStyleBuilder};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::Text;
use esp_idf_svc::hal::delay::Delay;
use esp_idf_svc::hal::gpio::*;
use esp_idf_svc::hal::peripheral::Peripheral;
use esp_idf_svc::hal::spi::config::Config as SpiConfig;
use esp_idf_svc::hal::spi::config::DriverConfig as SpiDriverConfig;
use esp_idf_svc::hal::spi::{SpiDeviceDriver, SpiDriver};
use esp_idf_svc::hal::units::Hertz;
use mipidsi::interface::SpiInterface;
use mipidsi::models::ST7789;
use mipidsi::options::{ColorInversion, ColorOrder, Orientation, Rotation};
use mipidsi::Builder;

use airhound::board;
use airhound::protocol::VERSION;

// ── Display geometry ─────────────────────────────────────────────────

const W: i32 = 240;
const H: i32 = 135;
const ROW_H: i32 = 14;
const LINE_W: usize = (W / 6) as usize;

// ── Color palette ────────────────────────────────────────────────────

const BG: Rgb565 = Rgb565::BLACK;
const HEADER_BG: Rgb565 = Rgb565::new(2, 4, 12);
const FG: Rgb565 = Rgb565::WHITE;
const ACCENT: Rgb565 = Rgb565::new(0, 50, 0);
const DIM: Rgb565 = Rgb565::new(12, 24, 12);

// ── Screen renderer (same as no_std version) ─────────────────────────

struct Screen<'a, D> {
    display: &'a mut D,
    y: i32,
    buf: heapless::String<40>,
}

impl<'a, D: DrawTarget<Color = Rgb565>> Screen<'a, D> {
    fn new(display: &'a mut D) -> Self {
        Self {
            display,
            y: 0,
            buf: heapless::String::new(),
        }
    }

    fn clear(&mut self) {
        let _ = self.display.clear(BG);
        self.y = 0;
    }

    fn skip(&mut self, pixels: i32) {
        self.y += pixels;
    }

    fn fill_band(&mut self, height: i32, color: Rgb565) {
        let _ = Rectangle::new(Point::new(0, self.y), Size::new(W as u32, height as u32))
            .into_styled(PrimitiveStyle::with_fill(color))
            .draw(self.display);
    }

    fn row(&mut self, color: Rgb565, args: core::fmt::Arguments<'_>) {
        self.buf.clear();
        let _ = core::fmt::write(&mut self.buf, args);
        self.pad();
        self.emit(color, BG, 0);
        self.y += ROW_H;
    }

    fn centered(&mut self, color: Rgb565, args: core::fmt::Arguments<'_>) {
        self.buf.clear();
        let _ = core::fmt::write(&mut self.buf, args);
        let x = (W - self.buf.len() as i32 * 6) / 2;
        self.emit(color, BG, x);
        self.y += ROW_H;
    }

    fn header(
        &mut self,
        title_args: core::fmt::Arguments<'_>,
        indicator: &str,
        indicator_color: Rgb565,
    ) {
        self.buf.clear();
        let _ = core::fmt::write(&mut self.buf, title_args);
        self.emit(FG, HEADER_BG, 0);

        let x = W - indicator.len() as i32 * 6 - 2;
        let _ = Text::new(
            indicator,
            Point::new(x, self.y + 10),
            Self::text_style(indicator_color, HEADER_BG),
        )
        .draw(self.display);
        self.y += ROW_H;
    }

    fn divider(&mut self) {
        let _ = Rectangle::new(Point::new(0, self.y), Size::new(W as u32, 1))
            .into_styled(PrimitiveStyle::with_fill(DIM))
            .draw(self.display);
        self.y += 3;
    }

    fn pad(&mut self) {
        while self.buf.len() < LINE_W {
            let _ = self.buf.push(' ');
        }
    }

    fn emit(&mut self, fg: Rgb565, bg: Rgb565, x: i32) {
        let _ = Text::new(
            &self.buf,
            Point::new(x, self.y + 10),
            Self::text_style(fg, bg),
        )
        .draw(self.display);
    }

    fn text_style(fg: Rgb565, bg: Rgb565) -> MonoTextStyle<'static, Rgb565> {
        MonoTextStyleBuilder::new()
            .font(&FONT_6X10)
            .text_color(fg)
            .background_color(bg)
            .build()
    }
}

macro_rules! row {
    ($s:expr, $color:expr, $($arg:tt)*) => {
        $s.row($color, format_args!($($arg)*))
    };
}

macro_rules! centered {
    ($s:expr, $color:expr, $($arg:tt)*) => {
        $s.centered($color, format_args!($($arg)*))
    };
}

// ── Screen implementations ───────────────────────────────────────────

fn draw_splash(display: &mut impl DrawTarget<Color = Rgb565>) {
    let mut s = Screen::new(display);
    s.clear();
    s.skip(42);
    centered!(s, FG, "AIRHOUND");
    centered!(s, ACCENT, "v{}", VERSION);
    s.skip(12);
    centered!(s, DIM, "RF Companion (std)");
}

fn draw_status(display: &mut impl DrawTarget<Color = Rgb565>) {
    let mut s = Screen::new(display);

    let scanning = crate::SCANNING.load(Ordering::Relaxed);
    s.header(
        format_args!(" AIRHOUND v{}", VERSION),
        if scanning { "[SCAN]" } else { "[STOP]" },
        if scanning { Rgb565::GREEN } else { Rgb565::RED },
    );

    row!(
        s,
        FG,
        " WiFi: {}    BLE: {}",
        crate::WIFI_MATCH_COUNT.load(Ordering::Relaxed),
        crate::BLE_MATCH_COUNT.load(Ordering::Relaxed)
    );

    let last = crate::LAST_MATCH.lock().map(|s| s.clone()).unwrap_or_default();
    if !last.is_empty() {
        row!(s, Rgb565::GREEN, " Last: {}", last);
    } else {
        row!(s, DIM, " Last: ---");
    }

    s.divider();

    let clients = crate::BLE_CLIENTS.load(Ordering::Relaxed);
    let up = crate::uptime_secs();
    row!(
        s,
        DIM,
        " BLE: {} client{}  Up: {:02}:{:02}:{:02}",
        clients,
        if clients == 1 { "" } else { "s" },
        up / 3600,
        (up % 3600) / 60,
        up % 60
    );

    let heap_free = unsafe { esp_idf_svc::sys::esp_get_free_heap_size() };
    let buzzer = if crate::BUZZER_ENABLED.load(Ordering::Relaxed) {
        "ON"
    } else {
        "OFF"
    };
    row!(
        s,
        DIM,
        " Heap: {}K free  Buzzer: {}",
        heap_free / 1024,
        buzzer
    );
}

// ── Display thread ───────────────────────────────────────────────────

pub fn display_thread(
    spi: impl Peripheral<P = impl esp_idf_svc::hal::spi::SpiAnyPins> + 'static,
    mosi: impl Peripheral<P = impl OutputPin> + 'static,
    clk: impl Peripheral<P = impl OutputPin> + 'static,
    cs_pin: impl Peripheral<P = impl OutputPin> + 'static,
    dc_pin: impl Peripheral<P = impl OutputPin> + 'static,
    rst_pin: impl Peripheral<P = impl IOPin> + 'static,
    bl_pin: impl Peripheral<P = impl OutputPin> + 'static,
) {
    log::info!("Display thread starting");

    // Manual hardware reset
    let mut rst = PinDriver::output(rst_pin).unwrap();
    rst.set_low().unwrap();
    std::thread::sleep(Duration::from_millis(20));
    rst.set_high().unwrap();
    std::thread::sleep(Duration::from_millis(120));
    log::info!("Display RST toggled");

    // SPI bus
    let spi_driver = SpiDriver::new(spi, clk, mosi, None::<AnyIOPin>, &SpiDriverConfig::new()).unwrap();

    let spi_config = SpiConfig::new()
        .baudrate(Hertz(board::DISPLAY_SPI_FREQ_MHZ * 1_000_000))
        .data_mode(embedded_hal::spi::MODE_0);

    let spi_device = SpiDeviceDriver::new(spi_driver, Some(cs_pin), &spi_config).unwrap();

    // mipidsi SPI interface
    let dc = PinDriver::output(dc_pin).unwrap();
    let mut buffer = [0u8; 512];
    let di = SpiInterface::new(spi_device, dc, &mut buffer);

    // Build display
    let mut delay = Delay::new_default();
    let mut display = Builder::new(ST7789, di)
        .display_size(135, 240)
        .display_offset(52, 40)
        .invert_colors(ColorInversion::Inverted)
        .color_order(ColorOrder::Bgr)
        .orientation(Orientation::new().rotate(Rotation::Deg90))
        .init(&mut delay)
        .unwrap();

    log::info!("Display initialized ({}x{} landscape)", W, H);

    // Backlight on
    let mut bl = PinDriver::output(bl_pin).unwrap();
    bl.set_high().unwrap();
    log::info!("Backlight on");

    // Splash screen
    draw_splash(&mut display);
    std::thread::sleep(Duration::from_secs(2));

    // Prepare for status loop
    {
        let mut s = Screen::new(&mut display);
        s.clear();
        s.fill_band(ROW_H, HEADER_BG);
    }

    loop {
        draw_status(&mut display);
        std::thread::sleep(Duration::from_millis(500));
    }
}
