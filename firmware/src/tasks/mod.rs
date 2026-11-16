//! The six tasks (`docs/firmware-design.md`, "Software architecture"), each the only owner of its hardware, plus
//! the input sampler that feeds the UI.

pub mod aprs;
pub mod decode;
pub mod dsp;
pub mod gps;
pub mod input;
pub mod link;
pub mod radio;
pub mod restore;
pub mod sstv;
pub mod storage;
pub mod ui;

use crate::chan::EVENTS;
use core::sync::atomic::{AtomicU8, Ordering::Relaxed};
use ferrite_core::msg::{Event, Kind, Source};

/// Boot checks, in the order the boot screen lists them.
pub const CHECK_MCU: u8 = 0;
pub const CHECK_FLASH: u8 = 1;
pub const CHECK_BK0: u8 = 2;
pub const CHECK_BK1: u8 = 3;
pub const CHECK_SI: u8 = 4;
pub const CHECK_CAL: u8 = 5;
/// Six, as the boot screen lists them (decided Oct 6, 2026): MCU, SPI flash, both BK4829s, SI4732, calibration.
pub const CHECKS: usize = 6;

pub const PENDING: u8 = 0;
pub const PASSED: u8 = 1;
pub const FAILED: u8 = 2;
/// Not tested at boot (the SI4732 stays in reset until FM broadcast or HF mode starts it): shown greyed.
pub const UNTESTED: u8 = 3;

/// SELF-5: the SI4732's revision from the boot probe (`SelfTest::si_rev`), written once by the Radio task before
/// its check passes.
pub static SI_REV: [AtomicU8; 5] = [const { AtomicU8::new(0) }; 5];

/// Each check's state. A static rather than only an event, so a result reported before the UI subscribes is
/// not lost.
pub static CHECK_STATE: [AtomicU8; CHECKS] = [const { AtomicU8::new(PENDING) }; CHECKS];


/// A boot check result: kept for the boot screen and announced on the event bus (for the log and the link).
pub fn check(source: Source, index: u8, ok: bool) {
    CHECK_STATE[index as usize].store(if ok { PASSED } else { FAILED }, Relaxed);
    let mut e = Event::new(Kind::Check, source);
    e.payload[0] = index;
    e.payload[1] = ok as u8;
    EVENTS.immediate_publisher().publish_immediate(e);
}
