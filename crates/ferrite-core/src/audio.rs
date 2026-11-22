//! The audio between the DSP task's converters and the decoders and modulators (roadmap 2f, 5b, 5c), shared by
//! the firmware and the host tests so both run the same arithmetic.
//!
//! - **In:** ADC1 samples PA2 (the BK4829s' EARO) at [`ADC_HZ`], 12 bits unsigned. [`Decim`] takes away the DC
//!   bias, low-passes and keeps every second sample: [`crate::decoders::RATE_HZ`] (24 kHz since Oct 22, 2026,
//!   decision 240; 12 kHz before) signed 16-bit, which is what every decoder in the slot takes. Stock samples at 9.6 kHz with no filter (`analysis/audio_path.md`); 48 kHz
//!   leaves room for the faster modes (POCSAG 2400, 4FSK) and the filter keeps what is above 6 kHz out.
//! - **Out:** DAC1 (PA4) plays [`DAC_HZ`] samples of 12 bits into the transmitting BK4829's microphone input
//!   (PC12 high, as stock: `analysis/audio_path.md`); [`to_dac`] maps a signed sample onto the DAC's range.

use crate::decoders::RATE_HZ;
/// The modem, for the firmware's DSP task (it plays [`uafsk::Modulator`]'s samples).
pub use uafsk;

/// ADC1's rate on PA2.
pub const ADC_HZ: u32 = 48_000;
/// ADC samples per decoder sample.
pub const DECIM: usize = (ADC_HZ / RATE_HZ) as usize;
/// DAC1's rate for AFSK (and later other modulators). Stock plays AFSK at 38.4 kHz; 48 kHz is the same timer
/// arithmetic as the ADC's.
pub const DAC_HZ: u32 = 48_000;
/// AFSK's peak, full scale 32767, for the APRS beacon and `txtest afsk` alike (APRS-14, decisions 133 and 135):
/// 3.0 kHz deviation, APRS's usual, in the `data` TX audio set-up every APRS burst now uses. Bench-measured on the
/// black radio (chip 0, wide FM, RTL-SDR, Oct 13, 2026): `data` 5500 made 4.2 kHz and 50 % of it 2.16 kHz (linear),
/// 71 % 3.03-3.09 kHz with twist -0.18 dB, so 5500 x 0.71. (In the voice set-up MIC AGC holds it near 4 kHz: 8000
/// made 4.35 kHz, 5500 4.0 kHz.)
pub const AFSK_AMP: i16 = 3_900;
/// APRS-49: 0 5 8 4's levels around it (deviation is linear in the amplitude): Low about 2.5 kHz, Mid 3.0 kHz
/// (`AFSK_AMP`), High about 3.5 kHz. A named static: the DSP task reads it, so it sits in fast flash (slowcheck).
pub static AFSK_AMPS: [i16; 3] = [3_250, AFSK_AMP, 4_550];

/// Each half of the ADC's DMA buffer: 8 ms at 48 kHz (decision 358; 2 ms until Nov 4, 2026). The DSP task gets 125
/// blocks a second of [`BLOCK_OUT`] decoder samples, and has a whole half to take each before the DMA writes over
/// it: the longer half rides out anything holding the task off (APRS-56.4's `late`).
pub const BLOCK_IN: usize = 384;
pub const BLOCK_OUT: usize = BLOCK_IN / DECIM;
/// Each half of the DAC's DMA buffer: 2.7 ms at 48 kHz.
pub const BLOCK_DAC: usize = 128;

const N_TAPS: usize = 31;
/// The low-pass ahead of the 2:1 step (4:1 until decision 240, the same taps): 31 taps (Kaiser-windowed sinc, beta 6, 5 kHz), Q15, summing to 1. Flat
/// across AFSK's tones (1200 Hz +0.01 dB, 2200 Hz -0.04 dB: no tilt against the upper tone, which de-emphasised
/// audio already lacks); what folds back onto the audio band is 61 dB down or more at 4:1 and over 90 dB at 2:1 (from 21-27 kHz). Until
/// Oct 21, 2026: 15 taps, -1.1 dB at 2200 Hz and only 18 dB of fold-back rejection (WA8LMF track 2 through the
/// radio's path: 992 -> 997 frames). A `static`: read per sample by the DSP task, and only named statics stay in
/// the firmware's fast flash (unnamed constants go to the slow part: firmware/memory.x).
static TAPS: [i32; N_TAPS] = [-4, 7, 46, 104, 135, 69, -150, -490, -788, -780, -199, 1070, 2877, 4799, 6275, 6826, 6275, 4799, 2877, 1070, -199, -780, -788, -490, -150, 69, 135, 104, 46, 7, -4];
/// The data path's low-pass (decision 316): 31 taps, Kaiser-windowed sinc, beta 6, 11 kHz, Q15, summing to 1. Flat to
/// 8 kHz (-0.01 dB), -2.1 dB at 10 kHz, -13 dB at 12 kHz; what folds back below 10 kHz (14 kHz and up) is 56 dB down or
/// more. [`TAPS`] cut the receive audio from 4 kHz (-2.1 dB, -6 at 5 kHz, -16.6 at 6.3 kHz: bench log R16), which
/// fast data needs (route 9 is -2.5 dB at 5 kHz, -8.9 at 8 kHz).
static WIDE_TAPS: [i32; N_TAPS] = [
    4, 27, -8, -104, -22, 256, 150, -489, -484, 780, 1211, -1070, -2876, 1285, 10214, 15020, 10214, 1285, -2876, -1070, 1211, 780, -484, -489, 150, 256, -22,
    -104, -8, 27, 4,
];

/// The ADC side: DC removal, the low-pass and the 2:1 step, one block at a time. 72 bytes.
pub struct Decim {
    /// The last [`TAPS`] inputs, centred, as a ring.
    hist: [i16; N_TAPS],
    at: u8,
    /// Inputs since the last output.
    phase: u8,
    /// The running mean (DC), Q16 of an ADC count: a one-pole average over 2,048 samples (43 ms).
    dc_q16: i32,
    /// APRS-56.6: the first sample seeds the mean (a fresh capture's DC isn't mid-scale: from 2048 the mean took
    /// 100 ms and more to settle, a packet's flags under its step).
    primed: bool,
    /// [`TAPS`], or [`WIDE_TAPS`] for the data path.
    taps: &'static [i32; N_TAPS],
}

impl Default for Decim {
    fn default() -> Self {
        Self::new()
    }
}

impl Decim {
    pub const fn new() -> Self {
        Decim { hist: [0; N_TAPS], at: 0, phase: 0, dc_q16: 2048 << 16, primed: false, taps: &TAPS }
    }

    /// The data path (decision 316): flat to 8 kHz, for fast data modes and the bench's raw stream.
    pub const fn wide() -> Self {
        Decim { taps: &WIDE_TAPS, ..Self::new() }
    }

    /// ADC counts (0-4095) in, decoder samples out through `out`, one in two. The ADC's 12 bits become
    /// the top of a 16-bit sample (a full-scale swing is +-32767).
    #[inline]
    pub fn push(&mut self, adc: u16, mut out: impl FnMut(i16)) {
        let x = (adc as i32) << 16;
        if !self.primed {
            (self.dc_q16, self.primed) = (x, true);
        }
        self.dc_q16 += (x - self.dc_q16) >> 11;
        let s = ((x - self.dc_q16) >> 12).clamp(-32768, 32767) as i16;
        self.hist[self.at as usize] = s;
        self.at = if self.at as usize + 1 == N_TAPS { 0 } else { self.at + 1 };
        self.phase += 1;
        if self.phase as usize == DECIM {
            self.phase = 0;
            // The ring's oldest sample is at `at`.
            let mut acc = 0i32;
            let mut j = self.at as usize;
            for &t in self.taps.iter() {
                acc += t * self.hist[j] as i32;
                j = if j + 1 == N_TAPS { 0 } else { j + 1 };
            }
            out((acc >> 15).clamp(-32768, 32767) as i16);
        }
    }

    /// A block of ADC counts.
    pub fn block(&mut self, adc: &[u16], mut out: impl FnMut(i16)) {
        for &a in adc {
            self.push(a, &mut out);
        }
    }
}

/// A signed sample as a 12-bit DAC code, centred on 2048.
#[inline]
pub fn to_dac(s: i16) -> u16 {
    ((s as i32 >> 4) + 2048).clamp(0, 4095) as u16
}

/// The AFSK settings for transmission at [`DAC_HZ`]: `txdelay_ms` of flags first (KISS's TX delay, 300 ms by
/// default: KISS rules in chapter 26), so the transmitter is up before the frame.
pub fn afsk_tx(txdelay_ms: u16) -> uafsk::TxConfig {
    afsk_tx_at(txdelay_ms, 1)
}

/// [`afsk_tx`] at one of [`AFSK_AMPS`]' levels (APRS-49: 0 Low, 1 Mid, 2 High).
pub fn afsk_tx_at(txdelay_ms: u16, level: u8) -> uafsk::TxConfig {
    // A flag is 8 bits at 1200 Bd: 6.67 ms.
    let flags = (txdelay_ms as u32 * 3).div_ceil(20) as u16;
    uafsk::TxConfig { rate_hz: DAC_HZ, preamble: flags.max(1), postamble: 3, amp: AFSK_AMPS[level.min(2) as usize] }
}

/// The bits of one transmission (before NRZI): plain AX.25, or FX.25 (APRS-45) when asked for and the frame fits
/// the code.
pub enum TxBits<'a> {
    Plain(uafsk::hdlc::Framer<'a>),
    Fx25(uafsk::ufx25::Bits<'a>),
}

impl Iterator for TxBits<'_> {
    type Item = u8;
    #[inline]
    fn next(&mut self) -> Option<u8> {
        match self {
            TxBits::Plain(b) => b.next(),
            TxBits::Fx25(b) => b.next(),
        }
    }
}

/// APRS-45: `frame` (AX.25 without its FCS) as bits to send with `cfg`'s flags: wrapped in FX.25 with `check`
/// check bytes (16, 32 or 64) into `enc`, or plain AX.25 for 0 or a frame too long for the code (`enc` None
/// then). `enc` holds the block while it is sent (272 bytes).
pub fn tx_bits<'a>(frame: &'a [u8], check: u8, cfg: &uafsk::TxConfig, enc: &'a mut Option<uafsk::ufx25::Encoder>) -> TxBits<'a> {
    *enc = match check {
        16 | 32 | 64 => uafsk::ufx25::Encoder::new(frame, check as usize).ok(),
        _ => None,
    };
    match enc {
        Some(e) => TxBits::Fx25(e.bits(cfg.preamble.max(4), cfg.postamble.max(2))),
        None => TxBits::Plain(uafsk::hdlc::Framer::new(frame, cfg.preamble.max(1), cfg.postamble.max(1))),
    }
}

const _: () = assert!(ADC_HZ.is_multiple_of(RATE_HZ) && BLOCK_IN.is_multiple_of(DECIM));

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec::Vec;

    /// APRS-56.6: a capture starting on a level away from mid-scale has no DC step: the first outputs are near 0,
    /// not the offset decaying over 100 ms.
    #[test]
    fn a_fresh_capture_starts_without_a_dc_step() {
        let mut d = Decim::new();
        let mut out = Vec::new();
        for _ in 0..96 {
            d.push(1700, |x| out.push(x));
        }
        assert!(out.iter().all(|&x| x.abs() < 100), "{:?}", &out[..8]);
    }

    /// A tone through the decimator: its level after the filter, in Q15 of the input's.
    fn gain(hz: f64) -> f64 {
        let mut d = Decim::new();
        let mut out = Vec::new();
        for n in 0..48_000 {
            let x = 2048.0 + 1000.0 * (2.0 * core::f64::consts::PI * hz * n as f64 / ADC_HZ as f64).sin();
            d.push(x.round() as u16, |s| out.push(s));
        }
        let tail = &out[out.len() / 2..];
        let peak = tail.iter().map(|s| (*s as i32).abs()).max().unwrap() as f64;
        peak / (1000.0 * 16.0)
    }

    /// The same by RMS (a tone near the output's Nyquist rate has few samples a cycle, so its peak misleads).
    fn rms_gain(d: Decim, hz: f64) -> f64 {
        let mut d = d;
        let mut out = Vec::new();
        for n in 0..48_000 {
            let x = 2048.0 + 1000.0 * (2.0 * core::f64::consts::PI * hz * n as f64 / ADC_HZ as f64).sin();
            d.push(x.round() as u16, |s| out.push(s as f64));
        }
        let tail = &out[out.len() / 2..];
        (tail.iter().map(|s| s * s).sum::<f64>() / tail.len() as f64).sqrt() * core::f64::consts::SQRT_2 / (1000.0 * 16.0)
    }

    #[test]
    fn decimator_passes_afsk_and_stops_aliases() {
        assert!(gain(1200.0) > 0.9 && gain(2200.0) > 0.85, "AFSK tones pass: {} {}", gain(1200.0), gain(2200.0));
        // DSP-3: at 24 kHz (2:1) what folds onto AFSK's tones comes from 21-23 kHz; the filter's own rejection
        // above its 5 kHz corner is checked at 10 kHz.
        assert!(gain(10_000.0) < 0.06, "10 kHz kept out ({})", gain(10_000.0));
        assert!(gain(22_000.0) < 0.001, "22 kHz folds onto 2 kHz: kept out ({})", gain(22_000.0));
        // One output per two inputs, and DC is gone.
        let mut d = Decim::new();
        let mut n = 0;
        let mut last = 0;
        for _ in 0..40_000 {
            d.push(3000, |s| {
                n += 1;
                last = s;
            });
        }
        assert_eq!(n, 40_000 / DECIM);
        assert_eq!(DECIM, 2, "24 kHz from 48 kHz (decision 240)");
        assert!(last.abs() < 40, "DC removed: {last}");
    }

    /// Decision 316: the data path is flat to 8 kHz (fast data's tones and NRZ's top), and what folds back from above
    /// 14 kHz stays out.
    #[test]
    fn wide_decimator_flat_to_8k() {
        for hz in [300.0, 1000.0, 4000.0, 6300.0, 8000.0] {
            let g = rms_gain(Decim::wide(), hz);
            assert!((0.98..1.02).contains(&g), "{hz} Hz: {g}");
        }
        assert!(rms_gain(Decim::wide(), 10_000.0) > 0.75);
        assert!(rms_gain(Decim::wide(), 16_000.0) < 0.002, "16 kHz folds onto 8 kHz");
        assert!(rms_gain(Decim::wide(), 20_000.0) < 0.002, "20 kHz folds onto 4 kHz");
    }

    #[test]
    fn dac_codes() {
        assert_eq!(to_dac(0), 2048);
        assert_eq!(to_dac(i16::MAX), 4095);
        assert_eq!(to_dac(i16::MIN), 0);
        assert_eq!(afsk_tx(300).preamble, 45);
    }

    /// APRS-45: no FX.25 check bytes (0, or a count the code doesn't have) sends plain AX.25: the framer's bits,
    /// flags first. A default decimator is a new one.
    #[test]
    fn plain_bits_and_default_decimator() {
        let cfg = afsk_tx(300);
        let frame = [0x82u8, 0xA0, 0x40, 0x61];
        for check in [0, 8] {
            let mut enc = None;
            let bits: Vec<u8> = tx_bits(&frame, check, &cfg, &mut enc).collect();
            assert!(enc.is_none());
            assert_eq!(bits[..8], [0, 1, 1, 1, 1, 1, 1, 0], "a flag, LSB first");
            let want: Vec<u8> = uafsk::hdlc::Framer::new(&frame, cfg.preamble.max(1), cfg.postamble.max(1)).collect();
            assert_eq!(bits, want);
        }
        let (mut a, mut b) = (Decim::default(), Decim::new());
        let (mut x, mut y) = (Vec::new(), Vec::new());
        for n in 0..3_000u16 {
            a.push(n % 4096, |s| x.push(s));
            b.push(n % 4096, |s| y.push(s));
        }
        assert!(!x.is_empty());
        assert_eq!(x, y);
    }
}
