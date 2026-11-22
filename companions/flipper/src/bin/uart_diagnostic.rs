//! Experimental API87.1 diagnostic. Default launch never opens UART.
#![no_std]
#![no_main]
extern crate flipperzero_alloc;
extern crate flipperzero_rt;
use airhound_flipper::diagnostic::{self as d, Command, Phase, Run};
use core::{
    cell::UnsafeCell,
    ffi::{CStr, c_void},
    fmt::Write,
    ptr,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};
use flipperzero_rt::{entry, manifest};
use flipperzero_sys as sys;
manifest!(
    name = "AirHound UART diagnostic",
    stack_size = 8192,
    app_version = 1
);
entry!(main);

// One FAP entry owns this buffer; no ISR/GUI callback accesses it. Static storage
// avoids a hidden 48 KiB stack temporary and has a visible linked-memory footprint.
struct CaptureMemory(UnsafeCell<[u8; d::CAPACITY]>);
unsafe impl Sync for CaptureMemory {}
static RAW: CaptureMemory = CaptureMemory(UnsafeCell::new([0; d::CAPACITY]));
static CANCEL: AtomicBool = AtomicBool::new(false);

struct Rx {
    stream: *mut sys::FuriStreamBuffer,
    dropped: AtomicU32,
    errors: AtomicU32,
}
impl Drop for Rx {
    fn drop(&mut self) {
        unsafe {
            sys::furi_stream_buffer_free(self.stream);
        }
    }
}
unsafe extern "C" fn receive(
    handle: *mut sys::FuriHalSerialHandle,
    event: sys::FuriHalSerialRxEvent,
    context: *mut c_void,
) {
    // Context outlives registered ISR. SerialGuard stops RX before Rx is dropped.
    unsafe {
        let rx = &*context.cast::<Rx>();
        if event.0 & 1 != 0 {
            let b = sys::furi_hal_serial_async_rx(handle);
            if sys::furi_stream_buffer_send(rx.stream, ptr::from_ref(&b).cast(), 1, 0) != 1 {
                rx.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
        if event.0 & 0x3c != 0 {
            rx.errors.fetch_add(1, Ordering::Relaxed);
        }
    }
}
struct SerialGuard<'a> {
    handle: *mut sys::FuriHalSerialHandle,
    _rx: &'a Rx,
}
impl Drop for SerialGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            sys::furi_hal_serial_async_rx_stop(self.handle);
            sys::furi_hal_serial_deinit(self.handle);
            sys::furi_hal_serial_control_release(self.handle);
        }
    }
}
impl SerialGuard<'_> {
    fn send(&self, command: Command) {
        let bytes = match command {
            Command::Start => d::START,
            Command::Stop => d::STOP,
        };
        unsafe {
            sys::furi_hal_serial_tx(self.handle, bytes.as_ptr(), bytes.len());
        }
    }
}

unsafe extern "C" fn draw(canvas: *mut sys::Canvas, _: *mut c_void) {
    unsafe {
        sys::canvas_clear(canvas);
        sys::canvas_set_font(canvas, sys::FontPrimary);
        sys::canvas_draw_str(canvas, 2, 14, c"AirHound diagnostic".as_ptr());
        sys::canvas_set_font(canvas, sys::FontSecondary);
        sys::canvas_draw_str(canvas, 2, 31, c"Passive beacon / raw UART".as_ptr());
        sys::canvas_draw_str(canvas, 2, 44, c"Local deadline: 2 seconds".as_ptr());
        sys::canvas_draw_str(canvas, 2, 58, c"Back: request stop".as_ptr());
    }
}
unsafe extern "C" fn input(event: *mut sys::InputEvent, _: *mut c_void) {
    unsafe {
        if (*event).key == sys::InputKeyBack && (*event).type_ == sys::InputTypeShort {
            CANCEL.store(true, Ordering::Relaxed);
        }
    }
}
struct Screen {
    gui: *mut sys::Gui,
    view: *mut sys::ViewPort,
}
impl Screen {
    fn new() -> Self {
        unsafe {
            let gui = sys::furi_record_open(c"gui".as_ptr()).cast();
            let view = sys::view_port_alloc();
            sys::view_port_draw_callback_set(view, Some(draw), ptr::null_mut());
            sys::view_port_input_callback_set(view, Some(input), ptr::null_mut());
            sys::gui_add_view_port(gui, view, sys::GuiLayerFullscreen);
            Self { gui, view }
        }
    }
}
impl Drop for Screen {
    fn drop(&mut self) {
        unsafe {
            sys::gui_remove_view_port(self.gui, self.view);
            sys::view_port_free(self.view);
            sys::furi_record_close(c"gui".as_ptr());
        }
    }
}
struct Storage(*mut sys::Storage);
impl Storage {
    fn new() -> Self {
        Self(unsafe { sys::furi_record_open(c"storage".as_ptr()).cast() })
    }
}
impl Drop for Storage {
    fn drop(&mut self) {
        unsafe {
            sys::furi_record_close(c"storage".as_ptr());
        }
    }
}
struct File {
    raw: *mut sys::File,
    closed: bool,
}
impl File {
    fn create(storage: &Storage, path: &CStr) -> Option<Self> {
        unsafe {
            let raw = sys::storage_file_alloc(storage.0);
            let mut file = Self { raw, closed: false };
            if sys::storage_file_open(raw, path.as_ptr(), sys::FSAM_WRITE, sys::FSOM_CREATE_NEW) {
                Some(file)
            } else {
                file.close();
                None
            }
        }
    }
    fn close(&mut self) -> bool {
        if self.closed {
            return true;
        }
        self.closed = true;
        unsafe { sys::storage_file_close(self.raw) }
    }
}
impl d::Output for File {
    fn write(&mut self, bytes: &[u8]) -> usize {
        unsafe { sys::storage_file_write(self.raw, bytes.as_ptr().cast(), bytes.len().min(512)) }
    }
    fn close(&mut self) -> bool {
        File::close(self)
    }
}
impl Drop for File {
    fn drop(&mut self) {
        if !self.closed {
            self.close();
        }
        unsafe {
            sys::storage_file_free(self.raw);
        }
    }
}
fn path(tick: u32, suffix: &str) -> heapless::String<128> {
    let mut p = heapless::String::new();
    let _ = write!(p, "/ext/apps_data/airhound_diag/run-{tick:08x}.{suffix}\0");
    p
}
fn cpath(p: &str) -> &CStr {
    CStr::from_bytes_with_nul(p.as_bytes()).unwrap()
}
fn now() -> u32 {
    unsafe { sys::furi_get_tick() }
}

fn capture() -> Result<(), &'static str> {
    // Core deadlines use milliseconds; reject a different firmware tick rate.
    if unsafe { sys::furi_kernel_get_tick_frequency() } != 1000 {
        return Err("Unsupported clock rate");
    }
    let heap_at_entry = unsafe { sys::memmgr_get_free_heap() };
    if heap_at_entry < 16 * 1024 {
        return Err("Insufficient free heap; UART not opened");
    }
    let storage = Storage::new();
    let mkdir =
        unsafe { sys::storage_common_mkdir(storage.0, c"/ext/apps_data/airhound_diag".as_ptr()) };
    if mkdir != sys::FSE_OK && mkdir != sys::FSE_EXIST {
        return Err("Cannot create output directory");
    }
    let id = now();
    let raw_path = path(id, "serial");
    let begin_path = path(id, "begin.json");
    let result_path = path(id, "json");
    let mut begin =
        File::create(&storage, cpath(&begin_path)).ok_or("Output already exists / unavailable")?;
    let began=d::persist(b"{\"schema\":1,\"complete\":false,\"mode\":\"passive_beacon\",\"reset_stop_state\":\"unknown\"}\n", &mut begin);
    if !began.written || !began.closed {
        return Err("Cannot retain begin marker");
    }
    let mut raw_file =
        File::create(&storage, cpath(&raw_path)).ok_or("Cannot create raw output")?;
    // Create the result file before UART so collisions cannot overwrite evidence.
    let mut result_file =
        File::create(&storage, cpath(&result_path)).ok_or("Cannot create result output")?;
    CANCEL.store(false, Ordering::Relaxed);
    let screen = Screen::new();
    let rx = Rx {
        stream: unsafe { sys::furi_stream_buffer_alloc(4096, 1) },
        dropped: AtomicU32::new(0),
        errors: AtomicU32::new(0),
    };
    let heap_before_uart = unsafe { sys::memmgr_get_free_heap() };
    if heap_before_uart < 8 * 1024 {
        return Err("Insufficient heap reserve; UART not opened");
    }
    let handle = unsafe { sys::furi_hal_serial_control_acquire(sys::FuriHalSerialIdUsart) };
    if handle.is_null() {
        return Err("UART is owned by another app");
    }
    let serial = SerialGuard { handle, _rx: &rx };
    unsafe {
        sys::furi_hal_serial_init(handle, 115200);
        sys::furi_hal_serial_async_rx_start(
            handle,
            Some(receive),
            ptr::from_ref(&rx).cast_mut().cast(),
            true,
        );
    }
    // Sole command owner. No info command (it changes Marauder's scan mode).
    let mut run = Run::new(now());
    serial.send(Command::Stop);
    let mut count = 0usize;
    let mut overflow = false;
    let mut chunk = [0u8; 512];
    // SAFETY: only this entry owns RAW; ISR writes only to its separate stream.
    let raw = unsafe { &mut *RAW.0.get() };
    while run.phase != Phase::Done {
        let loss = overflow
            || rx.dropped.load(Ordering::Relaxed) > 0
            || rx.errors.load(Ordering::Relaxed) > 0;
        if let Some(command) = run.poll(now(), CANCEL.load(Ordering::Relaxed), loss) {
            serial.send(command);
        }
        if run.phase == Phase::Done {
            break;
        }
        let n = unsafe {
            sys::furi_stream_buffer_receive(rx.stream, chunk.as_mut_ptr().cast(), chunk.len(), 0)
        };
        if n > 0 {
            if run.retain_bytes() {
                let keep = n.min(raw.len() - count);
                raw[count..count + keep].copy_from_slice(&chunk[..keep]);
                count += keep;
                overflow |= keep != n;
            }
            run.feed(now(), &chunk[..n]);
        } else {
            unsafe {
                sys::furi_delay_ms(2);
            }
        }
    }
    // Stop ISR before freeing ring/context. Storage work starts only after local
    // stop/drain has completed or timed out; timeout remains explicitly unknown.
    drop(serial);
    // IRQ is disabled now: accepted bytes can remain queued even with no drop.
    // Preserve their count and mark the retained stream incomplete, never silently
    // present a clean transport when discarding this final bounded ring tail.
    let unread_ring_bytes = unsafe { sys::furi_stream_buffer_bytes_available(rx.stream) };
    let dropped = rx.dropped.load(Ordering::Relaxed);
    let errors = rx.errors.load(Ordering::Relaxed);
    drop(rx);
    drop(screen);
    let raw_result = d::persist(&raw[..count], &mut raw_file);
    let e = run.evidence;
    let mut report = heapless::String::<1024>::new();
    let formatted = writeln!(
        report,
        "{{\"schema\":1,\"airhound_version\":\"{}\",\"mode\":\"passive_beacon\",\"capture_accepted\":false,\"firmware_version_verified\":false,\"ack_basis\":\"unauthenticated_firmware_console_text\",\"preflight_stop_text_ack\":{},\"start_sent\":{},\"start_text_ack\":{},\"stop_text_ack\":{},\"stop_attempts\":{},\"stop_after_ms\":{},\"cancelled\":{},\"buffer_overflow\":{},\"uart_dropped_bytes\":{},\"uart_error_events\":{},\"unread_ring_bytes\":{},\"framing_error\":{},\"framing_complete\":{},\"heap_at_entry\":{},\"heap_before_uart\":{},\"pcap_records\":{},\"raw_bytes\":{},\"raw_write_complete\":{},\"raw_file_closed\":{}}}",
        env!("CARGO_PKG_VERSION"),
        e.preflight_stop_text_ack,
        e.start_sent,
        e.start_text_ack,
        e.stop_text_ack,
        e.stop_attempts,
        e.stop_after_ms,
        e.cancelled,
        e.overflow,
        dropped,
        errors,
        unread_ring_bytes,
        e.malformed,
        run.witness.outside(),
        heap_at_entry,
        heap_before_uart,
        run.witness.packets,
        count,
        raw_result.written,
        raw_result.closed
    );
    if formatted.is_err() {
        return Err("Metadata formatting failed; retain files");
    }
    let report_result = d::persist(report.as_bytes(), &mut result_file);
    if !raw_result.written || !raw_result.closed || !report_result.written || !report_result.closed
    {
        return Err("Storage failed; retain partial files");
    }
    if !run.raw_ready(dropped, errors, unread_ring_bytes) {
        return Err("Incomplete diagnostic; inspect files");
    }
    Ok(())
}
fn main(args: Option<&CStr>) -> i32 {
    if args != Some(c"capture") {
        if args.is_some() && args != Some(c"selftest") {
            flipperzero::dialogs::alert("Unknown mode. UART not opened.");
            return 1;
        }
        let passed = d::synthetic_self_test();
        flipperzero::dialogs::alert(if passed {
            "SYNTHETIC self-test passed\nNo UART or radio opened.\nCapture requires explicit mode."
        } else {
            "Self-test failed. No UART opened."
        });
        return i32::from(!passed);
    }
    match capture() {
        Ok(()) => {
            flipperzero::dialogs::alert(
                "Raw diagnostic saved.\nHost validation still required.\nRF loss is unknown.",
            );
            0
        }
        Err(message) => {
            flipperzero::dialogs::alert(message);
            1
        }
    }
}
