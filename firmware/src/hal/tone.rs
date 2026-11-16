//! Test tone on DAC channel 1 (PA4, the audio DAC): a hardware triangle wave clocked by TMR6, so it costs
//! no CPU. Channel 2 (PA5, PA drive) is never enabled here, and the DAC goes back into reset when the tone
//! stops, which is the safe state. Mirrors hal_tone.
use super::gpio::{pin, Mode, GPIOA};
use super::regs::*;
use super::{clock, modify, rd, wr};
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

static ACTUAL_HZ: AtomicU32 = AtomicU32::new(0);
/// What was asked for (hz, level), for a re-time at a speed switch (PWR-24.9).
static ASKED: AtomicU32 = AtomicU32::new(0);

/// Level 1-11: peak-to-peak = 2^(level+1) - 1 counts.
pub fn start(hz: u32, level: u8) {
    if hz == 0 {
        return stop();
    }
    let level = level.clamp(1, 11) as u32;
    let amp = (1 << (level + 1)) - 1; // triangle peak, counts
    let trig = hz * 2 * amp; // DAC steps per second
    let clk = clock::tmr_hz(); // TMR6's clock at the current speed (PWR-24.9)
    ASKED.store(hz << 8 | level, Relaxed);
    let total = ((clk + trig / 2) / trig).max(2);
    let psc = (total - 1) / 65536 + 1; // a prescaler only when the period alone won't fit 16 bits
    let div = ((total + psc / 2) / psc).clamp(2, 65536);
    ACTUAL_HZ.store(clk / psc / div / (2 * amp), Relaxed);

    pin(GPIOA, 4).config(Mode::Analog);
    modify(CRM_APB1EN, 0, CRM_APB1_TMR6 | CRM_APB1_DAC);
    modify(CRM_APB1RST, CRM_APB1_DAC, 0);
    wr(TMR6 + TMR_CTRL1, 0);
    wr(TMR6 + TMR_DIV, psc - 1);
    wr(TMR6 + TMR_PR, div - 1);
    wr(TMR6 + TMR_CTRL2, TMR_CTRL2_PTOS_OVF);
    wr(TMR6 + TMR_SWEVT, 1); // load PR
    wr(DAC_DHR12R1, 2048 - amp / 2); // centre the triangle on mid-scale
    // Channel 1 only; every channel 2 bit (PA drive) is written as zero.
    let ctrl = DAC_CTRL_EN1 | DAC_CTRL_TEN1 | DAC_CTRL_TSEL1_TMR6 | DAC_CTRL_WAVE1_TRIANGLE | level << 8;
    wr(DAC_CTRL, ctrl & !DAC_CTRL_CH2_MASK);
    wr(TMR6 + TMR_CTRL1, 1);
}

pub fn stop() {
    ASKED.store(0, Relaxed);
    wr(TMR6 + TMR_CTRL1, 0);
    dac_reset();
    ACTUAL_HZ.store(0, Relaxed);
}

/// DAC held in reset: its outputs cannot override the GPIO safe state of PA4 / PA5.
pub fn dac_reset() {
    modify(CRM_APB1RST, 0, CRM_APB1_DAC);
}

/// 0 when off; the actual (rounded) frequency.
pub fn hz() -> u32 {
    ACTUAL_HZ.load(Relaxed)
}

/// DAC1 is taken: a tone, or the playback (TMR6 runs; SQL-20.3: the open beep waits its turn).
pub fn dac_busy() -> bool {
    playing() || rd(CRM_APB1EN) & CRM_APB1_TMR6 != 0 && rd(TMR6 + TMR_CTRL1) & 1 != 0 // CNTEN
}

/// A tone is playing (TMR6 is the tone's, not the playback's).
pub fn playing() -> bool {
    ASKED.load(Relaxed) != 0
}

/// PWR-24.9: a tone playing across a speed switch starts again at the new speed (same pitch and level).
pub(super) fn retime() {
    let a = ASKED.load(Relaxed);
    if a != 0 {
        start(a >> 8, (a & 0xFF) as u8);
    }
}
