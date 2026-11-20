//! LINK-19.6: the SPI flash's work since power-on, for `ui radio` (where a slow write's time goes): the chip's reads
//! and bytes read, programs, erases, suspends, the time reads waited for a program or erase to end (SAFE-1.3), and
//! the store's passes over every channel slot (CHAN-36.1). Counters only: nothing reads them to decide anything.
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

pub static READS: AtomicU32 = AtomicU32::new(0);
pub static READ_BYTES: AtomicU32 = AtomicU32::new(0);
pub static PROGRAMS: AtomicU32 = AtomicU32::new(0);
pub static ERASES: AtomicU32 = AtomicU32::new(0);
pub static SUSPENDS: AtomicU32 = AtomicU32::new(0);
pub static WAIT_US: AtomicU32 = AtomicU32::new(0);
pub static PASSES: AtomicU32 = AtomicU32::new(0);

/// Adds `n` to a counter (wrapping).
pub fn add(c: &AtomicU32, n: u32) {
    c.store(c.load(Relaxed).wrapping_add(n), Relaxed);
}

/// Every counter, in the order above.
pub fn all() -> [u32; 7] {
    [&READS, &READ_BYTES, &PROGRAMS, &ERASES, &SUSPENDS, &WAIT_US, &PASSES].map(|c| c.load(Relaxed))
}
