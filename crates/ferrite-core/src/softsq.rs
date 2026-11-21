//! Software squelch (SQL-18, decision 269): the Radio task decides open and closed itself, every tick, from the
//! chip's RSSI (REG_67) and noise (REG_65), with the same thresholds stock programs into the chip (SQL-15, SQL-16:
//! REG_78's RSSI open and close, REG_4F's noise open and ceiling). The chip's own squelch waits out its open delay
//! (SQL-17) and holds through the noise burst after a carrier drops; here a carrier opens within one or two readings
//! and the squelch shuts the moment the noise reaches the ceiling, which cuts that tail. The idea is the one nicFW's
//! documentation describes (polling noise and RSSI instead of the chip's detection); the rules here are ours.
//!
//! 0 9 5 (SQL-18.3) brings the chip's decision back: the UI task passes the setting on here each frame.
use core::sync::atomic::{AtomicBool, Ordering::Relaxed};

static CHIP: AtomicBool = AtomicBool::new(false);

/// The chip decides (0 9 5 Chip), not this module.
pub fn chip_decides() -> bool {
    CHIP.load(Relaxed)
}

pub fn set_chip_decides(on: bool) {
    CHIP.store(on, Relaxed);
}

/// SQL-18.1: how long the readings must pass in a row to open on ordinary receive, a short debounce in place of the
/// chip's open delay; and at least [`MIN_READS`] of them, whatever the poll (SQL-18.5: each reading counts for the
/// time since the one before, so the 50 ms poll of a lone slot opens in 100 ms, not 150, and inside Battery Save's
/// wake window; a review of Nov 20, 2026). At the 10 ms tick that is three readings, as decision 293 chose: on a
/// band whose RSSI sits over the open threshold with nothing on air (black indoors on 446.500, Oct 25, 2026), only
/// the noise test holds it shut, and a brief dip under it opened the squelch. A scan step or priority look opens on
/// the first.
pub const OPEN_MS: u16 = 30;
/// SQL-18.5: the fewest readings in a row that open, fade or hit the noise ceiling, whatever their time.
pub const MIN_READS: u8 = 2;
/// SQL-18.2 (decision 293): open, the noise closes it at least this far over the open threshold. The chip's own
/// ceiling is 1 over it, so a weak signal whose noise wobbled across opened and shut again and again.
pub const NOISE_HYST: u8 = 3;
/// How long the RSSI must stay under its close threshold (the noise still under the ceiling), and at least
/// [`MIN_READS`] readings, to close: a weak, fading signal rides through a short dip.
pub const FADE_MS: u16 = 30;

/// The thresholds, from the REG_78 and REG_4F words the chip was given (`SqCal::regs`): RSSI on REG_67's scale
/// (dBm = v / 2 - 160), noise on REG_65's (0-127).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub rssi_open: u16,
    pub rssi_close: u16,
    pub noise_open: u8,
    pub ceiling: u8,
}

impl Limits {
    pub fn from_regs(regs: (u16, u16)) -> Limits {
        Limits {
            rssi_open: regs.0 >> 8,
            rssi_close: regs.0 & 0xFF,
            noise_open: (regs.1 & 0x7F) as u8,
            ceiling: (regs.1 >> 8 & 0x7F) as u8,
        }
    }
}

/// One chip's squelch: open or closed, and the readings counted toward a change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SoftSq {
    open: bool,
    /// Readings in a row toward a change, and their time (ms).
    n: u8,
    ms: u16,
    /// SQL-18.5: readings in a row at the noise ceiling (open).
    noisy: u8,
}

impl SoftSq {
    /// Closed, nothing counted (a `const` for the Radio task's static).
    pub const DEFAULT: SoftSq = SoftSq { open: false, n: 0, ms: 0, noisy: 0 };

    /// The next decision from one reading, `dt_ms` after the one before (SQL-18.5: the poll is 10 ms or 50 ms).
    /// `fast`: open on the first reading that passes.
    pub fn step(&mut self, l: &Limits, rssi: u16, noise: u8, fast: bool, dt_ms: u16) -> bool {
        let count = |n: &mut u8, ms: &mut u16| {
            *n = n.saturating_add(1);
            *ms = ms.saturating_add(dt_ms);
        };
        if !self.open {
            if rssi >= l.rssi_open && noise <= l.noise_open {
                count(&mut self.n, &mut self.ms);
            } else {
                (self.n, self.ms) = (0, 0);
            }
            if fast && self.n >= 1 || self.n >= MIN_READS && self.ms >= OPEN_MS {
                *self = SoftSq { open: true, ..SoftSq::DEFAULT };
            }
            return self.open;
        }
        // the noise ceiling: the carrier's gone (two readings in a row, so one noisy reading doesn't cut it)
        if noise >= l.ceiling.max(l.noise_open.saturating_add(NOISE_HYST)) {
            self.noisy = self.noisy.saturating_add(1);
            if self.noisy >= MIN_READS {
                *self = SoftSq::DEFAULT;
            }
            return self.open;
        }
        self.noisy = 0;
        if rssi < l.rssi_close {
            count(&mut self.n, &mut self.ms);
            if self.n >= MIN_READS && self.ms >= FADE_MS {
                *self = SoftSq::DEFAULT;
            }
        } else {
            (self.n, self.ms) = (0, 0);
        }
        self.open
    }

    pub fn open(&self) -> bool {
        self.open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl SoftSq {
        /// A reading at the 10 ms tick.
        fn st(&mut self, l: &Limits, rssi: u16, noise: u8, fast: bool) -> bool {
            self.step(l, rssi, noise, fast, 10)
        }
    }

    /// 443.475 at level 4 on the test radios (`sqcal`'s stock_thresholds): RSSI open 0x59, close 0x55; noise 40
    /// with 1 of hysteresis.
    const L: Limits = Limits { rssi_open: 0x59, rssi_close: 0x55, noise_open: 0x27, ceiling: 0x28 };
    /// A carrier: well over the RSSI threshold, quiet.
    const SIG: (u16, u8) = (0x80, 10);
    /// An empty channel as black read it (Oct 23, 2026): no RSSI to speak of, noise 46-50.
    const EMPTY: (u16, u8) = (0x40, 48);

    /// SQL-18: the thresholds come straight from the words the chip is given.
    #[test]
    fn limits_from_the_chips_words() {
        assert_eq!(Limits::from_regs((0x5955, 0x2827)), L);
    }

    /// SQL-18.1: ordinary receive opens on the third reading in a row that passes; a scan step on the first.
    #[test]
    fn opens_after_three_readings_or_one_when_fast() {
        let mut s = SoftSq::default();
        assert!(!s.st(&L, SIG.0, SIG.1, false));
        assert!(!s.st(&L, SIG.0, SIG.1, false));
        assert!(s.st(&L, SIG.0, SIG.1, false));
        let mut f = SoftSq::default();
        assert!(f.st(&L, SIG.0, SIG.1, true));
        // a lone passing reading between empty ones never opens
        let mut b = SoftSq::default();
        for r in [SIG, EMPTY, SIG, EMPTY, SIG, EMPTY] {
            assert!(!b.st(&L, r.0, r.1, false));
        }
        // decision 293: the band's RSSI over the open threshold with nothing on air, the noise dipping under it for
        // two readings at a time: never opens
        let mut d = SoftSq::default();
        for n in [48, 39, 39, 47, 38, 39, 50, 39, 46] {
            assert!(!d.st(&L, 0x74, n, false), "noise {n}");
        }
    }

    /// SQL-18.1: both have to pass: a strong signal under noise doesn't open, nor does quiet without the RSSI.
    #[test]
    fn needs_rssi_and_quiet() {
        let mut s = SoftSq::default();
        for _ in 0..5 {
            assert!(!s.st(&L, 0x80, 45, false), "noise over the open threshold");
        }
        for _ in 0..5 {
            assert!(!s.st(&L, 0x50, 5, false), "RSSI under the open threshold");
        }
        let mut z = SoftSq::default();
        assert!(z.st(&L, 0x59, 0x27, true), "at the thresholds themselves");
    }

    /// SQL-18.2, SQL-18.5: open, the noise reaching the ceiling (at least NOISE_HYST over the open threshold,
    /// decision 293) on two readings in a row shuts it: the carrier's gone, no tail; one noisy reading doesn't.
    #[test]
    fn the_ceiling_closes_on_two_readings() {
        let mut s = SoftSq::default();
        s.st(&L, SIG.0, SIG.1, true);
        assert!(s.st(&L, SIG.0, 0x28, false), "the chip's ceiling, 1 over: stays (a weak signal's wobble)");
        assert!(s.st(&L, SIG.0, 0x29, false), "2 over: stays");
        assert!(s.st(&L, SIG.0, 0x2A, false), "3 over once: rides it");
        assert!(s.st(&L, SIG.0, SIG.1, false), "quiet again");
        assert!(s.st(&L, SIG.0, 0x2A, false));
        assert!(!s.st(&L, SIG.0, 0x2A, false), "twice in a row, even with RSSI left: shut");
    }

    /// SQL-18.5: at the 50 ms poll of a lone slot, two passing readings open (100 ms, not 150): inside Battery Save's
    /// 180 ms wake window; one alone still doesn't.
    #[test]
    fn the_50_ms_poll_opens_on_two_readings() {
        let mut s = SoftSq::default();
        assert!(!s.step(&L, SIG.0, SIG.1, false, 50), "one reading isn't enough");
        assert!(s.step(&L, SIG.0, SIG.1, false, 50));
        let mut b = SoftSq::default();
        for r in [SIG, EMPTY, SIG, EMPTY] {
            assert!(!b.step(&L, r.0, r.1, false, 50), "alternating never opens");
        }
    }

    /// SQL-18.5: at 50 ms a fade closes on its second reading under the threshold (100 ms), a noisy one on its second
    /// at the ceiling; one of either rides.
    #[test]
    fn the_50_ms_poll_fades_and_closes_on_two() {
        let mut s = SoftSq::default();
        s.step(&L, SIG.0, SIG.1, true, 50);
        assert!(s.step(&L, 0x50, 20, false, 50), "one under: rides");
        assert!(!s.step(&L, 0x50, 20, false, 50), "two: closed");
        s.step(&L, SIG.0, SIG.1, true, 50);
        assert!(s.step(&L, SIG.0, 0x2A, false, 50), "one noisy: rides");
        assert!(!s.step(&L, SIG.0, 0x2A, false, 50), "two noisy: closed");
    }

    /// SQL-18.5: the first reading after a retune counts no time (dt 0): it can't open alone, the next one does once
    /// 30 ms have passed.
    #[test]
    fn a_reading_with_no_time_counts_toward_two() {
        let mut s = SoftSq::default();
        assert!(!s.step(&L, SIG.0, SIG.1, false, 0));
        assert!(!s.step(&L, SIG.0, SIG.1, false, 20), "two readings but only 20 ms");
        assert!(s.step(&L, SIG.0, SIG.1, false, 10), "30 ms");
    }

    /// SQL-18.2: a fading signal under the RSSI close threshold rides two readings and closes on the third; a
    /// reading back over it starts the count again.
    #[test]
    fn a_fade_closes_after_three_readings() {
        let mut s = SoftSq::default();
        s.st(&L, SIG.0, SIG.1, true);
        assert!(s.st(&L, 0x50, 20, false));
        assert!(s.st(&L, 0x50, 20, false));
        assert!(s.st(&L, 0x56, 20, false), "back over the close threshold");
        assert!(s.st(&L, 0x50, 20, false));
        assert!(s.st(&L, 0x50, 20, false));
        assert!(!s.st(&L, 0x50, 20, false));
        assert!(!s.open());
    }

    /// SQL-18.3: software decides until the setting says otherwise.
    #[test]
    fn software_by_default() {
        assert!(!chip_decides());
    }

    /// Only ever set false here: other tests read the shared flag as false (the default) at the same time.
    #[test]
    fn set_chip_decides_off() {
        set_chip_decides(false);
        assert!(!chip_decides());
    }
}
