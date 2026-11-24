//! Pure M5 rendering shared by startup and cooperative periodic redraws.
//! Each step completes all synchronous DrawTarget operations before returning.
//! No UART deadline or target execution-time guarantee is implied by a yield.
use crate::{observations::Counts, protocol::VERSION};
use embedded_graphics::mono_font::ascii::FONT_6X10;
use embedded_graphics::mono_font::{MonoTextStyle, MonoTextStyleBuilder};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::Text;

// ── Display geometry ──────────────────────────────────────────────────

/// Landscape dimensions after 90° rotation.
pub const W: i32 = 240;
pub const H: i32 = 135;

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
        self.pad();
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

pub fn draw_splash(display: &mut impl DrawTarget<Color = Rgb565>) {
    let mut s = Screen::new(display);
    s.clear();
    s.skip(42);
    centered!(s, FG, "AIRHOUND");
    centered!(s, ACCENT, "v{}", VERSION);
    s.skip(12);
    centered!(s, DIM, "RF Companion");
}

/// A consistent frame; all firmware locks are released before rendering/yielding.
pub struct Status {
    pub scanning: bool,
    pub details: bool,
    pub counts: Counts,
    pub internal_free: usize,
    pub external_free: usize,
    pub wifi_packets: u32,
    pub wifi_drops: u32,
    pub last_match: heapless::String<32>,
    pub clients: u32,
    pub uptime_seconds: u32,
    pub sound: bool,
}

const STATUS_STEPS: usize = 10;

/// Initial splash cleanup remains synchronous and finishes before radio startup.
pub fn prepare_status(display: &mut impl DrawTarget<Color = Rgb565>) {
    let mut s = Screen::new(display);
    s.clear();
    s.fill_band(ROW_H, HEADER_BG);
}

fn draw_status_step(display: &mut impl DrawTarget<Color = Rgb565>, v: &Status, step: usize) {
    let mut s = Screen::new(display);
    s.y = if step <= 6 {
        step as i32 * ROW_H
    } else {
        6 * ROW_H + 3 + (step - 7) as i32 * ROW_H
    };
    match step {
        0 => s.header(
            format_args!(" AIRHOUND v{}", VERSION),
            if v.scanning { "[SCAN]" } else { "[STOP]" },
            if v.scanning {
                Rgb565::GREEN
            } else {
                Rgb565::RED
            },
        ),
        1 if v.details => row!(s, FG, " DEVICE DETAILS"),
        2 if v.details => row!(s, FG, " Internal RAM free: {} bytes", v.internal_free),
        3 if v.details => row!(s, FG, " PSRAM free: {} bytes", v.external_free),
        4 if v.details => row!(s, FG, " WiFi packets: {}", v.wifi_packets),
        5 if v.details => row!(s, FG, " WiFi queue drops: {}", v.wifi_drops),
        1 => row!(
            s,
            DIM,
            " Addresses since boot{}",
            if v.counts.limited { " (limit)" } else { "" }
        ),
        2 => row!(
            s,
            FG,
            " APs: {}  Other WiFi: {}",
            v.counts.aps,
            v.counts.wifi_other
        ),
        3 => row!(s, FG, " BLE devices: {}", v.counts.ble),
        4 => row!(
            s,
            ACCENT,
            " Signature hits: WiFi {}  BLE {}",
            v.counts.wifi_hits,
            v.counts.ble_hits
        ),
        5 if !v.last_match.is_empty() => row!(s, Rgb565::GREEN, " Last: {}", v.last_match),
        5 => row!(s, DIM, " Last: ---"),
        6 => s.divider(),
        7 => row!(
            s,
            DIM,
            " BLE: {} client{}  Up: {:02}:{:02}:{:02}",
            v.clients,
            if v.clients == 1 { "" } else { "s" },
            v.uptime_seconds / 3600,
            (v.uptime_seconds % 3600) / 60,
            v.uptime_seconds % 60
        ),
        8 => row!(s, FG, " Sound: {}", if v.sound { "ON" } else { "MUTED" }),
        9 => row!(
            s,
            FG,
            " A: {}  B: {}",
            if v.details { "Overview" } else { "Details" },
            if v.sound { "Mute" } else { "Sound on" }
        ),
        _ => unreachable!(),
    }
}

/// Startup preserves complete synchronous drawing before DISPLAY_READY.
pub fn draw_status(display: &mut impl DrawTarget<Color = Rgb565>, status: &Status) {
    for step in 0..STATUS_STEPS {
        draw_status_step(display, status, step);
    }
}

/// Only one bounded row (header includes its short indicator) or divider per
/// poll. The synchronous row buffer and SPI transactions are gone before await.
/// Row duration still needs target measurement: this is scheduling, not WCET.
pub async fn draw_status_cooperative(
    display: &mut impl DrawTarget<Color = Rgb565>,
    status: &Status,
) {
    for step in 0..STATUS_STEPS {
        draw_status_step(display, status, step);
        if step + 1 < STATUS_STEPS {
            embassy_futures::yield_now().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{
        convert::Infallible,
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    struct Canvas {
        pixels: Rc<RefCell<Vec<Rgb565>>>,
        touched_y: Rc<RefCell<Vec<i32>>>,
    }
    impl Canvas {
        fn new() -> Self {
            Self {
                pixels: Rc::new(RefCell::new(vec![Rgb565::BLACK; (W * H) as usize])),
                touched_y: Rc::new(RefCell::new(Vec::new())),
            }
        }
    }
    impl OriginDimensions for Canvas {
        fn size(&self) -> Size {
            Size::new(W as u32, H as u32)
        }
    }
    impl DrawTarget for Canvas {
        type Color = Rgb565;
        type Error = Infallible;
        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Rgb565>>,
        {
            let mut image = self.pixels.borrow_mut();
            let mut touched = self.touched_y.borrow_mut();
            for Pixel(p, c) in pixels {
                if (0..W).contains(&p.x) && (0..H).contains(&p.y) {
                    image[(p.y * W + p.x) as usize] = c;
                    touched.push(p.y);
                }
            }
            Ok(())
        }
    }
    fn status(details: bool) -> Status {
        Status {
            scanning: true,
            details,
            counts: Counts {
                aps: 17,
                wifi_other: 19,
                ble: 23,
                wifi_hits: 3,
                ble_hits: 5,
                limited: true,
            },
            internal_free: 3448,
            external_free: 2_000_000,
            wifi_packets: 4321,
            wifi_drops: 2,
            last_match: heapless::String::try_from("synthetic signature").unwrap(),
            clients: 1,
            uptime_seconds: 3661,
            sound: true,
        }
    }
    // Frozen346 synchronous layout, with only global reads replaced by fixture
    // values. This independent whole-frame path checks migration pixel parity.
    fn legacy_reference(display: &mut impl DrawTarget<Color = Rgb565>, v: &Status) {
        let mut s = Screen::new(display);

        let scanning = v.scanning;
        s.header(
            format_args!(" AIRHOUND v{}", VERSION),
            if scanning { "[SCAN]" } else { "[STOP]" },
            if scanning { Rgb565::GREEN } else { Rgb565::RED },
        );

        let details = v.details;
        let counts = &v.counts;
        if details {
            row!(s, FG, " DEVICE DETAILS");
            row!(s, FG, " Internal RAM free: {} bytes", v.internal_free);
            row!(s, FG, " PSRAM free: {} bytes", v.external_free);
            row!(s, FG, " WiFi packets: {}", v.wifi_packets);
            row!(s, FG, " WiFi queue drops: {}", v.wifi_drops);
        } else {
            row!(
                s,
                DIM,
                " Addresses since boot{}",
                if counts.limited { " (limit)" } else { "" }
            );
            row!(
                s,
                FG,
                " APs: {}  Other WiFi: {}",
                counts.aps,
                counts.wifi_other
            );
            row!(s, FG, " BLE devices: {}", counts.ble);
            row!(
                s,
                ACCENT,
                " Signature hits: WiFi {}  BLE {}",
                counts.wifi_hits,
                counts.ble_hits
            );

            let last = &v.last_match;
            if !last.is_empty() {
                row!(s, Rgb565::GREEN, " Last: {}", last);
            } else {
                row!(s, DIM, " Last: ---");
            }
        }
        s.divider();

        let clients = v.clients;
        let up = v.uptime_seconds;
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

        row!(s, FG, " Sound: {}", if v.sound { "ON" } else { "MUTED" });
        row!(
            s,
            FG,
            " A: {}  B: {}",
            if details { "Overview" } else { "Details" },
            if v.sound { "Mute" } else { "Sound on" }
        );
    }

    #[test]
    fn cooperative_rows_preserve_both_pages_and_allow_other_uart_work() {
        for details in [false, true] {
            let s = status(details);
            let mut expected = Canvas::new();
            legacy_reference(&mut expected, &s);
            let mut display = Canvas::new();
            let pixels = display.pixels.clone();
            let touched = display.touched_y.clone();
            let mut future = pin!(draw_status_cooperative(&mut display, &s));
            let uart_bytes = Cell::new(0);
            let payload = [b'x'; 512];
            // Real UART cooperative state machine, with an always-ready fake
            // FIFO and fixed time: proves poll opportunity, not target latency.
            let mut uart = pin!(crate::serial_output::drain_cooperative(
                &payload,
                |chunk, offset| {
                    uart_bytes.set(uart_bytes.get() + chunk.len());
                    *offset = chunk.len();
                    Ok::<_, ()>(crate::serial_output::Step::Complete)
                },
                || 0,
                |_| {}
            ));
            let mut uart_done = false;
            let mut cx = Context::from_waker(Waker::noop());
            for step in 0..STATUS_STEPS {
                touched.borrow_mut().clear();
                let result = future.as_mut().poll(&mut cx);
                assert_eq!(result.is_ready(), step + 1 == STATUS_STEPS);
                let ys = touched.borrow();
                assert!(!ys.is_empty());
                let (lo, hi) = if step <= 5 {
                    (step as i32 * 14, step as i32 * 14 + 14)
                } else if step == 6 {
                    (84, 85)
                } else {
                    (87 + (step as i32 - 7) * 14, 101 + (step as i32 - 7) * 14)
                };
                assert!(
                    ys.iter().all(|y| *y >= lo && *y < hi),
                    "one row/divider per poll"
                );
                drop(ys);
                if !uart_done {
                    uart_done = matches!(uart.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
                }
            }
            assert!(uart_done);
            assert_eq!(uart_bytes.get(), 512);
            assert_eq!(*pixels.borrow(), *expected.pixels.borrow());
        }
    }

    #[test]
    fn startup_and_maximum_width_frames_match_original_pixels() {
        for details in [false, true] {
            let mut s = status(details);
            s.scanning = false;
            s.sound = false;
            s.clients = u32::MAX;
            s.uptime_seconds = u32::MAX;
            s.internal_free = usize::MAX;
            s.external_free = usize::MAX;
            s.wifi_packets = u32::MAX;
            s.wifi_drops = u32::MAX;
            s.counts.aps = usize::MAX;
            s.counts.wifi_other = usize::MAX;
            s.counts.ble = usize::MAX;
            s.counts.wifi_hits = usize::MAX;
            s.counts.ble_hits = usize::MAX;
            s.last_match = heapless::String::try_from("12345678901234567890123456789012").unwrap();
            let mut expected = Canvas::new();
            legacy_reference(&mut expected, &s);
            let mut actual = Canvas::new();
            draw_status(&mut actual, &s);
            assert_eq!(*actual.pixels.borrow(), *expected.pixels.borrow());
            let mut cooperative = Canvas::new();
            embassy_futures::block_on(draw_status_cooperative(&mut cooperative, &s));
            assert_eq!(*cooperative.pixels.borrow(), *expected.pixels.borrow());
        }
    }

    #[test]
    fn cancellation_releases_draw_target_and_next_page_fully_overwrites_frame() {
        let mut display = Canvas::new();
        let first = status(false);
        let second = status(true);
        {
            let mut future = pin!(draw_status_cooperative(&mut display, &first));
            let mut cx = Context::from_waker(Waker::noop());
            for _ in 0..3 {
                assert!(future.as_mut().poll(&mut cx).is_pending());
            }
        }
        // A new draw can borrow the same target immediately after cancellation.
        embassy_futures::block_on(draw_status_cooperative(&mut display, &second));
        let mut expected = Canvas::new();
        legacy_reference(&mut expected, &second);
        assert_eq!(*display.pixels.borrow(), *expected.pixels.borrow());
    }
}
