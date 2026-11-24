//! SAFE-8.1, SAFE-8.2 (decision 364): a fault or a hang ends in a restart, not a stop, and the next start says so.
//! A fault keeps its address in RAM the start-up code doesn't clear (`.uninit`), then resets; a hang is the
//! watchdog's reset (hal::wdt). Three restarts without a minute of running between them stop the radio in the
//! safe state with the red LED, as every fault did before, so a fault at start can't loop for ever. A power cut
//! (the battery out) loses the record: the next start is an ordinary one.
use core::mem::MaybeUninit;
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

const MAGIC: u32 = 0xFE77_1E5A;
/// Restarts in a row before the radio stays stopped.
pub const MAX_IN_A_ROW: u32 = 3;
/// Running this long clears the count (SAFE-8.1).
pub const SETTLED_MS: u64 = 60_000;
/// `pc` for a watchdog reset (no address).
pub const HANG: u32 = 1;

/// [MAGIC, pc (0: none known), restarts in a row, a check word]. The stock bootloader runs between the reset and
/// Ferrite and uses RAM of its own: a record it overwrote fails the check and reads as no record.
#[link_section = ".uninit.RESTART"]
static mut REC: MaybeUninit<[u32; 4]> = MaybeUninit::uninit();

/// This start's restart, for the UI task (SAFE-8.2): 0 none, [`HANG`], or the fault's address (the high bit set
/// when it is unknown); and the restarts in a row.
pub static RESTARTED: AtomicU32 = AtomicU32::new(0);
pub static IN_A_ROW: AtomicU32 = AtomicU32::new(0);

fn check(pc: u32, n: u32) -> u32 {
    MAGIC ^ pc.rotate_left(7) ^ n.rotate_left(19)
}

fn read() -> Option<(u32, u32)> {
    // SAFETY: plain words in RAM, reached only through these volatile accesses (interrupts are off at start-up and
    // in the fault path; the settle clear writes from thread mode alone).
    let r = unsafe { read_volatile(addr_of!(REC) as *const [u32; 4]) };
    (r[0] == MAGIC && r[3] == check(r[1], r[2])).then_some((r[1], r[2]))
}

fn write(pc: u32, n: u32) {
    // SAFETY: as in `read`.
    unsafe { write_volatile(addr_of_mut!(REC) as *mut [u32; 4], [MAGIC, pc, n, check(pc, n)]) }
}

/// The fault path: the address kept with the count; whether to restart (false: the radio stays stopped).
pub fn fault(pc: Option<u32>) -> bool {
    let n = read().map_or(0, |(_, n)| n) + 1;
    write(pc.unwrap_or(1 << 31), n);
    n < MAX_IN_A_ROW
}

/// At start-up (reset cause `cause`, `hal::sys::RESET_*`): what this start follows, kept for the UI task. False
/// when it is one restart too many: the caller stops in the safe state.
pub fn at_start(cause: u32) -> bool {
    use crate::hal::sys::{RESET_POR, RESET_WDT};
    let rec = if cause & RESET_POR != 0 { None } else { read() };
    let (pc, n) = match rec {
        // the fault path's own reset
        Some((pc, n)) if pc != 0 && cause & RESET_WDT == 0 => (pc, n),
        // a hang: the watchdog ran out
        _ if cause & RESET_WDT != 0 => (HANG, rec.map_or(0, |(_, n)| n) + 1),
        // an ordinary start (power, the updater's reset, `reboot`): no restart, the count from before kept
        Some((_, n)) => (0, n),
        None => (0, 0),
    };
    write(0, n); // the address is reported once; the count stays until the radio has run a minute
    if pc != 0 {
        RESTARTED.store(pc, Relaxed);
        IN_A_ROW.store(n, Relaxed);
    }
    n < MAX_IN_A_ROW || pc == 0
}

/// A minute of running: the count back to 0.
pub fn settled() {
    write(0, 0);
}
