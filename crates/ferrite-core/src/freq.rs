//! Frequencies in 10 Hz units, the unit the BK4829 frequency registers use.

pub type Freq10 = u32;

pub const fn mhz(x: u32) -> Freq10 {
    x * 100_000
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Band {
    Hf,
    Vhf,
    B200,
    B300,
    Uhf,
}

/// Band and owning chip (0 = BK4829 #0, 64-300 MHz; 1 = BK4829 #1, 300 MHz up and HF), as stock splits them.
pub fn band(f: Freq10) -> (Band, u8) {
    match f {
        _ if f < mhz(64) => (Band::Hf, 1),
        _ if f < mhz(200) => (Band::Vhf, 0),
        _ if f < mhz(300) => (Band::B200, 0),
        _ if f < mhz(400) => (Band::B300, 1),
        _ => (Band::Uhf, 1),
    }
}

/// What the BK4829s receive: home VFOs and channels. The datasheet says 18-580 and 760-1160 MHz; as nicFW does,
/// Ferrite tunes 18-630 and 756-1300 MHz, both ends included, where the PLL is reported to lock (VFO-21; to
/// confirm on the bench, and pulled back if it won't lock at 1.2 GHz). Receive only above 1000 MHz: no transmit
/// plan reaches it.
/// Below 18 MHz is HF mode's, on the SI4732 (VFO-14).
pub const RX_RANGES: [(Freq10, Freq10); 2] = [(mhz(18), mhz(630)), (mhz(756), mhz(1300))];

/// Whether a chip can receive `f` ([`RX_RANGES`], inclusive: 630.000 and 1300.000 are in, VFO-21).
pub fn receivable(f: Freq10) -> bool {
    RX_RANGES.iter().any(|&(lo, hi)| f >= lo && f <= hi)
}

/// Whether `f` needs the band relay (PC4) switched to the HF path: chip 1 below 64 MHz ([`band`]'s HF).
pub fn needs_relay(f: Freq10) -> bool {
    band(f).0 == Band::Hf
}

/// Digits of v, at least `width` of them, into buf from pos; keeps one byte spare like the C version
/// (which NUL-terminates), so it truncates the same way.
fn put_uint(buf: &mut [u8], mut pos: usize, mut v: u32, width: usize) -> usize {
    let (mut tmp, mut n) = ([0u8; 10], 0);
    loop {
        tmp[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if v == 0 || n == 10 {
            break;
        }
    }
    while n < width {
        tmp[n] = b'0';
        n += 1;
    }
    while n > 0 && pos + 1 < buf.len() {
        n -= 1;
        buf[pos] = tmp[n];
        pos += 1;
    }
    pos
}

/// "146.520 00": MHz, three decimals, space, the last two digits (100 Hz and 10 Hz). Returns the length.
pub fn format(f: Freq10, buf: &mut [u8]) -> usize {
    if buf.is_empty() {
        return 0;
    }
    let (mhz, rest) = (f / 100_000, f % 100_000);
    let mut p = put_uint(buf, 0, mhz, 1);
    if p + 1 < buf.len() {
        buf[p] = b'.';
        p += 1;
    }
    p = put_uint(buf, p, rest / 100, 3);
    if p + 1 < buf.len() {
        buf[p] = b' ';
        p += 1;
    }
    put_uint(buf, p, rest % 100, 2)
}

/// What the user typed on the keypad ("147.06", "443.475", "7.1") in 10 Hz units; None for anything that
/// is not a plain decimal MHz value.
pub fn parse(s: &[u8]) -> Option<Freq10> {
    let (mut mhz, mut frac, mut digits, mut fdigits, mut dot) = (0u32, 0u32, 0, 0, false);
    for &c in s {
        match c {
            b'.' if dot => return None,
            b'.' => dot = true,
            b'0'..=b'9' if !dot => {
                digits += 1;
                if digits > 4 {
                    return None;
                }
                mhz = mhz * 10 + (c - b'0') as u32;
            }
            b'0'..=b'9' => {
                fdigits += 1;
                if fdigits > 5 {
                    return None;
                }
                frac = frac * 10 + (c - b'0') as u32;
            }
            _ => return None,
        }
    }
    if digits == 0 {
        return None;
    }
    for _ in fdigits..5 {
        frac *= 10;
    }
    Some(mhz * 100_000 + frac)
}

/// The airband's voice band, where 8.33 kHz channels are named (VFO-18.2): 117.975 to 137.000 MHz, the band ICAO
/// Annex 10 Volume V 4.1.1 allots to the aeronautical mobile (R) service.
pub const AIR_COM: (Freq10, Freq10) = (11_797_500, 13_700_000);

/// Display only: the value is an 8.33 kHz channel name, not a frequency, and is drawn with three decimals and no
/// 10 Hz digits ([`channel_name`]). Never tuned: [`channel_freq`] gives the frequency a name stands for.
pub const NAMED: Freq10 = 1 << 31;

/// VFO-18.2: the channel name of an airband frequency on the 8.33 kHz grid, as ICAO Annex 10 Volume V Table
/// 4-1 (bis) pairs them: each 25 kHz block keeps its 25 kHz name (118.000) for its base, and its other two 8.33 kHz
/// channels are named 10 and 15 kHz above the base (118.00833 is 118.010, 118.01667 is 118.015). The base's own
/// 8.33 kHz name (118.005) is the same frequency; the 25 kHz name is shown, so 121.500 stays 121.500. None off
/// the grid or off the band. The name is in 10 Hz units (11_801_000 for 118.010).
pub fn channel_name(f: Freq10) -> Option<Freq10> {
    if !(AIR_COM.0..AIR_COM.1).contains(&f) {
        return None;
    }
    let (block, r) = (f / 2500 * 2500, f % 2500);
    match r {
        0 => Some(f),
        833 => Some(block + 1000),
        1667 => Some(block + 1500),
        _ => None,
    }
}

/// VFO-18.2: the frequency an airband channel name stands for (Table 4-1 (bis)): a 25 kHz block's base name
/// (118.000) and its first 8.33 kHz name (118.005) are the base, +10 kHz (118.010) is +8.333 kHz and +15 kHz
/// (118.015) is +16.667 kHz. None for any other value: the unused +20 kHz (118.020), a value off the 5 kHz names
/// (VDL Mode 3's 118.001-118.004 aren't voice) or off the band.
pub fn channel_freq(name: Freq10) -> Option<Freq10> {
    if !(AIR_COM.0..AIR_COM.1).contains(&name) {
        return None;
    }
    let (block, r) = (name / 2500 * 2500, name % 2500);
    match r {
        0 | 500 => Some(block),
        1000 => Some(block + 833),
        1500 => Some(block + 1667),
        _ => None,
    }
}

/// f for display with the 8.33 kHz step (VFO-18.2): its channel name tagged [`NAMED`] on the airband's grid, else
/// f itself.
pub fn shown_833(f: Freq10) -> Freq10 {
    channel_name(f).map_or(f, |n| n | NAMED)
}

/// Tiny self-check for the on-target image.
pub fn selftest() -> bool {
    let mut b = [0u8; 16];
    format(mhz(146) + 52_000, &mut b);
    let f = parse(b"443.475");
    b[0] == b'1' && b[3] == b'.' && b[7] == b' ' && b[9] == b'0' && f == Some(44_347_500) && band(44_347_500).1 == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(f: Freq10, len: usize) -> std::string::String {
        let mut b = [0u8; 16];
        let n = format(f, &mut b[..len]);
        std::string::String::from_utf8(b[..n].to_vec()).unwrap()
    }

    #[test]
    fn formatting() {
        assert_eq!(fmt(14_652_000, 16), "146.520 00");
        assert_eq!(fmt(44_347_500, 16), "443.475 00");
        assert_eq!(fmt(710_000, 16), "7.100 00");
        assert_eq!(fmt(14_652_012, 16), "146.520 12");
        assert_eq!(fmt(14_652_000, 5).len(), 4); // truncates, never overflows
    }

    #[test]
    fn parsing() {
        assert_eq!(parse(b"147.06"), Some(14_706_000));
        assert_eq!(parse(b"443.475"), Some(44_347_500));
        assert_eq!(parse(b"7.1"), Some(710_000));
        assert_eq!(parse(b"146.52012"), Some(14_652_012));
        assert_eq!(parse(b"146.520123"), None); // finer than 10 Hz
        assert_eq!(parse(b"1.2.3"), None);
        assert_eq!(parse(b""), None);
        assert_eq!(parse(b".5"), None);
        assert_eq!(parse(b"12a"), None);
    }

    #[test]
    fn bands() {
        assert_eq!(band(mhz(27)), (Band::Hf, 1));
        assert_eq!(band(mhz(64)), (Band::Vhf, 0));
        assert_eq!(band(mhz(146)).1, 0);
        assert_eq!(band(mhz(220)), (Band::B200, 0));
        assert_eq!(band(mhz(300)), (Band::B300, 1));
        assert_eq!(band(mhz(446)), (Band::Uhf, 1));
        assert!(selftest());
    }

    /// VFO-21: 18-630 and 756-1300 MHz, both ends included; the gap and HF mode's range below aren't.
    #[test]
    fn receivable_ranges() {
        for f in [mhz(18), mhz(146), mhz(630), mhz(756), mhz(1300)] {
            assert!(receivable(f), "{f}");
        }
        for f in [mhz(18) - 1, mhz(630) + 1, mhz(700), mhz(756) - 1, mhz(1300) + 1, 0] {
            assert!(!receivable(f), "{f}");
        }
        assert!(needs_relay(mhz(28)) && !needs_relay(mhz(146)));
    }

    /// Room for nothing writes nothing; more than four MHz digits is no frequency.
    #[test]
    fn format_and_parse_limits() {
        assert_eq!(fmt(14_652_000, 0), "");
        assert_eq!(fmt(14_652_000, 4), "146");
        assert_eq!(fmt(130_000_000, 16), "1300.000 00");
        assert_eq!(parse(b"12345.6"), None);
        assert_eq!(parse(b"1300"), Some(130_000_000));
    }
    /// VFO-18.2: the pairing of ICAO Annex 10 Volume V Table 4-1 (bis), its 118.000-118.100 rows, both ways.
    #[test]
    fn channel_names_833() {
        let n = |s: &[u8]| parse(s).unwrap();
        // (channel name, frequency) as the table lists them; the 25 kHz names and their first 8.33 name share one
        for (name, f) in [
            ("118.000", "118.00000"),
            ("118.005", "118.00000"),
            ("118.010", "118.00833"),
            ("118.015", "118.01667"),
            ("118.025", "118.02500"),
            ("118.030", "118.02500"),
            ("118.035", "118.03333"),
            ("118.040", "118.04167"),
            ("118.050", "118.05000"),
            ("118.055", "118.05000"),
            ("118.060", "118.05833"),
            ("118.065", "118.06667"),
            ("118.075", "118.07500"),
            ("118.080", "118.07500"),
            ("118.085", "118.08333"),
            ("118.090", "118.09167"),
            ("118.100", "118.10000"),
            ("121.500", "121.50000"),
            ("136.990", "136.99167"),
        ] {
            assert_eq!(channel_freq(n(name.as_bytes())), Some(n(f.as_bytes())), "{name}");
        }
        // Frequency to name: the 25 kHz name for a block's base, so 121.500 stays 121.500
        for (f, name) in [("118.00000", "118.000"), ("118.00833", "118.010"), ("118.01667", "118.015"), ("118.04167", "118.040"), ("121.50000", "121.500"), ("117.97500", "117.975")] {
            assert_eq!(channel_name(n(f.as_bytes())), Some(n(name.as_bytes())), "{f}");
        }
        // Every name maps back to itself or to its block's base name, all through the band
        let mut g = AIR_COM.0;
        while g < AIR_COM.1 {
            for o in [0, 833, 1667] {
                let name = channel_name(g + o).unwrap();
                assert_eq!(channel_freq(name), Some(g + o));
            }
            g += 2500;
        }
        // Not names: +20 kHz, VDL Mode 3's slots, the 10 Hz digits, off the band
        for s in ["118.020", "118.045", "118.001", "118.024", "118.0083", "117.970", "137.000", "108.000", "146.010"] {
            assert_eq!(channel_freq(n(s.as_bytes())), None, "{s}");
        }
        // Not on the grid or off the band: no name, and shown as the frequency
        for f in [n(b"118.008"), n(b"118.010"), n(b"137.00000"), n(b"146.00833")] {
            assert_eq!(channel_name(f), None);
            assert_eq!(shown_833(f), f);
        }
        assert_eq!(shown_833(n(b"118.00833")), n(b"118.010") | NAMED);
    }
}
