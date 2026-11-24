//! # nmea
//!
//! An incremental NMEA 0183 reader for GPS modules (the RT-950 Pro has an ATGM336H on a UART): feed bytes as
//! they arrive, read the fix. `no_std`, no allocation, integer only, about 300 bytes of state. Understands GGA,
//! RMC and GSV from any talker (GP, GN, GL, GA, BD, GB); checksums are checked, bad or overlong sentences counted
//! and dropped.

#![no_std]

#[derive(Clone, Copy, Default)]
pub struct Sat {
    pub prn: u8,
    pub snr: u8, // 0 = tracked but no signal
    pub talker: u8,
}

#[derive(Clone, Copy)]
pub struct Nmea {
    pub sentences: u32,
    pub checksum_errors: u32,
    pub overlong: u32,
    pub last_type: [u8; 5], // e.g. "GNRMC"; zeros until the first good sentence
    pub valid: bool,        // RMC status A
    pub quality: u8,        // GGA fix quality, 0 = none
    pub sats_used: u8,      // GGA
    pub sats_view: u8,      // GSV, summed over constellations
    pub gsv_view: [u8; 6],  // per talker: GP GL GA BD GB GQ
    pub sat: [Sat; 24],     // from GSV
    pub nsat: u8,
    pub hdop10: u16,
    pub alt_dm: i32,
    pub lat_e7: i32, // degrees x 1e7, south and west negative
    pub lon_e7: i32,
    pub utc_hms: u32,  // hhmmss
    pub date_dmy: u32, // ddmmyy, from RMC
    pub speed_kn10: u32, // speed over ground, tenths of a knot (RMC)
    pub course10: u16,   // course over ground, tenths of a degree from true north (RMC; 0 when not moving)
    line: [u8; 88],
    len: u8,
    in_sentence: bool,
}

const TALKERS: [&[u8; 2]; 6] = [b"GP", b"GL", b"GA", b"BD", b"GB", b"GQ"];

/// Field i (0 = the address field) of a comma-separated sentence body.
fn field(body: &[u8], i: usize) -> Option<&[u8]> {
    body.split(|&c| c == b',').nth(i)
}

/// Unsigned decimal with up to `frac` fractional digits, scaled by 10^frac. None when empty or bad.
fn fixed(s: &[u8], frac: u32) -> Option<i64> {
    if s.is_empty() || s.len() > 15 {
        return None;
    }
    let (mut v, mut seen, mut dot) = (0i64, 0, false);
    for &c in s {
        match c {
            b'.' if dot => return None,
            b'.' => dot = true,
            b'0'..=b'9' => {
                if dot {
                    if seen >= frac {
                        continue;
                    }
                    seen += 1;
                }
                v = v.checked_mul(10)?.checked_add((c - b'0') as i64)?;
            }
            _ => return None,
        }
    }
    // 15 digits scaled by 10^frac overflowed an i64 (found by fuzzing): too big for any field, refused
    for _ in seen..frac {
        v = v.checked_mul(10)?;
    }
    Some(v)
}

fn num(body: &[u8], idx: usize, frac: u32) -> Option<i64> {
    field(body, idx).and_then(|f| fixed(f, frac))
}

fn fixed_signed(s: &[u8], frac: u32) -> Option<i64> {
    match s {
        [b'-', rest @ ..] => fixed(rest, frac).map(|v| -v),
        _ => fixed(s, frac),
    }
}

/// ddmm.mmmm or dddmm.mmmm plus hemisphere -> degrees x 1e7.
fn coord(s: &[u8], h: &[u8], out: &mut i32) {
    let Some(v) = fixed(s, 5) else { return }; // ddmm.mmmmm x 1e5
    if h.is_empty() {
        return;
    }
    let (deg, min_e5) = (v / 10_000_000, v % 10_000_000);
    let e7 = deg * 10_000_000 + (min_e5 * 10 + 3) / 6; // minutes / 60, rounded
    // a number past i32 would have wrapped into some other position: refused
    let Ok(e7) = i32::try_from(e7) else { return };
    *out = if h[0] == b'S' || h[0] == b'W' { -e7 } else { e7 };
}

fn hexval(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

impl Default for Nmea {
    fn default() -> Self {
        Self::new()
    }
}

impl Nmea {
    pub const fn new() -> Self {
        Nmea {
            sentences: 0, checksum_errors: 0, overlong: 0, last_type: [0; 5], valid: false, quality: 0,
            sats_used: 0, sats_view: 0, gsv_view: [0; 6], sat: [Sat { prn: 0, snr: 0, talker: 0 }; 24], nsat: 0,
            hdop10: 0, alt_dm: 0, lat_e7: 0, lon_e7: 0, utc_hms: 0, date_dmy: 0, speed_kn10: 0, course10: 0,
            line: [0; 88], len: 0,
            in_sentence: false,
        }
    }

    /// A position fix: RMC valid or GGA quality above 0.
    pub fn has_fix(&self) -> bool {
        self.valid || self.quality > 0
    }

    /// True when a sentence with a good checksum was just parsed.
    pub fn feed(&mut self, c: u8) -> bool {
        if c == b'$' {
            self.in_sentence = true;
            self.len = 0;
            return false;
        }
        if !self.in_sentence {
            return false;
        }
        if c == b'\r' || c == b'\n' {
            self.in_sentence = false;
            let line = self.line;
            let mut s = &line[..self.len as usize];
            if let Some(nul) = s.iter().position(|&b| b == 0) {
                s = &s[..nul]; // a C string in the original: a NUL from the wire ends it
            }
            let ok = match s.iter().rposition(|&b| b == b'*') {
                Some(star) if star + 2 < s.len() => {
                    let sum = s[..star].iter().fold(0u8, |a, &b| a ^ b);
                    match (hexval(s[star + 1]), hexval(s[star + 2])) {
                        (Some(hi), Some(lo)) if sum == hi << 4 | lo => Some(star),
                        _ => None,
                    }
                }
                _ => None,
            };
            let Some(star) = ok else {
                self.checksum_errors += 1;
                return false;
            };
            self.sentences += 1;
            self.parse(&s[..star]);
            return true;
        }
        if self.len as usize >= self.line.len() - 1 {
            self.in_sentence = false;
            self.overlong += 1;
            return false;
        }
        self.line[self.len as usize] = c;
        self.len += 1;
        false
    }

    fn parse(&mut self, body: &[u8]) {
        let Some(addr) = field(body, 0).filter(|a| a.len() == 5) else { return };
        self.last_type.copy_from_slice(addr);
        match &addr[2..] {
            b"GGA" => {
                if let Some(v) = num(body, 1, 0) {
                    self.utc_hms = v as u32;
                }
                self.quality = num(body, 6, 0).unwrap_or(0) as u8;
                self.sats_used = num(body, 7, 0).unwrap_or(0) as u8;
                self.hdop10 = num(body, 8, 1).unwrap_or(0) as u16;
                if let Some(v) = field(body, 9).and_then(|f| fixed_signed(f, 1)) {
                    self.alt_dm = v as i32;
                }
                if self.quality != 0 {
                    if let (Some(f), Some(g)) = (field(body, 2), field(body, 3)) {
                        coord(f, g, &mut self.lat_e7);
                    }
                    if let (Some(f), Some(g)) = (field(body, 4), field(body, 5)) {
                        coord(f, g, &mut self.lon_e7);
                    }
                }
            }
            b"RMC" => {
                if let Some(v) = num(body, 1, 0) {
                    self.utc_hms = v as u32;
                }
                self.valid = field(body, 2) == Some(&b"A"[..]);
                if self.valid {
                    if let (Some(f), Some(g)) = (field(body, 3), field(body, 4)) {
                        coord(f, g, &mut self.lat_e7);
                    }
                    if let (Some(f), Some(g)) = (field(body, 5), field(body, 6)) {
                        coord(f, g, &mut self.lon_e7);
                    }
                    self.speed_kn10 = num(body, 7, 1).unwrap_or(0).clamp(0, u32::MAX as i64) as u32;
                    self.course10 = num(body, 8, 1).unwrap_or(0).clamp(0, 3599) as u16;
                }
                if let Some(v) = num(body, 9, 0) {
                    self.date_dmy = v as u32;
                }
            }
            b"GSV" => {
                let Some(slot) = TALKERS.iter().position(|t| addr[..2] == t[..]) else { return };
                // Each constellation reports its own total in view; the sum is what the receiver sees.
                if let Some(v) = num(body, 3, 0) {
                    self.gsv_view[slot] = v as u8;
                    let sum: u32 = self.gsv_view.iter().map(|&n| n as u32).sum();
                    self.sats_view = sum.min(255) as u8;
                }
                // satellite list: message 1 of a talker's group starts that talker afresh
                if num(body, 2, 0) == Some(1) {
                    let mut keep = 0;
                    for i in 0..self.nsat as usize {
                        if self.sat[i].talker as usize != slot {
                            self.sat[keep] = self.sat[i];
                            keep += 1;
                        }
                    }
                    self.nsat = keep as u8;
                }
                let nfields = 1 + body.iter().filter(|&&c| c == b',').count();
                let blocks = if nfields >= 4 { (nfields - 4) / 4 } else { 0 }; // 4.10 adds a trailing signal ID
                for k in 0..blocks.min(4) {
                    let prn = num(body, 4 + 4 * k, 0).unwrap_or(-1);
                    let snr = num(body, 7 + 4 * k, 0).unwrap_or(-1);
                    if prn <= 0 || prn > 255 || self.nsat as usize >= self.sat.len() {
                        continue;
                    }
                    self.sat[self.nsat as usize] = Sat { prn: prn as u8, snr: snr.clamp(0, 99) as u8, talker: slot as u8 };
                    self.nsat += 1;
                }
            }
            _ => {}
        }
    }
}

