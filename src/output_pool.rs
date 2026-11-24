//! Fixed output storage. Payloads never move into an asynchronous task's state.
//!
//! The M5 allocates this entire object in tested PSRAM before starting tasks.
//! Only normal tasks may use it; the global logger uses a separate DRAM queue.
use core::{
    cell::{RefCell, UnsafeCell},
    marker::PhantomData,
    mem::MaybeUninit,
};
use embassy_sync::{
    blocking_mutex::{raw::RawMutex, Mutex},
    signal::Signal,
};

pub const SLOT_BYTES: usize = 512;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Observation,
    Detection,
    Health,
    Command,
    Diagnostic,
}
impl Kind {
    fn reserved(self) -> bool {
        matches!(self, Self::Health | Self::Command)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Reason {
    Admission,
    Closed,
    Cancelled,
    Timeout,
    Write,
    Disabled,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    Full,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BleAdmission {
    Enqueued,
    Full,
    NoClient,
    /// Unexpected concurrent/reentrant handoff; rejected instead of reordering.
    HandoffBusy,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    Queued,
    Full,
    UartClosed(BleAdmission),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UartOutcome {
    Sent,
    /// Original record was lost; only its delimiter was recovered.
    TimeoutResynchronized,
    Closed,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TimingStats {
    pub max_completed_us: u32,
    pub completed_overruns: u32,
    pub last_timeout_us: u32,
    pub last_timeout_length: u16,
    pub last_timeout_accepted: u16,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub occupied: u16,
    pub queued: u16,
    pub high_water: u16,
    pub max_residence_us: u32,
    pub max_poll_gap_us: u32,
    pub timing: TimingStats,
    pub drops: u32,
    pub by_kind: [u32; 5],
    pub by_reason: [u32; 6],
    pub uart_lost_after_ble_enqueue: u32,
    pub handoff_rejections: u32,
    pub saturated: bool,
    pub closed: bool,
}
fn increment(value: &mut u32, saturated: &mut bool) {
    *saturated |= *value == u32::MAX;
    *value = value.saturating_add(1);
}
impl Stats {
    fn loss(&mut self, kind: Kind, reason: Reason, ble_enqueued: bool) {
        increment(&mut self.drops, &mut self.saturated);
        increment(&mut self.by_kind[kind as usize], &mut self.saturated);
        increment(&mut self.by_reason[reason as usize], &mut self.saturated);
        if ble_enqueued {
            increment(&mut self.uart_lost_after_ble_enqueue, &mut self.saturated);
        }
    }
}

/// Sampled pool-only counters; legacy StreamStats also includes DRAM logger staging loss.
pub fn diagnostic_stats(
    s: Stats,
    capacity: usize,
    reserved: usize,
) -> Result<crate::protocol::MsgBuffer, core::fmt::Error> {
    crate::serial_output::diagnostic(log::Level::Info, format_args!(
        "Output pool: cap={} reserve={} used={} queued={} high={} residence_us={} poll_gap_us={} closed={} saturated={} pool_drops={} obs={} detect={} health={} command={} diag_pool={} admission={} closed_drop={} cancel={} timeout={} write={} disabled={} ble_enqueued_uart_lost={} handoff_reject={}",
        capacity, reserved, s.occupied, s.queued, s.high_water, s.max_residence_us, s.max_poll_gap_us,
        s.closed, s.saturated, s.drops, s.by_kind[0], s.by_kind[1], s.by_kind[2], s.by_kind[3], s.by_kind[4],
        s.by_reason[0], s.by_reason[1], s.by_reason[2], s.by_reason[3], s.by_reason[4], s.by_reason[5], s.uart_lost_after_ble_enqueue, s.handoff_rejections))
}

/// Separate line preserves the existing pool/loss diagnostic schema. Values are
/// FIFO-acceptance timing, not physical wire completion or measured display time.
pub fn diagnostic_timing(s: TimingStats) -> Result<crate::protocol::MsgBuffer, core::fmt::Error> {
    crate::serial_output::diagnostic(log::Level::Info, format_args!(
        "UART service: no_acceptance_us={} total_cap_us={} max_completed_us={} completed_overruns={} last_timeout_us={} last_timeout_len={} last_timeout_accepted={}",
        crate::serial_output::NO_ACCEPTANCE_BUDGET_US, crate::serial_output::MAX_SERVICE_BUDGET_US,
        s.max_completed_us, s.completed_overruns, s.last_timeout_us,
        s.last_timeout_length, s.last_timeout_accepted))
}

struct State<const N: usize> {
    writer_taken: bool,
    handoff_active: bool,
    draining: usize,
    free: [u8; N],
    free_len: usize,
    queue: [u8; N],
    head: usize,
    count: usize,
    lengths: [u16; N],
    kinds: [Kind; N],
    committed_us: [u64; N],
    stats: Stats,
}
impl<const N: usize> State<N> {
    fn new() -> Self {
        assert!(N > 0 && N <= u8::MAX as usize);
        let mut free = [0; N];
        for (index, slot) in free.iter_mut().enumerate() {
            *slot = index as u8;
        }
        Self {
            writer_taken: false,
            handoff_active: false,
            draining: 0,
            free,
            free_len: N,
            queue: [0; N],
            head: 0,
            count: 0,
            lengths: [0; N],
            kinds: [Kind::Observation; N],
            committed_us: [0; N],
            stats: Stats::default(),
        }
    }
    fn release(&mut self, index: usize) {
        // Only the unique reservation/draining owner can release a slot.
        assert!(self.free_len < N && self.stats.occupied > 0);
        self.free[self.free_len] = index as u8;
        self.free_len += 1;
        self.stats.occupied -= 1;
    }
}

pub struct Pool<M: RawMutex, const N: usize, const RESERVED: usize> {
    state: Mutex<M, RefCell<State<N>>>,
    payload: UnsafeCell<[[u8; SLOT_BYTES]; N]>,
    ready: Signal<M, ()>,
    free: Signal<M, ()>,
}
// State publication is protected by M. Distinct live handles own disjoint
// payloads. A payload is immutable after commit until its draining handle drops.
unsafe impl<M: RawMutex + Sync, const N: usize, const R: usize> Sync for Pool<M, N, R> {}

impl<M: RawMutex, const N: usize, const R: usize> Pool<M, N, R> {
    /// Initialize directly in a uniquely owned, aligned allocation.
    ///
    /// # Safety
    /// `storage` is writable storage of exactly Self's layout, uniquely owned
    /// for the returned lifetime. It must not be accessed/moved until all users
    /// are gone. The payload is zeroed in place, not built on the caller's stack.
    pub unsafe fn init_in(storage: &mut MaybeUninit<Self>) -> &mut Self {
        assert!(R < N);
        let p = storage.as_mut_ptr();
        unsafe {
            core::ptr::addr_of_mut!((*p).state).write(Mutex::new(RefCell::new(State::new())));
            core::ptr::addr_of_mut!((*p).payload)
                .cast::<u8>()
                .write_bytes(0, N * SLOT_BYTES);
            core::ptr::addr_of_mut!((*p).ready).write(Signal::new());
            core::ptr::addr_of_mut!((*p).free).write(Signal::new());
            storage.assume_init_mut()
        }
    }
    pub fn stats(&self) -> Stats {
        self.state.lock(|s| s.borrow().stats)
    }
    pub fn writer_owner(&self) -> Option<WriterOwner<'_, M, N, R>> {
        self.state.lock(|s| {
            let mut s = s.borrow_mut();
            if s.writer_taken || s.stats.closed {
                return None;
            }
            s.writer_taken = true;
            Some(WriterOwner {
                pool: self,
                task_only: PhantomData,
            })
        })
    }
    pub fn rejected(&self, kind: Kind, reason: Reason) {
        self.state
            .lock(|s| s.borrow_mut().stats.loss(kind, reason, false));
    }
    /// Task-only JSON adapter. Ordinary saturation never bypasses admission.
    /// Once UART is permanently closed, the existing independent BLE channel
    /// can still receive newly produced JSON. No UART callback is available.
    pub fn publish_message(
        &self,
        message: &crate::protocol::DeviceMessage<'_>,
        kind: Kind,
        now_us: impl FnOnce() -> u64,
        closed_ble: impl FnMut(&[u8]) -> BleAdmission,
    ) -> Result<Delivery, core::fmt::Error> {
        match self.reserve(kind) {
            Ok(slot) => slot.publish_message(message, now_us, closed_ble),
            Err(Unavailable::Full) => {
                self.rejected(kind, Reason::Admission);
                Ok(Delivery::Full)
            }
            Err(Unavailable::Closed) => self.closed_message(message, kind, true, closed_ble),
        }
    }
    fn closed_message(
        &self,
        message: &crate::protocol::DeviceMessage<'_>,
        kind: Kind,
        count_loss: bool,
        mut closed_ble: impl FnMut(&[u8]) -> BleAdmission,
    ) -> Result<Delivery, core::fmt::Error> {
        // All production pool users are normal tasks on the single Embassy
        // executor. Enforce the non-overlap boundary as well: an outstanding
        // draining lease or reentrant/concurrent fallback cannot be overtaken.
        let accepted = self.state.lock(|s| {
            let mut s = s.borrow_mut();
            if s.handoff_active || s.draining != 0 {
                let stats = &mut s.stats;
                increment(&mut stats.handoff_rejections, &mut stats.saturated);
                if count_loss {
                    stats.loss(kind, Reason::Closed, false);
                }
                return false;
            }
            s.handoff_active = true;
            true
        });
        if !accepted {
            return Ok(Delivery::UartClosed(BleAdmission::HandoffBusy));
        }
        let _handoff = ClosedHandoff { pool: self };
        // At most N previously committed records. No new admission is possible;
        // each lock covers metadata only, with payload copying outside it. This
        // synchronous handoff preserves FIFO before the new direct BLE record.
        while let Some(mut older) = self.take_at(None, true) {
            if crate::serial_output::is_observation(older.bytes())
                && closed_ble(older.bytes()) == BleAdmission::Enqueued
            {
                older.mark_ble_enqueued();
            }
            older.fail(Reason::Disabled);
        }
        // No new PSRAM payload is acquired. The local 512B JSON buffer never
        // survives an await, and diagnostics cannot enter this typed adapter.
        let mut line = crate::protocol::MsgBuffer::new();
        line.resize_default(SLOT_BYTES)
            .map_err(|_| core::fmt::Error)?;
        let length = crate::comm::serialize_message(message, &mut line).ok_or(core::fmt::Error)?;
        line.truncate(length);
        let outcome = closed_ble(&line);
        self.state.lock(|s| {
            let mut s = s.borrow_mut();
            if count_loss {
                s.stats
                    .loss(kind, Reason::Closed, outcome == BleAdmission::Enqueued);
            } else if outcome == BleAdmission::Enqueued {
                let stats = &mut s.stats;
                increment(&mut stats.uart_lost_after_ble_enqueue, &mut stats.saturated);
            }
        });
        Ok(Delivery::UartClosed(outcome))
    }
    pub fn reserve(&self, kind: Kind) -> Result<Reservation<'_, M, N, R>, Unavailable> {
        let index = self.state.lock(|s| {
            let mut s = s.borrow_mut();
            if s.stats.closed {
                return Err(Unavailable::Closed);
            }
            if s.free_len == 0 || (!kind.reserved() && s.free_len <= R) {
                return Err(Unavailable::Full);
            }
            s.free_len -= 1;
            let index = s.free[s.free_len] as usize;
            s.kinds[index] = kind;
            s.stats.occupied += 1;
            s.stats.high_water = s.stats.high_water.max(s.stats.occupied);
            Ok(index)
        })?;
        Ok(Reservation {
            pool: self,
            index,
            live: true,
            task_only: PhantomData,
        })
    }
    /// One command producer may wait for space. Dropping this future holds no
    /// slot. Failure closes admission and wakes the waiter rather than hanging.
    pub async fn reserve_command(&self) -> Result<Reservation<'_, M, N, R>, Unavailable> {
        let mut waiting = CommandWait {
            pool: self,
            armed: true,
        };
        loop {
            match self.reserve(Kind::Command) {
                Err(Unavailable::Full) => self.free.wait().await,
                result => {
                    waiting.armed = false;
                    return result;
                }
            }
        }
    }
    pub fn take(&self, now_us: u64) -> Option<Draining<'_, M, N, R>> {
        self.take_at(Some(now_us), false)
    }
    fn take_at(&self, now_us: Option<u64>, handoff: bool) -> Option<Draining<'_, M, N, R>> {
        let entry = self.state.lock(|s| {
            let mut s = s.borrow_mut();
            if s.count == 0 || (s.handoff_active && !handoff) {
                return None;
            }
            let index = s.queue[s.head] as usize;
            s.head = (s.head + 1) % N;
            s.count -= 1;
            s.stats.queued -= 1;
            s.draining += 1;
            if let Some(now_us) = now_us {
                let residence = now_us
                    .wrapping_sub(s.committed_us[index])
                    .min(u32::MAX as u64) as u32;
                s.stats.max_residence_us = s.stats.max_residence_us.max(residence);
            }
            Some((index, s.lengths[index] as usize))
        })?;
        Some(Draining {
            pool: self,
            index: entry.0,
            len: entry.1,
            live: true,
            ble_enqueued: false,
            pending_failure: None,
            task_only: PhantomData,
        })
    }
    pub async fn receive(&self, now: impl Fn() -> u64) -> Option<Draining<'_, M, N, R>> {
        loop {
            if let Some(line) = self.take(now()) {
                return Some(line);
            }
            if self.stats().closed {
                return None;
            }
            self.ready.wait().await;
        }
    }
    /// Close admission in constant time. The consumer reclaims queued slots one
    /// at a time; outstanding exclusive reservations reject later commits.
    pub fn close(&self) {
        self.state.lock(|s| s.borrow_mut().stats.closed = true);
        self.free.signal(());
        self.ready.signal(());
    }
    pub fn poll_gap(&self, micros: u64) {
        self.state.lock(|s| {
            let mut s = s.borrow_mut();
            s.stats.max_poll_gap_us = s
                .stats
                .max_poll_gap_us
                .max(micros.min(u32::MAX as u64) as u32);
        });
    }
    #[cfg(any(feature = "firmware", test))]
    fn line_timing(&self, sample: crate::serial_output::LineTiming) {
        self.state.lock(|s| {
            let mut s = s.borrow_mut();
            let stats = &mut s.stats;
            let micros = sample.elapsed_us.min(u32::MAX as u64) as u32;
            stats.saturated |= sample.elapsed_us > u32::MAX as u64;
            if sample.completed {
                stats.timing.max_completed_us = stats.timing.max_completed_us.max(micros);
                if sample.overrun {
                    increment(&mut stats.timing.completed_overruns, &mut stats.saturated);
                }
            } else {
                stats.timing.last_timeout_us = micros;
                stats.timing.last_timeout_length = sample.length;
                stats.timing.last_timeout_accepted = sample.accepted;
            }
        });
    }
    fn release(&self, index: usize, reason: Option<Reason>, ble: bool, draining: bool) {
        self.state.lock(|s| {
            let mut s = s.borrow_mut();
            if let Some(reason) = reason {
                let kind = s.kinds[index];
                s.stats.loss(kind, reason, ble);
            }
            if draining {
                assert!(s.draining > 0);
                s.draining -= 1;
            }
            s.release(index);
        });
        self.free.signal(());
    }
}

struct ClosedHandoff<'a, M: RawMutex, const N: usize, const R: usize> {
    pool: &'a Pool<M, N, R>,
}
impl<M: RawMutex, const N: usize, const R: usize> Drop for ClosedHandoff<'_, M, N, R> {
    fn drop(&mut self) {
        self.pool
            .state
            .lock(|s| s.borrow_mut().handoff_active = false);
    }
}

struct CommandWait<'a, M: RawMutex, const N: usize, const R: usize> {
    pool: &'a Pool<M, N, R>,
    armed: bool,
}
impl<M: RawMutex, const N: usize, const R: usize> Drop for CommandWait<'_, M, N, R> {
    fn drop(&mut self) {
        if self.armed {
            self.pool.rejected(Kind::Command, Reason::Cancelled);
        }
    }
}

/// Unique task-lifetime owner; no await/copy occurs inside its bounded cleanup.
pub struct WriterOwner<'a, M: RawMutex, const N: usize, const R: usize> {
    pool: &'a Pool<M, N, R>,
    task_only: PhantomData<*mut ()>,
}
impl<M: RawMutex, const N: usize, const R: usize> Drop for WriterOwner<'_, M, N, R> {
    fn drop(&mut self) {
        self.pool.close();
        while let Some(line) = self.pool.take_at(None, false) {
            line.fail(Reason::Disabled);
        }
    }
}

pub struct Reservation<'a, M: RawMutex, const N: usize, const R: usize> {
    pool: &'a Pool<M, N, R>,
    index: usize,
    live: bool,
    task_only: PhantomData<*mut ()>,
}
impl<M: RawMutex, const N: usize, const R: usize> Reservation<'_, M, N, R> {
    pub fn publish_message(
        mut self,
        message: &crate::protocol::DeviceMessage<'_>,
        now_us: impl FnOnce() -> u64,
        closed_ble: impl FnMut(&[u8]) -> BleAdmission,
    ) -> Result<Delivery, core::fmt::Error> {
        let Some(length) = crate::comm::serialize_message(message, self.buffer()) else {
            self.discard_unpublished();
            return Err(core::fmt::Error);
        };
        let pool = self.pool;
        let kind = pool.state.lock(|s| s.borrow().kinds[self.index]);
        match self.commit(length, now_us()) {
            Ok(()) => Ok(Delivery::Queued),
            // Commit already attributed this record's UART loss. Do not
            // increment it twice if closure raced serialization.
            Err(Unavailable::Closed) => pool.closed_message(message, kind, false, closed_ble),
            Err(Unavailable::Full) => unreachable!(),
        }
    }
    pub fn buffer(&mut self) -> &mut [u8] {
        // Unique reservation owns this slot until consuming commit/drop. No
        // queued or draining reference can coexist with this mutable borrow.
        unsafe {
            &mut *self
                .pool
                .payload
                .get()
                .cast::<[u8; SLOT_BYTES]>()
                .add(self.index)
        }
    }
    pub fn commit(mut self, length: usize, now_us: u64) -> Result<(), Unavailable> {
        assert!(length > 0 && length <= SLOT_BYTES);
        let result = self.pool.state.lock(|s| {
            let mut s = s.borrow_mut();
            if s.stats.closed {
                return Err(Unavailable::Closed);
            }
            let tail = (s.head + s.count) % N;
            s.lengths[self.index] = length as u16;
            s.committed_us[self.index] = now_us;
            s.queue[tail] = self.index as u8;
            s.count += 1;
            s.stats.queued += 1;
            Ok(())
        });
        self.live = false;
        if result.is_ok() {
            self.pool.ready.signal(());
        } else {
            self.pool
                .release(self.index, Some(Reason::Closed), false, false);
        }
        result
    }
    /// Serialization failures have their existing separate error counter.
    pub fn discard_unpublished(mut self) {
        self.live = false;
        self.pool.release(self.index, None, false, false);
    }
}
impl<M: RawMutex, const N: usize, const R: usize> Drop for Reservation<'_, M, N, R> {
    fn drop(&mut self) {
        if self.live {
            self.pool
                .release(self.index, Some(Reason::Cancelled), false, false);
        }
    }
}

pub struct Draining<'a, M: RawMutex, const N: usize, const R: usize> {
    pool: &'a Pool<M, N, R>,
    index: usize,
    len: usize,
    live: bool,
    ble_enqueued: bool,
    pending_failure: Option<Reason>,
    task_only: PhantomData<*mut ()>,
}
impl<M: RawMutex, const N: usize, const R: usize> Draining<'_, M, N, R> {
    pub fn bytes(&self) -> &[u8] {
        // Only this draining handle owns the immutable published slot.
        unsafe {
            core::slice::from_raw_parts(
                self.pool
                    .payload
                    .get()
                    .cast::<[u8; SLOT_BYTES]>()
                    .add(self.index)
                    .cast::<u8>(),
                self.len,
            )
        }
    }
    pub fn mark_ble_enqueued(&mut self) {
        self.ble_enqueued = true;
    }
    pub fn complete(mut self) {
        self.live = false;
        self.pool.release(self.index, None, false, true);
    }
    pub fn fail(mut self, reason: Reason) {
        self.live = false;
        self.pool.close();
        self.pool.release(
            self.index,
            Some(self.pending_failure.unwrap_or(reason)),
            self.ble_enqueued,
            true,
        );
    }

    /// Sole-writer adapter. BLE admission happens once before this operation.
    /// A timeout may leave a prefix in the FIFO; retain the exclusive lease
    /// until an LF is accepted, or close on resync failure/cancellation/panic.
    /// Neither the damaged payload nor BLE delivery is retried.
    #[cfg(any(feature = "firmware", test))]
    pub async fn write_cooperative<E>(
        mut self,
        mut output_step: impl FnMut(&[u8], &mut usize) -> Result<crate::serial_output::Step, E>,
        mut now_micros: impl FnMut() -> u64,
        mut poll_gap: impl FnMut(u64),
    ) -> UartOutcome {
        use crate::serial_output::{drain_cooperative_observed, resynchronize_lf, LineError};
        match drain_cooperative_observed(
            self.bytes(),
            &mut output_step,
            &mut now_micros,
            &mut poll_gap,
            |sample| self.pool.line_timing(sample),
        )
        .await
        {
            Ok(()) => {
                self.complete();
                UartOutcome::Sent
            }
            Err(LineError::Timeout) => {
                // Defer exactly one loss attribution until release; Drop must
                // preserve this original reason if recovery is cancelled.
                self.pending_failure = Some(Reason::Timeout);
                if resynchronize_lf(&mut output_step, &mut now_micros, &mut poll_gap)
                    .await
                    .is_ok()
                {
                    self.live = false;
                    self.pool
                        .release(self.index, Some(Reason::Timeout), self.ble_enqueued, true);
                    UartOutcome::TimeoutResynchronized
                } else {
                    self.fail(Reason::Timeout);
                    UartOutcome::Closed
                }
            }
            Err(_) => {
                self.fail(Reason::Write);
                UartOutcome::Closed
            }
        }
    }
}
impl<M: RawMutex, const N: usize, const R: usize> Drop for Draining<'_, M, N, R> {
    fn drop(&mut self) {
        if self.live {
            // Cancellation may follow partial output: never splice another line.
            self.pool.close();
            self.pool.release(
                self.index,
                Some(self.pending_failure.unwrap_or(Reason::Cancelled)),
                self.ble_enqueued,
                true,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    #[test]
    fn watchdog_expired_prefix_resynchronizes_before_next_health_without_ble_retry() {
        use crate::serial_output::Step;
        use core::cell::{Cell, RefCell};
        let pool = pool::<8, 2>();
        let _owner = pool.writer_owner().unwrap();
        let damaged = [b'x'; 512];
        let health = b"{\"type\":\"status\",\"uptime\":30}\n";
        let next = b"{\"type\":\"stream_stats\",\"output_drops\":1}\n";
        assert!(send(&pool, Kind::Observation, &damaged, 0));
        assert!(send(&pool, Kind::Health, health, 0));
        assert!(send(&pool, Kind::Health, next, 0));
        let uart = RefCell::new(std::vec::Vec::new());
        let ble = RefCell::new(std::vec::Vec::new());
        let now = Cell::new(0);
        let mut line = pool.take(0).unwrap();
        ble.borrow_mut().push(line.bytes().to_vec());
        line.mark_ble_enqueued();
        let mut write = std::boxed::Box::pin(line.write_cooperative(
            |chunk, count| {
                if uart.borrow().is_empty() || chunk == b"\n" {
                    uart.borrow_mut().extend_from_slice(chunk);
                    *count = chunk.len();
                    Ok::<_, ()>(Step::Complete)
                } else {
                    // FIFO contention consumed part of the original budget.
                    now.set(12_000);
                    Ok(Step::Blocked)
                }
            },
            || now.get(),
            |gap| pool.poll_gap(gap),
        ));
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(write.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(uart.borrow().len(), 16);
        // Force the reviewed no-acceptance threshold. This models scheduler
        // absence, not a measured display duration or a physical UART cause.
        now.set(40_000);
        assert_eq!(
            write.as_mut().poll(&mut cx),
            Poll::Ready(UartOutcome::TimeoutResynchronized)
        );
        drop(write);
        assert_eq!(&uart.borrow()[..], &[&damaged[..16], b"\n"].concat());
        assert!(!pool.stats().closed);
        for expected in [health.as_slice(), next.as_slice()] {
            let mut line = pool.take(now.get()).unwrap();
            assert_eq!(line.bytes(), expected);
            ble.borrow_mut().push(line.bytes().to_vec());
            line.mark_ble_enqueued();
            let mut write = std::boxed::Box::pin(line.write_cooperative(
                |chunk, count| {
                    uart.borrow_mut().extend_from_slice(chunk);
                    *count = chunk.len();
                    Ok::<_, ()>(Step::Complete)
                },
                || now.get(),
                |_| {},
            ));
            assert_eq!(write.as_mut().poll(&mut cx), Poll::Ready(UartOutcome::Sent));
        }
        let wire = uart.borrow();
        let lines: std::vec::Vec<_> = wire.split(|b| *b == b'\n').collect();
        #[derive(serde::Deserialize)]
        struct Health {
            uptime: u64,
        }
        #[derive(serde::Deserialize)]
        struct Stream {
            output_drops: u64,
        }
        assert!(serde_json_core::from_slice::<Health>(lines[0]).is_err());
        let (parsed, used) = serde_json_core::from_slice::<Health>(lines[1]).unwrap();
        assert_eq!(used, lines[1].len());
        assert_eq!(parsed.uptime, 30);
        let (parsed, used) = serde_json_core::from_slice::<Stream>(lines[2]).unwrap();
        assert_eq!(used, lines[2].len());
        assert_eq!(parsed.output_drops, 1);
        assert_eq!(
            *ble.borrow(),
            [damaged.to_vec(), health.to_vec(), next.to_vec()]
        );
        let stats = pool.stats();
        assert_eq!(stats.drops, 1);
        assert_eq!(stats.by_reason[Reason::Timeout as usize], 1);
        assert_eq!(stats.uart_lost_after_ble_enqueue, 1);
        assert_eq!(stats.occupied, 0);
        assert!(!stats.closed);
        assert_eq!(stats.timing.last_timeout_length, 512);
        assert_eq!(stats.timing.last_timeout_accepted, 16);
        assert_eq!(stats.timing.last_timeout_us, 40_000);
        assert_eq!(stats.timing.completed_overruns, 0);
    }

    #[test]
    fn completed_lf_crossing_total_is_reported_once_without_loss_or_resync() {
        use crate::serial_output::Step;
        use core::cell::{Cell, RefCell};
        let pool = pool::<4, 1>();
        let _owner = pool.writer_owner().unwrap();
        let mut record = [b'x'; 94];
        record[93] = b'\n';
        assert!(send(&pool, Kind::Observation, &record, 0));
        let now = Cell::new(0u64);
        let uart = RefCell::new(Vec::new());
        let mut line = pool.take(0).unwrap();
        line.mark_ble_enqueued();
        let mut write = std::boxed::Box::pin(line.write_cooperative(
            |chunk, n| {
                uart.borrow_mut().extend_from_slice(chunk);
                *n = chunk.len();
                // Positive acceptance stays <40ms apart. The final step crosses
                // only the immutable 62,040us total allowance while accepting LF.
                now.set(if uart.borrow().len() == 94 {
                    63_000
                } else {
                    now.get() + 10_000
                });
                Ok::<_, ()>(Step::Complete)
            },
            || now.get(),
            |_| {},
        ));
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(write.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(write.as_mut().poll(&mut cx), Poll::Ready(UartOutcome::Sent));
        drop(write);
        assert_eq!(*uart.borrow(), record);
        let s = pool.stats();
        assert_eq!(s.timing.max_completed_us, 63_000);
        assert_eq!(s.timing.completed_overruns, 1);
        assert_eq!(
            (s.drops, s.uart_lost_after_ble_enqueue, s.occupied),
            (0, 0, 0)
        );
        assert!(!s.closed);
    }

    #[test]
    fn owned_lease_delivers_large_frame_with_observed_scheduling_gaps_once() {
        use crate::serial_output::Step;
        use core::cell::{Cell, RefCell};
        let pool = pool::<4, 1>();
        let _owner = pool.writer_owner().unwrap();
        let mut record = [b'x'; 310];
        record[309] = b'\n';
        assert!(send(&pool, Kind::Detection, &record, 0));
        let now = Cell::new(0u64);
        let uart = RefCell::new(Vec::new());
        let mut line = pool.take(0).unwrap();
        line.mark_ble_enqueued();
        let mut write = std::boxed::Box::pin(line.write_cooperative(
            |chunk, n| {
                uart.borrow_mut().extend_from_slice(chunk);
                *n = chunk.len();
                Ok::<_, ()>(Step::Complete)
            },
            || now.get(),
            |gap| pool.poll_gap(gap),
        ));
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            match write.as_mut().poll(&mut cx) {
                Poll::Pending => now.set(now.get() + 19_882),
                Poll::Ready(outcome) => {
                    assert_eq!(outcome, UartOutcome::Sent);
                    break;
                }
            }
        }
        drop(write);
        assert_eq!(*uart.borrow(), record);
        let s = pool.stats();
        assert_eq!(s.timing.max_completed_us, 79_528);
        assert_eq!(s.timing.completed_overruns, 0);
        assert_eq!(s.max_poll_gap_us, 19_882);
        assert_eq!(
            (s.drops, s.uart_lost_after_ble_enqueue, s.occupied),
            (0, 0, 0)
        );
    }

    #[test]
    fn blocked_resync_has_its_own_absolute_bound_and_never_writes_next_json() {
        use crate::serial_output::{Step, RESYNC_BUDGET_US};
        use core::cell::{Cell, RefCell};
        let pool = pool::<4, 1>();
        let _owner = pool.writer_owner().unwrap();
        assert!(send(&pool, Kind::Observation, &[b'x'; 512], 0));
        assert!(send(&pool, Kind::Health, b"{\"next\":true}\n", 0));
        let now = Cell::new(0);
        let uart = RefCell::new(std::vec::Vec::new());
        let mut line = pool.take(0).unwrap();
        line.mark_ble_enqueued();
        let mut write = std::boxed::Box::pin(line.write_cooperative(
            |chunk, count| {
                if chunk == b"\n" {
                    return Ok::<_, ()>(Step::Blocked);
                }
                uart.borrow_mut().extend_from_slice(chunk);
                *count = chunk.len();
                Ok(Step::Complete)
            },
            || now.get(),
            |_| {},
        ));
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(write.as_mut().poll(&mut cx), Poll::Pending);
        now.set(40_000);
        assert_eq!(write.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(pool.stats().occupied, 2, "damaged lease remains exclusive");
        now.set(40_000 + RESYNC_BUDGET_US - 1);
        assert_eq!(write.as_mut().poll(&mut cx), Poll::Pending);
        now.set(40_000 + RESYNC_BUDGET_US);
        assert_eq!(
            write.as_mut().poll(&mut cx),
            Poll::Ready(UartOutcome::Closed)
        );
        drop(write);
        assert_eq!(&uart.borrow()[..], &[b'x'; 64]);
        assert!(pool.stats().closed);
        assert_eq!(pool.stats().queued, 1);
        assert_eq!(pool.stats().drops, 1);
        assert_eq!(pool.stats().by_reason[Reason::Timeout as usize], 1);
        assert_eq!(pool.stats().uart_lost_after_ble_enqueue, 1);
    }

    #[test]
    fn cancellation_during_lf_retains_original_timeout_once_and_closes() {
        use crate::serial_output::Step;
        use core::cell::Cell;
        let pool = pool::<4, 1>();
        let _owner = pool.writer_owner().unwrap();
        assert!(send(&pool, Kind::Observation, &[b'x'; 512], 0));
        let now = Cell::new(0);
        let mut line = pool.take(0).unwrap();
        line.mark_ble_enqueued();
        let mut write = std::boxed::Box::pin(line.write_cooperative(
            |chunk, count| {
                if chunk == b"\n" {
                    return Ok::<_, ()>(Step::Blocked);
                }
                *count = chunk.len();
                Ok(Step::Complete)
            },
            || now.get(),
            |_| {},
        ));
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(write.as_mut().poll(&mut cx), Poll::Pending);
        now.set(40_000);
        assert_eq!(write.as_mut().poll(&mut cx), Poll::Pending);
        drop(write);
        let stats = pool.stats();
        assert!(stats.closed);
        assert_eq!(stats.occupied, 0);
        assert_eq!(stats.drops, 1);
        assert_eq!(stats.by_reason[Reason::Timeout as usize], 1);
        assert_eq!(stats.by_reason[Reason::Cancelled as usize], 0);
        assert_eq!(stats.uart_lost_after_ble_enqueue, 1);
    }

    #[test]
    fn resync_hardware_error_or_panic_gate_closes_without_payload_retry() {
        use crate::serial_output::Step;
        use core::cell::{Cell, RefCell};
        for cause in ["UART error", "PANICKED ownership gate"] {
            let pool = pool::<4, 1>();
            let _owner = pool.writer_owner().unwrap();
            assert!(send(&pool, Kind::Observation, &[b'x'; 512], 0));
            let now = Cell::new(0);
            let uart = RefCell::new(std::vec::Vec::new());
            let mut write = std::boxed::Box::pin(pool.take(0).unwrap().write_cooperative(
                |chunk, count| {
                    if chunk == b"\n" {
                        return Err(cause);
                    }
                    uart.borrow_mut().extend_from_slice(chunk);
                    *count = chunk.len();
                    Ok(Step::Complete)
                },
                || now.get(),
                |_| {},
            ));
            let mut cx = Context::from_waker(Waker::noop());
            assert_eq!(write.as_mut().poll(&mut cx), Poll::Pending);
            now.set(40_000);
            assert_eq!(
                write.as_mut().poll(&mut cx),
                Poll::Ready(UartOutcome::Closed)
            );
            drop(write);
            assert_eq!(&uart.borrow()[..], &[b'x'; 64]);
            assert!(pool.stats().closed);
            assert_eq!(pool.stats().drops, 1);
            assert_eq!(pool.stats().by_reason[Reason::Timeout as usize], 1);
        }
    }

    #[test]
    fn hard_uart_failure_never_attempts_delimiter_recovery() {
        let pool = pool::<4, 1>();
        let _owner = pool.writer_owner().unwrap();
        assert!(send(&pool, Kind::Health, b"{\"health\":true}\n", 0));
        let calls = core::cell::Cell::new(0);
        let mut write = std::boxed::Box::pin(pool.take(0).unwrap().write_cooperative(
            |_, _| {
                calls.set(calls.get() + 1);
                Err(())
            },
            || 0,
            |_| {},
        ));
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(
            write.as_mut().poll(&mut cx),
            Poll::Ready(UartOutcome::Closed)
        );
        drop(write);
        assert_eq!(calls.get(), 1);
        assert!(pool.stats().closed);
        assert_eq!(pool.stats().by_reason[Reason::Write as usize], 1);
    }
    type TestPool<const N: usize, const R: usize> = Pool<NoopRawMutex, N, R>;
    fn pool<const N: usize, const R: usize>() -> Box<TestPool<N, R>> {
        let mut storage = Box::<TestPool<N, R>>::new_uninit();
        unsafe {
            TestPool::init_in(storage.as_mut());
            storage.assume_init()
        }
    }
    fn send<const N: usize, const R: usize>(
        pool: &TestPool<N, R>,
        kind: Kind,
        bytes: &[u8],
        time: u64,
    ) -> bool {
        match pool.reserve(kind) {
            Ok(mut slot) => {
                slot.buffer()[..bytes.len()].copy_from_slice(bytes);
                slot.commit(bytes.len(), time).unwrap();
                true
            }
            Err(error) => {
                pool.rejected(
                    kind,
                    if error == Unavailable::Full {
                        Reason::Admission
                    } else {
                        Reason::Closed
                    },
                );
                false
            }
        }
    }
    #[test]
    fn finite_burst_old_four_slots_lose_records_new_pool_preserves_order() {
        let old = pool::<4, 0>();
        let new = pool::<64, 8>();
        for i in 0..32u8 {
            send(&old, Kind::Observation, &[i, b'\n'], 10);
            assert!(send(&new, Kind::Observation, &[i, b'\n'], 10));
        }
        assert_eq!(old.stats().drops, 28);
        assert_eq!(new.stats().drops, 0);
        for i in 0..32u8 {
            let line = new.take(100).unwrap();
            assert_eq!(line.bytes(), &[i, b'\n']);
            line.complete();
        }
        assert_eq!(
            (
                new.stats().occupied,
                new.stats().high_water,
                new.stats().max_residence_us
            ),
            (0, 32, 90)
        );
    }
    #[test]
    fn sustained_overload_is_bounded_and_reserve_protects_structured_health_only() {
        let pool = pool::<64, 8>();
        for _ in 0..1000 {
            send(&pool, Kind::Observation, b"scan\n", 0);
        }
        assert_eq!((pool.stats().occupied, pool.stats().drops), (56, 944));
        assert!(!send(&pool, Kind::Diagnostic, b"Memory stats\n", 0));
        for _ in 0..4 {
            assert!(send(&pool, Kind::Health, b"status\n", 0));
            assert!(send(&pool, Kind::Command, b"reply\n", 0));
        }
        assert!(!send(&pool, Kind::Health, b"overflow\n", 0));
        let s = pool.stats();
        assert_eq!((s.occupied, s.high_water, s.queued), (64, 64, 64));
        assert_eq!(s.by_kind, [944, 0, 1, 0, 1]);
        assert_eq!(s.drops, s.by_kind.iter().sum());
        for _ in 0..64 {
            pool.take(100).unwrap().complete();
        }
        assert_eq!(pool.stats().occupied, 0);
    }
    #[test]
    fn exclusive_reservations_do_not_alias_and_drop_releases_exactly_once() {
        let pool = pool::<4, 1>();
        let mut a = pool.reserve(Kind::Observation).unwrap();
        let mut b = pool.reserve(Kind::Observation).unwrap();
        a.buffer().fill(17);
        b.buffer().fill(29);
        assert!(a.buffer().iter().all(|&b| b == 17));
        drop(a);
        b.commit(512, 0).unwrap();
        let held = pool.take(1).unwrap();
        for _ in 0..100 {
            let mut other = pool.reserve(Kind::Health).unwrap();
            other.buffer().fill(99);
            other.discard_unpublished();
            assert!(held.bytes().iter().all(|&b| b == 29));
        }
        held.complete();
        assert_eq!((pool.stats().occupied, pool.stats().drops), (0, 1));
        assert_eq!(pool.stats().by_reason[Reason::Cancelled as usize], 1);
    }
    #[test]
    fn close_refuses_outstanding_commit_and_wakes_pending_command_without_stealing_slots() {
        let pool = pool::<4, 1>();
        let mut reserved = pool.reserve(Kind::Health).unwrap();
        reserved.buffer()[0] = b'\n';
        for _ in 0..3 {
            assert!(send(&pool, Kind::Health, b"health\n", 0));
        }
        let command = pool.reserve_command();
        let mut command = pin!(command);
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(command.as_mut().poll(&mut cx), Poll::Pending));
        pool.close();
        assert!(matches!(
            command.as_mut().poll(&mut cx),
            Poll::Ready(Err(Unavailable::Closed))
        ));
        assert_eq!(reserved.commit(1, 2), Err(Unavailable::Closed));
        while let Some(line) = pool.take(3) {
            line.fail(Reason::Disabled);
        }
        assert_eq!(
            (
                pool.stats().occupied,
                pool.stats().queued,
                pool.stats().drops
            ),
            (0, 0, 4)
        );
        assert_eq!(pool.stats().by_reason[Reason::Closed as usize], 1);
        assert_eq!(pool.stats().by_reason[Reason::Disabled as usize], 3);
    }
    #[test]
    fn cancelling_waiter_holds_no_slot_and_partial_writer_cancellation_latches_closed() {
        let pool = pool::<4, 1>();
        for _ in 0..4 {
            send(&pool, Kind::Health, b"record\n", 0);
        }
        {
            let future = pool.reserve_command();
            let mut future = pin!(future);
            let mut cx = Context::from_waker(Waker::noop());
            assert!(matches!(future.as_mut().poll(&mut cx), Poll::Pending));
        }
        assert_eq!((pool.stats().occupied, pool.stats().drops), (4, 1));
        assert_eq!(pool.stats().by_kind[Kind::Command as usize], 1);
        let mut line = pool.take(10).unwrap();
        line.mark_ble_enqueued();
        drop(line);
        assert!(pool.stats().closed);
        assert_eq!(pool.stats().uart_lost_after_ble_enqueue, 1);
        while let Some(line) = pool.take(11) {
            line.fail(Reason::Disabled);
        }
        assert_eq!((pool.stats().drops, pool.stats().occupied), (5, 0));
    }

    #[test]
    fn cancelling_actual_writer_future_reaps_queued_slots_and_wakes_command() {
        let pool = pool::<4, 1>();
        for _ in 0..4 {
            send(&pool, Kind::Health, b"record\n", 123);
        }
        let command = pool.reserve_command();
        let mut command = pin!(command);
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(command.as_mut().poll(&mut cx), Poll::Pending));
        {
            let writer = async {
                let _owner = pool.writer_owner().unwrap();
                assert!(pool.writer_owner().is_none());
                let mut line = pool.take(124).unwrap();
                line.mark_ble_enqueued();
                crate::serial_output::drain_cooperative(
                    line.bytes(),
                    |_, _| Ok::<_, ()>(crate::serial_output::Step::Blocked),
                    || 0,
                    |_| {},
                )
                .await
                .unwrap();
                line.complete();
            };
            let mut writer = pin!(writer);
            assert!(matches!(writer.as_mut().poll(&mut cx), Poll::Pending));
            // Dropping this real suspended writer invokes both ownership guards.
        }
        let s = pool.stats();
        assert_eq!((s.occupied, s.queued, s.drops), (0, 0, 4));
        assert_eq!(s.by_reason[Reason::Cancelled as usize], 1);
        assert_eq!(s.by_reason[Reason::Disabled as usize], 3);
        assert_eq!(s.uart_lost_after_ble_enqueue, 1);
        assert_eq!(s.max_residence_us, 1); // cleanup never invents a clock sample
        assert!(matches!(
            command.as_mut().poll(&mut cx),
            Poll::Ready(Err(Unavailable::Closed))
        ));
        assert!(pool.writer_owner().is_none());
    }
    #[test]
    fn counters_saturate_and_gap_telemetry_never_wraps() {
        let pool = pool::<4, 1>();
        pool.state.lock(|s| s.borrow_mut().stats.drops = u32::MAX);
        pool.rejected(Kind::Diagnostic, Reason::Admission);
        pool.poll_gap(u64::MAX);
        assert!(pool.stats().saturated);
        assert_eq!(
            (pool.stats().drops, pool.stats().max_poll_gap_us),
            (u32::MAX, u32::MAX)
        );
    }

    #[test]
    fn worst_case_pool_telemetry_remains_one_complete_non_observation_line() {
        let s = Stats {
            occupied: 64,
            queued: 64,
            high_water: 64,
            max_residence_us: u32::MAX,
            max_poll_gap_us: u32::MAX,
            timing: TimingStats::default(),
            drops: u32::MAX,
            by_kind: [u32::MAX; 5],
            by_reason: [u32::MAX; 6],
            uart_lost_after_ble_enqueue: u32::MAX,
            handoff_rejections: u32::MAX,
            saturated: true,
            closed: true,
        };
        let line = diagnostic_stats(s, 64, 8).unwrap();
        assert!(line.len() <= SLOT_BYTES);
        assert_eq!(line.last(), Some(&b'\n'));
        assert_eq!(line.iter().filter(|&&c| c == b'\n').count(), 1);
        assert!(!crate::serial_output::is_observation(&line));
        assert!(!line.contains(&b'{'));
    }

    #[test]
    fn bounded_timing_metadata_saturates_and_formats_without_record_payloads() {
        use crate::serial_output::LineTiming;
        let pool = pool::<4, 1>();
        pool.state
            .lock(|s| s.borrow_mut().stats.timing.completed_overruns = u32::MAX);
        pool.line_timing(LineTiming {
            length: 512,
            accepted: 512,
            elapsed_us: u64::MAX,
            completed: true,
            overrun: true,
        });
        pool.line_timing(LineTiming {
            length: 512,
            accepted: 256,
            elapsed_us: u64::MAX,
            completed: false,
            overrun: true,
        });
        let s = pool.stats();
        assert!(s.saturated);
        assert_eq!(s.timing.max_completed_us, u32::MAX);
        assert_eq!(s.timing.completed_overruns, u32::MAX);
        assert_eq!(s.timing.last_timeout_us, u32::MAX);
        assert_eq!(core::mem::size_of::<TimingStats>(), 16);
        let line = diagnostic_timing(s.timing).unwrap();
        assert!(line.len() <= SLOT_BYTES);
        assert_eq!(line.last(), Some(&b'\n'));
        assert_eq!(line.iter().filter(|&&c| c == b'\n').count(), 1);
        assert!(!line.contains(&b'{'));
        let text = core::str::from_utf8(&line).unwrap();
        assert!(text.contains("completed_overruns=4294967295"));
        assert!(text.contains("last_timeout_len=512 last_timeout_accepted=256"));
    }

    fn status(uptime: u32) -> crate::protocol::DeviceMessage<'static> {
        crate::protocol::DeviceMessage::Status {
            scanning: true,
            uptime,
            heap_free: 3448,
            ble_clients: 1,
            board: "m5stickc_plus2",
            version: "0.2.0",
        }
    }

    #[test]
    fn uart_failure_preserves_bounded_new_ble_json_and_command_replies_without_uart_retry() {
        use crate::{
            protocol::{DeviceMessage, MsgBuffer},
            serial_output::{drain_cooperative, LineError, Step},
        };
        use core::cell::{Cell, RefCell};
        use embassy_sync::channel::Channel;
        let pool = pool::<8, 2>();
        let ble = Channel::<NoopRawMutex, MsgBuffer, 4>::new();
        let ble_drops = Cell::new(0);
        let connected = Cell::new(true);
        let admit = |bytes: &[u8]| {
            if !connected.get() {
                return BleAdmission::NoClient;
            }
            let mut line = MsgBuffer::new();
            line.extend_from_slice(bytes).unwrap();
            if ble.try_send(line).is_ok() {
                BleAdmission::Enqueued
            } else {
                ble_drops.set(ble_drops.get() + 1);
                BleAdmission::Full
            }
        };
        let owner = pool.writer_owner().unwrap();
        for n in [1, 2] {
            assert_eq!(
                pool.publish_message(
                    &status(n),
                    Kind::Health,
                    || 0,
                    |_| panic!("healthy UART uses its FIFO")
                ),
                Ok(Delivery::Queued)
            );
        }
        let mut first = pool.take(1).unwrap();
        assert_eq!(admit(first.bytes()), BleAdmission::Enqueued);
        first.mark_ble_enqueued();
        let uart = RefCell::new(std::vec::Vec::new());
        {
            let write = drain_cooperative(
                first.bytes(),
                |chunk, count| {
                    if !uart.borrow().is_empty() {
                        return Err("UART write fault");
                    }
                    *count = 3;
                    uart.borrow_mut().extend_from_slice(&chunk[..3]);
                    Ok(Step::Progress)
                },
                || 0,
                |_| {},
            );
            let mut write = pin!(write);
            let mut cx = Context::from_waker(Waker::noop());
            assert_eq!(
                write.as_mut().poll(&mut cx),
                Poll::Ready(Err(LineError::Output("UART write fault")))
            );
        }
        first.fail(Reason::Write);
        // Already committed JSON has one BLE attempt; its later removal cannot
        // also enter the new-message fallback path.
        let mut queued = pool.take(2).unwrap();
        assert_eq!(admit(queued.bytes()), BleAdmission::Enqueued);
        queued.mark_ble_enqueued();
        queued.fail(Reason::Disabled);
        drop(owner);
        assert!(pool.stats().closed);
        let reply = DeviceMessage::CommandResult {
            cmd: "status",
            ok: true,
            error: None,
        };
        assert_eq!(
            pool.publish_message(&status(3), Kind::Health, || 3, admit),
            Ok(Delivery::UartClosed(BleAdmission::Enqueued))
        );
        assert_eq!(
            pool.publish_message(&reply, Kind::Command, || 4, admit),
            Ok(Delivery::UartClosed(BleAdmission::Enqueued))
        );
        assert_eq!(
            pool.publish_message(&status(5), Kind::Health, || 5, admit),
            Ok(Delivery::UartClosed(BleAdmission::Full))
        );
        connected.set(false);
        assert_eq!(
            pool.publish_message(&status(6), Kind::Health, || 6, admit),
            Ok(Delivery::UartClosed(BleAdmission::NoClient))
        );
        assert_eq!((ble.len(), ble_drops.get()), (4, 1));
        for n in [1, 2, 3] {
            let line = ble.try_receive().unwrap();
            let text = core::str::from_utf8(&line).unwrap();
            assert!(text.contains(&format!("\"uptime\":{n},")));
            assert!(text.ends_with("}\n"));
        }
        assert_eq!(
            ble.try_receive().unwrap().as_slice(),
            b"{\"type\":\"command_result\",\"cmd\":\"status\",\"ok\":true}\n"
        );
        assert!(ble.is_empty());
        assert_eq!(uart.borrow().as_slice(), b"{\"t");
        let s = pool.stats();
        assert_eq!(
            (s.drops, s.uart_lost_after_ble_enqueue, s.occupied),
            (6, 4, 0)
        );
        assert_eq!(s.by_reason, [0, 4, 0, 0, 1, 1]);
    }

    #[test]
    fn full_pool_does_not_bypass_admission_and_closed_reserved_reply_counts_once() {
        let pool = pool::<4, 1>();
        for _ in 0..3 {
            send(&pool, Kind::Observation, b"observation\n", 0);
        }
        assert_eq!(
            pool.publish_message(
                &status(1),
                Kind::Observation,
                || 1,
                |_| panic!("Full must not use BLE fallback")
            ),
            Ok(Delivery::Full)
        );
        let slot = pool.reserve(Kind::Command).unwrap();
        pool.close();
        let calls = core::cell::Cell::new(0);
        assert_eq!(
            slot.publish_message(
                &status(2),
                || 2,
                |_| {
                    calls.set(calls.get() + 1);
                    BleAdmission::Enqueued
                }
            ),
            Ok(Delivery::UartClosed(BleAdmission::Enqueued))
        );
        assert_eq!(
            (
                calls.get(),
                pool.stats().drops,
                pool.stats().uart_lost_after_ble_enqueue
            ),
            (1, 5, 1)
        );
        assert_eq!(pool.stats().occupied, 0);
        while let Some(line) = pool.take(3) {
            line.fail(Reason::Disabled);
        }
        assert_eq!((pool.stats().occupied, pool.stats().drops), (0, 5));
    }

    #[test]
    fn producer_during_failed_writer_yield_hands_off_old_json_before_new_without_duplicates() {
        use crate::protocol::MsgBuffer;
        use embassy_sync::channel::Channel;
        let pool = pool::<8, 2>();
        let ble = Channel::<NoopRawMutex, MsgBuffer, 4>::new();
        let admit = |bytes: &[u8]| {
            let mut line = MsgBuffer::new();
            line.extend_from_slice(bytes).unwrap();
            if ble.try_send(line).is_ok() {
                BleAdmission::Enqueued
            } else {
                BleAdmission::Full
            }
        };
        for n in [1, 2] {
            pool.publish_message(&status(n), Kind::Health, || 0, |_| panic!("healthy path"))
                .unwrap();
        }
        // This is the real post-write-failure scheduling boundary: the current
        // line was already BLE-forwarded, but an older queued line remains.
        let writer = async {
            let _owner = pool.writer_owner().unwrap();
            let mut first = pool.take(1).unwrap();
            assert_eq!(admit(first.bytes()), BleAdmission::Enqueued);
            first.mark_ble_enqueued();
            first.fail(Reason::Write);
            embassy_futures::yield_now().await;
            while let Some(mut old) = pool.receive(|| 3).await {
                if admit(old.bytes()) == BleAdmission::Enqueued {
                    old.mark_ble_enqueued();
                }
                old.fail(Reason::Disabled);
            }
        };
        let mut writer = pin!(writer);
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(writer.as_mut().poll(&mut cx), Poll::Pending));
        assert_eq!(pool.stats().queued, 1);
        pool.publish_message(&status(3), Kind::Health, || 2, admit)
            .unwrap();
        assert_eq!(pool.stats().queued, 0);
        assert!(matches!(writer.as_mut().poll(&mut cx), Poll::Ready(())));
        for n in [1, 2, 3] {
            let bytes = ble.try_receive().unwrap();
            assert!(core::str::from_utf8(&bytes)
                .unwrap()
                .contains(&format!("\"uptime\":{n},")));
        }
        assert!(ble.is_empty());
        assert_eq!(
            (pool.stats().drops, pool.stats().uart_lost_after_ble_enqueue),
            (3, 3)
        );
    }

    #[test]
    fn outstanding_lease_or_reentrant_handoff_is_rejected_instead_of_reordering() {
        use core::cell::Cell;
        let pool = pool::<8, 2>();
        for n in [1, 2] {
            pool.publish_message(&status(n), Kind::Health, || 0, |_| unreachable!())
                .unwrap();
        }
        let held = pool.take(1).unwrap();
        pool.close();
        assert_eq!(
            pool.publish_message(
                &status(3),
                Kind::Health,
                || 2,
                |_| panic!("must not overtake held lease")
            ),
            Ok(Delivery::UartClosed(BleAdmission::HandoffBusy))
        );
        held.fail(Reason::Write);
        let callbacks = Cell::new(0);
        let result = pool.publish_message(
            &status(4),
            Kind::Health,
            || 3,
            |_| {
                callbacks.set(callbacks.get() + 1);
                assert!(pool.take(3).is_none()); // writer cannot race the handoff
                assert_eq!(
                    pool.publish_message(
                        &status(99),
                        Kind::Health,
                        || 3,
                        |_| panic!("reentrant handoff")
                    ),
                    Ok(Delivery::UartClosed(BleAdmission::HandoffBusy))
                );
                BleAdmission::Enqueued
            },
        );
        assert_eq!(result, Ok(Delivery::UartClosed(BleAdmission::Enqueued)));
        assert_eq!(callbacks.get(), 2); // queued2, then new4; no nested99
        assert_eq!(pool.stats().handoff_rejections, 3);
        assert_eq!((pool.stats().occupied, pool.stats().drops), (0, 6));
    }
}
