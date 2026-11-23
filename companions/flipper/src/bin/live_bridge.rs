//! Explicit RPC-only live bridge. Launch and Hello never open UART.
#![no_std]
#![no_main]
extern crate alloc;
extern crate flipperzero_alloc;
extern crate flipperzero_rt;
use airhound_flipper::{
    bridge::{self as wire, Body, Header, Hello, Message, State},
    bridge_control::{Controller, UartAction},
    bridge_io::{self, Output},
    diagnostic as d,
};
use alloc::boxed::Box;
use core::{
    ffi::{CStr, c_void},
    ptr,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering},
};
use flipperzero_rt::{entry, manifest};
use flipperzero_sys as sys;
manifest!(
    name = "AirHound live bridge",
    stack_size = 8192,
    app_version = 1
);
entry!(main);
const FOREVER: u32 = u32::MAX;
static CANCEL: AtomicBool = AtomicBool::new(false);
#[repr(C)]
#[derive(Clone, Copy)]
struct Packet {
    bytes: [u8; wire::MAX_MESSAGE],
    len: u16,
}
impl Packet {
    const fn new() -> Self {
        Self {
            bytes: [0; wire::MAX_MESSAGE],
            len: 0,
        }
    }
}
struct Shared {
    rpc: AtomicPtr<sys::RpcAppSystem>,
    mutex: *mut sys::FuriMutex,
    commands: *mut sys::FuriMessageQueue,
    outgoing: *mut sys::FuriMessageQueue,
    closed: AtomicBool,
    exit: AtomicBool,
    sender_done: AtomicBool,
    callbacks: AtomicU32,
    queue_loss: AtomicU32,
}
unsafe impl Sync for Shared {}
impl Shared {
    fn lock(&self) {
        unsafe {
            sys::furi_mutex_acquire(self.mutex, FOREVER);
        }
    }
    fn unlock(&self) {
        unsafe {
            sys::furi_mutex_release(self.mutex);
        }
    }
    fn put(&self, p: &Packet) -> bool {
        let ok = unsafe {
            sys::furi_message_queue_put(self.outgoing, ptr::from_ref(p).cast(), 0)
                == sys::FuriStatusOk
        };
        if !ok {
            self.queue_loss.fetch_add(1, Ordering::Relaxed);
        }
        ok
    }
}
unsafe extern "C" fn rpc_event(event: *const sys::RpcAppSystemEvent, context: *mut c_void) {
    unsafe {
        let s = &*context.cast::<Shared>();
        s.callbacks.fetch_add(1, Ordering::AcqRel);
        let event = &*event;
        if event.type_ == sys::RpcAppEventTypeSessionClose {
            // Callback delivery itself can be delayed behind SDK pipe_send. The UART
            // loop's own heartbeat deadline does not depend on this notification.
            s.closed.store(true, Ordering::Release);
            s.lock();
            let rpc = s.rpc.swap(ptr::null_mut(), Ordering::AcqRel);
            if !rpc.is_null() {
                sys::rpc_system_app_set_callback(rpc, None, ptr::null_mut());
            }
            s.unlock();
        } else {
            let mut accepted = false;
            if event.type_ == sys::RpcAppEventTypeAppExit {
                s.exit.store(true, Ordering::Release);
                accepted = true;
            } else if event.type_ == sys::RpcAppEventTypeDataExchange
                && event.data.type_ == sys::RpcAppSystemEventDataTypeBytes
            {
                let data = event.data.__bindgen_anon_1.bytes;
                if !data.ptr.is_null() && data.size <= wire::MAX_MESSAGE {
                    let bytes = core::slice::from_raw_parts(data.ptr, data.size);
                    if matches!(
                        wire::decode(bytes),
                        Ok(Message {
                            body: Body::Command(_),
                            ..
                        })
                    ) {
                        let mut p = Packet::new();
                        p.len = bytes.len() as u16;
                        p.bytes[..bytes.len()].copy_from_slice(bytes);
                        accepted =
                            sys::furi_message_queue_put(s.commands, ptr::from_ref(&p).cast(), 0)
                                == sys::FuriStatusOk;
                        if !accepted {
                            s.queue_loss.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
            s.lock();
            let rpc = s.rpc.load(Ordering::Acquire);
            if !rpc.is_null() && (2..=7).contains(&event.type_.0) {
                sys::rpc_system_app_confirm(rpc, accepted);
            }
            s.unlock();
        }
        s.callbacks.fetch_sub(1, Ordering::AcqRel);
    }
}
unsafe extern "C" fn send_loop(context: *mut c_void) -> i32 {
    unsafe {
        let s = &*context.cast::<Shared>();
        let mut p = Packet::new();
        loop {
            if sys::furi_message_queue_get(s.outgoing, ptr::from_mut(&mut p).cast(), 20)
                == sys::FuriStatusOk
            {
                s.lock();
                let rpc = s.rpc.load(Ordering::Acquire);
                if !rpc.is_null() {
                    sys::rpc_system_app_exchange_data(rpc, p.bytes.as_ptr(), p.len as usize);
                }
                s.unlock();
            } else if s.sender_done.load(Ordering::Acquire) {
                break;
            }
        }
        0
    }
}
struct Emit {
    sequence: u32,
    instance: u64,
    app_started: u32,
}
impl Emit {
    fn body(&mut self, s: &Shared, c: &Controller, request_id: u32, body: Body<'_>) -> bool {
        let Some(nonce) = c.nonce() else { return true };
        let Some(seq) = self.sequence.checked_add(1) else {
            s.queue_loss.fetch_add(1, Ordering::Relaxed);
            return false;
        };
        self.sequence = seq;
        let mut p = Packet::new();
        match wire::encode(
            Message {
                header: Header {
                    request_id,
                    nonce,
                    app_instance: self.instance,
                    sequence: seq,
                },
                body,
            },
            &mut p.bytes,
        ) {
            Ok(n) => {
                p.len = n as u16;
                s.put(&p)
            }
            Err(_) => {
                s.queue_loss.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }
    fn status(&mut self, s: &Shared, c: &Controller, rx: &Rx, bytes: u64, unread: u32) -> bool {
        let mut state = c.status(now());
        state.app_uptime_ms = now().wrapping_sub(self.app_started);
        state.uart_rx_bytes = bytes;
        state.uart_dropped_bytes = rx.dropped.load(Ordering::Relaxed) as u64;
        state.uart_error_events = rx.errors.load(Ordering::Relaxed) as u64;
        state.rpc_dropped_messages = s.queue_loss.load(Ordering::Relaxed) as u64;
        state.unread_ring_bytes = unread;
        if state.evidence & wire::STOP_COMPLETE != 0 {
            // Pause keeps UART owned. Recheck the ring at publication as well
            // as the poll boundary; terminal teardown already stopped the ISR.
            state.unread_ring_bytes = state
                .unread_ring_bytes
                .max(unsafe { sys::furi_stream_buffer_bytes_available(rx.stream) as u32 });
        }
        self.body(
            s,
            c,
            c.request_id(),
            Body::Status(bridge_io::checked_status(state)),
        )
    }
}
fn now() -> u32 {
    unsafe {
        ((sys::furi_get_tick() as u64) * 1000 / (sys::furi_kernel_get_tick_frequency() as u64))
            as u32
    }
}
struct Rx {
    stream: *mut sys::FuriStreamBuffer,
    dropped: AtomicU32,
    errors: AtomicU32,
    sequence: AtomicU32,
    last_received: AtomicU32,
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
            rx.last_received.store(now(), Ordering::Release);
            rx.sequence.fetch_add(1, Ordering::Release);
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
    fn send(&self, command: UartAction) {
        let bytes = match command {
            UartAction::StartBeacon => d::START,
            UartAction::Stop => d::STOP,
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
        sys::canvas_draw_str(canvas, 2, 14, c"AirHound live bridge".as_ptr());
        sys::canvas_set_font(canvas, sys::FontSecondary);
        sys::canvas_draw_str(canvas, 2, 31, c"Controlled by AirHound".as_ptr());
        sys::canvas_draw_str(canvas, 2, 44, c"Watchdog: 3 seconds".as_ptr());
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

struct Link<'a, 'rx> {
    shared: &'a Shared,
    emit: &'a mut Emit,
    serial: &'a mut Option<SerialGuard<'rx>>,
    rx: &'rx Rx,
    received: u64,
    unread: u32,
}
impl Output for Link<'_, '_> {
    fn status(&mut self, c: &Controller) -> bool {
        self.emit
            .status(self.shared, c, self.rx, self.received, self.unread)
    }
    fn raw(&mut self, c: &Controller, bytes: &[u8]) -> bool {
        self.emit.body(
            self.shared,
            c,
            0,
            Body::Raw {
                generation: c.generation,
                app_uptime_ms: now().wrapping_sub(self.emit.app_started),
                bytes,
            },
        )
    }
    fn uart(&mut self, c: &Controller, action: UartAction) -> bool {
        if self.serial.is_none() && c.state == State::Preflight {
            let handle = unsafe { sys::furi_hal_serial_control_acquire(sys::FuriHalSerialIdUsart) };
            if handle.is_null() {
                return false;
            }
            unsafe {
                sys::furi_hal_serial_init(handle, 115200);
                sys::furi_hal_serial_async_rx_start(
                    handle,
                    Some(receive),
                    ptr::from_ref(self.rx).cast_mut().cast(),
                    true,
                );
            }
            *self.serial = Some(SerialGuard {
                handle,
                _rx: self.rx,
            });
        }
        if let Some(serial) = self.serial {
            serial.send(action);
            true
        } else {
            false
        }
    }
}
// RPC provides the pointer via its own application-start path, as in the pinned
// firmware applications. Standalone/default launch never opens UART.
fn rpc_context(args: Option<&CStr>) -> Option<*mut sys::RpcAppSystem> {
    let bytes = args?.to_bytes();
    let text = core::str::from_utf8(bytes).ok()?.strip_prefix("RPC ")?;
    if text.is_empty() || text.len() > 8 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let address = u32::from_str_radix(text, 16).ok()?;
    if !(0x2000_0000..0x2003_0000).contains(&address) || address % 4 != 0 {
        return None;
    }
    Some(address as *mut sys::RpcAppSystem)
}
fn main(args: Option<&CStr>) -> i32 {
    let Some(rpc) = rpc_context(args) else {
        return 0;
    };
    if unsafe { sys::memmgr_get_free_heap() } < 24 * 1024 {
        // No UART or callback has been installed at this point.
        return 1;
    }
    CANCEL.store(false, Ordering::Relaxed);
    let shared = Box::new(Shared {
        rpc: AtomicPtr::new(rpc),
        mutex: unsafe { sys::furi_mutex_alloc(sys::FuriMutexTypeNormal) },
        commands: unsafe {
            sys::furi_message_queue_alloc(8, core::mem::size_of::<Packet>() as u32)
        },
        outgoing: unsafe {
            sys::furi_message_queue_alloc(16, core::mem::size_of::<Packet>() as u32)
        },
        closed: AtomicBool::new(false),
        exit: AtomicBool::new(false),
        sender_done: AtomicBool::new(false),
        callbacks: AtomicU32::new(0),
        queue_loss: AtomicU32::new(0),
    });
    let context = ptr::from_ref(&*shared).cast_mut().cast();
    let sender = unsafe {
        sys::furi_thread_alloc_ex(
            c"AirHound RPC send".as_ptr(),
            4096,
            Some(send_loop),
            context,
        )
    };
    let rx = Rx {
        stream: unsafe { sys::furi_stream_buffer_alloc(4096, 1) },
        dropped: AtomicU32::new(0),
        errors: AtomicU32::new(0),
        sequence: AtomicU32::new(0),
        last_received: AtomicU32::new(now()),
    };
    let instance =
        unsafe { ((sys::furi_hal_random_get() as u64) << 32) | sys::furi_hal_random_get() as u64 }
            .max(1);
    let mut c = Controller::new(instance, now());
    let mut emit = Emit {
        sequence: 0,
        instance,
        app_started: now(),
    };
    let screen = Screen::new();
    unsafe {
        sys::rpc_system_app_set_callback(rpc, Some(rpc_event), context);
        sys::furi_thread_start(sender);
        sys::rpc_system_app_send_started(rpc);
    }
    let mut serial: Option<SerialGuard<'_>> = None;
    let mut received = 0u64;
    let mut unread = 0u32;
    let mut last_status = now();
    let launched = emit.app_started;
    let mut previous = (c.state, c.cause, c.evidence, c.generation, c.request_id());
    loop {
        let drained_sequence = rx.sequence.load(Ordering::Acquire);
        // Drain old-phase bytes BEFORE processing controls or publishing a new
        // preflight-ready boundary. A still-nonempty ring suppresses quiet.
        if serial.is_some() {
            let mut bytes = [0u8; 248];
            let n = unsafe {
                sys::furi_stream_buffer_receive(
                    rx.stream,
                    bytes.as_mut_ptr().cast(),
                    bytes.len(),
                    0,
                )
            };
            if n > 0 {
                received = received.saturating_add(n as u64);
                let mut link = Link {
                    shared: &shared,
                    emit: &mut emit,
                    serial: &mut serial,
                    rx: &rx,
                    received,
                    unread,
                };
                bridge_io::received(&mut c, now(), &bytes[..n], &mut link);
            }
        }
        let mut action = None;
        // A bounded number of commands prevents a flooded RPC queue starving UART deadlines.
        for _ in 0..2 {
            let mut p = Packet::new();
            if unsafe {
                sys::furi_message_queue_get(shared.commands, ptr::from_mut(&mut p).cast(), 0)
            } != sys::FuriStatusOk
            {
                break;
            }
            let Ok(message) = wire::decode(&p.bytes[..p.len as usize]) else {
                continue;
            };
            let Body::Command(command) = message.body else {
                continue;
            };
            match c.request(message.header, command, now()) {
                Ok(accepted) => {
                    let sent = if accepted.hello {
                        emit.body(
                            &shared,
                            &c,
                            message.header.request_id,
                            Body::Hello(Hello {
                                capabilities: wire::CAPABILITIES,
                                api_major: 87,
                                api_minor: 1,
                                uart_baud: 115200,
                                watchdog_ms: wire::WATCHDOG_MS,
                                app_version: env!("CARGO_PKG_VERSION"),
                                source_revision: option_env!("AIRHOUND_SOURCE_REVISION")
                                    .unwrap_or(""),
                                source_modified: match option_env!("AIRHOUND_SOURCE_MODIFIED") {
                                    Some("0") => 0,
                                    Some("1") => 1,
                                    _ => 2,
                                },
                            }),
                        )
                    } else {
                        emit.body(
                            &shared,
                            &c,
                            message.header.request_id,
                            Body::Ack { command, result: 0 },
                        )
                    };
                    if !sent {
                        action = c.fail(now(), 8);
                    } else if accepted.uart.is_some() {
                        action = accepted.uart;
                    }
                    // A newly accepted UART action must execute before another lifecycle command.
                    if action.is_some() {
                        break;
                    }
                }
                Err(result) => {
                    // Never send a bound response for a foreign nonce/instance.
                    if c.nonce() == Some(message.header.nonce)
                        && message.header.app_instance == instance
                    {
                        emit.body(
                            &shared,
                            &c,
                            message.header.request_id,
                            Body::Ack { command, result },
                        );
                    }
                }
            }
        }
        if shared.closed.load(Ordering::Acquire)
            && !matches!(c.state, State::Stopped | State::Failed)
        {
            action = c.fail(now(), 4).or(action);
        } else if (shared.exit.load(Ordering::Acquire) || CANCEL.load(Ordering::Relaxed))
            && !matches!(c.state, State::Stopped | State::Failed)
        {
            action = c.fail(now(), 9).or(action);
        } else if rx.dropped.load(Ordering::Relaxed) > 0 || rx.errors.load(Ordering::Relaxed) > 0 {
            action = c.fail(now(), 5).or(action);
        } else if shared.queue_loss.load(Ordering::Relaxed) > 0 {
            action = c.fail(now(), 8).or(action);
        }
        if c.nonce().is_none() && now().wrapping_sub(launched) > 5000 {
            shared.exit.store(true, Ordering::Release);
        }
        let pending = bridge_io::pending_rx(
            drained_sequence,
            rx.sequence.load(Ordering::Acquire),
            unsafe { sys::furi_stream_buffer_bytes_available(rx.stream) } > 0,
            now().wrapping_sub(rx.last_received.load(Ordering::Acquire)),
        );
        let mut link = Link {
            shared: &shared,
            emit: &mut emit,
            serial: &mut serial,
            rx: &rx,
            received,
            unread,
        };
        if let Some(command) = action {
            bridge_io::dispatch(&mut c, now(), command, &mut link);
        } else {
            bridge_io::poll(&mut c, now(), pending, &mut link);
        }
        if matches!(c.state, State::Stopped | State::Failed) && serial.is_some() {
            drop(serial.take()); // unregister ISR before final counters and context release
            unread = unsafe { sys::furi_stream_buffer_bytes_available(rx.stream) } as u32;
            if unread > 0
                || rx.dropped.load(Ordering::Relaxed) > 0
                || rx.errors.load(Ordering::Relaxed) > 0
            {
                c.fail(now(), 5);
            }
            if shared.queue_loss.load(Ordering::Relaxed) > 0 {
                c.fail(now(), 8);
            }
            emit.status(&shared, &c, &rx, received, unread);
        }
        let state = (c.state, c.cause, c.evidence, c.generation, c.request_id());
        if c.nonce().is_some()
            && (state != previous
                || (!matches!(c.state, State::Stopped | State::Failed)
                    && now().wrapping_sub(last_status) >= 200))
        {
            emit.status(&shared, &c, &rx, received, unread);
            previous = state;
            last_status = now();
        }
        // Stop does not free RPC state from a different thread. The host closes its
        // owned RPC session after terminal status; that callback unregisters itself.
        if shared.closed.load(Ordering::Acquire)
            && matches!(c.state, State::Stopped | State::Failed)
        {
            break;
        }
        unsafe {
            sys::furi_delay_ms(2);
        }
    }
    drop(serial);
    drop(screen);
    shared.sender_done.store(true, Ordering::Release);
    unsafe {
        // SDK pipe_send may block while a host refuses reads. Keep all contexts alive;
        // UART has already stopped. Never force-free a blocked sender/callback.
        sys::furi_thread_join(sender);
        sys::furi_thread_free(sender);
        // Pinned RPC teardown invokes SessionClose from a timer-daemon pending
        // callback. Flush queues a firmware epilogue after that callback and
        // waits for it, proving even the FAP callback's return instructions have
        // completed before this executable/context can be unloaded. A counter
        // decremented just before callback return is not that barrier.
        sys::furi_timer_flush();
        while shared.callbacks.load(Ordering::Acquire) > 0 {
            sys::furi_delay_ms(1);
        }
        sys::furi_message_queue_free(shared.commands);
        sys::furi_message_queue_free(shared.outgoing);
        sys::furi_mutex_free(shared.mutex);
    }
    0
}
