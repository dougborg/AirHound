//! Ordered, nonblocking controller I/O contract, exercised with fake queues.
use crate::bridge::{STOP_COMPLETE, State, Status};
use crate::bridge_control::{Controller, UartAction};
/// An ISR arrival between the drain and boundary check is still pending input,
/// even if another read made the ring look empty. Recent arrivals are not quiet.
pub fn pending_rx(
    drained_sequence: u32,
    current_sequence: u32,
    buffered: bool,
    age_ms: u32,
) -> bool {
    buffered || drained_sequence != current_sequence || age_ms < 50
}
pub trait Output {
    fn status(&mut self, c: &Controller) -> bool;
    fn raw(&mut self, c: &Controller, bytes: &[u8]) -> bool;
    fn uart(&mut self, c: &Controller, action: UartAction) -> bool;
}
pub fn dispatch(c: &mut Controller, now: u32, action: UartAction, out: &mut impl Output) {
    let published = out.status(c);
    let action = if !published {
        c.fail(now, 8);
        // Stop is still required even when there is no room to report it.
        UartAction::Stop
    } else {
        action
    };
    if !out.uart(c, action) {
        c.fail(now, 5);
    }
}
pub fn received(c: &mut Controller, now: u32, bytes: &[u8], out: &mut impl Output) {
    c.feed(now, bytes);
    if c.state == State::Paused && !bytes.is_empty() {
        // Late producer input after the acknowledged quiet boundary is an
        // explicit session failure, not silently resumed capture.
        if let Some(stop) = c.fail(now, 13) {
            dispatch(c, now, stop, out);
        }
    }
    // The old phase's bytes precede any boundary caused by those bytes.
    if !out.raw(c, bytes)
        && let Some(stop) = c.fail(now, 8)
    {
        dispatch(c, now, stop, out)
    }
}
pub fn poll(c: &mut Controller, now: u32, buffered_input: bool, out: &mut impl Output) {
    if buffered_input {
        c.pending_input(now);
    }
    if let Some(action) = c.poll(now) {
        dispatch(c, now, action, out)
    }
}

/// Final publication can see counters newer than the controller's last poll.
/// Preserve those diagnostics, but never publish completion alongside known loss.
pub fn checked_status(mut status: Status) -> Status {
    if status.uart_dropped_bytes != 0
        || status.uart_error_events != 0
        || status.rpc_dropped_messages != 0
        || status.unread_ring_bytes != 0
    {
        status.evidence &= !STOP_COMPLETE;
    }
    status
}
