//! Firmware memory pools. Radio/RTOS global allocations remain internal.
use airhound::observations::{Counts, Observations};
use airhound::scanner::FrameType;
use core::cell::RefCell;
use critical_section::Mutex;

#[cfg(feature = "m5stickc")]
const CAPACITY: usize = 4096;
#[cfg(not(feature = "m5stickc"))]
const CAPACITY: usize = 128;
type Table = Observations<CAPACITY>;
static OBSERVATIONS: Mutex<RefCell<Option<&'static mut Table>>> = Mutex::new(RefCell::new(None));
#[cfg(feature = "m5stickc")]
static EXTERNAL: esp_alloc::EspHeap = esp_alloc::EspHeap::empty();

#[cfg(feature = "m5stickc")]
pub fn init(psram: esp_hal::peripherals::PSRAM<'static>) {
    let (start, size) = esp_hal::psram::psram_raw_parts(&psram);
    assert_eq!(size, 2 * 1024 * 1024, "Unexpected Plus2 PSRAM size");
    // No allocator or tasks can access this region yet. Exercise the whole
    // mapping with address-dependent patterns, including across cache lines.
    let words = size / core::mem::size_of::<u32>();
    let ptr = start.cast::<u32>();
    for invert in [false, true] {
        for i in 0..words {
            let pattern = (i as u32).wrapping_mul(0x9e37_79b9) ^ 0xa5a5_5a5a;
            unsafe {
                ptr.add(i)
                    .write_volatile(if invert { !pattern } else { pattern });
            }
        }
        for i in 0..words {
            let pattern = (i as u32).wrapping_mul(0x9e37_79b9) ^ 0xa5a5_5a5a;
            assert_eq!(
                unsafe { ptr.add(i).read_volatile() },
                if invert { !pattern } else { pattern },
                "PSRAM memory test failed"
            );
        }
    }
    // SAFETY: exclusively owned, tested mapped PSRAM; registered exactly once
    // during startup, never added to the global/internal allocator.
    unsafe {
        EXTERNAL.add_region(esp_alloc::HeapRegion::new(
            start,
            size,
            esp_alloc::MemoryCapability::External.into(),
        ));
    }
    let layout = core::alloc::Layout::new::<Table>();
    let allocation =
        unsafe { EXTERNAL.alloc_caps(esp_alloc::MemoryCapability::External.into(), layout) };
    assert!(!allocation.is_null(), "PSRAM observation allocation failed");
    // SAFETY: aligned, uniquely owned allocation of exactly Table's layout.
    // Intentionally retained for the firmware lifetime; never deallocated.
    let table = Table::init_in(unsafe { &mut *allocation.cast::<core::mem::MaybeUninit<Table>>() });
    critical_section::with(|cs| *OBSERVATIONS.borrow(cs).borrow_mut() = Some(table));
    // One fixed external output allocation, never a large stack temporary.
    let layout = core::alloc::Layout::new::<crate::psram_output::OutputPool>();
    let allocation =
        unsafe { EXTERNAL.alloc_caps(esp_alloc::MemoryCapability::External.into(), layout) };
    assert!(!allocation.is_null(), "PSRAM output allocation failed");
    let pool = unsafe {
        crate::psram_output::OutputPool::init_in(
            &mut *allocation.cast::<core::mem::MaybeUninit<crate::psram_output::OutputPool>>(),
        )
    };
    crate::psram_output::initialize(pool);
    log::info!(
        "PSRAM ready: bytes={} tested=true observation_capacity={}",
        size,
        CAPACITY
    );
}

#[cfg(not(feature = "m5stickc"))]
pub fn init() {
    static TABLE: static_cell::StaticCell<Table> = static_cell::StaticCell::new();
    let table = TABLE.init(Table::new());
    critical_section::with(|cs| *OBSERVATIONS.borrow(cs).borrow_mut() = Some(table));
}

pub fn wifi(mac: [u8; 6], frame: FrameType, matched: bool) {
    critical_section::with(|cs| {
        if let Some(table) = OBSERVATIONS.borrow(cs).borrow_mut().as_mut() {
            table.wifi(mac, frame, matched);
        }
    });
}
pub fn ble(mac: [u8; 6], matched: bool) {
    critical_section::with(|cs| {
        if let Some(table) = OBSERVATIONS.borrow(cs).borrow_mut().as_mut() {
            table.ble(mac, matched);
        }
    });
}
pub fn counts() -> Counts {
    critical_section::with(|cs| {
        OBSERVATIONS
            .borrow(cs)
            .borrow()
            .as_ref()
            .map(|t| t.counts())
            .unwrap_or_default()
    })
}
pub fn internal_free() -> usize {
    esp_alloc::HEAP.free_caps(esp_alloc::MemoryCapability::Internal.into())
}
pub fn external_free() -> usize {
    #[cfg(feature = "m5stickc")]
    {
        EXTERNAL.free()
    }
    #[cfg(not(feature = "m5stickc"))]
    {
        0
    }
}
