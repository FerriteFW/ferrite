//! ACARS (chapter 27b, on trial: a receive-only display, not yet connected to the radio): the messages heard,
//! kept in RAM only ([`AcarsLive`], held beside the APRS task's lists in the store's `extra`, [`crate::data::Data`]),
//! the frequencies the screen offers, and the sample traffic the simulator and the bench builds' `acars feed` give
//! it until the radio's modem feeds it.
//!
//! TODO(acars-radio): the Radio task tunes the VHF chip to [`FREQS`]`[set.acars_freq]` in AM while the screen is
//! open, runs the chip's FSK modem at 2400 bit/s (ferrite#192: `fsk rx acars`), and hands each frame's bytes after
//! SOH to [`AcarsLive::frame`] with the frequency's index and the time; and calls [`AcarsLive::tick`] once a second.

use core::cell::RefCell;
use ferrite_core::acars::{self, Frame, Log, Msg};
use ferrite_core::freq::Freq10;

/// Messages kept (newest first) and the bytes of text they share: about 3.1 KB of RAM with the two messages being
/// joined ([`ferrite_core::acars::JOIN_MAX`] each).
pub const KEEP: usize = 16;
pub const POOL: usize = 1536;
pub type AcarsLog = Log<KEEP, POOL>;

/// ACARS-3: the frequencies ◀▶ step through (10 Hz units), the first the default: 131.550 worldwide, then North
/// America's others, Japan's and Europe's.
pub static FREQS: [Freq10; 7] = [13_155_000, 13_172_500, 13_002_500, 12_912_500, 13_045_000, 13_145_000, 13_690_000];

/// The messages heard since power-on.
pub struct AcarsLive {
    log: RefCell<AcarsLog>,
}

impl Default for AcarsLive {
    fn default() -> Self {
        Self::new()
    }
}

impl AcarsLive {
    pub const fn new() -> Self {
        AcarsLive { log: RefCell::new(Log::new()) }
    }

    /// A frame's bytes after SOH, heard at `now_s` on [`FREQS`]`[freq]` (ACARS-2). A frame that doesn't parse is
    /// dropped; a bad CRC is kept, marked.
    pub fn frame(&self, bytes: &[u8], freq: u8, now_s: u32) -> Result<(), acars::Error> {
        let f = acars::parse(bytes)?;
        self.push(&f, freq, now_s);
        Ok(())
    }

    #[inline(never)]
    fn push(&self, f: &Frame, freq: u8, now_s: u32) {
        self.log.borrow_mut().push(f, freq, now_s);
    }

    /// Time passes: a message whose last block is overdue is kept as it is (ACARS-2.3).
    pub fn tick(&self, now_s: u32) {
        self.log.borrow_mut().tick(now_s);
    }

    pub fn len(&self) -> u16 {
        self.log.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn version(&self) -> u32 {
        self.log.borrow().version
    }

    /// Frames heard, those with a bad CRC, and copies dropped.
    pub fn counts(&self) -> (u16, u16, u16) {
        let l = self.log.borrow();
        (l.frames, l.bad, l.dups)
    }

    /// Message `i` (0 the newest), its text copied into `text` (cut to its length); the text's length.
    pub fn get(&self, i: u16, text: &mut [u8]) -> Option<(Msg, usize)> {
        let l = self.log.borrow();
        let (m, t) = l.get(i)?;
        let n = t.len().min(text.len());
        text[..n].copy_from_slice(&t[..n]);
        Some((m, n))
    }

    pub fn clear(&self) {
        self.log.borrow_mut().clear();
    }
}

/// The sample traffic (ACARS-8): made-up messages in the right format, Canadian and US carriers near Vancouver.
/// One uplink, one with a bad CRC, one of two blocks. Each with how long before the feed it came.
#[cfg(any(feature = "bench", not(target_os = "none")))]
pub mod sample {
    use super::AcarsLive;
    use ferrite_core::acars::{encode, Fields, DEL, NAK};

    const fn down(reg: &'static [u8], label: &[u8; 2], block: u8, msgno: &'static [u8], flight: &'static [u8], text: &'static [u8]) -> Fields<'static> {
        Fields { mode: b'2', reg, ack: NAK, label: *label, block, msgno, flight, text, more: false }
    }

    const fn up(reg: &'static [u8], label: &[u8; 2], block: u8, text: &'static [u8]) -> Fields<'static> {
        Fields { mode: b'2', reg, ack: b'3', label: *label, block, msgno: b"", flight: b"", text, more: false }
    }

    const fn more(f: Fields<'static>) -> Fields<'static> {
        Fields { more: true, ..f }
    }

    /// (seconds before the feed, the frame, its CRC spoiled), oldest first.
    pub static FRAMES: [(u16, Fields<'static>, bool); 11] = [
        (540, down(b"C-FWSX", b"QB", b'2', b"M11A", b"WS1702", b"OFF 0706 CYVR/CYYC"), false),
        (420, down(b"C-FGKZ", b"H1", b'5', b"M03A", b"AC0322", b"REQ WX UPDATE CYYZ AND ALTN CYOW"), false),
        (240, down(b"C-GKQB", b"80", b'1', b"M22A", b"PD0442", b"POS N4912.3 W12241.8,FL120,CYVR/CYYJ,ETA 0731"), false),
        (418, more(down(b"C-FIUA", b"H1", b'3', b"M01A", b"AC0115", b"POSN49123W123158,0712,350,CYVR,KSFO,")), false),
        (417, down(b"C-FIUA", b"H1", b'4', b"M01B", b"AC0115", b"FOB 182,SAT -52,WIND 270/085,NO DELAY EXPECTED"), false),
        (160, up(b"C-GWSZ", b"RA", b'B', b"METAR CYVR 240700Z 09008KT 15SM -RA BKN030 07/05 A2990"), false),
        (120, down(b"C-GWSZ", &[b'_', DEL], b'6', b"M02A", b"WS0123", b""), false),
        (95, down(b"N37502", b"5Z", b'7', b"M14A", b"UA0947", b"/ IN KSFO 0711 FUEL 042"), false),
        (70, down(b"N619AS", b"H1", b'8', b"M31A", b"AS0207", b"POSN47271W122187,0713,280,KSEA,KLAX"), true),
        (40, down(b"C-GTSF", b"SA", b'9', b"M07A", b"TS0811", b"0EV071240V"), false),
        (10, Fields { mode: b'2', reg: b"C-FJZS", ack: NAK, label: *b"Q0", block: b'1', msgno: b"", flight: b"", text: b"", more: false }, false),
    ];

    /// The first `n` sample frames, heard as if `now_s` was when the last came; the frames put in.
    pub fn feed(live: &AcarsLive, n: usize, freq: u8, now_s: u32) -> usize {
        let mut order: [usize; FRAMES.len()] = core::array::from_fn(|i| i);
        // oldest first, so the newest is at the top
        order.sort_unstable_by_key(|&i| core::cmp::Reverse(FRAMES[i].0));
        let mut b = [0u8; 256];
        let mut put = 0;
        for &i in order.iter().filter(|&&i| i < n) {
            let (ago, f, bad) = &FRAMES[i];
            let len = encode(f, &mut b);
            if *bad {
                b[len - 3] ^= 0x5A; // the CRC's first byte
            }
            if live.frame(&b[..len], freq, now_s.wrapping_sub(*ago as u32)).is_ok() {
                put += 1;
            }
        }
        live.tick(now_s);
        put
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ACARS-8: the samples are ten messages, the two blocks joined, one uplink, one with a bad CRC, all parsed.
    #[test]
    fn the_samples_make_ten_messages() {
        let live = AcarsLive::new();
        assert_eq!(sample::feed(&live, 99, 0, 1000), 11);
        assert_eq!(live.len(), 10);
        let mut t = [0u8; 512];
        let all: std::vec::Vec<_> = (0..10).map(|i| live.get(i, &mut t).unwrap().0).collect();
        assert_eq!(all.iter().filter(|m| m.up()).count(), 1);
        assert_eq!(all.iter().filter(|m| !m.crc_ok()).count(), 1);
        assert_eq!(all.iter().filter(|m| m.blocks == 2).count(), 1);
        let (m, n) = live.get(0, &mut t).unwrap();
        assert_eq!((m.reg(), n, m.at_s), (&b"C-FJZS"[..], 0, 990));
        // the newest heard again at once: a copy (ACARS-2.2)
        let mut b = [0u8; 64];
        let k = sample::FRAMES.len() - 1;
        let n = ferrite_core::acars::encode(&sample::FRAMES[k].1, &mut b);
        live.frame(&b[..n], 0, 1001).unwrap();
        assert_eq!((live.len(), live.counts()), (10, (12, 1, 1)));
    }
}
