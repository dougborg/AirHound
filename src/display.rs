/// Display driver for M5StickC Plus2 (ST7789V2, 135x240, SPI).
///
/// Renders screens via direct SPI writes (no framebuffer — the 64KB
/// required would exceed ESP32's heap). Uses the [`Screen`] renderer
/// to lay out text rows flicker-free: each row is padded to full display
/// width and drawn with an explicit `background_color`, so every pixel
/// is overwritten in a single pass with no intermediate blank frame.
use core::sync::atomic::Ordering;

use embedded_graphics::mono_font::ascii::FONT_6X10;
use embedded_graphics::mono_font::{MonoTextStyle, MonoTextStyleBuilder};
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

use static_cell::StaticCell;

use embassy_time::{Duration, Instant, Timer};

use crate::board;
use crate::protocol::VERSION;

// ── Display geometry ──────────────────────────────────────────────────

/// Landscape dimensions after 90° rotation.
const W: i32 = 240;
const H: i32 = 135;

/// Row height — FONT_6X10 is 10px tall; 14px gives 4px gap between rows.
const ROW_H: i32 = 14;

/// Characters per line — FONT_6X10 is 6px wide, 240 / 6 = 40.
const LINE_W: usize = (W / 6) as usize;

// ── Color palette ─────────────────────────────────────────────────────

const BG: Rgb565 = Rgb565::BLACK;
const HEADER_BG: Rgb565 = Rgb565::new(2, 4, 12);
const FG: Rgb565 = Rgb565::WHITE;
const ACCENT: Rgb565 = Rgb565::new(0, 50, 0);
const DIM: Rgb565 = Rgb565::new(12, 24, 12);

// ── Flicker-free screen renderer ──────────────────────────────────────
//
// Text is drawn with MonoTextStyle's background_color set, so each
// character writes both foreground and background pixels simultaneously.
// Rows are padded to LINE_W (40 chars = 240px) so the text draw covers
// every pixel — no separate fill/clear needed, no flicker.

/// Reusable screen renderer. Tracks a Y cursor and owns a shared
/// line buffer. Any screen function can create one, call row/centered/
/// divider/etc., and the cursor advances automatically.
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

    /// Clear the entire display to BG and reset cursor to top.
    fn clear(&mut self) {
        let _ = self.display.clear(BG);
        self.y = 0;
    }

    /// Advance the cursor without drawing anything.
    fn skip(&mut self, pixels: i32) {
        self.y += pixels;
    }

    /// Fill a full-width band at the current Y. Does NOT advance cursor —
    /// use for one-time background painting (e.g. header bg at startup).
    fn fill_band(&mut self, height: i32, color: Rgb565) {
        let _ = Rectangle::new(Point::new(0, self.y), Size::new(W as u32, height as u32))
            .into_styled(PrimitiveStyle::with_fill(color))
            .draw(self.display);
    }

    /// Draw a left-aligned, full-width padded row. Advances cursor.
    fn row(&mut self, color: Rgb565, args: core::fmt::Arguments<'_>) {
        self.buf.clear();
        let _ = core::fmt::write(&mut self.buf, args);
        self.pad();
        self.emit(color, BG, 0);
        self.y += ROW_H;
    }

    /// Draw centered text (no padding — caller should clear first). Advances cursor.
    fn centered(&mut self, color: Rgb565, args: core::fmt::Arguments<'_>) {
        self.buf.clear();
        let _ = core::fmt::write(&mut self.buf, args);
        let x = (W - self.buf.len() as i32 * 6) / 2;
        self.emit(color, BG, x);
        self.y += ROW_H;
    }

    /// Draw a header row with a right-aligned indicator. Advances cursor.
    /// The header background gap (between text cells and row edges) must
    /// be painted once at startup via [`fill_band`].
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

    /// Draw a 1px horizontal divider. Advances cursor.
    fn divider(&mut self) {
        let _ = Rectangle::new(Point::new(0, self.y), Size::new(W as u32, 1))
            .into_styled(PrimitiveStyle::with_fill(DIM))
            .draw(self.display);
        self.y += 3;
    }

    // ── Internal helpers ──────────────────────────────────────────────

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

/// Convenience: `row!(screen, COLOR, "fmt {}", args);`
macro_rules! row {
    ($s:expr, $color:expr, $($arg:tt)*) => {
        $s.row($color, format_args!($($arg)*))
    };
}

/// Convenience: `centered!(screen, COLOR, "fmt {}", args);`
macro_rules! centered {
    ($s:expr, $color:expr, $($arg:tt)*) => {
        $s.centered($color, format_args!($($arg)*))
    };
}

// ── Screen implementations ────────────────────────────────────────────

fn draw_splash(display: &mut impl DrawTarget<Color = Rgb565>) {
    let mut s = Screen::new(display);
    s.clear();
    s.skip(42);
    centered!(s, FG, "AIRHOUND");
    centered!(s, ACCENT, "v{}", VERSION);
    s.skip(12);
    centered!(s, DIM, "RF Companion");
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

    let last = critical_section::with(|cs| crate::LAST_MATCH.borrow(cs).borrow().clone());
    if !last.is_empty() {
        row!(s, Rgb565::GREEN, " Last: {}", last);
    } else {
        row!(s, DIM, " Last: ---");
    }

    s.divider();

    let clients = crate::BLE_CLIENTS.load(Ordering::Relaxed);
    let up = (Instant::now().as_millis() / 1000) as u32;
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

    let buzzer = if crate::BUZZER_ENABLED.load(Ordering::Relaxed) {
        "ON"
    } else {
        "OFF"
    };
    row!(
        s,
        DIM,
        " Heap: {}K free  Buzzer: {}",
        esp_alloc::HEAP.free() / 1024,
        buzzer
    );
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
            return;
        }
    };
    log::info!("Display initialized ({}x{} landscape)", W, H);

    // Turn on backlight AFTER display init (active high on M5StickC Plus2)
    let _bl = Output::new(bl_pin, Level::High, OutputConfig::default());
    log::info!("Backlight on");

    // Splash screen
    draw_splash(&mut display);
    Timer::after(Duration::from_secs(2)).await;

    // Prepare for status loop: clear splash, paint header bg once.
    // The header text draw covers the middle 10px each frame via
    // background_color, but the 4px row-edge gap needs a one-time fill.
    {
        let mut s = Screen::new(&mut display);
        s.clear();
        s.fill_band(ROW_H, HEADER_BG);
    }

    loop {
        draw_status(&mut display);
        Timer::after(Duration::from_millis(500)).await;
    }
}
