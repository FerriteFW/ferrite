//! The screen's backlight (PC6) and the keypad light (PB3), each dimmed by PWM at the same 25 kHz (manual DISP-16,
//! DISP-22, DISP-23). The UI task gives both the same duty, 0 7 4 2's level, so one brightness level dims both.
//!
//! PC6's timer functions are TMR8 channel 1 and, on TMR3's full remap, TMR3 channel 1. Ferrite's TMR3 is the Embassy
//! time driver (its channel 1 is the alarm), so the screen's PWM comes from TMR8 channel 1 instead, PC6's own
//! (unremapped) timer function on the AT32F403A (datasheet pin table: PC6 = TMR8_CH1; TMR3_CH1 only with the remap).
//! PB3's only timer function is TMR2 channel 2 on TMR2's partial remap (RM_AT32F403A_407 V2.07, 7.4.2 IOMUX_REMAP
//! bits 9-8 TMR2_MUX = 01: CH1/EXT PA15, CH2 PB3, CH3 PA2, CH4 PA3). Of those only CH2's output is enabled; PA15
//! (`board::REPEAT_AUDIO`) stays a GPIO output and PA2 (audio capture) analog, so the remap moves nothing else.
//! PB3 is JTDO after reset: `board::init` has turned JTAG off (SWJTAG_MUX = 010) before this runs, and the remap
//! write sets that field to 010 again rather than trusting its read-back. The audio capture's ADC trigger, TMR2 CH2
//! in stock, moved to TMR4 CH4 for this (`super::audio`). Everything that knows this is in this file, to check on
//! the radio with `mem32`:
//!
//! - TMR8 at 0x4001_3400: CTRL1 +0x00 = 0x81 (counting, period buffered), CM1 +0x18 = 0x68 (PWM mode A, C1DT
//!   buffered), CCTRL +0x20 = 0x01 (channel 1 out, active high), DIV +0x28 = 0, PR +0x2C = 4799 (25 kHz from the
//!   120 MHz timer clock: above flicker and hearing; 959 at Low Power Mode's 24 MHz, PWR-24.9), C1DT +0x34 = the duty in counts (0 dark, 4800 full on),
//!   BRK +0x44 bit 15 (outputs on: an advanced timer's).
//! - TMR2 at 0x4000_0000: CTRL1 = 0x81, CM1 +0x18 = 0x6800 (channel 2 PWM mode A, C2DT buffered), CCTRL +0x20 =
//!   0x10 (channel 2 only), DIV = 0, PR = 4799, C2DT +0x38 = the keypad's duty in counts (0 off).
//! - IOMUX_REMAP 0x4001_0004: bits 9-8 = 01, bits 26-24 = 010.
//! - PC6 and PB3 as peripheral outputs: GPIOC CFGLR (0x4001_1000) bits 24-27 = 0xB, GPIOB CFGLR (0x4001_0C00)
//!   bits 12-15 = 0xB.
use super::gpio::{pin, Mode, GPIOB, GPIOC};
use super::regs::*;
use super::{clock, modify, rd, wr};
use core::sync::atomic::{AtomicU16, Ordering::Relaxed};
use ferrite_core::clocktree::Tree;

/// The PWM frequency.
pub const PWM_HZ: u32 = 25_000;
/// Counts per period (PR + 1) at a speed: 4800 at 120 MHz, 960 at Low Power Mode's 24 (PWR-24.9).
fn period(t: Tree) -> u32 {
    t.tmr_hz() / PWM_HZ
}
/// The duties last set, thousandths, for a re-time at a speed switch.
static SCREEN: AtomicU16 = AtomicU16::new(0);
static KEYS: AtomicU16 = AtomicU16::new(0);

const CTRL1_CNTEN: u32 = 1 << 0;
const CTRL1_PRBEN: u32 = 1 << 7;
/// CM1: channel 1 output, PWM mode A (C1OCTRL = 110), its data register buffered (C1OBEN).
const CM1_C1_PWM_A: u32 = 6 << 4 | 1 << 3;
/// CM1: channel 2 the same (C2OCTRL bits 12-14, C2OBEN bit 11); channel 1 left frozen and off.
const CM1_C2_PWM_A: u32 = 6 << 12 | 1 << 11;
const CCTRL_C1EN: u32 = 1 << 0;
const CCTRL_C2EN: u32 = 1 << 4;

/// TMR8 and TMR2 on, both at 0 (dark), then PC6 and PB3 handed to them. Called once by the UI task before the
/// panel lights, after `board::init` (JTAG off, both pins low outputs).
pub fn init() {
    modify(CRM_APB2EN, 0, CRM_APB2_TMR8);
    wr(TMR8 + TMR_CTRL1, 0);
    wr(TMR8 + TMR_DIV, 0);
    let p = period(clock::tree());
    wr(TMR8 + TMR_PR, p - 1);
    wr(TMR8 + TMR_C1DT, 0);
    wr(TMR8 + TMR_CM1, CM1_C1_PWM_A);
    wr(TMR8 + TMR_CCTRL, CCTRL_C1EN);
    wr(TMR8 + TMR_SWEVT, 1); // load PR and C1DT now
    wr(TMR8 + TMR_BRK, TMR_BRK_OEN);
    wr(TMR8 + TMR_CTRL1, CTRL1_PRBEN | CTRL1_CNTEN);
    pin(GPIOC, 6).config(Mode::AfPp);

    modify(CRM_APB1EN, 0, CRM_APB1_TMR2);
    modify(IOMUX_REMAP, IOMUX_TMR2_MUX_MASK | IOMUX_SWJTAG_MASK, IOMUX_TMR2_MUX_PB3 | IOMUX_SWJTAG_SWD_ONLY);
    wr(TMR2 + TMR_CTRL1, 0);
    wr(TMR2 + TMR_DIV, 0);
    wr(TMR2 + TMR_PR, p - 1);
    wr(TMR2 + TMR_C2DT, 0);
    wr(TMR2 + TMR_CM1, CM1_C2_PWM_A);
    wr(TMR2 + TMR_CCTRL, CCTRL_C2EN);
    wr(TMR2 + TMR_SWEVT, 1);
    wr(TMR2 + TMR_CTRL1, CTRL1_PRBEN | CTRL1_CNTEN);
    pin(GPIOB, 3).config(Mode::AfPp);
}

/// Thousandths to counts: 0 is dark (the output held low), 1000 full on (always high).
fn counts(t: Tree, permille: u16) -> u32 {
    let p = period(t);
    if permille >= 1000 {
        p
    } else {
        p * permille as u32 / 1000
    }
}

/// The screen's duty in thousandths.
pub fn set(permille: u16) {
    SCREEN.store(permille, Relaxed);
    wr(TMR8 + TMR_C1DT, counts(clock::tree(), permille));
}

/// The keypad light's duty in thousandths (0 off).
pub fn keypad(permille: u16) {
    KEYS.store(permille, Relaxed);
    wr(TMR2 + TMR_C2DT, counts(clock::tree(), permille));
}

/// The keypad light's PWM at `hz` (0: the normal [`PWM_HZ`]) by TMR2's prescaler, the duty unchanged (a bench test,
/// ferrite#74: whether a lower frequency is heard in the audio). A speed switch keeps it (`retime` writes PR only).
#[cfg(feature = "bench")]
pub fn keypad_hz(hz: u16) {
    let div = if hz == 0 { 0 } else { (PWM_HZ / hz as u32).clamp(1, 65_536) - 1 };
    if rd(TMR2 + TMR_DIV) != div {
        wr(TMR2 + TMR_DIV, div);
        wr(TMR2 + TMR_SWEVT, 1);
    }
}

/// PWR-24.9: both PWMs at 25 kHz from the new tree, at the same duties (interrupts are off). Before `init`
/// (TMR8 unclocked) nothing to do. Period and duty are loaded together by a software event (the counters restart:
/// one short period, unseen).
pub(super) fn retime(t: Tree) {
    if rd(CRM_APB2EN) & CRM_APB2_TMR8 == 0 {
        return;
    }
    let p = period(t);
    wr(TMR8 + TMR_PR, p - 1);
    wr(TMR8 + TMR_C1DT, counts(t, SCREEN.load(Relaxed)));
    wr(TMR8 + TMR_SWEVT, 1);
    wr(TMR2 + TMR_PR, p - 1);
    wr(TMR2 + TMR_C2DT, counts(t, KEYS.load(Relaxed)));
    wr(TMR2 + TMR_SWEVT, 1);
}
