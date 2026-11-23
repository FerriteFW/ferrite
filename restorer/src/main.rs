//! # The restorer image
//!
//! Back to stock for a radio that isn't running Ferrite (BACKUP-13, decision 405): Ferrite Console loads this
//! through the stock bootloader, writes the owner's backup back into Ferrite's ranges with the same requests and
//! the same code as Ferrite's own way back (`STOCK_RESTORE`, BACKUP-7, LINK-8; `ferrite_store::restore`), then
//! `FINISH` restarts into the bootloader and Console sends the stock firmware. If the bootloader's window is missed,
//! the restorer simply starts again (Ferrite would set up its storage over the restored data; this never does).
//!
//! Writes go only inside `ferrite_core::layout::WRITABLE` ([`nor`], the firmware's guard): never calibration, the
//! bootloader's fonts or its update area. It answers only `STOCK_RESTORE`, plain frames only (no compression,
//! as the firmware's restore loop); anything else is `busy`. One thread, no executor: the restore's futures run
//! with `ferrite_store::block`, the chip waited for by a busy wait. The HAL, the board's pins and safe state and
//! the panel bus are the firmware's own files (`#[path]` below), as the reader's.
#![no_std]
#![no_main]

#[allow(dead_code, unexpected_cfgs)]
#[path = "../../firmware/src/board.rs"]
mod board;
mod bringup;
/// A fault stops the restorer in the safe state (it never carries on after one).
mod restart {
    pub fn fault(_pc: Option<u32>) -> bool {
        false
    }
    /// The stop's power-switch exit (SAFE-8.1) clears the firmware's count; the restorer keeps none.
    pub fn settled() {}
}
#[allow(dead_code)]
#[path = "../../firmware/src/bus.rs"]
mod bus;
mod chan;
#[allow(dead_code, unexpected_cfgs)]
#[path = "../../firmware/src/hal/mod.rs"]
mod hal;
mod nor;
#[path = "../../firmware/src/report.rs"] // allows its own dead code
mod report;
#[allow(dead_code)]
#[path = "../../firmware/src/ring.rs"]
mod ring;
mod screen;

use board::{POWER_HOLD, POWER_SW};
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
use cortex_m_rt::{entry, exception, ExceptionFrame};
use ferrite_core::link::{self as wire, msg, Status, MAX_PAYLOAD, MAX_WIRE};
use ferrite_store::restore::{serve, After};
use ferrite_ui::boot::{self, BootInfo};
use hal::{clock, sys, time, uart};
use report::UART_CPS;

/// How the tools know this image (flash.py, Console's core): never a stock image to pad, never Ferrite.
#[used]
#[no_mangle]
static RESTORER_MARK: [u8; 24] = *b"Ferrite restorer: BACK13";

/// The cable's rate: the link's (LINK-11), as Ferrite answers at power-on.
const BAUD: u32 = 115_200;

// ---- start-up (as firmware/src/main.rs) ----

// Entered by a jump from the bootloader: interrupts masked, thread mode on MSP and privileged, then the safe state
// and the bootloader's SysTick and interrupts off, before RAM is initialised.
core::arch::global_asm!(
    ".global __pre_init",
    ".type __pre_init,%function",
    ".thumb_func",
    "__pre_init:",
    "cpsid i",
    "movs r0, #0",
    "msr control, r0",
    "isb",
    "b before_ram",
);

#[no_mangle]
extern "C" fn before_ram() {
    board::before_ram();
}

/// Model tag at 0x080033E0: the stock updater and bootloader check it.
#[link_section = ".model_tag"]
#[used]
#[no_mangle]
static MODEL_TAG: [u8; 16] = *b"RT-950      \0\0\0\0";

/// The same tag at 0x080037E0: the bootloader compares it at every power-on ("Firmware Error!" when it differs).
#[link_section = ".model_tag_boot"]
#[used]
#[no_mangle]
static MODEL_TAG_BOOT: [u8; 16] = *b"RT-950      \0\0\0\0";

type Vector = unsafe extern "C" fn();

/// 96 peripheral vectors: the cable's receive, every other one the fault path.
#[link_section = ".vector_table.interrupts"]
#[no_mangle]
#[used]
static __INTERRUPTS: [Vector; 96] = {
    let mut v: [Vector; 96] = [stray; 96];
    v[hal::irq::UART4 as usize] = uart4;
    v
};

extern "C" fn stray() {
    board::fault()
}
extern "C" fn uart4() {
    uart::isr(UART_CPS)
}

#[exception]
unsafe fn HardFault(ef: &ExceptionFrame) -> ! {
    board::fault_at(Some(ef.pc()))
}

#[exception]
unsafe fn DefaultHandler(_: i16) {
    board::fault()
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    board::fault()
}

// ---- time ----

/// Milliseconds since start-up, from the cycle counter (which wraps every 36 s at 120 MHz: called far more often).
fn now_ms() -> u32 {
    static LAST: AtomicU32 = AtomicU32::new(0);
    static MS: AtomicU32 = AtomicU32::new(0);
    static PART: AtomicU32 = AtomicU32::new(0);
    let c = time::cycles();
    let d = c.wrapping_sub(LAST.swap(c, Relaxed)) as u64 + PART.load(Relaxed) as u64;
    let per = clock::mhz() as u64 * 1000;
    PART.store((d % per) as u32, Relaxed);
    MS.fetch_add((d / per) as u32, Relaxed).wrapping_add((d / per) as u32)
}

// ---- the power switch ----

/// PE0 high (switched off) for 50 ms: as Ferrite (PWR-17) and hwtest, the power hold is released.
#[derive(Default)]
struct PowerSwitch {
    off_since: Option<u32>,
}

impl PowerSwitch {
    fn off(&mut self) -> bool {
        if !POWER_SW.read() {
            self.off_since = None;
            return false;
        }
        let t = now_ms();
        let since = *self.off_since.get_or_insert(t);
        t.wrapping_sub(since) >= 50
    }
}

/// The radio off: safe state, the screen dark, the power hold released. Still running half a second later (the
/// switch back on): a reset, which starts the reader again through the bootloader.
fn power_off(why: &[u8]) -> ! {
    uart::write(UART_CPS, why);
    uart::flush(UART_CPS);
    cortex_m::interrupt::disable();
    board::safe_state();
    hal::backlight::set(0);
    POWER_HOLD.write(false);
    time::delay_ms_busy(500);
    cortex_m::peripheral::SCB::sys_reset()
}


// ---- the restore ----

fn screen(lcd: &mut screen::Screen, note: &[u8]) {
    let b = BootInfo { version: ferrite_core::VERSION, callsign: None, line: Some(b"Back to stock"), owner: None, checks: &[], note: Some(note) };
    boot::draw(lcd, &b);
}

/// One answer out on the cable, whole.
fn send(ty: u8, seq: u8, ans: &[u8]) {
    let mut w = [0u8; MAX_WIRE];
    if let Some(k) = wire::build(ty | wire::ANSWER, seq, ans, &mut w) {
        uart::write(UART_CPS, &w[..k]);
    }
}

#[entry]
fn main() -> ! {
    board::init();
    uart::init(UART_CPS, BAUD);
    // SAFETY: start-up is done; the cable's receive interrupt may run from here.
    unsafe { cortex_m::interrupt::enable() };
    let mut lcd = screen::Screen::new();
    screen(&mut lcd, b"Waiting for Console: keep the cable in");
    let mut chip = nor::Chip::new();
    let (size, ranges, uid) = (chip.size(), nor::WRITE_OK, sys::uid());
    let mut power = PowerSwitch::default();
    let mut rx = [0u8; MAX_WIRE];
    let (mut rn, mut overflow, mut started) = (0usize, false, false);
    let mut ans = [0u8; MAX_PAYLOAD];
    loop {
        if power.off() {
            power_off(b"");
        }
        let Some(b) = uart::getc(UART_CPS) else { continue };
        if b != 0 {
            if rn < rx.len() {
                rx[rn] = b;
                rn += 1;
            } else {
                overflow = true;
            }
            continue;
        }
        let len = core::mem::take(&mut rn);
        if len == 0 || core::mem::take(&mut overflow) {
            continue; // a sync 0x00, or a frame too long for any request
        }
        let (ty, seq, n, after) = match wire::parse(&mut rx[..len]) {
            Err(e) => {
                ans[0] = Status::BadFrame as u8;
                (e.ty, e.seq, 1, After::Next)
            }
            Ok(f) if f.ty != msg::STOCK_RESTORE => {
                ans[0] = Status::Busy as u8;
                (f.ty, f.seq, 1, After::Next)
            }
            Ok(f) => {
                let (ty, seq) = (f.ty, f.seq);
                let mut p = [0u8; MAX_PAYLOAD];
                let pl = f.payload.len();
                p[..pl].copy_from_slice(f.payload);
                if !started {
                    started = true;
                    screen(&mut lcd, b"Restoring: keep the cable in");
                }
                let (n, after) = ferrite_store::block(serve(&mut chip, size, ranges, uid, &p[..pl], &mut ans));
                (ty, seq, n, after)
            }
        };
        send(ty, seq, &ans[..n]);
        if after == After::Reset {
            screen(&mut lcd, b"Done: restarting");
            uart::flush(UART_CPS);
            // The PC has its probes going before the bootloader's ~200 ms window opens (as Ferrite's restore).
            time::delay_ms_busy(300);
            sys::reboot()
        }
    }
}
