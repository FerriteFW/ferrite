//! APRS mode (manual chapter 25, APRS-13 and APRS-15 to APRS-21): slot C taken for APRS. The slot's own VFO or
//! channel stays as it was in [`crate::Slot`]; while the mode is on, what goes to the Radio task for C is the
//! APRS frequency and settings instead ([`App::tuned_freq`], [`App::tuned_cfg`]), C is watched whatever Watch
//! says, and nothing on the radio can change C (APRS-19). Turning the mode off sends C's own state again.
//!
//! The APRS settings that aren't in [`crate::settings::Settings`] (the frequency, beacon mode and power) are
//! [`AprsCfg`], kept by the APRS store's owner ([`crate::data::Data::aprs_cfg`], [`crate::data::Op::AprsCfg`]);
//! the App holds a copy for the commands it sends, loaded at the first tick and written back when it changes.
//!
//! The message retry schedule (APRS-13) and the timed beacon are here too, for the APRS task and the
//! simulator's engine ([`crate::sample::Sample::aprs_tick`]) alike.

use crate::data::{Data, Fix, Pos};
use crate::screens::lists::{age_text, Txt};
use crate::slot::SlotCfg;
use crate::App;
use ferrite_core::freq::{mhz, needs_relay, Freq10};
use ferrite_core::msg::RadioCmd;
use ferrite_core::txplan::Region;
use ferrite_ui::text::Buf;
use ufmt::uwrite;

/// The slot APRS mode takes.
pub const APRS_SLOT: u8 = 2;

/// APRS-4: the heard list holds this many stations, in the APRS task's RAM only (never flash: it changes every
/// few minutes on a busy frequency). The oldest heard goes when a new one comes; power off empties it.
pub const HEARD_MAX: u16 = 50; // 20 until decision 246

/// The APRS settings kept with the APRS store (0 5 6 1 - 0 5 6 4, 0 5 9 1, 0 5 4, 0 5 9 5, 0 5 7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AprsCfg {
    /// The frequency in 5 kHz units; 0 = the region's (APRS-15).
    pub f5k: u16,
    /// Beacon mode: 0 off, 1 manual, then every [`BEACON_MINS`] minutes, and smart ([`SMART`]; 10-16 were the
    /// distance modes, read as smart since decision 272). Bit 7: off with the mode kept, set by the APRS list's "Beacon on/off" until
    /// 0.8.3 (decision 375 removed it); a saved one still reads as off.
    pub beacon: u8,
    /// Beacon and message power: 0 low (APRS-20's default), 1 mid, 2 high.
    pub power: u8,
    /// The path beacons and messages ask for (0 5 9 1): an index into [`PATHS`].
    pub path: u8,
    /// The digipeater (0 5 7, APRS-12, APRS-26): bit 0 on, bit 1 our callsign as an alias too, bits 4-6 the wait
    /// before relaying in seconds (0-5).
    pub digi: u8,
    /// A beacon after each voice transmission (0 5 6 2, APRS-27).
    pub after_voice: bool,
    /// The custom path (0 5 9 2, APRS-26) as typed, like `WIDE1-1,WIDE2-2`, zero-padded; used when
    /// [`AprsCfg::path`] is [`PATH_CUSTOM`]. Checked by [`path_ok`] before it is kept.
    pub custom: [u8; CUSTOM_LEN],
    /// Our SSID (0 5 5 1, APRS-30): 0-15, sent after the callsign; 0 sends the bare callsign.
    pub ssid: u8,
    /// FX.25 on transmit (0 5 9 5, APRS-45): check bytes, one of [`FX25_CHECKS`] (0 = off).
    pub fx25: u8,
    /// The message sound (0 7 5 5, APRS-6.1): an index into `tonegen::CHIMES` (0 off).
    pub sound: u8,
    /// Our map symbol (0 5 5 2, APRS-47): an index into [`SYMBOLS`].
    pub symbol: u8,
    /// TX delay (0 5 9 3, APRS-48): flags before our own frames, in 10 ms (10-100: 100-1000 ms).
    pub txdelay: u8,
    /// TX level (0 5 9 4, APRS-49): 0 Low, 1 Mid, 2 High (`audio::AFSK_AMPS`).
    pub tx_level: u8,
    /// The comment (0 5 5 3, APRS-50) sent at the end of every position report, zero-padded (empty: none).
    pub comment: [u8; COMMENT_LEN],
    /// Position ambiguity (0 5 5 6, APRS-51): digits dropped from both coordinates, 0 (exact) to 4.
    pub ambiguity: u8,
    /// Mic-E (0 5 5 7, APRS-52): 0 off (the uncompressed report), else the standard status sent, an index into
    /// [`MIC_E_NAMES`] (Off duty to Priority; Emergency isn't offered).
    pub mic_e: u8,
    /// APRS-54: telemetry (0 5 6 4): the battery, MCU temperature and beacon power after our position comment.
    pub telemetry: bool,
    /// APRS-50.1: the beacon text on every timed or smart beacon (0), or every 2nd, 4th or 8th (1-3: 1 << n);
    /// Beacon now and a position query's answer always carry it.
    pub text_every: u8,
    /// APRS-57: auto reply (0 5 9 9): bit 0 on; bits 4-5 the delay, an index into [`REPLY_DELAYS_S`].
    pub reply: u8,
    /// APRS-57: who gets the reply, as typed: `*` everyone, `VE7*` every callsign starting so, else one
    /// callsign-SSID exactly; zero-padded.
    pub reply_to: [u8; REPLY_TO_LEN],
    /// APRS-57: the reply, zero-padded (empty: none is sent).
    pub reply_text: [u8; REPLY_TEXT_LEN],
    /// APRS-13.6: tries after the first while no ack comes, 0 (off: sent once) to 4 ([`RETRIES_DEFAULT`]).
    pub retries: u8,
}

/// APRS-57.1: [`AprsCfg::reply_matches`]'s rule, on the pattern as typed.
pub fn reply_pattern(pat: &[u8], call: &[u8]) -> bool {
    let up = |c: &u8| c.to_ascii_uppercase();
    match pat.iter().position(|&c| c == b'*') {
        Some(n) => call.len() >= n && call[..n].iter().map(up).eq(pat[..n].iter().map(up)),
        None if pat.is_empty() => true,
        None => {
            // SSID 0 may be written or not: VE7ABC and VE7ABC-0 are the same station
            let bare = |c: &[u8]| if c.ends_with(b"-0") { c.len() - 2 } else { c.len() };
            call[..bare(call)].iter().map(up).eq(pat[..bare(pat)].iter().map(up))
        }
    }
}

/// APRS-50: the comment's room.
pub const COMMENT_LEN: usize = 40;
/// APRS-50.1: how often the beacon text goes (0 5 6 5).
pub const TEXT_EVERY: [&str; 4] = ["All", "1/2", "1/4", "1/8"];
/// APRS-57: auto reply's delays (0 5 9 9), as Kenwood's: 10 s by default.
pub const REPLY_DELAYS_S: [u16; 4] = [10, 30, 60, 120];
pub const REPLY_DELAY_NAMES: [&str; 4] = ["10 s", "30 s", "1 min", "2 min"];
/// APRS-57: the pattern's room (a callsign-SSID: 9) and the reply's (45, as Kenwood's).
pub const REPLY_TO_LEN: usize = 9;
pub const REPLY_TEXT_LEN: usize = 45;
/// APRS-57: everyone, until set.
pub const REPLY_TO_DEFAULT: [u8; REPLY_TO_LEN] = *b"*\0\0\0\0\0\0\0\0";
/// APRS-57.2: one reply to a station in this long, whatever it sends (two radios replying to each other stop at
/// one reply each).
pub const REPLY_HOLD_S: u32 = 30 * 60;
/// APRS-51: 0 5 5 6's names, by the digits dropped.
pub const AMBIGUITY_NAMES: [&str; 5] = ["Exact", "1 digit", "2 digits", "3 digits", "4 digits"];
/// APRS-52: 0 5 5 7's names; the status's message bits are 8 minus its place (Off duty 111 to Priority 001).
pub const MIC_E_NAMES: [&str; 8] = ["Off", "Off duty", "En route", "In service", "Returning", "Committed", "Special", "Priority"];

/// APRS-47: the symbols 0 5 5 2 offers, as (name, table, code) in the primary table. Person by default, what the
/// beacons always sent (a handheld's SSID 7).
pub const SYMBOLS: [(&str, u8, u8); 10] = [
    ("Person", b'/', b'['),
    ("Car", b'/', b'>'),
    ("Jeep", b'/', b'j'),
    ("Truck", b'/', b'k'),
    ("Van", b'/', b'v'),
    ("RV", b'/', b'R'),
    ("Motorcycle", b'/', b'<'),
    ("Bicycle", b'/', b'b'),
    ("House", b'/', b'-'),
    ("Boat", b'/', b's'),
];
/// APRS-48: 0 5 9 3's range and default, in 10 ms.
pub const TXDELAY_MIN: u8 = 10;
pub const TXDELAY_MAX: u8 = 100;
pub const TXDELAY_DEFAULT: u8 = 30;
/// APRS-49: 0 5 9 4's names.
pub const TX_LEVELS: [&str; 3] = ["Low", "Mid", "High"];

impl AprsCfg {
    /// APRS-47: the symbol's table and code.
    pub fn symbol_chars(&self) -> (u8, u8) {
        let (_, t, c) = SYMBOLS[self.symbol as usize % SYMBOLS.len()];
        (t, c)
    }

    /// APRS-48: the TX delay in 10 ms, in range.
    pub fn txdelay_10ms(&self) -> u8 {
        self.txdelay.clamp(TXDELAY_MIN, TXDELAY_MAX)
    }
}

/// APRS-45: 0 5 9 5's choices (check bytes; 0 = off) and their names. 16 by default: Alex, decision 230 (a default
/// to review before the first release).
pub const FX25_CHECKS: [u8; 4] = [0, 16, 32, 64];
pub const FX25_NAMES: [&str; 4] = ["Off", "16 bytes", "32 bytes", "64 bytes"];
pub const FX25_DEFAULT: u8 = 16;

/// APRS-30: the SSID by default (decision 191: a handheld), and what each number means by aprs.org's convention
/// (aprs.org/aprs11/SSIDs.txt), shortened to fit the row.
pub const SSID_DEFAULT: u8 = 7;
pub const SSID_NAMES: [&str; 16] = [
    "Primary", "Generic", "Generic", "Generic", "Generic", "Phone/net", "Special", "Handheld", "Boat/RV",
    "Mobile", "I-Gate/net", "Balloon/air", "Tracker", "Weather", "Truck", "Generic",
];

/// The custom path's room: two entries of up to 9 characters would be 19, so two short ones (WIDE1-1,WIDE2-2).
pub const CUSTOM_LEN: usize = 16;

/// The path entries a frame asks for: up to two (APRS-26).
#[derive(Clone, Copy, Debug)]
pub struct PathNames<'a> {
    names: [&'a [u8]; 2],
    n: usize,
}

impl<'a> core::ops::Deref for PathNames<'a> {
    type Target = [&'a [u8]];
    fn deref(&self) -> &Self::Target {
        &self.names[..self.n]
    }
}

impl AprsCfg {
    pub const DEFAULT: AprsCfg = AprsCfg { f5k: 0, beacon: 0, power: 0, path: PATH_DEFAULT, digi: 0, after_voice: false, custom: [0; CUSTOM_LEN], ssid: SSID_DEFAULT, fx25: FX25_DEFAULT, sound: ferrite_core::tonegen::CHIME_DEFAULT, symbol: 0, txdelay: TXDELAY_DEFAULT, tx_level: 1, comment: [0; COMMENT_LEN], ambiguity: 0, mic_e: 0, telemetry: false, text_every: 0, reply: 0, reply_to: REPLY_TO_DEFAULT, reply_text: [0; REPLY_TEXT_LEN], retries: RETRIES_DEFAULT };

    /// APRS-13.6: how often a message is tried in all: the first, then the retries (0 5 2's options).
    pub fn tries(&self) -> u8 {
        self.retries.min(RETRIES_DEFAULT) + 1
    }

    /// The path's entries: one of [`PATHS`], or the custom path split at its comma.
    pub fn path(&self) -> PathNames<'_> {
        let mut p = PathNames { names: [b"", b""], n: 0 };
        if self.path == PATH_CUSTOM {
            for e in self.custom_text().split(|&c| c == b',').filter(|e| !e.is_empty()).take(2) {
                p.names[p.n] = e;
                p.n += 1;
            }
        } else {
            for &e in PATHS[self.path as usize % PATHS.len()].1 {
                p.names[p.n] = e;
                p.n += 1;
            }
        }
        p
    }

    /// The custom path as typed (empty: none set).
    pub fn custom_text(&self) -> &[u8] {
        let n = self.custom.iter().position(|&c| c == 0).unwrap_or(CUSTOM_LEN);
        &self.custom[..n]
    }

    /// The comment as typed (APRS-50; empty: none).
    pub fn comment_text(&self) -> &[u8] {
        let n = self.comment.iter().position(|&c| c == 0).unwrap_or(COMMENT_LEN);
        &self.comment[..n]
    }

    /// APRS-57: auto reply is on.
    pub fn reply_on(&self) -> bool {
        self.reply & 1 != 0
    }

    /// APRS-57: the wait before the reply, in seconds.
    pub fn reply_delay_s(&self) -> u16 {
        REPLY_DELAYS_S[(self.reply >> 4) as usize % REPLY_DELAYS_S.len()]
    }

    /// APRS-57: the pattern as typed (empty reads as everyone).
    pub fn reply_to_text(&self) -> &[u8] {
        let n = self.reply_to.iter().position(|&c| c == 0).unwrap_or(REPLY_TO_LEN);
        &self.reply_to[..n]
    }

    /// APRS-57: the reply as typed.
    pub fn reply_text(&self) -> &[u8] {
        let n = self.reply_text.iter().position(|&c| c == 0).unwrap_or(REPLY_TEXT_LEN);
        &self.reply_text[..n]
    }

    /// APRS-57.1: a station the pattern takes: `*` (or none) everyone; text then `*` every callsign starting so;
    /// else the callsign-SSID exactly ("VE7ABC" is SSID 0 only; "VE7ABC*" every SSID). Case doesn't matter.
    pub fn reply_matches(&self, call: &[u8]) -> bool {
        reply_pattern(self.reply_to_text(), call)
    }

    /// A stored path value 0 5 9 1 can hold: one of the [`PATHS`], and Custom only once one is set.
    pub fn path_ok(&self, v: u8) -> bool {
        (v as usize) < PATHS.len() && (v != PATH_CUSTOM || !self.custom_text().is_empty())
    }

    /// 0 5 9 1 stepped `d` places through the values it can hold, round the end.
    pub fn path_step(&self, d: i32) -> u8 {
        let n = PATHS.len() as i32;
        let mut v = self.path as i32;
        for _ in 0..d.unsigned_abs() {
            for _ in 0..n {
                v = (v + d.signum()).rem_euclid(n);
                if self.path_ok(v as u8) {
                    break;
                }
            }
        }
        v as u8
    }

    /// 0 5 9 1's name for the path in use.
    pub fn path_name(&self) -> &[u8] {
        if self.path == PATH_CUSTOM { self.custom_text() } else { PATHS[self.path as usize % PATHS.len()].0.as_bytes() }
    }

    /// APRS-45: FX.25 check bytes for a frame the APRS task sends (`what`, as [`crate::aprs_live::Tick`] names
    /// it): 0 5 9 5's for our own frames (beacons, messages, acks, answers), 0 (plain AX.25) for a digipeat, which
    /// goes out as it came.
    pub fn fx25_for(&self, what: &str) -> u8 {
        if what == "Digipeat" { 0 } else { self.fx25 }
    }

    /// 0 5 9 5's choice: an index into [`FX25_CHECKS`].
    pub fn fx25_index(&self) -> u8 {
        FX25_CHECKS.iter().position(|&c| c == self.fx25).unwrap_or(0) as u8
    }

    /// The digipeater is on (0 5 7 1).
    pub fn digi_on(&self) -> bool {
        self.digi & DIGI_ON != 0
    }

    /// The digipeater also relays frames that name our callsign (0 5 7 2).
    pub fn digi_mycall(&self) -> bool {
        self.digi & DIGI_MYCALL != 0
    }

    /// Seconds the digipeater waits before relaying (0 5 7 3).
    pub fn digi_wait_s(&self) -> u8 {
        (self.digi >> 4 & 7).min(DIGI_WAIT_MAX)
    }

    /// The beacon mode, 0 when off.
    pub fn mode(&self) -> u8 {
        if self.beacon & BEACON_PAUSED != 0 { 0 } else { self.beacon & !BEACON_PAUSED }
    }

    /// Minutes between timed beacons; None in the other modes.
    pub fn every_min(&self) -> Option<u8> {
        BEACON_MINS.get((self.mode() as usize).wrapping_sub(TIMED)).copied()
    }

    /// Smart beaconing (APRS-24), and a distance mode saved before decision 272.
    pub fn smart(&self) -> bool {
        (DROPPED..=SMART).contains(&(self.mode() as usize))
    }

    /// Automatic beacons (timed or smart; not manual or off): APRS-29's beacons alone, and APRS-33's
    /// query answers.
    pub fn beacons_on(&self) -> bool {
        self.mode() as usize >= TIMED
    }
}

/// The paths offered (0 5 9 1, APRS-26), indexed by the stored value: name and entries. The custom path (0 5 9 2)
/// keeps the value it has always had, [`PATH_CUSTOM`] (3), so a radio set to it before APRSAT and ARISS were added
/// still is; its row here is a placeholder (its name and entries are the typed path's).
pub const PATHS: [(&str, &[&[u8]]); 6] = [
    ("None", &[]),
    ("WIDE1-1", &[b"WIDE1-1"]),
    ("WIDE1-1,WIDE2-1", &[b"WIDE1-1", b"WIDE2-1"]),
    ("", &[]),
    // APRS-26.1 (decision 292): the satellite and ISS digipeaters' aliases
    ("APRSAT", &[b"APRSAT"]),
    ("ARISS", &[b"ARISS"]),
];
/// 0 5 9 1 on the custom path (0 5 9 2).
pub const PATH_CUSTOM: u8 = 3;

/// A custom path (0 5 9 2, APRS-26) is one or two entries split by a comma, each a callsign or alias of 1-6
/// capitals and digits with an optional -SSID of 1-15 (WIDE1-1, WIDE2-2, a club's call), and fits
/// [`CUSTOM_LEN`].
pub fn path_ok(t: &[u8]) -> bool {
    let entry = |e: &[u8]| {
        let (call, ssid) = match e.iter().position(|&c| c == b'-') {
            Some(i) => (&e[..i], Some(&e[i + 1..])),
            None => (e, None),
        };
        let call_ok = (1..=6).contains(&call.len()) && call.iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        let ssid_ok = ssid.is_none_or(|s| {
            !s.is_empty() && s.len() <= 2 && s.iter().all(u8::is_ascii_digit) && s[0] != b'0' && {
                let v = s.iter().fold(0u32, |a, &c| a * 10 + (c - b'0') as u32);
                (1..=15).contains(&v)
            }
        });
        call_ok && ssid_ok
    };
    !t.is_empty() && t.len() <= CUSTOM_LEN && t.split(|&c| c == b',').count() <= 2 && t.split(|&c| c == b',').all(entry)
}
/// WIDE1-1,WIDE2-1, the usual path and the default.
pub const PATH_DEFAULT: u8 = 2;
/// [`AprsCfg::digi`]'s bits.
pub const DIGI_ON: u8 = 1;
pub const DIGI_MYCALL: u8 = 2;
pub const DIGI_WAIT_MAX: u8 = 5;
/// 0 5 7 2's values.
pub const DIGI_ALIASES: [&str; 2] = ["WIDE1-1", "WIDE1-1, my call"];

/// Bit 7 of [`AprsCfg::beacon`].
const BEACON_PAUSED: u8 = 0x80;
/// Timed beacons (0 5 6 1). APRS-9's 30 s floor is well under the shortest.
pub const BEACON_MINS: [u8; 8] = [1, 2, 5, 10, 15, 20, 30, 60];
const TIMED: usize = 2;
/// 10-16 held the distance modes until decision 272 (dropped: Smart covers moving); a saved one is smart now.
const DROPPED: usize = TIMED + BEACON_MINS.len();
/// The smart mode's number, kept where it was saved.
pub const SMART: usize = DROPPED + 7;
/// The highest saved mode number + 1 (the link's check).
pub const BEACON_MODES: usize = SMART + 1;
/// 0 5 6 1's steps: off, manual, the timed ones, smart.
pub const BEACON_STEPS: usize = DROPPED + 1;
pub const POWERS: [&str; 3] = ["Low", "Mid", "High"];

/// A message is tried at most this often (APRS-13; fewer with fewer retries, APRS-13.6), the waits between tries,
/// and after the last try how long an ack may still come before it is marked failed.
pub const TRIES: u8 = 5;
/// APRS-13.6: the retries after the first try, by default (five tries in all, as before the setting).
pub const RETRIES_DEFAULT: u8 = TRIES - 1;
pub const RETRY_S: [u16; 4] = [30, 60, 120, 240];
pub const LAST_WAIT_S: u16 = 240;

/// The longest APRS information field taken or sent (decision 86): AX.25's 256 bytes, nothing valid cut. The
/// buffers that hold a frame are already this size: `uafsk`'s HDLC frame (330 bytes: 10 addresses, control,
/// PID, 256 bytes of information, the FCS) and `uaprs`'s KISS decoder (330), so it costs no RAM; a longer frame
/// is dropped as invalid.
pub const INFO_MAX: usize = 256;

/// After try `n` (1-5): seconds until the next try, or (after the fifth) until it is marked failed.
/// APRS-13.6: the wait after try `n` of `tries`: the schedule's, or [`LAST_WAIT_S`] after the last.
pub fn retry_wait_of(n: u8, tries: u8) -> u16 {
    if n >= tries { LAST_WAIT_S } else { retry_wait(n) }
}

pub fn retry_wait(n: u8) -> u16 {
    RETRY_S.get((n as usize).wrapping_sub(1)).copied().unwrap_or(LAST_WAIT_S)
}

/// The last automatic beacon, for the next one's decision: when, and the heading and position then.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LastBeacon {
    pub at_s: u32,
    pub heading: u16,
    pub pos: Option<Pos>,
}

impl LastBeacon {
    pub fn new(at_s: u32, fix: Option<Fix>) -> LastBeacon {
        LastBeacon { at_s, heading: fix.map_or(0, |f| f.heading), pos: fix.map(|f| f.pos) }
    }
}

/// An automatic beacon is due (APRS-24), the first as soon as the mode allows:
/// - timed: `every` minutes since the last;
/// - smart: [`uaprs::beacon`]'s SmartBeaconing on one curve from walking to driving (decision 271): 10 km/h and
///   faster every 3 min, under 3 km/h every 30 min, between them every 500 m, a turn of 28 degrees plus
///   255 / speed after 30 s; without a fix, the slow rate.
pub fn beacon_due(c: &AprsCfg, last: Option<LastBeacon>, now_s: u32, fix: Option<Fix>) -> bool {
    if let Some(m) = c.every_min() {
        return last.is_none_or(|l| now_s.wrapping_sub(l.at_s) >= m as u32 * 60);
    }
    if c.smart() {
        let mut b = uaprs::beacon::Beaconer::new(uaprs::beacon::SmartBeacon::default());
        if let Some(l) = last {
            b.sent(l.at_s, l.heading);
        }
        let (speed, heading) = fix.map_or((0, 0), |f| (f.speed_kmh, f.heading));
        return b.due(now_s, speed, heading);
    }
    false
}

/// The region's APRS frequency (APRS-15): US, Canada and Other 144.390, UK and Europe 144.800, Australia
/// 145.175, New Zealand 144.575, Japan 144.640 MHz. No region chosen yet: 144.390.
pub fn region_freq(r: Option<Region>) -> Freq10 {
    let k = match r {
        Some(Region::Uk | Region::Europe) => 800,
        Some(Region::Australia) => 1175,
        Some(Region::NewZealand) => 575,
        Some(Region::Japan) => 640,
        _ => 390,
    };
    mhz(144) + k * 100
}

/// The frequency's range on 0 5 4: chip 0's (APRS-21).
pub(crate) const F_MIN: Freq10 = mhz(64);
pub(crate) const F_MAX: Freq10 = mhz(300);

const LOADED: u8 = 1;
const DIRTY: u8 = 2;
/// A voice transmission ended (APRS-27); the next tick asks for the beacon.
const AFTER_VOICE: u8 = 4;
/// APRS-29.2: the beacon mode Tracker starts with when 0 5 6 1 is off or manual: every 10 min.
pub const TRACKER_BEACON: u8 = 5;

/// What held C at the last tick, [`Holder::bits`] (KISS-9, APRS-29: KISS or beacons turning on or off move it, from
/// any path that sets them).
const HELD: u8 = 0x70;

/// What holds slot C as the APRS slot (APRS-19, KISS-9, APRS-29), in this precedence when more than one does:
/// APRS mode, then KISS, then beacons. Only the first two give C receive priority on its chip (APRS-21.1,
/// decision 181); beacons alone lock C and send on it, nothing more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holder {
    Mode,
    Kiss,
    Beacon,
    /// Pager mode (DEC-20, APRS-29.3): C listens for POCSAG pages on 0 4 2's frequency.
    Pager,
}

impl Holder {
    /// As toasts and the lock's reason say it.
    pub fn name(self) -> &'static str {
        match self {
            Holder::Mode => "APRS mode",
            Holder::Kiss => "KISS",
            Holder::Beacon => "Tracker",
            Holder::Pager => "Pager",
        }
    }

    /// The readback's word (`holder=`).
    pub fn word(self) -> &'static str {
        match self {
            Holder::Mode => "mode",
            Holder::Kiss => "kiss",
            Holder::Beacon => "beacon", // Tracker (APRS-29.2); the word Console's Live View reads, kept
            Holder::Pager => "pager", // DEC-20.2
        }
    }

    fn bits(h: Option<Holder>) -> u8 {
        (match h {
            None => 0,
            Some(Holder::Mode) => 1,
            Some(Holder::Kiss) => 2,
            Some(Holder::Beacon) => 3,
            Some(Holder::Pager) => 4,
        }) << 4
    }
}

impl App {
    /// APRS mode is on (0 5 3).
    pub fn aprs_on(&self) -> bool {
        // PWR-24: no APRS in Low Power Mode. KISS-10: KISS on suspends it (the app does APRS; the setting is kept).
        // SAT-14: and it pauses while the satellite screen is open.
        self.set.aprs_mode && !self.low_power() && !self.set.kiss_on() && !self.sat_open()
    }

    /// APRS-22: without APRS mode, the active slot is on a channel flagged APRS, so what it receives is decoded
    /// (into the heard list and messages; nothing is sent: transmitting is APRS mode's).
    pub fn aprs_flag_decode(&self) -> bool {
        let s = self.slot(self.active());
        !self.set.aprs_mode && !self.low_power() && !s.vfo_mode && s.ch.is_some_and(|c| c.flags & crate::chan::FLAG_APRS != 0)
    }

    /// Automatic beacons are on (0 5 6 1 timed or smart; APRS-24), and only with 0 5 3 on On or Tracker: 0 5 3 is
    /// the master switch, so beacons never hold C or go out with it Off (APRS-29.1, decision 439; Alex: "if aprs is
    /// off dont beacon", ferrite#101). Manual ("Beacon now" only) doesn't count.
    pub fn beacons_on(&self) -> bool {
        (self.set.aprs_mode || self.set.tracker) && self.aprs.beacons_on() && !self.low_power() && !self.set.kiss_on() // KISS-10
    }

    /// KISS-10: why the radio won't send APRS of its own now (KISS on: the app sends), if so.
    pub fn kiss_holds_aprs(&self) -> Option<&'static str> {
        self.set.kiss_on().then_some("KISS on: the app sends")
    }

    /// What holds slot C as the APRS slot, if anything: APRS mode (APRS-15), KISS (KISS-9: its frames go out
    /// there) or automatic beacons (APRS-29), in that precedence.
    pub fn aprs_holder(&self) -> Option<Holder> {
        if self.low_power() {
            None // PWR-24: Low Power Mode holds nothing on C
        } else if self.set.kiss_on() {
            Some(Holder::Kiss) // KISS-10: first, APRS mode and beacons wait while it is on
        } else if self.set.aprs_mode {
            Some(Holder::Mode)
        } else if self.set.pocsag_on {
            Some(Holder::Pager) // DEC-20: 0 5 3's Pager
        } else if self.set.tracker && self.beacons_on() {
            Some(Holder::Beacon) // APRS-29.2: Tracker
        } else {
            None
        }
    }

    /// GPS-1.5 (a): APRS wants positions: APRS mode on, or Tracker holding C for its beacons (APRS-29.2; Tracker
    /// leaves `aprs_mode` off, so `aprs_on` alone missed it and GPS Auto never woke for a Tracker beacon).
    pub fn aprs_positions(&self) -> bool {
        self.aprs_on() || matches!(self.aprs_holder(), Some(Holder::Beacon))
    }

    /// TX-24.2: another transmission is under way (a person's PTT, or a data key-up: an APRS frame, an SSTV
    /// picture), so the APRS task waits before keying for its own.
    pub fn tx_busy(&self) -> bool {
        self.radio.keyed.is_some() || self.radio.data_keyed
    }

    /// Slot C is the APRS slot: APRS mode, KISS or beacons hold it (APRS-15, KISS-9, APRS-29).
    pub fn aprs_c(&self) -> bool {
        self.aprs_holder().is_some()
    }

    /// What the APRS task may send now (APRS-22, APRS-29, KISS-10): everything with APRS mode, beacons alone, or
    /// nothing; [`crate::aprs_live::Sending::may_send`] checks a waiting frame against it again (APRS-10.1).
    pub fn aprs_sending(&self) -> crate::aprs_live::Sending {
        use crate::aprs_live::Sending;
        if self.aprs_on() {
            Sending::All
        } else if self.beacons_on() {
            Sending::Beacons
        } else {
            Sending::Nothing
        }
    }

    /// C has receive priority on its chip (APRS-21.1, decision 181): APRS mode or KISS hold it. Beacons alone
    /// don't: C is then an ordinary watched slot.
    pub fn aprs_prio(&self) -> bool {
        self.aprs_on() || self.kiss_on() || self.pager_on()
    }

    /// Pager mode holds C (DEC-20): 0 5 3 on Pager, not in Low Power Mode (PWR-24).
    pub fn pager_on(&self) -> bool {
        self.aprs_holder() == Some(Holder::Pager)
    }

    /// Slot `s` is the APRS slot now.
    pub fn aprs_slot(&self, s: u8) -> bool {
        s == APRS_SLOT && self.aprs_c()
    }

    /// APRS-21.2: slot `s` (A or B) receives on the APRS frequency while C is the APRS slot, so it defers to C: no
    /// turns of its own (the Radio task's watch), its card "APRS on C" until C receives.
    pub fn defers_to_c(&self, s: u8) -> bool {
        self.aprs_c() && s < APRS_SLOT && self.tuned_freq(s) == self.c_freq()
    }

    /// APRS-21.3: slot `s` (A or B) transmits on the APRS frequency while C is the APRS slot: voice there needs a
    /// person's PTT twice; VOX and remote PTT never key it.
    pub fn aprs_voice_gate(&self, s: u8) -> bool {
        self.aprs_c() && !self.pager_on() && s < APRS_SLOT && crate::screens::transmit::tx_freq(self, s) == self.aprs_freq()
    }

    /// Why the APRS slot can't change (APRS-19, KISS-9, APRS-29): what holds it, nothing more (decision 181).
    pub(crate) fn aprs_holds(&self) -> &'static str {
        match self.aprs_holder() {
            Some(Holder::Kiss) => "Slot C held by KISS",
            Some(Holder::Beacon) => "Slot C held by Tracker",
            Some(Holder::Pager) => "Slot C held by Pager",
            _ => "Slot C held by APRS mode",
        }
    }

    /// The settings were loaded and C sent as they say: no toast for what was already so (KISS-9).
    pub(crate) fn c_settled(&mut self) {
        self.aprs_st = self.aprs_st & !HELD | Holder::bits(self.aprs_holder());
    }

    /// The APRS-slot commands for the Radio task: C kept awake with receive priority (APRS mode, KISS:
    /// APRS-21.1), and C as the APRS slot (any holder: its squelch's open delay 0, SQL-17).
    pub(crate) fn send_aprs_slot(&mut self) {
        self.out.push(RadioCmd::Awake((self.aprs_prio() as u8) << APRS_SLOT));
        self.out.push(RadioCmd::AprsSlot((self.aprs_c() as u8) << APRS_SLOT));
    }

    /// C becomes the APRS slot or goes back, or what holds it changes (APRS-15, APRS-16, KISS-9, APRS-29): it
    /// stops scanning, retunes, and is watched while it is, kept awake with priority while APRS mode or KISS hold
    /// it.
    pub(crate) fn c_follow(&mut self) {
        self.c_settled();
        if self.aprs_c() && self.off & 1 << APRS_SLOT != 0 {
            self.off &= !(1 << APRS_SLOT); // HOME-15.1: the APRS slot is never off
            self.out.push(RadioCmd::SlotOff(self.off));
        }
        if self.scan_mask & 1 << APRS_SLOT != 0 {
            self.scan_mask &= !(1 << APRS_SLOT);
            self.out.push(RadioCmd::Scan { slot: APRS_SLOT, on: false }); // C stops scanning to listen to APRS
        }
        self.send_slot(APRS_SLOT);
        self.out.push(RadioCmd::Watch(self.watch_mask()));
        self.send_aprs_slot();
    }

    pub fn aprs_cfg(&self) -> &AprsCfg {
        &self.aprs
    }

    /// The APRS frequency: 0 5 4, or the region's.
    /// APRS-30: our APRS address: the callsign without any SSID typed into it, then 0 5 5 1's (none for 0).
    /// Empty without a callsign.
    pub fn aprs_call(&self) -> Buf<10> {
        let c = self.callsign();
        let base = &c[..c.iter().position(|&b| b == b'-').unwrap_or(c.len())];
        let mut b = Buf::from(base);
        if !base.is_empty() && self.aprs.ssid != 0 {
            let _ = ufmt::uwrite!(b, "-{}", self.aprs.ssid);
        }
        b
    }

    pub fn aprs_freq(&self) -> Freq10 {
        match self.aprs.f5k {
            0 => region_freq(self.set.region()),
            k => k as Freq10 * 500,
        }
    }

    /// What C is held on: 0 4 2's frequency in Pager mode (DEC-20), else the APRS frequency.
    pub fn c_freq(&self) -> Freq10 {
        if self.pager_on() { self.set.pocsag_freq } else { self.aprs_freq() }
    }

    /// What the slot is tuned to: its own frequency, or C's held one on the APRS slot.
    pub fn tuned_freq(&self, s: u8) -> Freq10 {
        if self.aprs_slot(s) { self.c_freq() } else { self.slot(s).freq() }
    }

    /// The slot's settings as the radio has them: on the APRS slot FM, no tones, simplex, busy lockout by
    /// carrier (APRS-10) and the beacon power (APRS-20); the squelch is C's own.
    pub fn tuned_cfg(&self, s: u8) -> SlotCfg {
        let own = *self.slot(s).cfg();
        if !self.aprs_slot(s) {
            return own;
        }
        SlotCfg { mode: 0, narrow: false, power: self.aprs.power % 3, busy: 1, squelch: own.squelch, ..SlotCfg::default() }
    }

    /// APRS-2, APRS-15.2: C's quick mute follows 0 5 9 6 while APRS mode is on, and is off without it.
    pub(crate) fn aprs_mute_sync(&mut self) {
        let on = self.set.aprs_mode && self.set.aprs_mute;
        let m = if on { self.mute | 1 << APRS_SLOT } else { self.mute & !(1 << APRS_SLOT) };
        if m != self.mute {
            self.mute = m;
            self.send_mute();
        }
    }

    /// APRS mode on or off (0 5 3, APRS-15, APRS-16): C tunes to APRS and is watched, or goes back as it was.
    pub(crate) fn set_aprs_mode(&mut self, on: bool) {
        self.set.aprs_mode = on;
        self.c_follow();
        // APRS-15.2: C starts muted in APRS mode (packets decoded, not heard; \* unmutes) and is heard again after
        self.aprs_mute_sync();
        if on {
            let mut f = Buf::<20>::new();
            let _ = uwrite!(f, "C on {}", Txt(crate::slot::mhz3(self.aprs_freq()).as_bytes()));
            if self.set.aprs_mute {
                f.push(b", muted");
            }
            self.show_toast(b"APRS mode on", f.as_bytes(), false);
        } else if let Some(h) = self.aprs_holder() {
            let mut b = Buf::<24>::new();
            let _ = uwrite!(b, "C held by {}", h.name());
            self.show_toast(b"APRS mode off", b.as_bytes(), false); // KISS-9, APRS-29
        } else {
            self.show_toast(b"APRS mode off", b"slot C restored", false);
        }
    }

    /// 0 5 3 set (APRS-29.2): Off, Tracker, On (APRS mode) or KISS, one at a time. Tracker with beacons off or manual
    /// starts every 10 min (a tracker beacons); KISS uses 0 5 8's port. C follows, with a toast saying which.
    pub(crate) fn set_aprs_select(&mut self, v: u8) {
        let v = v % crate::settings::APRS_MODES.len() as u8;
        let was = self.set.aprs_select();
        if v != 4 {
            self.pager_was = None;
        }
        self.set.pocsag_on = v == 4; // DEC-20: Pager, the same switch as 0 4 2 2
        // Leaving Tracker: the beacon mode it set itself goes back to what it was (off or manual).
        if v != 1 {
            if let Some(was) = self.tracker_was.take() {
                if self.aprs.beacon == TRACKER_BEACON {
                    self.edit_aprs(was as i32, |c, v| c.beacon = v as u8);
                }
            }
        }
        let port = self.set.kiss_port_kept();
        self.set.kiss_via = port;
        self.set.tracker = v == 1;
        self.set.kiss_port = if v == 3 { port } else { 0 };
        if v == 1 && !self.aprs.beacons_on() {
            self.tracker_was = Some(self.aprs.beacon);
            self.edit_aprs(0, |c, _| c.beacon = TRACKER_BEACON);
        }
        self.set_aprs_mode(v == 2);
        match v {
            1 => {
                let mut b = Buf::<24>::new();
                self.beacon_mode_text(&mut b);
                self.show_toast(b"Tracker on", b.as_bytes(), false);
            }
            3 => self.show_toast(b"KISS on", crate::settings::KISS_VIA[(port as usize).saturating_sub(1) % 2].as_bytes(), false),
            0 if was == 4 => self.show_toast(b"Pager off", b"slot C restored", false),
            4 => {
                // DEC-20: what Pager took C from
                let mut b = Buf::<28>::new();
                let _ = uwrite!(b, "C on {}", Txt(crate::slot::mhz3(self.set.pocsag_freq).as_bytes()));
                if was != 0 && was != 4 {
                    let _ = uwrite!(b, ", {} off", crate::settings::APRS_WAS[was as usize]);
                }
                self.show_toast(b"Pager on", b.as_bytes(), false);
            }
            _ => {}
        }
    }

    /// 0 4 2 2, Decode (DEC-20): Pager on, remembering the APRS mode it replaces; off puts that back.
    pub(crate) fn set_pager(&mut self, on: bool) {
        if on == self.set.pocsag_on {
            return;
        }
        if on {
            let was = self.set.aprs_select();
            self.set_aprs_select(4);
            self.pager_was = Some(was);
        } else {
            let back = self.pager_was.take().unwrap_or(0);
            self.set_aprs_select(back);
        }
    }

    /// 0 4 2's frequency changed: C retunes when it is the pager slot (DEC-20).
    pub(crate) fn pager_retune(&mut self) {
        if self.pager_on() {
            self.send_slot(APRS_SLOT);
        }
    }

    /// New APRS settings: kept, written back at the next tick, and C retuned when it is the APRS slot.
    pub(crate) fn set_aprs(&mut self, c: AprsCfg) {
        self.aprs = c;
        self.aprs_st |= DIRTY;
        if self.aprs_c() {
            self.send_slot(APRS_SLOT);
        }
    }

    /// One APRS setting changed in place, then kept and sent as [`App::set_aprs`] does: the menu's many arms
    /// pass a plain `fn` each, so none of them copies the whole block twice (size review, Nov 23, 2026).
    #[inline(never)]
    pub(crate) fn edit_aprs(&mut self, d: i32, f: fn(&mut AprsCfg, i32)) {
        f(&mut self.aprs, d);
        self.aprs_st |= DIRTY;
        if self.aprs_c() {
            self.send_slot(APRS_SLOT);
        }
    }

    /// The custom path as typed (0 5 9 2).
    pub fn aprs_custom_path(&self) -> &[u8] {
        self.aprs.custom_text()
    }

    /// A custom path typed at 0 5 9 2, already checked by [`path_ok`]: kept and chosen (APRS-26).
    pub(crate) fn set_aprs_custom_path(&mut self, t: &[u8]) {
        let mut c = self.aprs;
        c.custom = [0; CUSTOM_LEN];
        c.custom[..t.len().min(CUSTOM_LEN)].copy_from_slice(&t[..t.len().min(CUSTOM_LEN)]);
        c.path = PATH_CUSTOM;
        self.set_aprs(c);
    }

    /// The comment being typed (APRS-50): kept as it changes, written back at the next tick (the store waits
    /// before saving); C isn't retuned for it.
    pub(crate) fn set_aprs_comment(&mut self, t: &[u8]) {
        let n = t.len().min(COMMENT_LEN);
        self.aprs.comment = [0; COMMENT_LEN];
        self.aprs.comment[..n].copy_from_slice(&t[..n]);
        self.aprs_st |= DIRTY;
    }

    /// APRS-57: the reply or its pattern being typed, kept as it changes (as the comment, APRS-50).
    pub(crate) fn set_aprs_reply_text(&mut self, t: &[u8]) {
        let n = t.len().min(REPLY_TEXT_LEN);
        self.aprs.reply_text = [0; REPLY_TEXT_LEN];
        self.aprs.reply_text[..n].copy_from_slice(&t[..n]);
        self.aprs_st |= DIRTY;
    }

    pub(crate) fn set_aprs_reply_to(&mut self, t: &[u8]) {
        let n = t.len().min(REPLY_TO_LEN);
        self.aprs.reply_to = [0; REPLY_TO_LEN];
        self.aprs.reply_to[..n].copy_from_slice(&t[..n]);
        self.aprs_st |= DIRTY;
    }

    /// 0 5 4 by the knob: 5 kHz steps from where it is, within chip 0's range.
    pub(crate) fn step_aprs_freq(&mut self, d: i32) {
        let f = (self.aprs_freq() as i64 + d as i64 * 500).clamp(F_MIN as i64, F_MAX as i64) as Freq10;
        let _ = self.set_aprs_freq(f); // clamped and on the grid already
    }

    /// 0 5 4 typed (MENU-31, TERM-12): 64-300 MHz on the 5 kHz grid it is stored in; the region's own frequency
    /// is kept as "the region's" (f5k 0), as the knob does.
    pub(crate) fn set_aprs_freq(&mut self, f: Freq10) -> Result<(), &'static str> {
        if !(F_MIN..=F_MAX).contains(&f) {
            return Err("Out of range: APRS takes 64-300 MHz");
        }
        if !f.is_multiple_of(500) {
            return Err("Off the grid: APRS takes 5 kHz steps");
        }
        let f5k = if f == region_freq(self.set.region()) { 0 } else { (f / 500) as u16 };
        self.edit_aprs(f5k as i32, |c, v| c.f5k = v as u16);
        Ok(())
    }

    /// 0 5 6 1 by the knob.
    pub(crate) fn step_beacon(&mut self, d: i32) {
        let at = if self.aprs.smart() { DROPPED } else { self.aprs.mode() as usize };
        let mut i = (at as i32 + d).rem_euclid(BEACON_STEPS as i32) as usize;
        // APRS-29.2: a tracker always beacons, so Tracker's steps skip off and manual
        let dir = if d < 0 { -1 } else { 1 };
        while self.set.aprs_select() == 1 && i < TIMED {
            i = (i as i32 + dir).rem_euclid(BEACON_STEPS as i32) as usize;
        }
        let m = if i == DROPPED { SMART } else { i };
        self.edit_aprs(m as i32, |c, v| c.beacon = v as u8);
    }

    /// "Off", "Manual", "Every 10 min", "Smart".
    pub(crate) fn beacon_mode_text<const N: usize>(&self, b: &mut Buf<N>) {
        b.clear();
        let c = self.aprs;
        match (c.mode(), c.every_min()) {
            (0, _) => b.set(b"Off"),
            (_, Some(m)) => {
                let _ = uwrite!(b, "Every {} min", m);
            }
            _ if c.smart() => b.set(b"Smart"),
            _ => b.set(b"Manual"),
        }
    }

    /// APRS-21: the slot that shares chip 0 (64–300 MHz) with the APRS slot, so the two take turns on it: A or B
    /// when it listens there (active or watched). None when C has the chip to itself, or isn't the APRS slot.
    pub fn aprs_shared(&self) -> Option<u8> {
        if !self.aprs_c() || ferrite_core::freq::band(self.aprs_freq()).1 != 0 {
            return None;
        }
        let listening = (self.watch_mask() | 1 << self.active) & !self.off; // HOME-15.1: an off slot shares nothing
        (0..APRS_SLOT).find(|&s| listening & 1 << s != 0 && ferrite_core::freq::band(self.tuned_freq(s)).1 == 0)
    }

    /// APRS-19: the APRS slot can't be changed; the toast names what holds it (KISS-9, APRS-29; decision 181).
    /// True when refused.
    pub(crate) fn aprs_refuse(&mut self, s: u8) -> bool {
        let Some(h) = self.aprs_holder().filter(|_| s == APRS_SLOT) else { return false };
        self.show_toast(b"Slot C held by", h.name().as_bytes(), false);
        true
    }

    /// The reset: APRS settings back to their defaults (the store's went with [`crate::data::Op::Wipe`]).
    pub(crate) fn reset_aprs(&mut self) {
        (self.aprs, self.aprs_st) = (AprsCfg::DEFAULT, LOADED | self.aprs_st & HELD);
        self.send_slot(APRS_SLOT);
    }

    /// Every tick: the APRS settings from their owner once, and written back after a change.
    pub(crate) fn aprs_tick(&mut self, data: &dyn Data) {
        if self.aprs_st & LOADED == 0 {
            self.aprs_st |= LOADED;
            let c = data.aprs_cfg();
            if c != self.aprs {
                self.aprs = c;
                if self.aprs_c() {
                    self.send_slot(APRS_SLOT);
                }
            }
        }
        // KISS-9, APRS-29: KISS or beacons turned on or off (the menu, `set`, a settings write): C follows, and
        // the toast names what holds it now.
        if Holder::bits(self.aprs_holder()) != self.aprs_st & HELD {
            self.c_follow();
            match self.aprs_holder() {
                Some(h) => self.show_toast(b"Slot C held by", h.name().as_bytes(), false),
                None => self.show_toast(b"Slot C", b"restored", false),
            }
        }
        if self.aprs_st & DIRTY != 0 {
            data.request(crate::data::Op::AprsCfg(self.aprs)); // NoData (the radio until the APRS store): kept in RAM
            self.aprs_st &= !DIRTY;
        }
        if self.aprs_st & AFTER_VOICE != 0 {
            self.aprs_st &= !AFTER_VOICE;
            if self.aprs_on() && self.aprs.after_voice {
                data.request(crate::data::Op::AfterVoice);
            }
        }
    }

    /// A voice transmission ended (PTT released): APRS-27's beacon, asked for at the next tick.
    pub(crate) fn voice_ended(&mut self) {
        self.aprs_st |= AFTER_VOICE;
    }

    /// The APRS card's beacon line (APRS-17, APRS-20, APRS-24): "Beacon off", "Beacon manual", "Beacon in 4m", "Smart beacon",
    /// "Beacon waiting: HF" while the active slot holds the relay in the HF state.
    pub(crate) fn beacon_text<const N: usize>(&self, data: &dyn Data, b: &mut Buf<N>) {
        b.clear();
        let st = data.beacon();
        if st.waiting && needs_relay(self.tuned_freq(self.active)) {
            b.set(b"Beacon waiting: HF");
        } else if st.waiting {
            b.set(b"Beacon waiting");
        } else {
            match (self.aprs.mode(), st.next_s) {
                (0, _) => b.set(b"Beacon off"),
                (1, _) => b.set(b"Beacon manual"),
                _ if self.aprs.smart() => b.set(b"Smart beacon"),
                // APRS-17.2.1 (decision 313): "Beacon in 5m", seconds in the last minute ("Beacon in 42s")
                (_, Some(s)) => {
                    b.set(b"Beacon in ");
                    crate::aprs_card::soon_text(s, b);
                }
                (_, None) => b.set(b"Beacon soon"),
            }
        }
    }
}

/// The card's "last" line: the newest of the last station heard and the last message in (APRS-17).
pub(crate) fn last_text<const N: usize>(data: &dyn Data, b: &mut Buf<N>) {
    b.clear();
    let st = data.station(0);
    let msg = (0..data.messages().min(8)).filter_map(|i| data.message(i)).find(|m| m.inbound);
    match (st, msg) {
        (_, Some(m)) if st.is_none_or(|s| m.ago_s <= s.ago_s) => {
            let _ = uwrite!(b, "{}: {}", Txt(m.peer.as_bytes()), Txt(m.text.as_bytes()));
        }
        (Some(s), _) => {
            let mut a = Buf::<12>::new();
            age_text(s.ago_s, &mut a);
            let _ = uwrite!(b, "{} \u{b7} {} ago", Txt(s.call.as_bytes()), Txt(a.as_bytes()));
        }
        _ => b.set(b"Nothing heard yet"),
    }
}

/// A weather station's report (APRS-23), read from the start of its comment, where the APRS task leaves the
/// weather data of a positional weather report ("270/007g012t072r000p000P000h65b10123", then any text). Units as
/// sent: mph, degrees F, hundredths of an inch, tenths of a millibar. Nothing is kept: it is read when the
/// station screen is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Wx {
    pub dir: Option<u16>,
    pub wind: Option<u16>,
    pub gust: Option<u16>,
    pub temp_f: Option<i16>,
    /// Rain in the last hour.
    pub rain: Option<u16>,
    pub hum: Option<u8>,
    pub baro: Option<u32>,
    /// Where the comment's own text starts.
    pub rest: usize,
}

/// APRS-23: the weather fields at the start of a comment, if it starts with them (wind direction and speed).
pub fn wx(c: &[u8]) -> Option<Wx> {
    // a number of `n` characters at i: digits, a leading '-' (temperature), or dots / spaces for "no data"
    let num = |i: usize, n: usize| -> Option<Option<i32>> {
        let f = c.get(i..i + n)?;
        if f.iter().all(|&b| b == b'.' || b == b' ') {
            return Some(None);
        }
        let (neg, d) = match f.split_first() {
            Some((b'-', d)) => (true, d),
            _ => (false, f),
        };
        if d.is_empty() || !d.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let v = d.iter().fold(0i32, |a, &b| a * 10 + (b - b'0') as i32);
        Some(Some(if neg { -v } else { v }))
    };
    let mut w = Wx::default();
    // "DDD/SSS", or the complete-weather "cDDDsSSS"
    let mut i = if c.get(3) == Some(&b'/') {
        (w.dir, w.wind) = (num(0, 3)?.map(|v| v as u16), num(4, 3)?.map(|v| v as u16));
        7
    } else if c.first() == Some(&b'c') && c.get(4) == Some(&b's') {
        (w.dir, w.wind) = (num(1, 3)?.map(|v| v as u16), num(5, 3)?.map(|v| v as u16));
        8
    } else {
        return None;
    };
    // then letter-tagged fields, each a fixed width, in any order
    while let Some(&k) = c.get(i) {
        let n = match k {
            b'g' | b't' | b'r' | b'p' | b'P' | b'L' | b'l' | b's' => 3,
            b'h' => 2,
            b'b' => 5,
            _ => break,
        };
        let Some(v) = num(i + 1, n) else { break };
        match k {
            b'g' => w.gust = v.map(|v| v as u16),
            b't' => w.temp_f = v.map(|v| v as i16),
            b'r' => w.rain = v.map(|v| v as u16),
            b'h' => w.hum = v.map(|v| if v == 0 { 100 } else { v as u8 }), // h00 is 100 %
            b'b' => w.baro = v.map(|v| v as u32),
            _ => {}
        }
        i += 1 + n;
    }
    w.rest = i;
    Some(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weather_reports() {
        let w = wx(b"270/007g012t072r001p000P000h65b10123 Ottawa WX").unwrap();
        assert_eq!((w.dir, w.wind, w.gust, w.temp_f, w.rain, w.hum, w.baro), (Some(270), Some(7), Some(12), Some(72), Some(1), Some(65), Some(10123)));
        assert_eq!(&b"270/007g012t072r001p000P000h65b10123 Ottawa WX"[w.rest..], b" Ottawa WX");
        let w = wx(b"c...s...g...t-05h00").unwrap();
        assert_eq!((w.dir, w.wind, w.temp_f, w.hum), (None, None, Some(-5), Some(100)));
        assert!(wx(b"Mobile, on 146.520").is_none());
        assert!(wx(b"").is_none());
    }

    #[test]
    fn region_frequencies() {
        assert_eq!(region_freq(Some(Region::Us)), 14_439_000);
        assert_eq!(region_freq(Some(Region::Canada)), 14_439_000);
        assert_eq!(region_freq(Some(Region::Other)), 14_439_000);
        assert_eq!(region_freq(None), 14_439_000);
        assert_eq!(region_freq(Some(Region::Uk)), 14_480_000);
        assert_eq!(region_freq(Some(Region::Europe)), 14_480_000);
        assert_eq!(region_freq(Some(Region::Australia)), 14_517_500);
        assert_eq!(region_freq(Some(Region::NewZealand)), 14_457_500);
        assert_eq!(region_freq(Some(Region::Japan)), 14_464_000);
    }

    #[test]
    fn retry_schedule() {
        let waits: [u16; 5] = core::array::from_fn(|i| retry_wait(i as u8 + 1));
        assert_eq!(waits, [30, 60, 120, 240, LAST_WAIT_S]);
        assert_eq!(TRIES, 5);
    }

    #[test]
    fn beacon_modes() {
        let at = |t| Some(LastBeacon { at_s: t, ..LastBeacon::default() });
        let mut c = AprsCfg::DEFAULT;
        assert_eq!((c.mode(), c.every_min()), (0, None));
        assert!(!beacon_due(&c, None, 0, None));
        c.beacon = 1;
        assert_eq!((c.mode(), c.every_min()), (1, None));
        assert!(!beacon_due(&c, None, 0, None));
        c.beacon = 2 + 3; // every 10 min
        assert_eq!(c.every_min(), Some(10));
        assert!(beacon_due(&c, None, 5, None));
        assert!(!beacon_due(&c, at(100), 699, None));
        assert!(beacon_due(&c, at(100), 700, None));
        c.beacon |= BEACON_PAUSED;
        assert_eq!(c.mode(), 0);
        assert!(!beacon_due(&c, at(0), 10_000, None));
    }
}
