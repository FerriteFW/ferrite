//! SAFE-8.1, SAFE-8.2 (decision 364): a fault or a hang ends in a restart, not a stop, and the next start says so.
//! A fault keeps its address in RAM the start-up code doesn't clear (`.uninit`), then resets; a hang is the
//! watchdog's reset (hal::wdt). Three restarts without a minute of running between them stop the radio in the
//! safe state with the red LED, as every fault did before, so a fault at start can't loop for ever.
//!
//! SAFE-8.3 (decision 448): a power cut (a power-off to leave a stop, the battery out) loses the RAM record, so the
//! restarts in a row are also kept in the SPI flash ([`keep`], ferrite-store `crash`) until Console has read them.
//! Never from the fault path: that may be the flash code's own fault, or leave the SPI bus mid-command, and its
//! state is not to be trusted. The fault path only notes the fault in RAM and resets, the third time too; the start
//! that follows decides, and writes the record before anything else runs: a restart before the tasks
//! start, a stop (SAFE-8.1) once the radio is in its safe state. Both are a freshly started chip, busy-waited.
use core::cell::Cell;
use core::mem::MaybeUninit;
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use embassy_sync::blocking_mutex::CriticalSectionMutex;
use ferrite_core::msg::CrashRec;

const MAGIC: u32 = 0xFE77_1E5B;
/// Restarts in a row before the radio stays stopped.
pub const MAX_IN_A_ROW: u32 = 3;
/// Running this long clears the count (SAFE-8.1).
pub const SETTLED_MS: u64 = 60_000;
/// `pc` for a watchdog reset (no address).
pub const HANG: u32 = 1;

/// [MAGIC, pc (0: none known), restarts in a row, the three newest restarts in a row (SAFE-8.3: a pc, [`HANG`], or
/// 0), a check word]. The stock bootloader runs between the reset and Ferrite and uses RAM of its own: a record it
/// overwrote fails the check and reads as no record.
#[link_section = ".uninit.RESTART"]
static mut REC: MaybeUninit<[u32; 7]> = MaybeUninit::uninit();
/// SAFE-8.3: the uptime in ticks, kept by the thread executor's loop ([`alive`]), and its complement.
#[link_section = ".uninit.RESTART_UP"]
static mut UP: MaybeUninit<[u32; 2]> = MaybeUninit::uninit();

/// This start's restart, for the UI task (SAFE-8.2): 0 none, [`HANG`], or the fault's address (the high bit set
/// when it is unknown); and the restarts in a row.
pub static RESTARTED: AtomicU32 = AtomicU32::new(0);
pub static IN_A_ROW: AtomicU32 = AtomicU32::new(0);

/// SAFE-8.3: this start's record, made by [`at_start`] for [`keep`].
static NOTE: CriticalSectionMutex<Cell<Option<CrashRec>>> = CriticalSectionMutex::new(Cell::new(None));
/// SAFE-8.3: the record Console hasn't read, for the UI task: its flash address (0: not stored) and contents.
pub static SAVED: CriticalSectionMutex<Cell<Option<(u32, CrashRec)>>> = CriticalSectionMutex::new(Cell::new(None));
/// SAFE-8.3: a record from before this power-on, not shown yet: the start's notice shows it.
pub static SHOW_SAVED: AtomicBool = AtomicBool::new(false);

type Rec = (u32, u32, [u32; 3]);

fn check(w: &[u32; 6]) -> u32 {
    w.iter().enumerate().fold(MAGIC, |c, (i, &x)| c ^ x.rotate_left(7 + 5 * i as u32))
}

fn read() -> Option<Rec> {
    // SAFETY: plain words in RAM, reached only through these volatile accesses (interrupts are off at start-up and
    // in the fault path; the settle clear writes from thread mode alone).
    let r = unsafe { read_volatile(addr_of!(REC) as *const [u32; 7]) };
    let w = [r[0], r[1], r[2], r[3], r[4], r[5]];
    (r[0] == MAGIC && r[6] == check(&w)).then_some((r[1], r[2], [r[3], r[4], r[5]]))
}

fn write(pc: u32, n: u32, h: [u32; 3]) {
    let w = [MAGIC, pc, n, h[0], h[1], h[2]];
    // SAFETY: as in `read`.
    unsafe { write_volatile(addr_of_mut!(REC) as *mut [u32; 7], [w[0], w[1], w[2], w[3], w[4], w[5], check(&w)]) }
}

/// SAFE-8.3: the thread executor's loop, each pass: the uptime for the next restart's record (two RAM words).
#[inline]
pub fn alive(ticks: u32) {
    // SAFETY: as in `read`; only the thread executor's loop writes it, `at_start` reads it before that runs.
    unsafe { write_volatile(addr_of_mut!(UP) as *mut [u32; 2], [ticks, !ticks]) }
}

/// The uptime [`alive`] kept, seconds (0 when none: a power-on, or overwritten).
fn uptime_s() -> u32 {
    // SAFETY: as in `read`.
    let u = unsafe { read_volatile(addr_of!(UP) as *const [u32; 2]) };
    if u[1] == !u[0] {
        (u[0] as u64 / embassy_time::TICK_HZ) as u32
    } else {
        0
    }
}

/// The fault path: the address kept with the count; whether to restart (false: the radio stays stopped). The
/// third in a row restarts too (SAFE-8.3): the start that follows stops the radio after keeping the record. Only a
/// fault past that (in the stop's own keeping) stops here, so a fault in [`keep`] can't loop for ever.
pub fn fault(pc: Option<u32>) -> bool {
    let (n, h) = read().map_or((0, [0; 3]), |(_, n, h)| (n, h));
    write(pc.unwrap_or(1 << 31), n + 1, h);
    n < MAX_IN_A_ROW
}

/// At start-up (reset cause `cause`, `hal::sys::RESET_*`): what this start follows, kept for the UI task. False
/// when it is one restart too many: the caller stops in the safe state.
pub fn at_start(cause: u32) -> bool {
    use crate::hal::sys::{RESET_POR, RESET_WDT};
    let rec = if cause & RESET_POR != 0 { None } else { read() };
    let up_s = if cause & RESET_POR != 0 { 0 } else { uptime_s() };
    alive(0);
    let h = rec.map_or([0; 3], |r| r.2);
    let (pc, n) = match rec {
        // the fault path's own reset
        Some((pc, n, _)) if pc != 0 && cause & RESET_WDT == 0 => (pc, n),
        // a hang: the watchdog ran out
        _ if cause & RESET_WDT != 0 => (HANG, rec.map_or(0, |r| r.1) + 1),
        // an ordinary start (power, the updater's reset, `reboot`): no restart, the count from before kept
        Some((_, n, _)) => (0, n),
        None => (0, 0),
    };
    // SAFE-8.3: the restarts in a row, newest first; cleared with the count
    let h = if pc != 0 { [pc, h[0], h[1]] } else if n == 0 { [0; 3] } else { h };
    write(0, n, h); // the address is reported once; the count stays until the radio has run a minute
    let go = n < MAX_IN_A_ROW || pc == 0;
    if pc != 0 {
        RESTARTED.store(pc, Relaxed);
        IN_A_ROW.store(n, Relaxed);
        let mut build = [0u8; 8];
        let b = ferrite_core::BUILD.as_bytes();
        let k = b.len().min(8);
        build[..k].copy_from_slice(&b[..k]);
        NOTE.lock(|c| c.set(Some(CrashRec { pcs: h, n, up_s, stopped: !go, build })));
    }
    go
}

/// SAFE-8.3: at start-up, before the tasks (a restart), or at a stop in the safe state (SAFE-8.1): this start's
/// record written to the SPI flash; at any other start the newest record Console hasn't read found, for the notice
/// (once) and the readback. Busy-waited: a written record is a program of 64 bytes, a sector erase every 64.
#[inline(never)] // in the slow flash (memory.x): start-up only
pub fn keep() {
    if cfg!(feature = "bringup") {
        return; // the bring-up build writes nothing to the chip
    }
    let note = NOTE.lock(|c| c.take());
    let Some(f) = crate::store::raw_chip() else {
        SAVED.lock(|c| c.set(note.map(|r| (0, r))));
        return;
    };
    // The watchdog isn't running at start-up; fed anyway, as the stop feeds it
    let mut spin = || {
        crate::hal::wdt::feed();
        crate::hal::time::delay_us(100);
    };
    use ferrite_store::crash;
    let saved = match note {
        // a restart's start shows its notice now; a stop's shows at the next start
        Some(r) => Some((crash::append(&f, &r, !r.stopped, &mut spin).unwrap_or(0), r)),
        None => match crash::newest(&f) {
            Ok(Some((at, r, shown, false))) => {
                if !shown {
                    SHOW_SAVED.store(true, Relaxed);
                    let _ = crash::mark_shown(&f, at, &mut spin);
                }
                Some((at, r))
            }
            _ => None,
        },
    };
    SAVED.lock(|c| c.set(saved));
}

/// A minute of running: the count back to 0.
pub fn settled() {
    write(0, 0, [0; 3]);
}
