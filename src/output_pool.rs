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

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub occupied: u16,
    pub queued: u16,
    pub high_water: u16,
    pub max_residence_us: u32,
    pub max_poll_gap_us: u32,
    pub drops: u32,
    pub by_kind: [u32; 5],
    pub by_reason: [u32; 6],
    pub uart_lost_after_ble_enqueue: u32,
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
        "Output pool: cap={} reserve={} used={} queued={} high={} residence_us={} poll_gap_us={} closed={} saturated={} pool_drops={} obs={} detect={} health={} command={} diag_pool={} admission={} closed_drop={} cancel={} timeout={} write={} disabled={} ble_enqueued_uart_lost={}",
        capacity, reserved, s.occupied, s.queued, s.high_water, s.max_residence_us, s.max_poll_gap_us,
        s.closed, s.saturated, s.drops, s.by_kind[0], s.by_kind[1], s.by_kind[2], s.by_kind[3], s.by_kind[4],
        s.by_reason[0], s.by_reason[1], s.by_reason[2], s.by_reason[3], s.by_reason[4], s.by_reason[5], s.uart_lost_after_ble_enqueue))
}

struct State<const N: usize> {
    writer_taken: bool,
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
        self.take_at(Some(now_us))
    }
    fn take_at(&self, now_us: Option<u64>) -> Option<Draining<'_, M, N, R>> {
        let entry = self.state.lock(|s| {
            let mut s = s.borrow_mut();
            if s.count == 0 {
                return None;
            }
            let index = s.queue[s.head] as usize;
            s.head = (s.head + 1) % N;
            s.count -= 1;
            s.stats.queued -= 1;
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
    fn release(&self, index: usize, reason: Option<Reason>, ble: bool) {
        self.state.lock(|s| {
            let mut s = s.borrow_mut();
            if let Some(reason) = reason {
                let kind = s.kinds[index];
                s.stats.loss(kind, reason, ble);
            }
            s.release(index);
        });
        self.free.signal(());
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
        while let Some(line) = self.pool.take_at(None) {
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
            self.pool.release(self.index, Some(Reason::Closed), false);
        }
        result
    }
    /// Serialization failures have their existing separate error counter.
    pub fn discard_unpublished(mut self) {
        self.live = false;
        self.pool.release(self.index, None, false);
    }
}
impl<M: RawMutex, const N: usize, const R: usize> Drop for Reservation<'_, M, N, R> {
    fn drop(&mut self) {
        if self.live {
            self.pool
                .release(self.index, Some(Reason::Cancelled), false);
        }
    }
}

pub struct Draining<'a, M: RawMutex, const N: usize, const R: usize> {
    pool: &'a Pool<M, N, R>,
    index: usize,
    len: usize,
    live: bool,
    ble_enqueued: bool,
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
        self.pool.release(self.index, None, false);
    }
    pub fn fail(mut self, reason: Reason) {
        self.live = false;
        self.pool.close();
        self.pool
            .release(self.index, Some(reason), self.ble_enqueued);
    }
}
impl<M: RawMutex, const N: usize, const R: usize> Drop for Draining<'_, M, N, R> {
    fn drop(&mut self) {
        if self.live {
            // Cancellation may follow partial output: never splice another line.
            self.pool.close();
            self.pool
                .release(self.index, Some(Reason::Cancelled), self.ble_enqueued);
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
            drops: u32::MAX,
            by_kind: [u32::MAX; 5],
            by_reason: [u32::MAX; 6],
            uart_lost_after_ble_enqueue: u32::MAX,
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
}
