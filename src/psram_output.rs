//! Task-only PSRAM output adapter. The runtime logger never calls this module.
use crate::{comm, protocol::DeviceMessage, serial_console::increment, SERIALIZE_ERRORS};
use airhound::output_pool::{Kind, Pool, Reason, Unavailable};
use core::sync::atomic::{AtomicPtr, Ordering};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

pub const CAPACITY: usize = 64;
pub const RESERVED: usize = 8;
pub type OutputPool = Pool<CriticalSectionRawMutex, CAPACITY, RESERVED>;
static POOL: AtomicPtr<OutputPool> = AtomicPtr::new(core::ptr::null_mut());

pub fn initialize(pool: &'static mut OutputPool) {
    assert!(POOL
        .compare_exchange(
            core::ptr::null_mut(),
            pool,
            Ordering::Release,
            Ordering::Relaxed
        )
        .is_ok());
}
pub fn pool() -> &'static OutputPool {
    let p = POOL.load(Ordering::Acquire);
    assert!(!p.is_null());
    // Initialized once before tasks, retained for firmware lifetime.
    unsafe { &*p }
}
fn reason(error: Unavailable) -> Reason {
    match error {
        Unavailable::Full => Reason::Admission,
        Unavailable::Closed => Reason::Closed,
    }
}
pub fn publish(message: &DeviceMessage<'_>, kind: Kind) {
    let pool = pool();
    let mut slot = match pool.reserve(kind) {
        Ok(slot) => slot,
        Err(error) => {
            pool.rejected(kind, reason(error));
            return;
        }
    };
    match comm::serialize_message(message, slot.buffer()) {
        Some(length) => {
            let _ = slot.commit(length, embassy_time::Instant::now().as_micros());
        }
        None => {
            slot.discard_unpublished();
            increment(&SERIALIZE_ERRORS);
        }
    }
}
pub fn publish_bytes(bytes: &[u8], kind: Kind) {
    let pool = pool();
    let mut slot = match pool.reserve(kind) {
        Ok(slot) => slot,
        Err(error) => {
            pool.rejected(kind, reason(error));
            return;
        }
    };
    assert!(!bytes.is_empty() && bytes.len() <= airhound::output_pool::SLOT_BYTES);
    slot.buffer()[..bytes.len()].copy_from_slice(bytes);
    let _ = slot.commit(bytes.len(), embassy_time::Instant::now().as_micros());
}
pub async fn command(message: &DeviceMessage<'_>) {
    let pool = pool();
    let mut slot = match pool.reserve_command().await {
        Ok(slot) => slot,
        Err(error) => {
            pool.rejected(Kind::Command, reason(error));
            return;
        }
    };
    match comm::serialize_message(message, slot.buffer()) {
        Some(length) => {
            let _ = slot.commit(length, embassy_time::Instant::now().as_micros());
        }
        None => {
            slot.discard_unpublished();
            increment(&SERIALIZE_ERRORS);
        }
    }
}

pub fn report() {
    let s = pool().stats();
    // Synchronous formatting only; no MsgBuffer or snapshot crosses an await.
    let line = airhound::output_pool::diagnostic_stats(s, CAPACITY, RESERVED);
    match line {
        Ok(line) => publish_bytes(&line, Kind::Health),
        Err(_) => {
            increment(&SERIALIZE_ERRORS);
            increment(&crate::serial_console::FORMAT_ERRORS);
        }
    }
}
