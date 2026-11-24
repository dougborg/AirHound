//! Bounded line formatting and incremental UART output, without allocation.
//!
//! The hardware adapter must serialize `ready` and `write` against every other
//! UART writer. Neither operation may wait for FIFO space. A blocked step leaves
//! the cursor unchanged so the caller can yield with interrupts enabled.

use core::fmt::{self, Write};

use crate::protocol::MsgBuffer;

pub const FIFO_CHUNK: usize = 16;
/// Legacy synchronous writer allowance; the M5 cooperative path uses two budgets.
pub const LINE_BUDGET_US: u64 = 20_000;
pub const NO_ACCEPTANCE_BUDGET_US: u64 = 40_000;
pub const MAX_SERVICE_BUDGET_US: u64 = 200_000;
pub const MAX_LINE_BYTES: usize = 512;

/// Finite service allowance, not a wire-time guarantee. Validate before widening
/// and multiplying: even 10 * 512 * 1_000_000 exceeds a 32-bit usize.
pub fn service_budget_us(length: usize) -> Option<u64> {
    if length > MAX_LINE_BYTES {
        return None;
    }
    let length = length as u64;
    let wire_us = (10 * length * 1_000_000).div_ceil(460_800);
    let yields = length.div_ceil(64).saturating_sub(1);
    Some((NO_ACCEPTANCE_BUDGET_US + wire_us + 20_000 * yields).min(MAX_SERVICE_BUDGET_US))
}

/// One terminal timing sample, excluding queue wait and subsequent LF recovery.
/// A final FIFO step may return after a deadline: completed bytes cannot be
/// undone. Such a completion is reported explicitly, never retried as a loss.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineTiming {
    pub length: u16,
    pub accepted: u16,
    pub elapsed_us: u64,
    pub completed: bool,
    pub overrun: bool,
}
/// Separately bounded delimiter recovery. This never extends a record's budget
/// or retries its payload; the original timed-out record remains lost.
pub const RESYNC_BUDGET_US: u64 = 20_000;

struct Line(MsgBuffer);

impl Write for Line {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        // A diagnostic is exactly one line. The installed host's legacy parser
        // searches for the first '{' even after a log prefix, so escape opening
        // braces as text as well as flattening CR/LF. A prefix alone is unsafe.
        for byte in text.bytes() {
            if byte == b'{' {
                self.0.extend_from_slice(b"\\x7b").map_err(|_| fmt::Error)?;
                continue;
            }
            self.0
                .push(if matches!(byte, b'\r' | b'\n') {
                    b' '
                } else {
                    byte
                })
                .map_err(|_| fmt::Error)?;
        }
        Ok(())
    }
}

pub fn diagnostic(level: log::Level, args: fmt::Arguments<'_>) -> Result<MsgBuffer, fmt::Error> {
    let mut line = Line(MsgBuffer::new());
    write!(line, "{level} - {args}")?;
    line.0.push(b'\n').map_err(|_| fmt::Error)?;
    Ok(line.0)
}

pub fn is_observation(line: &[u8]) -> bool {
    line.first() == Some(&b'{')
}

#[derive(Debug, PartialEq, Eq)]
pub enum Enqueue {
    Queued,
    Full,
    FormatError,
}

pub fn enqueue_diagnostic(
    level: log::Level,
    args: fmt::Arguments<'_>,
    enqueue: impl FnOnce(MsgBuffer) -> bool,
) -> Enqueue {
    match diagnostic(level, args) {
        Ok(line) => {
            if enqueue(line) {
                Enqueue::Queued
            } else {
                Enqueue::Full
            }
        }
        Err(_) => Enqueue::FormatError,
    }
}

pub trait Fifo {
    type Error;
    fn ready(&mut self) -> bool;
    /// Write a nonempty slice after ready returned true, without waiting.
    fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Blocked,
    Progress,
    Complete,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DrainError<E> {
    Write(E),
    InvalidProgress,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LineError<E> {
    Timeout,
    Output(E),
    InvalidProgress,
}

/// Bounded cooperative polling for a stable, externally owned payload. Only a
/// 16-byte internal scratch buffer survives a yield, never a 512-byte record.
/// Both budgets include other executor work. Only validated positive acceptance
/// renews the 40ms watchdog; the length-aware total allowance never renews.
#[cfg(any(feature = "firmware", test))]
pub fn drain_cooperative<'a, E: 'a>(
    line: &'a [u8],
    output_step: impl FnMut(&[u8], &mut usize) -> Result<Step, E> + 'a,
    now_micros: impl FnMut() -> u64 + 'a,
    poll_gap: impl FnMut(u64) + 'a,
) -> impl core::future::Future<Output = Result<(), LineError<E>>> + 'a {
    drain_cooperative_observed(line, output_step, now_micros, poll_gap, |_| {})
}

#[cfg(any(feature = "firmware", test))]
pub fn drain_cooperative_observed<'a, E: 'a>(
    line: &'a [u8],
    output_step: impl FnMut(&[u8], &mut usize) -> Result<Step, E> + 'a,
    now_micros: impl FnMut() -> u64 + 'a,
    poll_gap: impl FnMut(u64) + 'a,
    timing: impl FnMut(LineTiming) + 'a,
) -> impl core::future::Future<Output = Result<(), LineError<E>>> + 'a {
    // Return the inner future directly; do not retain another copy of its
    // arguments in an outer async state machine.
    drain_with_budget(
        line,
        output_step,
        now_micros,
        poll_gap,
        service_budget_us(line.len()),
        Some(NO_ACCEPTANCE_BUDGET_US),
        timing,
    )
}

/// The sole writer must retain its damaged record's lease across this call.
/// FIFO ordering places this LF after the accepted prefix and before the next
/// record. The hardware step still owns panic exclusion and never waits.
#[cfg(any(feature = "firmware", test))]
pub async fn resynchronize_lf<E>(
    output_step: impl FnMut(&[u8], &mut usize) -> Result<Step, E>,
    now_micros: impl FnMut() -> u64,
    poll_gap: impl FnMut(u64),
) -> Result<(), LineError<E>> {
    drain_with_budget(
        b"\n",
        output_step,
        now_micros,
        poll_gap,
        Some(RESYNC_BUDGET_US),
        None,
        |_| {},
    )
    .await
}

#[cfg(any(feature = "firmware", test))]
async fn drain_with_budget<E>(
    line: &[u8],
    mut output_step: impl FnMut(&[u8], &mut usize) -> Result<Step, E>,
    mut now_micros: impl FnMut() -> u64,
    mut poll_gap: impl FnMut(u64),
    budget_us: Option<u64>,
    no_acceptance_us: Option<u64>,
    mut timing: impl FnMut(LineTiming),
) -> Result<(), LineError<E>> {
    let budget_us = budget_us.ok_or(LineError::InvalidProgress)?;
    if line.is_empty() {
        return Ok(());
    }
    let started = now_micros();
    let mut accepted_at = started;
    let mut previous = started;
    let mut offset = 0;
    let mut quantum = 0;
    while offset < line.len() {
        let now = now_micros();
        poll_gap(now.wrapping_sub(previous));
        previous = now;
        if now.wrapping_sub(started) >= budget_us
            || no_acceptance_us.is_some_and(|limit| now.wrapping_sub(accepted_at) >= limit)
        {
            timing(LineTiming {
                length: line.len() as u16,
                accepted: offset as u16,
                elapsed_us: now.wrapping_sub(started),
                completed: false,
                overrun: true,
            });
            return Err(LineError::Timeout);
        }
        let length = (line.len() - offset).min(FIFO_CHUNK);
        let mut scratch = [0; FIFO_CHUNK];
        // Outside the hardware critical section; line may reside in PSRAM.
        scratch[..length].copy_from_slice(&line[offset..offset + length]);
        let mut wrote = 0;
        let step = output_step(&scratch[..length], &mut wrote).map_err(LineError::Output)?;
        if wrote > length
            || match step {
                Step::Blocked => wrote != 0,
                Step::Progress => wrote == 0 || wrote == length,
                Step::Complete => wrote != length,
            }
        {
            return Err(LineError::InvalidProgress);
        }
        offset += wrote;
        quantum += 1;
        // The cooperative record policy samples after acceptance too. Interrupts
        // may delay this step's return; resetting accepted_at first would hide an
        // expired interval. LF recovery retains its separate admission deadline.
        if no_acceptance_us.is_some() {
            let after = now_micros();
            let elapsed_us = after.wrapping_sub(started);
            let overrun = elapsed_us >= budget_us
                || no_acceptance_us.is_some_and(|limit| after.wrapping_sub(accepted_at) >= limit);
            if offset == line.len() || overrun {
                timing(LineTiming {
                    length: line.len() as u16,
                    accepted: offset as u16,
                    elapsed_us,
                    completed: offset == line.len(),
                    overrun,
                });
                return if offset == line.len() {
                    Ok(())
                } else {
                    Err(LineError::Timeout)
                };
            }
            if wrote > 0 {
                accepted_at = after;
            }
        } else if offset == line.len() {
            return Ok(());
        }
        if step == Step::Blocked || quantum == 4 {
            quantum = 0;
            embassy_futures::yield_now().await;
        }
    }
    Ok(())
}

#[derive(Default)]
pub struct Writer {
    failed: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WriteOutcome {
    Sent,
    Failed,
    Disabled,
}

impl Writer {
    pub fn write<E>(
        &mut self,
        line: &[u8],
        output_step: impl FnMut(&[u8], &mut usize) -> Result<Step, E>,
        now_micros: impl FnMut() -> u64,
    ) -> WriteOutcome {
        if self.failed {
            return WriteOutcome::Disabled;
        }
        if drain(line, output_step, now_micros).is_err() {
            self.failed = true;
            WriteOutcome::Failed
        } else {
            WriteOutcome::Sent
        }
    }
}

/// Bounded polling, not an asynchronous task operation. The adapter must leave
/// interrupts enabled between steps. The deadline never renews on progress.
/// At 460800/8N1 a maximum 512-byte line takes about 11.1ms on the wire; 20ms
/// allows FIFO contention/interrupt service, while bounding cooperative delay.
pub fn drain<E>(
    line: &[u8],
    mut output_step: impl FnMut(&[u8], &mut usize) -> Result<Step, E>,
    mut now_micros: impl FnMut() -> u64,
) -> Result<(), LineError<E>> {
    let started = now_micros();
    let mut offset = 0;
    loop {
        if now_micros().wrapping_sub(started) >= LINE_BUDGET_US {
            return Err(LineError::Timeout);
        }
        match output_step(line, &mut offset).map_err(LineError::Output)? {
            Step::Complete => return Ok(()),
            Step::Progress => {}
            Step::Blocked => core::hint::spin_loop(),
        }
    }
}

/// Perform at most one bounded FIFO write. No loop, sleep, or flush occurs here.
pub fn step<F: Fifo>(
    fifo: &mut F,
    line: &[u8],
    offset: &mut usize,
) -> Result<Step, DrainError<F::Error>> {
    if *offset == line.len() {
        return Ok(Step::Complete);
    }
    if *offset > line.len() {
        return Err(DrainError::InvalidProgress);
    }
    if !fifo.ready() {
        return Ok(Step::Blocked);
    }
    let end = (*offset + FIFO_CHUNK).min(line.len());
    let count = fifo.write(&line[*offset..end]).map_err(DrainError::Write)?;
    if count == 0 || count > end - *offset {
        return Err(DrainError::InvalidProgress);
    }
    *offset += count;
    Ok(if *offset == line.len() {
        Step::Complete
    } else {
        Step::Progress
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    #[test]
    fn cooperative_writer_yields_after_bounded_quantum_and_preserves_full_frame() {
        use core::{
            cell::{Cell, RefCell},
            future::Future,
            pin::pin,
            task::{Context, Poll, Waker},
        };
        let bytes = RefCell::new(Vec::new());
        let now = Cell::new(0u64);
        let gaps = Cell::new(0u64);
        let line = [b'x'; 512];
        let future = drain_cooperative(
            &line,
            |chunk, written| {
                bytes.borrow_mut().extend_from_slice(chunk);
                *written = chunk.len();
                Ok::<_, ()>(Step::Complete)
            },
            || now.get(),
            |gap| gaps.set(gaps.get().max(gap)),
        );
        // Captures references plus bounded state, never copies the 512B line.
        assert!(core::mem::size_of_val(&future) < 256);
        let mut future = pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        let mut turns = 0;
        loop {
            let before = bytes.borrow().len();
            let result = future.as_mut().poll(&mut cx);
            assert!(bytes.borrow().len() - before <= 64);
            if result == Poll::Ready(Ok(())) {
                break;
            }
            assert_eq!(result, Poll::Pending);
            // Independent work progresses at every bounded writer yield.
            turns += 1;
            now.set(now.get() + 1400);
        }
        assert_eq!((turns, gaps.get()), (7, 1400));
        assert_eq!(*bytes.borrow(), line);
    }

    #[test]
    fn cooperative_blocked_and_slow_output_have_distinct_fixed_limits() {
        use core::{
            cell::Cell,
            future::Future,
            pin::pin,
            task::{Context, Poll, Waker},
        };
        for slow_progress in [false, true] {
            let now = Cell::new(0u64);
            let sent = Cell::new(0usize);
            let line = [b'x'; 512];
            let future = drain_cooperative(
                &line,
                |_, written| {
                    if slow_progress {
                        *written = 1;
                        sent.set(sent.get() + 1);
                        Ok::<_, ()>(Step::Progress)
                    } else {
                        Ok(Step::Blocked)
                    }
                },
                || now.get(),
                |_| {},
            );
            let mut future = pin!(future);
            let mut cx = Context::from_waker(Waker::noop());
            loop {
                match future.as_mut().poll(&mut cx) {
                    Poll::Pending => now.set(now.get() + 2000),
                    Poll::Ready(result) => {
                        assert_eq!(result, Err(LineError::Timeout));
                        break;
                    }
                }
            }
            assert_eq!(now.get(), if slow_progress { 192_000 } else { 40_000 });
            assert!(sent.get() < 512);
        }
    }

    #[test]
    fn cooperative_service_tolerates_the_prior_synthetic_display_overlap() {
        use core::{
            cell::Cell,
            future::Future,
            pin::pin,
            task::{Context, Poll, Waker},
        };
        let now = Cell::new(0u64);
        let max_gap = Cell::new(0u64);
        let line = [b'x'; 512];
        let future = drain_cooperative(
            &line,
            |chunk, written| {
                *written = chunk.len();
                Ok::<_, ()>(Step::Complete)
            },
            || now.get(),
            |gap| max_gap.set(max_gap.get().max(gap)),
        );
        let mut future = pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        let mut turns = 0;
        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Pending => {
                    turns += 1;
                    // Preserve the old failing 12ms redraw model as regression
                    // evidence. This is not a measured display latency bound.
                    now.set(now.get() + 1400 + if turns == 2 { 12_000 } else { 0 });
                }
                Poll::Ready(result) => {
                    assert_eq!(result, Ok(()));
                    break;
                }
            }
        }
        assert_eq!(max_gap.get(), 13_400);
        assert_eq!(now.get(), 21_800);
    }

    #[test]
    fn panic_handoff_or_invalid_adapter_progress_cannot_continue_normal_output() {
        use core::{
            future::Future,
            pin::pin,
            task::{Context, Poll, Waker},
        };
        for fail_at in 0..32 {
            let mut writes = 0;
            let line = [b'x'; 512];
            let future = drain_cooperative(
                &line,
                |chunk, written| {
                    if writes == fail_at {
                        return Err("panic ownership");
                    }
                    writes += 1;
                    *written = chunk.len();
                    Ok(Step::Complete)
                },
                || 0,
                |_| {},
            );
            let mut future = pin!(future);
            let mut cx = Context::from_waker(Waker::noop());
            loop {
                if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
                    assert_eq!(result, Err(LineError::Output("panic ownership")));
                    break;
                }
            }
        }
        for wrote in [0, 17] {
            let future = drain_cooperative(
                b"0123456789abcdef",
                |_, n| {
                    *n = wrote;
                    Ok::<_, ()>(Step::Complete)
                },
                || 0,
                |_| {},
            );
            let mut future = pin!(future);
            let mut cx = Context::from_waker(Waker::noop());
            assert_eq!(
                future.as_mut().poll(&mut cx),
                Poll::Ready(Err(LineError::InvalidProgress))
            );
        }
    }

    #[test]
    fn observed_sized_frames_tolerate_repeated_scheduling_gaps_without_waiting_for_budget() {
        use core::{
            cell::{Cell, RefCell},
            future::Future,
            pin::pin,
            task::{Context, Poll, Waker},
        };
        for (length, expected_elapsed) in [(94, 19_882), (310, 79_528), (512, 139_174)] {
            let now = Cell::new(0u64);
            let bytes = RefCell::new(Vec::new());
            let sample = Cell::new(None);
            let line = vec![b'x'; length];
            let future = drain_cooperative_observed(
                &line,
                |chunk, n| {
                    assert!(chunk.len() <= 16);
                    bytes.borrow_mut().extend_from_slice(chunk);
                    *n = chunk.len();
                    Ok::<_, ()>(Step::Complete)
                },
                || now.get(),
                |_| {},
                |value| sample.set(Some(value)),
            );
            let mut future = pin!(future);
            let mut cx = Context::from_waker(Waker::noop());
            loop {
                let before = bytes.borrow().len();
                match future.as_mut().poll(&mut cx) {
                    Poll::Pending => {
                        assert_eq!(bytes.borrow().len() - before, 64);
                        now.set(now.get() + 19_882);
                    }
                    Poll::Ready(result) => {
                        assert_eq!(result, Ok(()));
                        break;
                    }
                }
            }
            assert_eq!(*bytes.borrow(), line);
            assert_eq!(now.get(), expected_elapsed);
            assert_eq!(
                sample.get(),
                Some(LineTiming {
                    length: length as u16,
                    accepted: length as u16,
                    elapsed_us: expected_elapsed,
                    completed: true,
                    overrun: false
                })
            );
        }
    }

    #[test]
    fn bounded_lengths_are_checked_before_arithmetic_or_fifo_access() {
        use core::{
            future::Future,
            pin::pin,
            task::{Context, Poll, Waker},
        };
        assert_eq!(service_budget_us(94), Some(62_040));
        assert_eq!(service_budget_us(310), Some(126_728));
        assert_eq!(service_budget_us(512), Some(191_112));
        assert_eq!(service_budget_us(usize::MAX), None);
        for length in [0, 513] {
            let line = vec![b'x'; length];
            let future = drain_cooperative(
                &line,
                |_, _| -> Result<Step, ()> { panic!("must not access FIFO") },
                || panic!("must not start clock"),
                |_| {},
            );
            let mut future = pin!(future);
            let mut cx = Context::from_waker(Waker::noop());
            assert_eq!(
                future.as_mut().poll(&mut cx),
                Poll::Ready(if length == 0 {
                    Ok(())
                } else {
                    Err(LineError::InvalidProgress)
                })
            );
        }
    }

    #[test]
    fn blocked_watchdog_threshold_and_clock_wrap_reject_before_next_step() {
        use core::{
            cell::Cell,
            future::Future,
            pin::pin,
            task::{Context, Poll, Waker},
        };
        for start in [0u64, u64::MAX - 20_000] {
            let now = Cell::new(start);
            let calls = Cell::new(0);
            let sample = Cell::new(None);
            let future = drain_cooperative_observed(
                b"short BLE\n",
                |_, _| {
                    calls.set(calls.get() + 1);
                    Ok::<_, ()>(Step::Blocked)
                },
                || now.get(),
                |_| {},
                |value| sample.set(Some(value)),
            );
            let mut future = pin!(future);
            let mut cx = Context::from_waker(Waker::noop());
            assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
            now.set(start.wrapping_add(39_999));
            assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
            now.set(start.wrapping_add(40_000));
            assert_eq!(
                future.as_mut().poll(&mut cx),
                Poll::Ready(Err(LineError::Timeout))
            );
            assert_eq!(calls.get(), 2);
            assert_eq!(sample.get().unwrap().elapsed_us, 40_000);
            assert_eq!(sample.get().unwrap().accepted, 0);
        }
    }

    #[test]
    fn tiny_positive_acceptance_cannot_renew_total_or_hide_step_overrun() {
        use core::{
            cell::Cell,
            future::Future,
            pin::pin,
            task::{Context, Poll, Waker},
        };
        // Four single-byte acceptances per turn keep renewing the watchdog,
        // but this short record still expires at its immutable total threshold.
        let now = Cell::new(0u64);
        let calls = Cell::new(0);
        let sample = Cell::new(None);
        let line = [b'x'; 94];
        let future = drain_cooperative_observed(
            &line,
            |_, n| {
                calls.set(calls.get() + 1);
                *n = 1;
                Ok::<_, ()>(Step::Progress)
            },
            || now.get(),
            |_| {},
            |value| sample.set(Some(value)),
        );
        let mut future = pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        for t in [0, 20_000, 40_000, 62_039] {
            now.set(t);
            assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
        }
        now.set(62_040);
        assert_eq!(
            future.as_mut().poll(&mut cx),
            Poll::Ready(Err(LineError::Timeout))
        );
        assert_eq!(calls.get(), 16);
        assert_eq!(sample.get().unwrap().accepted, 16);
        assert_eq!(sample.get().unwrap().elapsed_us, 62_040);
    }

    #[test]
    fn interrupted_step_reports_completed_frame_or_incomplete_timeout_without_retry() {
        use core::{
            cell::Cell,
            future::Future,
            pin::pin,
            task::{Context, Poll, Waker},
        };
        for (length, completed) in [(16, true), (17, false)] {
            for start in [0u64, u64::MAX - 100] {
                let now = Cell::new(start);
                let sample = Cell::new(None);
                let calls = Cell::new(0);
                let line = vec![b'x'; length];
                let future = drain_cooperative_observed(
                    &line,
                    |chunk, n| {
                        calls.set(calls.get() + 1);
                        *n = chunk.len();
                        now.set(start.wrapping_add(40_000));
                        Ok::<_, ()>(Step::Complete)
                    },
                    || now.get(),
                    |_| {},
                    |value| sample.set(Some(value)),
                );
                let mut future = pin!(future);
                let mut cx = Context::from_waker(Waker::noop());
                assert_eq!(
                    future.as_mut().poll(&mut cx),
                    Poll::Ready(if completed {
                        Ok(())
                    } else {
                        Err(LineError::Timeout)
                    })
                );
                assert_eq!(calls.get(), 1);
                assert_eq!(
                    sample.get(),
                    Some(LineTiming {
                        length: length as u16,
                        accepted: 16,
                        elapsed_us: 40_000,
                        completed,
                        overrun: true
                    })
                );
            }
        }
    }

    #[test]
    fn unvalidated_acceptance_never_renews_watchdog_or_creates_timing_success() {
        use core::{
            cell::Cell,
            future::Future,
            pin::pin,
            task::{Context, Poll, Waker},
        };
        for case in 0..5 {
            let sampled = Cell::new(false);
            let future = drain_cooperative_observed(
                b"0123456789abcdef",
                |_, n| {
                    let step = match case {
                        0 => {
                            *n = 0;
                            Step::Progress
                        }
                        1 => {
                            *n = 16;
                            Step::Progress
                        }
                        2 => {
                            *n = 1;
                            Step::Blocked
                        }
                        3 => {
                            *n = 0;
                            Step::Complete
                        }
                        _ => {
                            *n = 17;
                            Step::Complete
                        }
                    };
                    Ok::<_, ()>(step)
                },
                || 0,
                |_| {},
                |_| sampled.set(true),
            );
            let mut future = pin!(future);
            let mut cx = Context::from_waker(Waker::noop());
            assert_eq!(
                future.as_mut().poll(&mut cx),
                Poll::Ready(Err(LineError::InvalidProgress))
            );
            assert!(!sampled.get());
        }
    }

    #[derive(Default)]
    struct Sink {
        space: usize,
        bytes: Vec<u8>,
        writes: usize,
        fault: bool,
    }
    impl Fifo for Sink {
        type Error = ();
        fn ready(&mut self) -> bool {
            self.space > 0
        }
        fn write(&mut self, bytes: &[u8]) -> Result<usize, ()> {
            assert!(self.space > 0);
            assert!(bytes.len() <= FIFO_CHUNK);
            self.writes += 1;
            if self.fault {
                return Err(());
            }
            let n = self.space.min(bytes.len());
            self.bytes.extend_from_slice(&bytes[..n]);
            self.space -= n;
            Ok(n)
        }
    }

    #[test]
    fn blocked_fifo_never_writes_or_advances() {
        let mut sink = Sink::default();
        let mut cursor = 0;
        for _ in 0..1000 {
            assert_eq!(step(&mut sink, b"record\n", &mut cursor), Ok(Step::Blocked));
        }
        assert_eq!((cursor, sink.writes), (0, 0));
    }

    #[test]
    fn partial_writes_keep_lines_ordered_and_complete() {
        let log = diagnostic(
            log::Level::Info,
            format_args!("Button A\n{{\"fake\":1}}\rready"),
        )
        .unwrap();
        assert!(!is_observation(&log));
        let observation = b"{\"type\":\"ble_scan\",\"value\":123456789}\n";
        assert!(is_observation(observation));
        let mut sink = Sink::default();
        for line in [
            observation.as_slice(),
            log.as_slice(),
            observation.as_slice(),
        ] {
            let mut cursor = 0;
            loop {
                sink.space = 3;
                if step(&mut sink, line, &mut cursor).unwrap() == Step::Complete {
                    break;
                }
                assert_eq!(step(&mut sink, line, &mut cursor), Ok(Step::Blocked));
            }
        }
        assert_eq!(
            sink.bytes,
            [
                observation.as_slice(),
                log.as_slice(),
                observation.as_slice()
            ]
            .concat()
        );
        assert_eq!(sink.bytes.iter().filter(|&&b| b == b'\n').count(), 3);
    }

    #[test]
    fn overlong_diagnostic_is_rejected_instead_of_emitting_a_prefix() {
        assert!(diagnostic(log::Level::Warn, format_args!("{}", "x".repeat(512))).is_err());
        let line = diagnostic(log::Level::Info, format_args!("{}", "x".repeat(504))).unwrap();
        assert_eq!(line.len(), 512);
        assert_eq!(line.last(), Some(&b'\n'));
    }

    #[test]
    fn json_in_diagnostic_cannot_enter_legacy_host_json_parser() {
        let valid = "{\"type\":\"wifi_scan\",\"mac\":\"00:11:22:33:44:55\"}";
        let line = diagnostic(log::Level::Warn, format_args!("device reported:\n{valid}")).unwrap();
        // hostd/device_stream.rs searches for this byte anywhere in a line,
        // including after INFO/WARN prefixes. The diagnostic exposes none.
        assert!(!line.contains(&b'{'));
        assert_eq!(line.iter().filter(|&&b| b == b'\n').count(), 1);
        assert!(core::str::from_utf8(&line).unwrap().contains("\\x7b"));
        assert!(diagnostic(log::Level::Info, format_args!("{}", "{".repeat(128))).is_err());
    }

    #[test]
    fn write_failure_does_not_credit_unsent_bytes() {
        let mut sink = Sink {
            space: 16,
            fault: true,
            ..Sink::default()
        };
        let mut cursor = 0;
        assert_eq!(
            step(&mut sink, b"abc\n", &mut cursor),
            Err(DrainError::Write(()))
        );
        assert_eq!(cursor, 0);
        assert!(sink.bytes.is_empty());
    }

    #[test]
    fn pressure_has_bounded_capacity_and_no_partial_or_recursive_output() {
        let mut queue = heapless::Deque::<MsgBuffer, 4>::new();
        let mut queued = 0;
        let mut dropped = 0;
        for n in 0..100 {
            match enqueue_diagnostic(log::Level::Info, format_args!("button {n}"), |line| {
                queue.push_back(line).is_ok()
            }) {
                Enqueue::Queued => queued += 1,
                Enqueue::Full => dropped += 1,
                Enqueue::FormatError => panic!("short message"),
            }
        }
        assert_eq!((queued, dropped, queue.len()), (4, 96, 4));
        let outcome = enqueue_diagnostic(
            log::Level::Info,
            format_args!("{}", "x".repeat(512)),
            |_| panic!("must not enqueue partial line"),
        );
        assert_eq!(outcome, Enqueue::FormatError);
        for n in 0..4 {
            assert_eq!(
                queue.pop_front().unwrap().as_slice(),
                format!("INFO - button {n}\n").as_bytes()
            );
        }
        assert!(queue.is_empty());
    }

    #[test]
    fn full_fifo_and_slow_progress_both_reach_original_deadline() {
        for space in [0, 1] {
            let mut sink = Sink::default();
            let mut now = 0;
            let result = drain(
                &[b'x'; 512],
                |line, offset| {
                    sink.space = space;
                    step(&mut sink, line, offset)
                },
                || {
                    let before = now;
                    now += 1000;
                    before
                },
            );
            assert_eq!(result, Err(LineError::Timeout));
            assert!(now <= 21_000);
            assert_eq!(sink.bytes.len(), if space == 0 { 0 } else { 19 });
        }
    }

    #[test]
    fn partial_record_timeout_is_not_a_complete_frame() {
        let mut sink = Sink::default();
        let mut now = 0;
        let result = drain(
            b"{\"type\":\"ble_scan\"}\n",
            |line, offset| {
                sink.space = usize::from(sink.bytes.len() < 5);
                step(&mut sink, line, offset)
            },
            || {
                let before = now;
                now += 1000;
                before
            },
        );
        assert_eq!(result, Err(LineError::Timeout));
        assert_eq!(sink.bytes, b"{\"typ");
        assert!(!sink.bytes.contains(&b'\n'));
    }

    #[test]
    fn deadline_survives_clock_wrap_and_complete_line_is_exact() {
        let mut sink = Sink::default();
        let line = [b'x'; 512];
        let mut now = u64::MAX - 100;
        assert_eq!(
            drain(
                &line,
                |line, offset| {
                    sink.space = 7;
                    step(&mut sink, line, offset)
                },
                || {
                    let before = now;
                    now = now.wrapping_add(20);
                    before
                }
            ),
            Ok(())
        );
        assert_eq!(sink.bytes, line);
    }

    #[test]
    fn invalid_zero_or_excess_progress_fails_immediately() {
        struct Invalid(usize);
        impl Fifo for Invalid {
            type Error = ();
            fn ready(&mut self) -> bool {
                true
            }
            fn write(&mut self, _: &[u8]) -> Result<usize, ()> {
                Ok(self.0)
            }
        }
        for n in [0, 17] {
            assert_eq!(
                step(&mut Invalid(n), &[0; 16], &mut 0),
                Err(DrainError::InvalidProgress)
            );
        }
    }

    #[test]
    fn failed_line_permanently_disables_uart_without_splicing_later_frames() {
        let mut writer = Writer::default();
        let mut sink = Sink {
            space: 3,
            ..Sink::default()
        };
        let mut now = 0;
        assert_eq!(
            writer.write(
                b"{\"first\":1}\n",
                |line, cursor| step(&mut sink, line, cursor),
                || {
                    let before = now;
                    now += 1000;
                    before
                }
            ),
            WriteOutcome::Failed
        );
        let prefix = sink.bytes.clone();
        assert_eq!(
            writer.write(
                b"{\"second\":2}\n",
                |_, _| -> Result<Step, ()> { panic!("no more hardware writes") },
                || panic!("no polling")
            ),
            WriteOutcome::Disabled
        );
        assert_eq!(sink.bytes, prefix);
        assert_eq!(sink.bytes, b"{\"f");
    }
}
