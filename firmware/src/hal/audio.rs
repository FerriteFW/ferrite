//! The audio converters, each paced by a timer and fed by DMA into a ping-pong buffer (roadmap 2f, 5b, 5c):
//!
//! - **Capture:** ADC1 channel 2 (PA2, the BK4829s' EARO), triggered by TMR4 channel 4 at
//!   [`ferrite_core::audio::ADC_HZ`], DMA1 channel 1 into [`ADC_BUF`]; each half done raises DMA1 channel 1's
//!   interrupt, which signals [`ADC_HALF`]. Stock's path, at 48 kHz instead of 9.6 (`analysis/audio_path.md`:
//!   stock triggers on TMR2 CH2, DMA1 ch1). Ferrite's TMR2 dims the keypad light ([`super::backlight`], DISP-22),
//!   so the trigger is TMR4 CH4, the other timer compare ADC1's ordinary group takes. Only its internal compare
//!   event is used: TMR4 CH4's pin, PB9 (Bluetooth power), stays a general-purpose output, so the channel's
//!   output never reaches it.
//! - **Playback:** DAC1 (PA4, output buffer off as stock), triggered by TMR6's update at
//!   [`ferrite_core::audio::DAC_HZ`], DMA2 channel 3 from [`DAC_BUF`]; each half played signals [`DAC_HALF`] for
//!   the DSP task to refill. PC12 high routes DAC1 into the BK4829's microphone input while it plays, low after
//!   (stock: set at 0x080064EA, cleared at 0x08006578).
//!
//! The software reads (battery PA1, mic level PA0, the sensors: [`super::adc::read`]) go through ADC1's preempted
//! group, so they run between captures without stopping them. All of it is bench-confirm: nothing here has run
//! on a radio yet.

use super::gpio::{pin, Mode, Pin, GPIOA, GPIOC};
use super::regs::*;
use super::{clock, irq, modify, rd, wr};
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU8, Ordering::Relaxed};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as M;
use embassy_sync::signal::Signal;
use ferrite_core::audio::{ADC_HZ, BLOCK_DAC, BLOCK_IN, DAC_HZ};

const CRM_AHBEN: u32 = 0x4002_1014;
const CRM_AHB_DMA1: u32 = 1 << 0;
const CRM_AHB_DMA2: u32 = 1 << 1;

const DMA1: u32 = 0x4002_0000;
pub(super) const DMA2: u32 = 0x4002_0400;
const DMA_STS: u32 = 0x00;
const DMA_CLR: u32 = 0x04;
/// Channel n's registers (1-based): control, count, peripheral and memory address.
pub(super) const fn ch(dma: u32, n: u32) -> u32 {
    dma + 0x08 + 20 * (n - 1)
}
pub(super) const CH_CTRL: u32 = 0x00;
pub(super) const CH_DTCNT: u32 = 0x04;
pub(super) const CH_PADDR: u32 = 0x08;
pub(super) const CH_MADDR: u32 = 0x0C;
pub(super) const DMA_EN: u32 = 1 << 0;
const DMA_FDTIEN: u32 = 1 << 1; // transfer complete
const DMA_HDTIEN: u32 = 1 << 2; // half transfer
pub(super) const DMA_DTD_M2P: u32 = 1 << 4; // memory to peripheral
pub(super) const DMA_LM: u32 = 1 << 5; // circular
pub(super) const DMA_MINCM: u32 = 1 << 7;
const DMA_PWIDTH_16: u32 = 1 << 8;
const DMA_MWIDTH_16: u32 = 1 << 10;
const DMA_CHPL_HIGH: u32 = 2 << 12;
pub(super) const DMA_PWIDTH_32: u32 = 2 << 8;
pub(super) const DMA_MWIDTH_32: u32 = 2 << 10;
/// A channel's flags in DMA_STS / DMA_CLR: global, complete, half, error.
const fn flags(n: u32) -> u32 {
    0xF << (4 * (n - 1))
}
const fn half_flag(n: u32) -> u32 {
    1 << (4 * (n - 1) + 2)
}

/// CM2: channel 4 output, PWM mode A (C4OCTRL = 110, bits 12-14).
const TMR_CM2_C4_PWM1: u32 = 6 << 12;
const TMR_CCTRL_C4EN: u32 = 1 << 12;
const TMR_CTRL1_CNTEN: u32 = 1 << 0;

const ADC_PSQ: u32 = ADC1 + 0x38; // preempted sequence: with one conversion, the channel is in bits 15-19
const ADC_PDT1: u32 = ADC1 + 0x3C;
const ADC_CTRL2_DMAEN: u32 = 1 << 8;
const ADC_CTRL2_OCTESEL_MASK: u32 = 7 << 17;
/// OCTESEL = 0101: TMR4 CH4 event (RM_AT32F403A_407 V2.07, Table 19-1 and 19.6.3 ADC_CTRL2; OCTESEL[3] is bit 25,
/// left 0). Was 0011, TMR2 CH2, before the keypad light took TMR2.
const ADC_CTRL2_OCTESEL_TMR4CH4: u32 = 5 << 17;
const ADC_CTRL2_PCTESEL_SW: u32 = 7 << 12;
const ADC_CTRL2_PCTEN: u32 = 1 << 15;
const ADC_CTRL2_PCSWTRG: u32 = 1 << 21;
const ADC_STS_PCCE: u32 = 1 << 2;
/// Channel 2's sample time code in SPT2 (bits 6-8): 28.5 cycles, stock's.
const SPT2_CH2_MASK: u32 = 7 << 6;
const SPT2_CH2_28: u32 = 3 << 6;

const DAC_CTRL_D1OBDIS: u32 = 1 << 1;
const DAC_CTRL_D1DMAEN: u32 = 1 << 12;

/// DMA1 channel 1 (ADC1) and DMA2 channel 3 (DAC1) interrupt lines.
pub const IRQ_ADC_DMA: u16 = 11;
pub const IRQ_DAC_DMA: u16 = 58;
/// Above the DSP executor (6): a half is announced at once.
const PRIO: u8 = 5;

/// PC12: DAC1 into the transmitting BK4829's microphone input (stock's APRS switch; in `board::SAFE_LOW`).
pub const MIC_INJECT: Pin = pin(GPIOC, 12);

/// The capture buffer: two halves of [`BLOCK_IN`] samples, written by DMA (an AtomicU16 is a u16 in memory).
pub static ADC_BUF: [AtomicU16; 2 * BLOCK_IN] = [const { AtomicU16::new(0) }; 2 * BLOCK_IN];
/// The playback buffer: two halves of [`BLOCK_DAC`] DAC codes, read by DMA.
pub static DAC_BUF: [AtomicU16; 2 * BLOCK_DAC] = [const { AtomicU16::new(0) }; 2 * BLOCK_DAC];
/// Which half is ready (0 first, 1 second): captured, or played and free to refill.
pub static ADC_HALF: Signal<M, u8> = Signal::new();
pub static DAC_HALF: Signal<M, u8> = Signal::new();
/// Halves the DSP task didn't take in time (it was busy): a capture overrun, a block of audio lost (APRS-56.4: the
/// readback's `decode` line, `late`).
pub static ADC_LATE: AtomicU32 = AtomicU32::new(0);
/// The cycle count when the last capture half was announced: the DSP task's wake-up delay (APRS-56.4).
pub static ADC_AT: AtomicU32 = AtomicU32::new(0);
/// The capture runs (between [`capture_start`] and [`capture_stop`]).
pub static CAPTURING: AtomicBool = AtomicBool::new(false);

/// A timer's reload for `hz` from its clock (TMR4 and TMR6 are on APB1; with APB1 /2 they run at twice it, 120 MHz;
/// stock's 9599.2 Hz is 120 MHz / 12501). Fits 16 bits down to 1.9 kHz.
fn reload(hz: u32) -> u32 {
    clock::tree().reload(hz)
}

/// Capture on: PA2 analog, TMR4 at the ADC rate, ADC1 on its trigger, DMA1 channel 1 circular. ADC1 must have
/// been through [`super::adc::init`].
pub fn capture_start() {
    CAPTURING.store(true, Relaxed);
    pin(GPIOA, 2).config(Mode::Analog);
    critical_section::with(|_| modify(CRM_AHBEN, 0, CRM_AHB_DMA1)); // beside DMA2's users (`dma2_use`)
    modify(CRM_APB1EN, 0, CRM_APB1_TMR4);
    let c = ch(DMA1, 1);
    wr(c + CH_CTRL, 0);
    wr(DMA1 + DMA_CLR, flags(1));
    wr(c + CH_PADDR, ADC_ODT);
    wr(c + CH_MADDR, ADC_BUF.as_ptr() as u32);
    wr(c + CH_DTCNT, ADC_BUF.len() as u32);
    wr(c + CH_CTRL, DMA_MINCM | DMA_LM | DMA_PWIDTH_16 | DMA_MWIDTH_16 | DMA_CHPL_HIGH | DMA_HDTIEN | DMA_FDTIEN);
    modify(c + CH_CTRL, 0, DMA_EN);
    // ADC1: channel 2 alone in the ordinary group, short sample time, each conversion on TMR4 CH4, to DMA.
    modify(ADC_SPT2, SPT2_CH2_MASK, SPT2_CH2_28);
    wr(ADC_OSQ1, 0);
    wr(ADC_OSQ3, 2);
    modify(ADC_CTRL2, ADC_CTRL2_OCTESEL_MASK, ADC_CTRL2_OCTESEL_TMR4CH4 | ADC_CTRL2_OCTEN | ADC_CTRL2_DMAEN);
    // TMR4 CH4: PWM at the ADC rate, its compare edge the trigger (channel enabled as stock's TMR2 CH2 was; PB9
    // is a GPIO output, `board::BT_PWR`, so nothing reaches the pin).
    let pr = reload(ADC_HZ);
    wr(TMR4 + TMR_CTRL1, 0);
    wr(TMR4 + TMR_DIV, 0);
    wr(TMR4 + TMR_PR, pr - 1);
    wr(TMR4 + TMR_C4DT, pr / 2);
    wr(TMR4 + TMR_CM2, TMR_CM2_C4_PWM1);
    wr(TMR4 + TMR_CCTRL, TMR_CCTRL_C4EN);
    wr(TMR4 + TMR_SWEVT, 1);
    irq::enable(IRQ_ADC_DMA, PRIO);
    wr(TMR4 + TMR_CTRL1, TMR_CTRL1_CNTEN);
}

/// Capture off: the timer stopped, ADC1 back to software triggers, DMA off.
pub fn capture_stop() {
    CAPTURING.store(false, Relaxed);
    wr(TMR4 + TMR_CTRL1, 0);
    irq::disable(IRQ_ADC_DMA);
    modify(ch(DMA1, 1) + CH_CTRL, DMA_EN, 0);
    modify(ADC_CTRL2, ADC_CTRL2_OCTESEL_MASK | ADC_CTRL2_DMAEN, super::regs::ADC_CTRL2_OCTESEL_SW);
    // PWR-23: the capture's timer and DMA unclocked until the next start (DMA1 serves only the capture).
    modify(CRM_APB1EN, CRM_APB1_TMR4, 0);
    critical_section::with(|_| modify(CRM_AHBEN, CRM_AHB_DMA1, 0));
}

/// DMA1 channel 1: a capture half is full.
pub fn adc_isr() {
    let sts = rd(DMA1 + DMA_STS);
    wr(DMA1 + DMA_CLR, flags(1));
    if ADC_HALF.signaled() {
        ADC_LATE.fetch_add(1, Relaxed);
    }
    ADC_AT.store(super::time::cycles(), Relaxed);
    ADC_HALF.signal(if sts & half_flag(1) != 0 { 0 } else { 1 });
}

/// DMA2's users: the playback (channel 3) and the screen light's dither (`super::backlight`, channel 1,
/// DISP-23.1). DMA2 is clocked while either runs and unclocked when neither does (PWR-23).
pub(super) const DMA2_PLAY: u8 = 1;
pub(super) const DMA2_BACKLIGHT: u8 = 2;
static DMA2_USERS: AtomicU8 = AtomicU8::new(0);

/// `user` starts or stops using DMA2: its clock on with the first, off after the last (interrupts held off, so a
/// task and a speed switch can't race on the clock bit).
pub(super) fn dma2_use(user: u8, on: bool) {
    critical_section::with(|_| {
        let u = DMA2_USERS.load(Relaxed);
        let u = if on { u | user } else { u & !user };
        DMA2_USERS.store(u, Relaxed);
        if u != 0 {
            modify(CRM_AHBEN, 0, CRM_AHB_DMA2);
        } else {
            modify(CRM_AHBEN, CRM_AHB_DMA2, 0);
        }
    });
}

/// Playback on: PA4 analog, DAC1 on TMR6's update at the DAC rate, DMA2 channel 3 circular from [`DAC_BUF`]
/// (the caller has filled both halves), PC12 high.
pub fn play_start() {
    pin(GPIOA, 4).config(Mode::Analog);
    dma2_use(DMA2_PLAY, true);
    modify(CRM_APB1EN, 0, CRM_APB1_TMR6 | CRM_APB1_DAC);
    modify(CRM_APB1RST, CRM_APB1_DAC, 0);
    let c = ch(DMA2, 3);
    wr(c + CH_CTRL, 0);
    wr(DMA2 + DMA_CLR, flags(3));
    wr(c + CH_PADDR, DAC_DHR12R1);
    wr(c + CH_MADDR, DAC_BUF.as_ptr() as u32);
    wr(c + CH_DTCNT, DAC_BUF.len() as u32);
    wr(c + CH_CTRL, DMA_DTD_M2P | DMA_MINCM | DMA_LM | DMA_PWIDTH_16 | DMA_MWIDTH_16 | DMA_CHPL_HIGH | DMA_HDTIEN | DMA_FDTIEN);
    modify(c + CH_CTRL, 0, DMA_EN);
    // DAC1: channel 2 (the PA drive, `hal::pa`) left exactly as it is.
    modify(DAC_CTRL, !DAC_CTRL_CH2_MASK, DAC_CTRL_EN1 | DAC_CTRL_TEN1 | DAC_CTRL_TSEL1_TMR6 | DAC_CTRL_D1OBDIS | DAC_CTRL_D1DMAEN);
    wr(TMR6 + TMR_CTRL1, 0);
    wr(TMR6 + TMR_DIV, 0);
    wr(TMR6 + TMR_PR, reload(DAC_HZ) - 1);
    wr(TMR6 + TMR_CTRL2, TMR_CTRL2_PTOS_OVF);
    wr(TMR6 + TMR_SWEVT, 1);
    MIC_INJECT.output(true, Mode::OutPp);
    irq::enable(IRQ_DAC_DMA, PRIO);
    wr(TMR6 + TMR_CTRL1, TMR_CTRL1_CNTEN);
}

/// Playback off: PC12 low first, then the timer, DMA and DAC1's channel.
pub fn play_stop() {
    MIC_INJECT.write(false);
    wr(TMR6 + TMR_CTRL1, 0);
    irq::disable(IRQ_DAC_DMA);
    modify(ch(DMA2, 3) + CH_CTRL, DMA_EN, 0);
    // PWR-23: DAC channel 1 off (it was left on at mid-scale); channel 2, the PA drive (`hal::pa`), untouched.
    // DMA2 off unless the screen light's dither still uses it; TMR6 was stopped above (a beep starts it, clock
    // included, again).
    modify(DAC_CTRL, DAC_CTRL_D1DMAEN | DAC_CTRL_TEN1 | DAC_CTRL_EN1, 0);
    dma2_use(DMA2_PLAY, false);
    modify(CRM_APB1EN, CRM_APB1_TMR6, 0);
}

/// DMA2 channel 3: a playback half has gone out and may be refilled.
pub fn dac_isr() {
    let sts = rd(DMA2 + DMA_STS);
    wr(DMA2 + DMA_CLR, flags(3));
    DAC_HALF.signal(if sts & half_flag(3) != 0 { 0 } else { 1 });
}

/// One conversion through the preempted group (software read while the capture runs): 12 bits, 0 if the ADC
/// doesn't answer within about 1 s.
pub fn read_preempted(chan: u32) -> u16 {
    wr(ADC_PSQ, (chan & 0x1F) << 15);
    modify(ADC_CTRL2, 7 << 12, ADC_CTRL2_PCTESEL_SW | ADC_CTRL2_PCTEN);
    modify(ADC_STS, ADC_STS_PCCE, 0);
    modify(ADC_CTRL2, 0, ADC_CTRL2_PCSWTRG);
    let start = super::time::cycles();
    while rd(ADC_STS) & ADC_STS_PCCE == 0 {
        if super::time::cycles().wrapping_sub(start) > super::clock::sysclk_hz() {
            return 0;
        }
    }
    modify(ADC_STS, ADC_STS_PCCE, 0);
    (rd(ADC_PDT1) & 0xFFF) as u16
}

/// PWR-24.9: the capture's and the playback's sample timers for the new tree, those that are running (interrupts
/// are off). A beep on TMR6 is `super::tone`'s to re-time.
/// PWR-26.4: the capture (TMR4) or the playback or a beep (TMR6) is running: no speed switch now.
pub fn busy() -> bool {
    let on = |en: u32, tmr: u32| rd(CRM_APB1EN) & en != 0 && rd(tmr + TMR_CTRL1) & TMR_CTRL1_CNTEN != 0;
    on(CRM_APB1_TMR4, TMR4) || on(CRM_APB1_TMR6, TMR6) || super::tone::playing()
}

pub(super) fn retime(t: ferrite_core::clocktree::Tree) {
    if rd(CRM_APB1EN) & CRM_APB1_TMR4 != 0 && rd(TMR4 + TMR_CTRL1) & TMR_CTRL1_CNTEN != 0 {
        let pr = t.reload(ADC_HZ);
        wr(TMR4 + TMR_PR, pr - 1);
        wr(TMR4 + TMR_C4DT, pr / 2);
    }
    if !super::tone::playing() && rd(CRM_APB1EN) & CRM_APB1_TMR6 != 0 && rd(TMR6 + TMR_CTRL1) & TMR_CTRL1_CNTEN != 0 {
        wr(TMR6 + TMR_PR, t.reload(DAC_HZ) - 1);
    }
}
