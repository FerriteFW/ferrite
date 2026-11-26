//! The screen's backlight (PC6) and the keypad light (PB3), each dimmed by PWM (manual DISP-16, DISP-22, DISP-23):
//! the screen's at about 31.7 kHz with its period dithered (DISP-23.1, decision 472), the keypad's at 2 kHz
//! (decision 427). The UI task gives both the same level, 0 7 4 2's, so one brightness level dims both.
//!
//! PC6's timer functions are TMR8 channel 1 and, on TMR3's full remap, TMR3 channel 1. Ferrite's TMR3 is the Embassy
//! time driver (its channel 1 is the alarm), so the screen's PWM comes from TMR8 channel 1 instead, PC6's own
//! (unremapped) timer function on the AT32F403A (datasheet pin table: PC6 = TMR8_CH1; TMR3_CH1 only with the remap).
//! PB3's only timer function is TMR2 channel 2 on TMR2's partial remap (RM_AT32F403A_407 V2.07, 7.4.2 IOMUX_REMAP
//! bits 9-8 TMR2_MUX = 01: CH1/EXT PA15, CH2 PB3, CH3 PA2, CH4 PA3). Of those only CH2's output is enabled; PA15
//! (`board::REPEAT_AUDIO`) stays a GPIO output and PA2 (audio capture) analog, so the remap moves nothing else.
//! PB3 is JTDO after reset: `board::init` has turned JTAG off (SWJTAG_MUX = 010) before this runs, and the remap
//! write sets that field to 010 again rather than trusting its read-back. The audio capture's ADC trigger, TMR2 CH2
//! in stock, moved to TMR4 CH4 for this (`super::audio`).
//!
//! The screen's dither (DISP-23.1): DMA2 channel 1 writes TMR8's period register from [`PERIODS`] at every TMR8
//! overflow, circular, no CPU. TMR8_UP is DMA2 channel 1's request in the fixed mapping Ferrite uses
//! (RM_AT32F403A_407 V2.07, 9.3.7 Table 9-4; DMA_FLEX_EN stays 0 for both controllers, 9.3.8), asked for by TMR8
//! IDEN's OVFDEN (14.4.4.4). PR is buffered (PRBEN), so each write is the next period's. One plain transfer, not the
//! timer's DMA burst: a burst cut short by a stop could leave the timer and the table out of step. The compare
//! (C1DT) stays fixed, so each period's duty moves with its length (+/-1 %, unseen; the mean is the level's).
//! DMA2 channel 1's interrupt line is the Radio executor's (`main::IRQ_RADIO`): its interrupts stay off, so the
//! hardware never raises it. Dark and full on (and dither 0) are plain: no dither, DMA2 channel 1 off and, with
//! the playback off too, DMA2 unclocked (`super::audio::dma2_use`). Everything that knows this is in this file, to
//! check on the radio with `mem32`:
//!
//! - TMR8 at 0x4001_3400: CTRL1 +0x00 = 0x81 (counting, period buffered), IDEN +0x0C = 0x100 while dithering,
//!   CM1 +0x18 = 0x68 (PWM mode A, C1DT buffered), CCTRL +0x20 = 0x01 (channel 1 out, active high), DIV +0x28 = 0,
//!   PR +0x2C = 3786 (31.7 kHz from the 120 MHz timer clock) or, dithering, 3748 to 3824; 756 (748 to 764) at Low
//!   Power Mode's 24 MHz (PWR-24.9), C1DT +0x34 = the duty in counts (0 dark;
//!   the period, more than PR, full on: high all the time, no edges), BRK +0x44 bit 15 (outputs on: an advanced
//!   timer's).
//! - DMA2 at 0x4002_0400, channel 1 at +0x08: CTRL = 0xA31 while dithering (on, memory to peripheral, circular,
//!   memory increments, 32-bit both sides, low priority), DTCNT +0x0C counting down from 32, PADDR +0x10 =
//!   0x4001_342C (TMR8 PR), MADDR +0x14 = [`PERIODS`]. CRM AHBEN 0x4002_1014 bit 1 (DMA2's clock).
//! - TMR2 at 0x4000_0000: CTRL1 = 0x81, CM1 +0x18 = 0x6800 (channel 2 PWM mode A, C2DT buffered), CCTRL +0x20 =
//!   0x10 (channel 2 only), DIV = 0, PR = 59999 (11999 at 24 MHz), C2DT +0x38 = the keypad's duty in counts (0 off).
//! - IOMUX_REMAP 0x4001_0004: bits 9-8 = 01, bits 26-24 = 010.
//! - PC6 and PB3 as peripheral outputs: GPIOC CFGLR (0x4001_1000) bits 24-27 = 0xB, GPIOB CFGLR (0x4001_0C00)
//!   bits 12-15 = 0xB.
use super::audio::{ch, dma2_use, CH_CTRL, CH_DTCNT, CH_MADDR, CH_PADDR, DMA2, DMA2_BACKLIGHT, DMA_DTD_M2P, DMA_EN};
use super::audio::{DMA_LM, DMA_MINCM, DMA_MWIDTH_32, DMA_PWIDTH_32};
use super::gpio::{pin, Mode, GPIOB, GPIOC};
use super::regs::*;
use super::{clock, modify, rd, wr};
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering::Relaxed};
use ferrite_core::clocktree::Tree;
use ferrite_core::screenpwm::{self as pwm, DITHER, STEPS};

/// The keypad light's PWM frequency (DISP-23.1, ferrite#74, decision 427). At 25 kHz its edges were heard as noise
/// on 2 m (146.075 MHz, a multiple of 25 kHz), gone with the key light off. A twelfth of the edges: about 11 dB
/// less noise in any channel, the strong lines on 25 kHz multiples spread thin across every channel. Above the
/// flicker guidance (IEEE 1789: 1.25 kHz at full modulation), and not heard in the audio from 25 kHz down to 500 Hz
/// (black, Nov 22, 2026). The pins are already at the AT32's weakest drive (IOMC 11, normal; no slower edge to pick).
/// The screen's moved off 2 kHz (decision 472): its ripple reached the audio on some radios.
pub const KEYS_HZ: u32 = 2_000;
/// The keypad light's counts per period (PR + 1) at a speed: 60000 at 120 MHz (under the timers' 16-bit 65536),
/// 12000 at Low Power Mode's 24 (PWR-24.9).
fn keys_period(t: Tree) -> u32 {
    t.tmr_hz() / KEYS_HZ
}
/// The duties last set, thousandths, for a re-time at a speed switch.
static SCREEN: AtomicU16 = AtomicU16::new(0);
static KEYS: AtomicU16 = AtomicU16::new(0);
/// The screen's nominal frequency and dither depth (tenths of a percent): DISP-23.1's, or a bench build's
/// `lcd hz` / `lcd dither` (TERM-40).
static HZ: AtomicU32 = AtomicU32::new(pwm::SCREEN_HZ);
static DEPTH: AtomicU16 = AtomicU16::new(pwm::DITHER_TENTHS);
/// The dither runs (DMA2 channel 1 on).
static DITHERING: AtomicBool = AtomicBool::new(false);
/// The screen's periods less one (PR values), one per TMR8 overflow, read by DMA2 channel 1 in turn (DISP-23.1).
static PERIODS: [AtomicU32; STEPS] = [const { AtomicU32::new(0) }; STEPS];

const CTRL1_CNTEN: u32 = 1 << 0;
const CTRL1_PRBEN: u32 = 1 << 7;
/// IDEN: the overflow's DMA request (OVFDEN, RM 14.4.4.4).
const IDEN_OVFDEN: u32 = 1 << 8;
/// CM1: channel 1 output, PWM mode A (C1OCTRL = 110), its data register buffered (C1OBEN).
const CM1_C1_PWM_A: u32 = 6 << 4 | 1 << 3;
/// CM1: channel 2 the same (C2OCTRL bits 12-14, C2OBEN bit 11); channel 1 left frozen and off.
const CM1_C2_PWM_A: u32 = 6 << 12 | 1 << 11;
const CCTRL_C1EN: u32 = 1 << 0;
const CCTRL_C2EN: u32 = 1 << 4;
/// DMA2 channel 1 (TMR8_UP, RM 9.3.7 Table 9-4).
const DMA_CH: u32 = 1;

/// TMR8 and TMR2 on, both at 0 (dark), then PC6 and PB3 handed to them. Called once by the UI task before the
/// panel lights, after `board::init` (JTAG off, both pins low outputs).
pub fn init() {
    let t = clock::tree();
    modify(CRM_APB2EN, 0, CRM_APB2_TMR8);
    wr(TMR8 + TMR_CTRL1, 0);
    wr(TMR8 + TMR_DIV, 0);
    wr(TMR8 + TMR_PR, nominal(t) - 1);
    wr(TMR8 + TMR_C1DT, 0);
    wr(TMR8 + TMR_CM1, CM1_C1_PWM_A);
    wr(TMR8 + TMR_CCTRL, CCTRL_C1EN);
    wr(TMR8 + TMR_SWEVT, 1); // load PR and C1DT now
    wr(TMR8 + TMR_BRK, TMR_BRK_OEN);
    wr(TMR8 + TMR_CTRL1, CTRL1_PRBEN | CTRL1_CNTEN);
    pin(GPIOC, 6).config(Mode::AfPp);

    let p = keys_period(t);
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

/// The screen's nominal period in counts at a speed (DISP-23.1): 3787 at 120 MHz, 757 at 24.
fn nominal(t: Tree) -> u32 {
    pwm::counts(t.tmr_hz(), HZ.load(Relaxed))
}

/// The screen's duty in thousandths.
pub fn set(permille: u16) {
    SCREEN.store(permille, Relaxed);
    if rd(CRM_APB2EN) & CRM_APB2_TMR8 != 0 {
        critical_section::with(|_| screen(clock::tree()));
    }
}

/// Bench builds (TERM-40): the screen's nominal frequency (clamped to the bench's range) and dither depth in
/// tenths of a percent, on the fly; [`pwm::SCREEN_HZ`] and [`pwm::DITHER_TENTHS`] give DISP-23.1's back.
#[cfg(feature = "bench")]
pub fn tune(hz: u32, tenths: u16) {
    HZ.store(hz.clamp(pwm::HZ_MIN, pwm::HZ_MAX), Relaxed);
    DEPTH.store(tenths.min(pwm::DITHER_MAX), Relaxed);
    if rd(CRM_APB2EN) & CRM_APB2_TMR8 != 0 {
        critical_section::with(|_| {
            stop(); // a new table and period from the start
            screen(clock::tree());
        });
    }
}

/// The screen's PWM for the level in [`SCREEN`] at speed `t` (interrupts off: a speed switch can't come between).
/// Plain (DISP-23.1: dark, full on, no dither): the dither stopped, the nominal period, the compare 0 (low all the
/// time) or the whole period (high all the time; PWM mode A is high while C1DT > the count, and the count never
/// reaches PR + 1). Otherwise the dither's periods, the compare the level's share of the nominal period.
fn screen(t: Tree) {
    let permille = SCREEN.load(Relaxed);
    let depth = DEPTH.load(Relaxed);
    let n = nominal(t);
    match pwm::act(DITHERING.load(Relaxed), permille, depth) {
        pwm::Act::Plain => {
            stop();
            wr(TMR8 + TMR_PR, n - 1);
        }
        pwm::Act::Start => {
            wr(TMR8 + TMR_PR, n - 1); // until the first overflow's transfer
            for (p, &v) in PERIODS.iter().zip(DITHER.iter()) {
                p.store(pwm::step(n, depth, v) - 1, Relaxed);
            }
            start();
        }
        pwm::Act::Keep => {}
    }
    wr(TMR8 + TMR_C1DT, pwm::duty(n, permille));
}

/// The dither on: DMA2 channel 1 from the top of [`PERIODS`], then TMR8's overflow asking for it.
fn start() {
    dma2_use(DMA2_BACKLIGHT, true);
    let c = ch(DMA2, DMA_CH);
    wr(c + CH_CTRL, 0);
    wr(c + CH_PADDR, TMR8 + TMR_PR);
    wr(c + CH_MADDR, PERIODS.as_ptr() as u32);
    wr(c + CH_DTCNT, STEPS as u32);
    // Low priority (CHPL 00): the playback's channel 3 goes first.
    wr(c + CH_CTRL, DMA_DTD_M2P | DMA_MINCM | DMA_LM | DMA_PWIDTH_32 | DMA_MWIDTH_32);
    modify(c + CH_CTRL, 0, DMA_EN);
    modify(TMR8 + TMR_IDEN, 0, IDEN_OVFDEN);
    DITHERING.store(true, Relaxed);
}

/// The dither off (nothing if it wasn't on): no more requests, channel 1 off, DMA2's clock given back.
fn stop() {
    if !DITHERING.swap(false, Relaxed) {
        return;
    }
    modify(TMR8 + TMR_IDEN, IDEN_OVFDEN, 0);
    modify(ch(DMA2, DMA_CH) + CH_CTRL, DMA_EN, 0);
    dma2_use(DMA2_BACKLIGHT, false);
}

/// The keypad light's duty in thousandths (0 off).
pub fn keypad(permille: u16) {
    KEYS.store(permille, Relaxed);
    wr(TMR2 + TMR_C2DT, pwm::duty(keys_period(clock::tree()), permille));
}

/// PWR-24.9: both PWMs from the new tree, at the same duties (interrupts are off). Before `init` (TMR8 unclocked)
/// nothing to do. The screen's dither restarts with the new tree's periods; period and duty are loaded together by
/// a software event (the counters restart: one short period, unseen).
pub(super) fn retime(t: Tree) {
    if rd(CRM_APB2EN) & CRM_APB2_TMR8 == 0 {
        return;
    }
    stop();
    screen(t);
    wr(TMR8 + TMR_SWEVT, 1);
    let p = keys_period(t);
    wr(TMR2 + TMR_PR, p - 1);
    wr(TMR2 + TMR_C2DT, pwm::duty(p, KEYS.load(Relaxed)));
    wr(TMR2 + TMR_SWEVT, 1);
}

/// Bench builds (TERM-40): the screen's dither runs now as this file believes it (not while the light is dark or
/// full on, DISP-23.1), and as the hardware says: DMA2 clocked, its channel 1 on and TMR8's OVFDEN set. The `lcd`
/// readback shows both, so a light gone dark by its timeout (plain) isn't taken for a dither that failed.
#[cfg(feature = "bench")]
pub fn dithering() -> (bool, bool) {
    let hw = rd(super::audio::CRM_AHBEN) & super::audio::CRM_AHB_DMA2 != 0
        && rd(ch(DMA2, DMA_CH) + CH_CTRL) & DMA_EN != 0
        && rd(TMR8 + TMR_IDEN) & IDEN_OVFDEN != 0;
    (DITHERING.load(Relaxed), hw)
}

/// Bench builds (TERM-40): the screen light's duty now, thousandths (0 while dark).
#[cfg(feature = "bench")]
pub fn level() -> u16 {
    SCREEN.load(Relaxed)
}
