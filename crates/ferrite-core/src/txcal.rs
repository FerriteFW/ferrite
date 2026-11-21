//! The factory calibration's transmit side, read only (manual chapter 35, `analysis/calibration.md`): the PA drive
//! curves at 0x00F000-0x00F1BF, the battery gauge at 0x00F200 and the TX windows at 0x00F230/0x00F240, as stock
//! uses them. The Radio task's `tx` build asks [`TxCal::dac`] for the PA drive (DAC2, 8 bits) at key-up,
//! [`TxCal::in_window`] for the windows (CAL-11) and [`Gauge::tx_ok`] for the battery (PWR-20).
//! Nothing here writes anything: the calibration area is never written (SAFE).

use crate::freq::{mhz, Freq10};

/// Bytes read from the start of the calibration sector (0x00F000): the PA curves to 0x1BF, the gauge at 0x200,
/// the tuning block at 0x210 and the TX windows to 0x24F.
pub const READ_LEN: usize = 0x250;
/// Where the blank test looks (CAL-5): 0xF0E0 byte 0 = 0xFF is an erased sector.
const BLANK_AT: usize = 0xE0;
const GAUGE_AT: usize = 0x200;
/// The TX windows (CAL-11): 0xF230 holds windows 1-3 (stock's VHF, UHF and 220 classes) at offsets 0, 5 and 10,
/// 0xF240 window 4 (350), five bytes each: the enable byte (1 on), then the low and the high edge, each two bytes of
/// BCD giving whole MHz (01 36 = 136 MHz). Stock's loader, `analysis/calibration.md`.
const WINDOWS_AT: [usize; 4] = [0x230, 0x235, 0x23A, 0x240];
/// The factory crystal trim (CAL-12): 0xF210 byte 11 = [`TRIM_MARKER`] marks it valid, bytes 12-13 hold k, signed
/// 16-bit little-endian. Stock adds `(f / 100) * k / 435500` to every frequency it tunes, receive and transmit
/// (0x0801DDD0), about 0.023 ppm a unit ([`trimmed`]).
pub(crate) const TRIM_AT: usize = 0x21B;
/// CAL-6, CAL-18: the largest factory trim taken, 20 ppm (k / 43.55 ppm); a bigger one is invalid.
pub const TRIM_MAX: i16 = 871;
/// TONE-1: the transmitted sub-tone's deviation, 0xF210 byte 1 for CTCSS (REG_51 = 0x9000 | b1, 0x0800A488; also
/// the 55 Hz tail, 0x0801D078) and byte 2 for DCS (REG_51 = 0xA000 | b2, 0x0800D3E0), 7 bits each.
const TONE_AT: usize = 0x211;
pub const TRIM_MARKER: u8 = 0xA0;

/// Why the calibration locks transmit (TX-13, TX-14, CAL-5, CAL-6): the Radio task gets no [`TxCal`], and every
/// key-up is refused as Locked; receive is unaffected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// 0xF0E0 byte 0 = 0xFF: an erased sector (CAL-5).
    Blank,
    /// A PA curve entry the amateur bands reach is 0x00 or 0xFF (an erased or unprogrammed byte).
    PaCurve,
    /// The TX windows: blank (0xF230 byte 0 = 0xFF, where stock falls back to its built-in windows, which Ferrite
    /// never has: CAL-4), an enable byte other than 0x00, 0x01 or 0xFF, or an enabled window that isn't BCD with
    /// low < high (TX-14).
    Windows,
    /// The factory crystal trim is marked (0xF210 byte 11 = 0xA0) but over ±20 ppm ([`TRIM_MAX`], CAL-6): not a
    /// value a factory writes. Stock would apply it; Ferrite locks transmit and receives untrimmed. A trim within
    /// the limit is applied (CAL-12, [`trimmed`]); a radio without the marker (both radios here: 0xFF) has none.
    Trim,
    /// A sub-tone gain (0xF210 byte 1 or 2) doesn't fit REG_51's 7-bit field (TX-14); stock masks it with 0x7F.
    ToneGain,
    /// The PA curves out of order: at some entry the amateur bands reach, Low isn't below Mid or Mid below High
    /// (TX-14, CAL-6).
    LevelOrder,
    /// The MCU doesn't read as the AT32F403AVGT7 (TX-15).
    Mcu,
    /// The calibration sector's CRC isn't the one recorded at install (CAL-7, TX-14), or it couldn't be checked.
    Changed,
}

/// CAL-7: the calibration sector's CRC against the one recorded at install (the layout header, the store's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalCrc {
    /// The same as recorded.
    Same,
    /// None was recorded (the first boot of a build that records it): recorded now.
    Recorded,
    /// Not the one recorded: transmit locked.
    Changed,
    /// The store isn't there to say (flash failed, the bring-up build): transmit locked.
    Unknown,
}

/// TX-13 to TX-15 (TX audit F8): what the Radio task gets: the curves when the MCU check passed, the sector is the
/// one recorded at install and it parses; else why transmit is locked, the MCU first.
pub fn gate(cal: Result<TxCal, Fault>, mcu_ok: bool, crc: CalCrc) -> Result<TxCal, Fault> {
    if !mcu_ok {
        return Err(Fault::Mcu);
    }
    if matches!(crc, CalCrc::Changed | CalCrc::Unknown) {
        return Err(Fault::Changed);
    }
    cal
}

/// CAL-12: the factory crystal trim k when the sector marks one (0xF210 byte 11 = 0xA0), else None, whatever its
/// size (CAL-6 checks that). Stock's loader skips 0xF210 on a blank sector, so a blank one has none. `b` is the
/// sector from 0x00F000, at least to 0xF21D.
pub fn factory_trim(b: &[u8]) -> Option<i16> {
    (b[BLANK_AT] != 0xFF && b[TRIM_AT] == TRIM_MARKER).then(|| i16::from_le_bytes([b[TRIM_AT + 1], b[TRIM_AT + 2]]))
}

/// CAL-12: the factory trim to apply: k within ±[`TRIM_MAX`], else 0 (none marked, or invalid: CAL-6).
pub fn valid_trim(b: &[u8]) -> i16 {
    factory_trim(b).filter(|k| k.unsigned_abs() <= TRIM_MAX as u16).unwrap_or(0)
}

/// CAL-12: the word REG_38/39 gets for tuning `f` with the crystal trim `k`, stock's formula exactly (0x0801DDD0):
/// `f + (f / 100) * k / 435500`, a 64-bit product divided rounding toward zero. k = 0 is `f` itself. The user trim
/// (crystal trim, not built: an idea, decision 276) adds to k when it is.
pub fn trimmed(f: Freq10, k: i16) -> Freq10 {
    (f as i64 + (f / 100) as i64 * k as i64 / 435_500) as Freq10
}

/// A BCD byte's value, None if a digit is over 9.
fn bcd(b: u8) -> Option<u16> {
    (b >> 4 < 10 && b & 0x0F < 10).then_some((b >> 4) as u16 * 10 + (b & 0x0F) as u16)
}

/// One TX window from its five bytes: Ok(None) when off (enable 0x00, or 0xFF: an erased 0xF240), the edges in MHz
/// when on (enable 0x01, the only value stock's check takes).
fn window(w: &[u8]) -> Result<Option<(u16, u16)>, Fault> {
    let edge = |hi: u8, lo: u8| Some(bcd(hi)? * 100 + bcd(lo)?);
    match w[0] {
        0x00 | 0xFF => Ok(None),
        0x01 => match (edge(w[1], w[2]), edge(w[3], w[4])) {
            (Some(lo), Some(hi)) if lo < hi => Ok(Some((lo, hi))),
            _ => Err(Fault::Windows),
        },
        _ => Err(Fault::Windows),
    }
}
/// Stock's gauge when 0xF200 byte 0 is 0x00 or 0xFF; the seventh is 0xBB whatever the sector says.
const GAUGE_DEFAULT: [u8; 7] = [0x81, 0x87, 0x8E, 0x93, 0x98, 0xA0, 0xBB];

/// PWR-20: the battery in mV from the 12-bit PA1 reading: (reading >> 4) x 48.2 mV, before the user trim. Stock's
/// factor; the black radio's full pack read 8.312 V on a meter (Oct 9, 2026), which is reading 172.
pub fn battery_mv(adc: u16) -> u16 {
    ((adc >> 4) as u32 * 482 / 10) as u16
}

/// The PA drive curves: 3 levels (stock order: 0 High, 1 Mid, 2 Low) x 4 rows (UHF 400 MHz up in 10 MHz steps,
/// VHF 130-180 in 3 MHz, 200-300 in 5 MHz, 350-400 in 5 MHz) x 16, and the HF rows (15 MHz up in 5 MHz, 3 levels).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxCal {
    pa: [[[u8; 16]; 4]; 3],
    hf: [[u8; 16]; 3],
    /// The enabled TX windows, low and high edge in MHz; (0, 0) for a window that is off.
    win: [(u16, u16); 4],
    /// TONE-1: the sub-tone deviation bytes, CTCSS (0xF210 byte 1) and DCS (byte 2).
    tone: [u8; 2],
    /// CAL-12: the factory crystal trim k (0 for none), for the transmit frequency ([`TxCal::tx_word`]).
    trim: i16,
    /// CAL-6.1 (decision 412, issue #40): the band rows whose curves failed a check, a bit per [`ROW_NAMES`] row:
    /// transmit is locked on that band only ([`TxCal::pa`]).
    bad: u8,
}

/// CAL-6.1: the band rows the curve checks cover, as [`TxCal::curve_fault_at`] and the `bad` bits number them.
pub const ROW_NAMES: [&str; 4] = ["UHF", "VHF", "200 MHz", "HF"];

/// TONE-16's step around the calibration's sub-tone level: stock's "high" adds 0x0F to the CTCSS byte and 0x14 to
/// the DCS byte (0x0800A490, 0x0800D3EC, when its 0x20000C34+0x1A flag is set); Ferrite's Low takes the same off.
pub const TONE_STEP: [u8; 2] = [0x0F, 0x14];

const UHF: usize = 0;
const VHF: usize = 1;
const R200: usize = 2;
const R350: usize = 3;

impl TxCal {
    /// The curves and windows from the sector's first [`READ_LEN`] bytes; None when [`TxCal::read`] finds a fault.
    pub fn parse(b: &[u8; READ_LEN]) -> Option<TxCal> {
        TxCal::read(b).ok()
    }

    /// The curves and windows, or why transmit is locked: the sector blank (CAL-5), a curve the radio's bands use
    /// holding a 0 or 0xFF (CAL-6's plausibility), the TX windows blank or invalid (TX-14), a factory crystal
    /// trim over ±20 ppm ([`Fault::Trim`]).
    pub fn read(b: &[u8; READ_LEN]) -> Result<TxCal, Fault> {
        if b[BLANK_AT] == 0xFF {
            return Err(Fault::Blank);
        }
        if factory_trim(b).is_some_and(|k| k.unsigned_abs() > TRIM_MAX as u16) {
            return Err(Fault::Trim);
        }
        let tone = [b[TONE_AT], b[TONE_AT + 1]];
        if tone.iter().any(|&g| g > 0x7F) {
            return Err(Fault::ToneGain);
        }
        let mut c = TxCal { pa: [[[0; 16]; 4]; 3], hf: [[0; 16]; 3], win: [(0, 0); 4], tone, trim: valid_trim(b), bad: 0 };
        for (l, level) in c.pa.iter_mut().enumerate() {
            for (r, row) in level.iter_mut().enumerate() {
                let at = 0x40 * l + 0x10 * r;
                row.copy_from_slice(&b[at..at + 16]);
            }
        }
        for (l, row) in c.hf.iter_mut().enumerate() {
            let at = 0x190 + 0x10 * l;
            row.copy_from_slice(&b[at..at + 16]);
        }
        // CAL-6.1 (decision 412, issue #40): a curve the bands reach with a 0x00 or 0xFF entry (CAL-10), or with Low
        // not below High, locks transmit on its band only; the rest of the radio transmits. It locked every band:
        // a radio whose HF High row was tuned below its flat Mid (8 bytes, a legitimate sector) couldn't key on 2 m.
        // Mid isn't checked against either: a Mid above High only gives that level more drive.
        c.bad = Self::curve_faults(b);
        if b[WINDOWS_AT[0]] == 0xFF {
            return Err(Fault::Windows);
        }
        for (w, &at) in c.win.iter_mut().zip(WINDOWS_AT.iter()) {
            *w = window(&b[at..at + 5])?.unwrap_or((0, 0));
        }
        Ok(c)
    }

    /// CAL-6.1: the reached entries per row (row id, the sector's offset of its High row, the step between levels,
    /// entries reached): UHF 0-12, VHF 0-15, 200-300 0-11 (200-260 MHz), HF 0-11. Stock's order: level 0 is High.
    const ROWS: [(u8, usize, usize, usize); 4] = [(0, 0x10 * UHF, 0x40, 13), (1, 0x10 * VHF, 0x40, 16), (2, 0x10 * R200, 0x40, 12), (3, 0x190, 0x10, 12)];

    /// CAL-6.1, CAL-20.4: each row's first curve fault: the fault ([`Fault::PaCurve`], an erased or unprogrammed
    /// byte; [`Fault::LevelOrder`], Low not below High), the entry and the level (0 Low, 1 Mid, 2 High; 0 for the
    /// order).
    fn row_fault(b: &[u8; READ_LEN], base: usize, step: usize, n: usize) -> Option<(Fault, u8, u8)> {
        let at = |lvl: usize, i: usize| b[base + step * (2 - lvl) + i];
        for i in 0..n {
            for lvl in 0..3 {
                let v = at(lvl, i);
                if v == 0 || v == 0xFF {
                    return Some((Fault::PaCurve, i as u8, lvl as u8));
                }
            }
        }
        (0..n).find(|&i| at(0, i) >= at(2, i)).map(|i| (Fault::LevelOrder, i as u8, 0))
    }

    fn curve_faults(b: &[u8; READ_LEN]) -> u8 {
        Self::ROWS.iter().filter(|&&(_, base, step, n)| Self::row_fault(b, base, step, n).is_some()).fold(0, |m, &(id, ..)| m | 1 << id)
    }

    /// CAL-20.4: the first curve fault and where: the fault, then the row ([`ROW_NAMES`]), the entry and the level.
    /// None for curves without one.
    pub fn curve_fault_at(b: &[u8; READ_LEN]) -> Option<(Fault, [u8; 3])> {
        Self::ROWS.iter().find_map(|&(id, base, step, n)| Self::row_fault(b, base, step, n).map(|(f, i, l)| (f, [id, i, l])))
    }

    /// CAL-6.1: the band rows locked by a curve fault, a bit per [`ROW_NAMES`] row.
    pub fn locked_rows(&self) -> u8 {
        self.bad
    }

    /// CAL-6.1: the row `f` keys on, if the curve checks cover it.
    fn row_of(f: Freq10) -> Option<u8> {
        match f / 100_000 {
            400..560 => Some(0),
            130..200 => Some(1),
            200..300 => Some(2),
            18..64 => Some(3),
            _ => None,
        }
    }

    /// TX-11, CAL-6.1: the PA drive for `f` at `power`, or why not: [`Refusal::Locked`] on a band whose curves
    /// failed a check (the rest of the radio transmits), [`Refusal::NotCalibrated`] outside every row or on an
    /// unprogrammed entry ([`TxCal::dac`]).
    pub fn pa(&self, f: Freq10, power: u8) -> Result<u8, crate::txplan::Refusal> {
        if Self::row_of(f).is_some_and(|r| self.bad & 1 << r != 0) {
            return Err(crate::txplan::Refusal::Locked);
        }
        self.dac(f, power).ok_or(crate::txplan::Refusal::NotCalibrated)
    }

    /// SAT-8.1: may a transmission already keyed move to `f` (the satellite uplink's Doppler retune)? The key-up's
    /// checks that depend on the frequency, again: the region's band plan (or Extend TX's range), this radio's
    /// calibration windows, no emergency frequency (a retune is never a person's confirmed key-up), and a row in the
    /// power table at `power`. 0 (a receive-only transponder's uplink) is outside every plan.
    pub fn keyed_retune(&self, f: Freq10, region: crate::txplan::Region, extend: bool, power: u8) -> Result<(), crate::txplan::Refusal> {
        use crate::txplan::{self, Refusal};
        txplan::check(f, region, extend)?;
        if !self.in_window(f) {
            return Err(Refusal::NotCalibrated);
        }
        if txplan::emergency(f) {
            return Err(Refusal::Emergency);
        }
        self.pa(f, power).map(|_| ())
    }

    /// CAL-11, TX-1: `f` lies in one of this radio's enabled TX windows, low <= f < high as stock checks it
    /// (0x0800BD8C). 18-64 MHz is a window on any valid sector (TX-41): stock's loader replaces the windows with
    /// 18-64 MHz in its HF work mode (settings +0x62 == 0), so the sector's own windows never hold HF.
    pub fn in_window(&self, f: Freq10) -> bool {
        (mhz(18)..mhz(64)).contains(&f) || self.win.iter().any(|&(lo, hi)| (mhz(lo as u32)..mhz(hi as u32)).contains(&f))
    }

    /// TONE-1, TONE-16: REG_51's gain for the sub-tone sent with `tone` (the tone word: bit 15 DCS) at the Subtone
    /// level (0 Low, 1 Normal: the calibration's byte, 2 High: stock's step up), within the 7-bit field. Stock masks
    /// the sum with 0x7F, which wraps a high byte to a low level; Ferrite stops at 0x7F.
    pub fn tone_dev(&self, tone: u16, level: u8) -> u8 {
        let k = (tone & 0x8000 != 0) as usize;
        let (g, step) = (self.tone[k], TONE_STEP[k]);
        match level {
            0 => g.saturating_sub(step),
            1 => g,
            _ => (g + step).min(0x7F),
        }
    }

    /// CAL-12, CAL-13: the REG_38/39 word for transmitting on `f`, the factory trim applied; the TX-7 read-back
    /// expects it. The TX checks (windows, region plan, PA row) take `f` itself: the trim corrects the crystal so
    /// that the carrier lands on `f`.
    pub fn tx_word(&self, f: Freq10) -> Freq10 {
        trimmed(f, self.trim)
    }

    /// TONE-5: the 55 Hz tail's gain, the CTCSS byte as stock sends it (0x0801D078), whatever the Subtone level.
    pub fn tail_dev(&self) -> u8 {
        self.tone[0]
    }

    /// The PA drive for transmitting on `f` at Ferrite's `power` (0 Low, 1 Mid, 2 High), stock's lookup
    /// (`pa_cal_lookup`): the row by frequency, the entry by its step, clamped as stock clamps (175-200 MHz the VHF
    /// row's last entry, 300-350 MHz the 350 row's first), except that 280-300 MHz takes the 200 row's last entry
    /// where stock reads past it. None outside every row's span (TX-11, CAL-10, TX audit F16): below 18 MHz,
    /// 64-130 MHz (stock falls back to High VHF entry 0) and 560 MHz up (stock reads past the UHF row); `txplan`
    /// refuses those first, and the lookup refuses them too. None also for an entry the factory left unprogrammed
    /// (0x00 or 0xFF, CAL-10): both radios' 200-300 MHz rows end at 260 MHz (entries 13-15 0xFF, which stock would
    /// send to the DAC as full drive) and their UHF rows at 520 MHz.
    pub fn dac(&self, f: Freq10, power: u8) -> Option<u8> {
        let level = 2 - band_power(f, power).min(2) as usize; // stock's order: 0 is High
        let mhz = f / 100_000;
        let p = &self.pa[level];
        let v = match mhz {
            560.. => return None,
            400.. => p[UHF][((mhz - 400) / 10) as usize],
            350..=399 => p[R350][((mhz - 350) / 5) as usize],
            300..=349 => p[R350][0],
            // CAL-14, TX-9: the 200-300 MHz PA path (1.25 m, and 243.0 MHz), Low only (band_power).
            200..=299 => p[R200][(((mhz - 200) / 5) as usize).min(15)],
            130..=199 => p[VHF][(((mhz - 130) / 3) as usize).min(15)],
            18..=63 => self.hf[level][(((mhz - 15) / 5) as usize).min(11)],
            // Below 18 MHz and 64-130 MHz: stock takes entry 0 of the High VHF row whatever the level; refused here.
            _ => return None,
        };
        (v != 0 && v != 0xFF).then_some(v)
    }
}

/// TX-9: the level that actually keys on `f` for the chosen `power` (0 Low, 1 Mid, 2 High). On the 1.25 m PA path
/// (200-300 MHz, 243.0 MHz included) it is always Low: Mid and High draw more current there for no more RF out, and
/// only stress the finals (decision 260). Every other band keys at the chosen level.
pub fn band_power(f: Freq10, power: u8) -> u8 {
    if (200..300).contains(&(f / 100_000)) {
        0
    } else {
        power
    }
}

/// The battery gauge (PWR-20): seven thresholds on the PA1 reading >> 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gauge(pub [u8; 7]);

impl Gauge {
    /// 0xF200's seven bytes, or stock's defaults when byte 0 is 0x00 or 0xFF; the seventh is always 0xBB.
    pub fn parse(b: &[u8; READ_LEN]) -> Gauge {
        let mut g = [0u8; 7];
        g.copy_from_slice(&b[GAUGE_AT..GAUGE_AT + 7]);
        if g[0] == 0 || g[0] == 0xFF {
            g = GAUGE_DEFAULT;
        }
        g[6] = 0xBB;
        Gauge(g)
    }

    /// Transmit allowed on this reading (the 12-bit PA1 conversion): at or above the first threshold (the
    /// cut-off, about 6.2 V) and not above the seventh (about 9 V, stock's level 7).
    /// Stock's defaults, for builds that don't read the calibration (the receive-only build reads no PA curves).
    pub const fn defaults() -> Gauge {
        Gauge(GAUGE_DEFAULT)
    }

    pub fn tx_ok(&self, adc: u16) -> bool {
        let v = (adc >> 4).min(255) as u8;
        v >= self.0[0] && v <= self.0[6]
    }
}

impl Default for Gauge {
    fn default() -> Self {
        Gauge(GAUGE_DEFAULT)
    }
}

/// A sector that passes every check (CAL-6), for tests here and in the store's (CAL-20); unused in the firmware.
#[doc(hidden)]
pub mod testing {
    use super::*;

    /// The factory-default calibration's tuning block and TX windows (0xF210-0xF24F; identical on both test
    /// radios): 136-174, 400-520, 200-260 and 330-400 MHz, all enabled; no crystal trim (byte 11 0xFF).
    pub const FACTORY_F210: [u8; 0x40] = [
        0x37, 0x46, 0x3e, 0x1f, 0x1f, 0x28, 0x2a, 0x2a, 0x2a, 0x01, 0x08, 0xff, 0x00, 0x00, 0xff, 0xff, //
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, //
        0x01, 0x01, 0x36, 0x01, 0x74, 0x01, 0x04, 0x00, 0x05, 0x20, 0x01, 0x02, 0x00, 0x02, 0x60, 0xff, //
        0x01, 0x03, 0x30, 0x04, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, //
    ];

    /// The test sector's PA curve entry for stock's level `l` (0 High), row `r`, index `i`: High above Mid above
    /// Low at every index, as a real sector (TX-14).
    pub fn curve(l: usize, r: usize, i: usize) -> u8 {
        (180 - l * 40 + r * 4 + i) as u8
    }

    /// The HF rows' entries, the same way.
    pub fn hf_curve(l: usize, i: usize) -> u8 {
        (140 - l * 40 + i) as u8
    }

    /// A sector with the curves above, the gauge given, and the factory-default tuning block and windows.
    pub fn sector(gauge: [u8; 7]) -> [u8; READ_LEN] {
        let mut b = [0x55u8; READ_LEN];
        for l in 0..3 {
            for r in 0..4 {
                for i in 0..16 {
                    b[0x40 * l + 0x10 * r + i] = curve(l, r, i);
                }
            }
            for i in 0..16 {
                b[0x190 + 0x10 * l + i] = hf_curve(l, i);
            }
        }
        b[BLANK_AT] = 0x00;
        b[GAUGE_AT..GAUGE_AT + 7].copy_from_slice(&gauge);
        b[0x210..0x250].copy_from_slice(&FACTORY_F210);
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::testing::{curve, hf_curve, sector};

    /// SAT-8.1: a keyed uplink retune passes the key-up's frequency checks: a Doppler step inside 70 cm goes; outside
    /// the plan, 0, an emergency frequency or outside the calibration doesn't.
    #[test]
    fn a_keyed_retune_is_checked() {
        use crate::txplan::{Refusal, Region};
        let c = TxCal::read(&sector(GAUGE_DEFAULT)).expect("valid");
        let up = mhz(435) + 25_000;
        assert_eq!(c.keyed_retune(up, Region::Canada, false, 2), Ok(()));
        assert_eq!(c.keyed_retune(up + 1_000, Region::Canada, false, 2), Ok(()), "a Doppler step");
        assert_eq!(c.keyed_retune(0, Region::Canada, false, 2), Err(Refusal::OutOfBand), "a receive-only transponder");
        assert_eq!(c.keyed_retune(mhz(1268), Region::Canada, false, 2), Err(Refusal::OutOfBand), "23 cm");
        assert_eq!(c.keyed_retune(mhz(406) + 2_500, Region::Canada, true, 2), Err(Refusal::Emergency), "406 MHz, even with Extend TX");
        assert_eq!(c.keyed_retune(mhz(160), Region::Canada, false, 2), Err(Refusal::OutOfBand), "outside the plan");
    }

    /// CAL-20.4: where a curve check failed: an erased byte in HF's Mid row, then Low not below High on VHF.
    #[test]
    fn curve_fault_located() {
        let mut b = sector(GAUGE_DEFAULT);
        assert_eq!(TxCal::curve_fault_at(&b), None);
        b[0x190 + 0x10 + 3] = 0xFF; // HF, stock's level 1 (Mid), entry 3
        assert_eq!(TxCal::curve_fault_at(&b), Some((Fault::PaCurve, [3, 3, 1])));
        let mut b = sector(GAUGE_DEFAULT);
        b[0x80 + 0x10 * VHF + 5] = b[0x10 * VHF + 5]; // VHF entry 5: Low equal to High
        assert_eq!(TxCal::curve_fault_at(&b), Some((Fault::LevelOrder, [1, 5, 0])));
    }

    /// CAL-6.1 (issue #40, decision 412): the reporter's sector: our factory-default one with its HF High row tuned
    /// below the flat Mid (8 bytes). It locked every band; it's valid now, every band transmits, HF included.
    #[test]
    fn a_tuned_hf_row_is_valid() {
        let mut b = sector(GAUGE_DEFAULT);
        for (i, v) in [145u8, 155, 172, 188, 165, 145, 110, 105].into_iter().enumerate() {
            b[0x190 + 1 + i] = v; // High HF, entries 1-8
            b[0x190 + 0x10 + 1 + i] = 150; // Mid flat at 150: above High at entries 1, 6, 7, 8
            b[0x190 + 0x20 + 1 + i] = 100; // Low flat at 100
        }
        let c = TxCal::read(&b).expect("valid");
        assert_eq!(c.locked_rows(), 0);
        assert_eq!(TxCal::curve_fault_at(&b), None);
        for f in [mhz(21), mhz(28), mhz(52), mhz(146), mhz(446)] {
            assert!(c.pa(f, 2).is_ok(), "{f}");
        }
    }

    /// CAL-6.1: a curve fault locks its band only: an erased HF byte refuses 28 MHz as Locked and keys 2 m and
    /// 70 cm; Low above High on VHF locks 2 m only.
    #[test]
    fn a_curve_fault_locks_its_band_only() {
        use crate::txplan::Refusal;
        let mut b = sector(GAUGE_DEFAULT);
        b[0x190 + 0x20 + 2] = 0xFF; // Low HF entry 2
        let c = TxCal::read(&b).expect("the sector stands");
        assert_eq!(c.locked_rows(), 1 << 3);
        assert_eq!(c.pa(mhz(28), 2), Err(Refusal::Locked));
        assert_eq!(c.pa(mhz(28), 0), Err(Refusal::Locked), "every level on that band");
        assert!(c.pa(mhz(146), 2).is_ok() && c.pa(mhz(446), 2).is_ok());
        let mut b = sector(GAUGE_DEFAULT);
        b[0x80 + 0x10 * VHF + 7] = b[0x10 * VHF + 7] + 1; // Low above High at VHF entry 7
        let c = TxCal::read(&b).expect("the sector stands");
        assert_eq!(c.locked_rows(), 1 << 1);
        assert_eq!(c.pa(mhz(146), 2), Err(Refusal::Locked));
        assert!(c.pa(mhz(446), 2).is_ok() && c.pa(mhz(28), 2).is_ok());
        assert_eq!(c.pa(mhz(100), 2), Err(Refusal::NotCalibrated), "outside every row, as before");
    }

    /// TX-13, CAL-4, CAL-5, CAL-6.1: calibration is checked, never replaced: an erased sector locks transmit, a hole
    /// in a used row locks its band, instead of falling back to built-in curves.
    #[test]
    fn blank_and_holes_refuse() {
        let mut b = sector(GAUGE_DEFAULT);
        assert!(TxCal::parse(&b).is_some());
        b[BLANK_AT] = 0xFF;
        assert_eq!(TxCal::parse(&b), None, "CAL-5: an erased sector");
        let mut b = sector(GAUGE_DEFAULT);
        b[0x40 + 0x10 * VHF + 5] = 0xFF;
        let c = TxCal::parse(&b).expect("CAL-6.1: a hole locks its band, not the sector");
        assert_eq!(c.pa(mhz(146), 1), Err(crate::txplan::Refusal::Locked), "a hole in the Mid VHF row: 2 m locked");
        let mut b = sector(GAUGE_DEFAULT);
        b[UHF * 0x10 + 14] = 0xFF;
        assert!(TxCal::parse(&b).is_some(), "UHF entries past 520 MHz aren't reached on amateur bands");
    }

    /// TX-14, CAL-6.1, TX-15, CAL-7 (TX audit F8): Low below High at every reached entry, its band locked
    /// otherwise (Mid unchecked); the MCU check and the CRC since install gate the result.
    #[test]
    fn level_order_mcu_and_crc() {
        let b = sector(GAUGE_DEFAULT);
        assert!(TxCal::read(&b).is_ok());
        let mut o = b;
        o[0x40 + 0x10 * VHF + 7] = o[0x10 * VHF + 7]; // Mid = High at VHF entry 7: fine
        assert_eq!(TxCal::read(&o).map(|c| c.locked_rows()), Ok(0));
        let mut o = b;
        o[0x190 + 0x20 + 3] = 0xFE; // Low HF (stock's third row) above High at entry 3
        assert_eq!(TxCal::read(&o).map(|c| c.locked_rows()), Ok(1 << 3));
        let mut o = b;
        o[0x80 + 0x10 * UHF + 14] = 0xFE; // past 520 MHz: not reached on the amateur bands
        assert!(TxCal::read(&o).is_ok());
        let c = TxCal::read(&b);
        assert!(gate(c, true, CalCrc::Same).is_ok() && gate(c, true, CalCrc::Recorded).is_ok());
        assert_eq!(gate(c, false, CalCrc::Same).err(), Some(Fault::Mcu));
        assert_eq!(gate(c, true, CalCrc::Changed).err(), Some(Fault::Changed));
        assert_eq!(gate(c, true, CalCrc::Unknown).err(), Some(Fault::Changed));
        assert_eq!(gate(Err(Fault::Blank), false, CalCrc::Changed).err(), Some(Fault::Mcu), "the MCU first");
    }

    /// CAL-11, TX-1.2, TX-14, TX-41: the windows as stock checks them (low <= f < high, whole MHz from BCD), the
    /// factory default's four; 18-64 MHz as stock's HF work mode replaces them, whatever the sector's windows say.
    #[test]
    fn tx_windows_as_stock() {
        let c = TxCal::read(&sector(GAUGE_DEFAULT)).unwrap();
        for (f, inside) in [
            (mhz(146) + 52_000, true),
            (mhz(136), true),
            (mhz(136) - 1, false),
            (mhz(174) - 1, true),
            (mhz(174), false),
            (mhz(190), false),
            (mhz(223) + 50_000, true),
            (mhz(260), false),
            (mhz(330), true),
            (mhz(446), true),
            (mhz(520), false),
            (mhz(28) + 40_000, true),
            (mhz(52), true),
            (mhz(27) + 18_500, true),
            (mhz(18), true),
            (mhz(18) - 1, false),
            (mhz(64), false),
        ] {
            assert_eq!(c.in_window(f), inside, "{f}");
        }
        // A window turned off (enable 0x00) is out, whatever its edges hold.
        let mut b = sector(GAUGE_DEFAULT);
        b[0x235] = 0x00;
        b[0x236] = 0xAB;
        let c = TxCal::read(&b).unwrap();
        assert!(!c.in_window(mhz(446)) && c.in_window(mhz(146)));
        // An erased 0xF240: window 4 off, the rest kept.
        let mut b = sector(GAUGE_DEFAULT);
        b[0x240..0x250].fill(0xFF);
        let c = TxCal::read(&b).unwrap();
        assert!(!c.in_window(mhz(350)) && c.in_window(mhz(446)));
    }

    /// TX-13, TX-14, CAL-4: blank or invalid windows lock transmit instead of falling back to stock's built-in
    /// ones: 0xF230 byte 0 erased, a digit over 9, low not below high, an enable byte stock doesn't write.
    #[test]
    fn bad_windows_lock() {
        let bad = |at: usize, v: u8| {
            let mut b = sector(GAUGE_DEFAULT);
            b[at] = v;
            TxCal::read(&b).err()
        };
        assert_eq!(bad(0x230, 0xFF), Some(Fault::Windows), "blank: never stock's built-in windows");
        assert_eq!(bad(0x232, 0x3A), Some(Fault::Windows), "not BCD");
        assert_eq!(bad(0x238, 0x03), Some(Fault::Windows), "UHF high edge 320, below its low edge 400");
        assert_eq!(bad(0x234, 0x36), Some(Fault::Windows), "VHF low = high (136)");
        assert_eq!(bad(0x23A, 0x02), Some(Fault::Windows), "enable byte 2");
        assert_eq!(bad(0x234, 0x75), None, "136-175 MHz: still a window");
        assert!(TxCal::parse(&sector(GAUGE_DEFAULT)).is_some());
    }

    /// CAL-12, CAL-6 (decision 222, TX audit M4): a factory crystal trim (marker 0xA0) within ±20 ppm is applied to
    /// the transmit word; one over 20 ppm locks transmit; no marker (both radios' 0xFF) or k without it is none.
    #[test]
    fn factory_trim_applies() {
        let b = sector(GAUGE_DEFAULT);
        assert_eq!(factory_trim(&b), None, "factory default: no marker");
        assert_eq!(TxCal::read(&b).unwrap().tx_word(mhz(446)), mhz(446));
        let mut b = sector(GAUGE_DEFAULT);
        b[TRIM_AT..TRIM_AT + 3].copy_from_slice(&[0xA0, 0x2C, 0xFF]); // k = -212, about -4.9 ppm
        assert_eq!(factory_trim(&b), Some(-212));
        let c = TxCal::read(&b).unwrap();
        // 446.000 MHz: 44_600_000 + 446_000 * -212 / 435_500 = 44_600_000 - 217 (2.17 kHz low).
        assert_eq!(c.tx_word(mhz(446)), mhz(446) - 217);
        assert_eq!(c.tx_word(mhz(146) + 52_000), mhz(146) + 52_000 - 71, "-71.3 rounds toward zero");
        b[TRIM_AT + 1..TRIM_AT + 3].copy_from_slice(&TRIM_MAX.to_le_bytes());
        assert!(TxCal::read(&b).is_ok(), "20 ppm: the limit is in");
        b[TRIM_AT + 1..TRIM_AT + 3].copy_from_slice(&(-TRIM_MAX - 1).to_le_bytes());
        assert_eq!(TxCal::read(&b).err(), Some(Fault::Trim), "over 20 ppm: locked");
        assert_eq!(valid_trim(&b), 0, "and receive runs untrimmed");
        let mut b = sector(GAUGE_DEFAULT);
        b[TRIM_AT..TRIM_AT + 3].copy_from_slice(&[0x00, 0x2C, 0xFF]);
        assert_eq!(valid_trim(&b), 0, "k without the marker: stock ignores it");
    }

    /// CAL-12: stock's formula, one unit about 0.023 ppm: +100 at 435.5 MHz is +1 kHz, -100 at 145.5 MHz -334 Hz.
    #[test]
    fn trim_formula_as_stock() {
        assert_eq!(trimmed(mhz(435) + 50_000, 100), mhz(435) + 50_000 + 100);
        assert_eq!(trimmed(mhz(145) + 50_000, -100), mhz(145) + 50_000 - 33);
        assert_eq!(trimmed(mhz(1300), TRIM_MAX), mhz(1300) + 2_600, "20 ppm at the top of receive, no overflow");
        assert_eq!(trimmed(mhz(18), 0), mhz(18));
    }

    /// CAL-10 (decision 222): an entry the factory left 0xFF is refused, never sent to the DAC: the radios' 200-300
    /// MHz Low row (0xF0A0) ends at 260 MHz and their UHF rows at 520 MHz, with 0xFF after.
    #[test]
    fn unprogrammed_entries_refused() {
        let mut b = sector(GAUGE_DEFAULT);
        b[0xA0..0xB0].copy_from_slice(&[0x7D, 0x7D, 0x7D, 0x7D, 0x7D, 0x7D, 0x7D, 0x7D, 0x7D, 0x7D, 0x7D, 0x7D, 0x64, 0xFF, 0xFF, 0xFF]);
        b[0x0D..0x10].fill(0xFF);
        let c = TxCal::read(&b).unwrap();
        assert_eq!(c.dac(mhz(223) + 50_000, 0), Some(0x7D));
        assert_eq!(c.dac(mhz(262), 0), Some(0x64));
        for f in [mhz(265), mhz(280), mhz(299)] {
            assert_eq!(c.dac(f, 0), None, "{f}: 0xFF, stock's full drive");
        }
        assert_eq!(c.dac(mhz(529), 2), Some(curve(0, UHF, 12)));
        assert_eq!(c.dac(mhz(530), 2), None);
    }

    /// TONE-1.2, TONE-1.3, TONE-16, TONE-5, TX-14 (TX audit F7): the sub-tone's deviation from the calibration, the
    /// factory default's 0x46 CTCSS and 0x3E DCS at Normal, stock's step for High, the same step down for Low, 7 bits at
    /// most; the tail takes the CTCSS byte. A byte over 0x7F locks transmit.
    #[test]
    fn subtone_deviation_from_the_calibration() {
        let c = TxCal::read(&sector(GAUGE_DEFAULT)).unwrap();
        let dcs = 0x8000 | 0o23;
        assert_eq!((c.tone_dev(885, 1), c.tone_dev(dcs, 1)), (0x46, 0x3E));
        assert_eq!((c.tone_dev(885, 2), c.tone_dev(dcs, 2)), (0x46 + 0x0F, 0x3E + 0x14));
        assert_eq!((c.tone_dev(885, 0), c.tone_dev(dcs, 0)), (0x46 - 0x0F, 0x3E - 0x14));
        assert_eq!(c.tail_dev(), 0x46);
        let mut b = sector(GAUGE_DEFAULT);
        b[TONE_AT] = 0x7A;
        assert_eq!(TxCal::read(&b).unwrap().tone_dev(885, 2), 0x7F, "never past the field (stock wraps)");
        b[TONE_AT + 1] = 0x80;
        assert_eq!(TxCal::read(&b).err(), Some(Fault::ToneGain));
    }

    /// CAL-14, TX-9: the 1.25 m PA path (200-300 MHz, 243.0 MHz included) keys Low whatever level is chosen
    /// (decision 260), from stock's lookup ((MHz-200)/5 on the Low 200-300 row); a hole or a level out of order in
    /// its reached entries still locks.
    #[test]
    fn band_125m_low_only() {
        let c = TxCal::parse(&sector(GAUGE_DEFAULT)).unwrap();
        for (f, i) in [(mhz(223) + 50_000, 4), (mhz(243), 8), (mhz(259), 11)] {
            for power in 0..3 {
                assert_eq!(c.dac(f, power), Some(curve(2, R200, i)), "{f}: level {power} keys Low");
            }
        }
        assert_eq!(band_power(mhz(223), 2), 0);
        assert_eq!(band_power(mhz(200), 2), 0, "200 MHz: Low");
        assert_eq!(band_power(mhz(299) + 99_999, 2), 0, "the whole path");
        assert_eq!(band_power(mhz(199) + 99_999, 2), 2, "below the path: as chosen");
        assert_eq!(band_power(mhz(146), 2), 2, "2 m keeps the chosen level");
        assert_eq!(band_power(mhz(446), 1), 1, "70 cm too");
        let mut b = sector(GAUGE_DEFAULT);
        b[0x80 + 0x10 * R200 + 4] = 0xFF;
        assert_eq!(TxCal::read(&b).map(|c| c.locked_rows()), Ok(1 << 2), "a hole in Low 200-300 at 220 MHz: 1.25 m locked");
        let mut b = sector(GAUGE_DEFAULT);
        b[0x80 + 0x10 * R200 + 4] = b[0x10 * R200 + 4];
        assert_eq!(TxCal::read(&b).map(|c| c.locked_rows()), Ok(1 << 2), "Low = High at 220 MHz");
        let mut b = sector(GAUGE_DEFAULT);
        b[0x10 * R200 + 12] = 0xFF;
        assert!(TxCal::read(&b).is_ok(), "260 MHz up isn't reached");
    }

    /// TX-8, CAL-8, CAL-9, TX-11, CAL-10: the PA drive comes only from the band's calibration curve at the chosen
    /// level (1.25 m too, CAL-14), by stock's lookup (High is stock's row 0, Low its row 2), and never
    /// reads past its row's 16 entries; outside every row the lookup refuses (TX audit F16).
    #[test]
    fn lookup_as_stock() {
        let c = TxCal::parse(&sector(GAUGE_DEFAULT)).unwrap();
        let at = |l: usize, r: usize, i: usize| Some(curve(l, r, i));
        // High is stock's level 0: 146.52 MHz is VHF entry (146-130)/3 = 5.
        assert_eq!(c.dac(mhz(146) + 52_000, 2), at(0, VHF, 5));
        assert_eq!(c.dac(mhz(146) + 52_000, 0), at(2, VHF, 5), "Low is stock's level 2");
        assert_eq!(c.dac(mhz(446), 1), at(1, UHF, 4), "UHF entry (446-400)/10 = 4, Mid");
        assert_eq!(c.dac(mhz(190), 2), at(0, VHF, 15), "175-200 MHz: entry 15");
        assert_eq!(c.dac(mhz(223) + 50_000, 2), at(2, R200, 4), "1.25 m: High chosen keys Low (CAL-14, TX-9)");
        assert_eq!(c.dac(mhz(28) + 40_000, 2), Some(hf_curve(0, 2)), "10 m: HF entry (28-15)/5 = 2");
        assert_eq!(c.dac(mhz(290), 2), at(2, R200, 15), "280-300: the (Low) row's last entry, not the next row");
        assert_eq!(c.dac(mhz(559), 2), at(0, UHF, 15), "the UHF row's last entry");
        assert_eq!(c.dac(mhz(355), 2), at(0, R350, 1), "350-400: entry (355-350)/5 = 1");
        assert_eq!(c.dac(mhz(320), 1), at(1, R350, 0), "300-350: the 350 MHz entry, not past the row");
        for f in [mhz(70), mhz(129) + 99_999, mhz(17) + 99_999, mhz(1), mhz(560), mhz(600)] {
            assert_eq!(c.dac(f, 2), None, "{f}: outside every row (stock: High VHF entry 0, or past the row)");
        }
    }

    /// PWR-20: the battery gauge is stock's: the PA1 reading against the seven thresholds at 0xF200 (stock's
    /// defaults when blank), transmit refused below the cut-off and above about 9 V.
    #[test]
    fn gauge() {
        let g = Gauge::parse(&sector([0xFF; 7]));
        assert_eq!(g, Gauge::default(), "blank: stock's defaults");
        assert!(!g.tx_ok(0x80 << 4), "below the cut-off");
        assert!(g.tx_ok(0x81 << 4) && g.tx_ok(0xBB << 4));
        assert!(!g.tx_ok(0xBC << 4), "over about 9 V");
        let g = Gauge::parse(&sector([0x70, 0x80, 0x90, 0xA0, 0xB0, 0xC0, 0xD0]));
        assert_eq!(g.0[6], 0xBB, "the seventh is always 0xBB");
        assert!(g.tx_ok(0x70 << 4));
    }
}

#[cfg(test)]
mod battery_tests {
    use super::*;

    /// PWR-20: stock's factor and gauge; the black radio's 8.312 V pack sits on the top level.
    #[test]
    fn battery_reading() {
        assert_eq!(battery_mv(172 << 4), 8290);
        let g = Gauge::defaults();
        assert!(crate::battery::percent(&g, 172 << 4) >= 90, "PWR-22: 8.29 V is over 90 %");
        assert_eq!(crate::battery::percent(&g, 0x80 << 4), 0);
    }
}
