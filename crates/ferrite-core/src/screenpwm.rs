//! The screen backlight's PWM as numbers (DISP-23.1, decision 472): its frequency, the dither table and the
//! periods the firmware's `hal::backlight` loads into TMR8, worked out here so they are tested on the PC.
//!
//! The screen's light runs at about 31.7 kHz, above the audio band: at 2 kHz (decision 427) some radios heard its
//! second harmonic, 4 kHz, in the receive audio (supply ripple reaching the audio amplifier). The keypad light
//! stays at 2 kHz (its 25 kHz edges were what made the RF lines of ferrite#74; it isn't heard).
//!
//! The frequency is "odd": 120 MHz / 3787 (3787 = 7 x 541, sharing no factor with 2, 3 or 5), so no harmonic of it
//! lands exactly on a 5, 6.25, 8.33, 12.5 or 25 kHz channel grid (nor the 9 or 10 kHz broadcast grids) other than
//! the timer clock's own multiples (there whatever the PWM does), and no small multiple of it (up to the 24th) comes within 1 % of a grid
//! step: no harmonic family that walks slowly across the channels. At Low Power Mode's 24 MHz the period is 757
//! (31.70 kHz; 757 is prime), the same property. Picked by scoring every period from 27 to 33 kHz coprime to 30 at
//! both clocks (the script is in decision 472).
//!
//! Each period is then dithered by up to about 1 %, from [`DITHER`] in turn: the n-th harmonic is spread over n %
//! of its frequency instead of sitting still as a carrier that can open squelch. On 2 m (146 MHz, n about 4600)
//! that's about +/-1.5 MHz, on 70 cm (446 MHz) about +/-4.4 MHz.

/// The screen light's nominal PWM frequency (DISP-23.1): 120 MHz / 3787 and 24 MHz / 757 by [`counts`].
pub const SCREEN_HZ: u32 = 31_687;
/// The dither's depth in tenths of a percent (DISP-23.1): +/-1.0 %.
pub const DITHER_TENTHS: u16 = 10;
/// The most a bench build lets the dither be (`lcd dither`, TERM-40): +/-5 %.
pub const DITHER_MAX: u16 = 50;
/// The bench's frequency range (`lcd hz`, TERM-40): the period fits TMR8's 16 bits at 120 MHz with the dither.
pub const HZ_MIN: u32 = 2_000;
pub const HZ_MAX: u32 = 60_000;
/// Periods in one turn of the dither table.
pub const STEPS: usize = 32;

/// The dither (DISP-23.1): 32 evenly spaced offsets from -127 to 127 (-1 to 1 of the depth), summing to 0 so the
/// mean period is the nominal one, in a fixed shuffled order (seeded 472). One turn is about 1 ms.
pub static DITHER: [i8; STEPS] = [
    12, 111, -20, -37, -45, -111, -127, -70, 94, 53, 4, 78, 70, -4, 20, 61, 86, 127, 29, 102, -94, -61, -29, -102,
    -86, 45, -119, -12, 37, -78, 119, -53,
];

/// The nominal period in timer counts for `hz` from a timer clock of `tmr_hz` (rounded): 3787 at 120 MHz, 757 at
/// 24 MHz for [`SCREEN_HZ`].
pub const fn counts(tmr_hz: u32, hz: u32) -> u32 {
    (tmr_hz + hz / 2) / hz
}

/// The period of one dither step (DISP-23.1): `nominal` moved by `v` / 127 of `tenths` / 1000, rounded.
pub const fn step(nominal: u32, tenths: u16, v: i8) -> u32 {
    let num = nominal as i64 * tenths as i64 * v as i64;
    let den = 127 * 1000;
    let off = if num >= 0 { (num + den / 2) / den } else { -((-num + den / 2) / den) };
    (nominal as i64 + off) as u32
}

/// Thousandths to compare counts of a period `p`: 0 dark, 1000 full on (the whole period, DISP-23.1).
pub const fn duty(p: u32, permille: u16) -> u32 {
    if permille >= 1000 {
        p
    } else {
        p * permille as u32 / 1000
    }
}

/// The light needs no PWM edges (DISP-23.1): dark, full on, or no dither asked for, so nothing to dither.
pub const fn plain(permille: u16, tenths: u16) -> bool {
    permille == 0 || permille >= 1000 || tenths == 0
}

/// What `hal::backlight::screen` does for a level (DISP-23.1), given whether the dither runs now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// Stop the dither (if it runs) and load the nominal period.
    Plain,
    /// Fill the table and start the dither.
    Start,
    /// Running already: only the compare changes.
    Keep,
}

/// [`Act`] for `permille` at depth `tenths`, the dither running or not.
pub const fn act(running: bool, permille: u16, tenths: u16) -> Act {
    if plain(permille, tenths) {
        Act::Plain
    } else if running {
        Act::Keep
    } else {
        Act::Start
    }
}

/// DMA2's users after `user` (a bit) starts (`on`) or stops using it: `hal::audio::dma2_use` clocks DMA2 while
/// the result isn't 0 (the playback and the screen light's dither, DISP-23.1, PWR-23).
pub const fn dma2_users(users: u8, user: u8, on: bool) -> u8 {
    if on { users | user } else { users & !user }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clocktree::{Tree, LOW};

    const FULL: Tree = Tree::new(15);

    /// DISP-23.1: the periods at both speeds, and the frequency they give.
    #[test]
    fn periods() {
        assert_eq!(FULL.tmr_hz(), 120_000_000);
        assert_eq!(counts(FULL.tmr_hz(), SCREEN_HZ), 3787);
        assert_eq!(counts(LOW.tmr_hz(), SCREEN_HZ), 757);
        // Both clear of the audio band and in the 27-33 kHz window.
        for p in [3787u32, 757 * 5] {
            let hz = 120_000_000 / p;
            assert!((27_000..=33_000).contains(&hz), "{hz}");
        }
        // The bench's range fits 16 bits with the most dither at 120 MHz, and keeps a usable period at 24 MHz.
        assert!(step(counts(FULL.tmr_hz(), HZ_MIN), DITHER_MAX, 127) <= 65_536);
        assert!(step(counts(LOW.tmr_hz(), HZ_MAX), DITHER_MAX, -127) >= 300);
    }

    /// DISP-23.1: no harmonic of the nominal frequency, at either clock, lies exactly on a 5, 6.25, 25/3, 12.5 or
    /// 25 kHz channel in 136-174 MHz or 400-480 MHz (exact in rationals: the timer's resolution is the period).
    /// Every p-th harmonic is a multiple of the timer clock itself (120 MHz: 480 MHz, the band's edge; 24 MHz: 144,
    /// 168, 408, 432, 456 MHz), on every grid but there from the clock whatever the PWM does: those are skipped.
    #[test]
    fn harmonics_off_grids() {
        // Grids in thousandths of a hertz (8.333 kHz = 25000/3 Hz exactly: 25_000_000/3 isn't whole, so scale by 3).
        for (tmr, p) in [(120_000_000u64, 3787u64), (24_000_000, 757)] {
            for (lo, hi) in [(136_000_000u64, 174_000_000u64), (400_000_000, 480_000_000)] {
                let k0 = (lo * p).div_ceil(tmr);
                let mut k = k0;
                while k * tmr < hi * p {
                    if k % p == 0 {
                        k += 1;
                        continue;
                    }
                    // Harmonic k is k * tmr / p hertz; on a grid g (in Hz * 3) when 3 * k * tmr is a multiple of p * g.
                    for g3 in [15_000u64, 18_750, 25_000, 37_500, 75_000] {
                        assert_ne!((3 * k * tmr) % (p * g3), 0, "harmonic {k} of {tmr}/{p} on the {} Hz grid", g3 / 3);
                    }
                    k += 1;
                }
                assert!(k > k0);
            }
        }
    }

    /// DISP-23.1: no small multiple of the frequency (up to the 24th) is within 1 % of a step of any channel grid,
    /// nor of the 9 and 10 kHz broadcast grids: harmonics don't drift slowly across the channels in families.
    #[test]
    fn not_commensurate() {
        let f = 120_000_000.0 / 3787.0;
        for g in [5_000.0, 6_250.0, 25_000.0 / 3.0, 12_500.0, 25_000.0, 9_000.0, 10_000.0] {
            for q in 1..=24 {
                let x = q as f64 * f / g;
                assert!((x - x.round()).abs() > 0.01, "{q} x {f} against {g}");
            }
        }
    }

    /// DISP-23.1: the table's mean is the nominal period, its spread +/-1 %, each value once.
    #[test]
    fn dither_table() {
        assert_eq!(DITHER.iter().map(|&v| v as i32).sum::<i32>(), 0);
        assert_eq!(DITHER.iter().min(), Some(&-127));
        assert_eq!(DITHER.iter().max(), Some(&127));
        let mut s = DITHER;
        s.sort();
        assert!(s.windows(2).all(|w| w[0] < w[1]));
        for (n, spread) in [(3787u32, 38u32), (757, 8)] {
            let ps: [u32; STEPS] = core::array::from_fn(|i| step(n, DITHER_TENTHS, DITHER[i]));
            assert_eq!(ps.iter().sum::<u32>(), n * STEPS as u32, "{n}: mean period");
            assert_eq!(*ps.iter().min().unwrap(), n - spread);
            assert_eq!(*ps.iter().max().unwrap(), n + spread);
        }
        // No dither: every step the nominal period.
        assert!(DITHER.iter().all(|&v| step(3787, 0, v) == 3787));
    }

    /// DISP-23.1: full brightness is the whole period (no edges), dark is none; both plain (no dither).
    #[test]
    fn duty_ends() {
        assert_eq!(duty(3787, 1000), 3787);
        assert_eq!(duty(3787, 0), 0);
        assert_eq!(duty(3787, 500), 1893);
        assert!(plain(0, 10) && plain(1000, 10) && plain(500, 0));
        assert!(!plain(500, 10));
    }

    /// DISP-23.1: the dither's start and stop as `hal::backlight` and `hal::audio::dma2_use` do them, against a
    /// model of the registers, through random levels, speed switches (`retime`: stop, then the level again), bench
    /// depth changes (`tune`) and playbacks starting and stopping. The flag the `lcd` readback shows always agrees
    /// with the hardware: DMA2 clocked, channel 1 on and TMR8's OVFDEN set exactly while it runs, and it runs
    /// exactly when the level isn't plain. (Black, Nov 25, 2026, read the flag set with DMA2 unclocked and OVFDEN
    /// clear: no sequence of these reaches that.)
    #[test]
    fn start_stop_agree_with_hardware() {
        const PLAY: u8 = 1;
        const LIGHT: u8 = 2;
        #[derive(Default)]
        struct Hw {
            flag: bool,
            users: u8,
            clock: bool,
            ovfden: bool,
            ch_en: bool,
            level: u16,
            depth: u16,
        }
        impl Hw {
            fn use_dma2(&mut self, user: u8, on: bool) {
                self.users = dma2_users(self.users, user, on);
                self.clock = self.users != 0;
            }
            fn stop(&mut self) {
                if !core::mem::replace(&mut self.flag, false) {
                    return;
                }
                self.ovfden = false;
                self.ch_en = false;
                self.use_dma2(LIGHT, false);
            }
            fn screen(&mut self) {
                match act(self.flag, self.level, self.depth) {
                    Act::Plain => self.stop(),
                    Act::Start => {
                        self.use_dma2(LIGHT, true);
                        self.ch_en = self.clock; // an unclocked DMA2 drops the write
                        self.ovfden = true;
                        self.flag = true;
                    }
                    Act::Keep => {}
                }
            }
        }
        let mut hw = Hw { depth: DITHER_TENTHS, ..Default::default() };
        let mut x = 472u32;
        let levels = [0u16, 45, 90, 355, 999, 1000];
        for _ in 0..20_000 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            match x % 6 {
                0 | 1 => hw.level = levels[(x >> 8) as usize % levels.len()],
                2 => hw.stop(), // retime's stop; the screen below is its second half
                3 => {
                    hw.depth = [0, DITHER_TENTHS, DITHER_MAX][(x >> 8) as usize % 3];
                    hw.stop();
                }
                4 => hw.use_dma2(PLAY, true),
                _ => hw.use_dma2(PLAY, false),
            }
            hw.screen();
            assert_eq!(hw.flag, !plain(hw.level, hw.depth));
            assert_eq!((hw.ovfden, hw.ch_en), (hw.flag, hw.flag));
            assert!(!hw.flag || hw.clock, "running with DMA2 unclocked");
            assert_eq!(hw.clock, hw.users != 0);
        }
    }
}
