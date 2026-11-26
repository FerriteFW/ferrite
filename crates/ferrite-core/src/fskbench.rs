//! TERM-43 (decision 305, a bench test): the BK4829's own FSK modem, its speeds tried on the bench. `fsk rx SPEED`
//! listens on the chip that has the active slot and counts frames; `fsk tx SPEED N` sends N test frames at the next
//! PTT key-up (Dry TX only). The PC side is `tools/bench-blade/fskmodem.py` (a bladeRF generator and a decoder of
//! captures), with the frame below byte for byte; `docs/research/bk4829-fsk-modem.md` has the registers and the
//! frame.
//!
//! The modem's registers (BK4829 register table and BK4819 application note, which match word for word): REG_58
//! mode, receive gain, preamble type and receive bandwidth; REG_59 FIFO clears, scramble, receive and transmit
//! enables, preamble length, sync length; REG_5A/5B the four sync bytes; REG_5C bit 6 CRC; REG_5D data length;
//! REG_5F the FIFO; REG_72 (tone 2's word) the bit rate ("the FSK rate register is tone 2's", app note); REG_70
//! bits 6:0 tone 2's gain, the transmit deviation. Stock V0.29 sets the modem up twice (`analysis/bk4829_reg_access.csv`):
//! FSK 2400 at 0x0801E078 (REG_58 0x00C9, REG_70 0x00AC, REG_72 0x60CA, REG_5C 0x5665, REG_5D 0x0F00) and FFSK
//! 1200/1800 at 0x0801E0D0 (0x3FC3, 0x00EC, 0x3065, 0xA730, 0x1B00). Every value here comes from those two; what
//! neither gives (1200 FSK's REG_58 bandwidth, FFSK 1200/2400's mode bits) is the table's.
//!
//! The frame ([`payload`]): [`PREAMBLE`] bytes of preamble, the chip's reset sync word [`SYNC`] (4 bytes), then
//! [`LEN`] data bytes: a frame counter (2 bytes, big-endian), [`PRBS_LEN`] bytes of PRBS9 (the same every frame,
//! [`prbs`]), and our own CRC-16/CCITT-FALSE of the 30 bytes before it ([`crc16`], big-endian), so a frame checks
//! itself with the chip's CRC on or off. The chip's own CRC (2 bytes after the data when on) is not in Beken's
//! documents: the bench finds it from a capture (the decoder tries the usual CRC-16s). Scramble stays off (its
//! polynomial isn't documented either).

use crate::txtest::Prbs9;

/// Data bytes per frame (REG_5D: [`LEN`] - 1), and FIFO words.
pub const LEN: usize = 32;
pub const WORDS: usize = LEN / 2;
/// PRBS9 bytes in a frame: between the counter and our CRC.
pub const PRBS_LEN: usize = LEN - 4;
/// Preamble bytes (REG_59 bits 7:4 = this - 1); type 00, 0xAA or 0x55 from sync byte 0's top bit.
pub const PREAMBLE: u8 = 8;
/// The sync word: the BK4829's reset REG_5A/5B (0x85CF, 0xAB45), sent byte 0 first; four bytes (REG_59 bit 3).
pub const SYNC: [u8; 4] = [0x85, 0xCF, 0xAB, 0x45];
/// The most frames one `fsk tx` sends.
pub const COUNT_MAX: u16 = 999;
/// Bit rates `fsk rx|tx N` takes for plain FSK: the documented 1200 and 2400, and the rest to find the modem's edge.
pub const BAUD_MIN: u16 = 300;
pub const BAUD_MAX: u16 = 9600;

/// The modem's kinds of signal (REG_58's transmit and receive modes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// FSK 1.2K / 2.4K: two-level FSK straight on the carrier (NRZ, F1D); the rate is REG_72's.
    Fsk,
    /// FFSK 1200 baud on 1200/1800 Hz audio tones (MDC-1200's).
    Ffsk1800,
    /// FFSK on 1200/2400 Hz audio tones: 1200 baud as documented, or 2400 baud (`acars`, ACARS's tones and rate,
    /// ferrite#192: the same mode with REG_72 at 2400, a bench question).
    Ffsk2400,
}

/// A speed: the mode and its bit rate (FFSK is 1200 but for `acars`, FFSK 1200/2400 at 2400).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Speed {
    pub mode: Mode,
    pub baud: u16,
}

impl Speed {
    /// `1200`, `2400` or any rate from [`BAUD_MIN`] to [`BAUD_MAX`] (plain FSK); `ffsk1800`, `ffsk2400`; `acars`.
    pub fn parse(w: &[u8]) -> Option<Speed> {
        let is = |s: &[u8]| w.eq_ignore_ascii_case(s);
        if is(b"acars") {
            return Some(Speed { mode: Mode::Ffsk2400, baud: 2400 });
        }
        if is(b"ffsk1800") {
            return Some(Speed { mode: Mode::Ffsk1800, baud: 1200 });
        }
        if is(b"ffsk2400") {
            return Some(Speed { mode: Mode::Ffsk2400, baud: 1200 });
        }
        if w.is_empty() || w.len() > 4 || !w.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let n = w.iter().fold(0u16, |n, &c| n * 10 + (c - b'0') as u16);
        (BAUD_MIN..=BAUD_MAX).contains(&n).then_some(Speed { mode: Mode::Fsk, baud: n })
    }

    /// The modes the documents name: FSK 1200 and 2400, both FFSKs. Other plain FSK rates are the bench's guesses.
    pub fn documented(self) -> bool {
        if self.mode != Mode::Fsk {
            return self.baud == 1200; // `acars` (FFSK at 2400) is the bench's guess
        }
        self.baud == 1200 || self.baud == 2400
    }

    /// REG_58 without its enable bit (the driver sets bit 0): transmit mode 15:13, receive mode 12:10, receive gain
    /// 9:8, bits 7:6 (undocumented; both of stock's set them), preamble type 5:4 (00), receive bandwidth 3:1.
    /// FSK 2400: stock's 0x00C8 (bandwidth 100, gain 0), also for every rate above 1200; FSK 1200: the same with the
    /// table's bandwidth 000; FFSK 1200/1800: stock's 0x3FC2 (modes 001/111, gain 3, bandwidth 001); FFSK
    /// 1200/2400: the table's modes 011/100 and bandwidth 100, gain 3 as stock's FFSK.
    pub fn reg58(self) -> u16 {
        match self.mode {
            Mode::Fsk if self.baud <= 1200 => 0x00C0,
            Mode::Fsk => 0x00C8,
            Mode::Ffsk1800 => 0x3FC2,
            Mode::Ffsk2400 => 0b011 << 13 | 0b100 << 10 | 3 << 8 | 0xC0 | 0b100 << 1,
        }
    }

    /// REG_72, the bit rate: baud x 10.32444 (tone 2's word for the 26 MHz crystal), truncated as stock's 2400
    /// (0x60CA) and 1200 (0x3065).
    pub fn rate_word(self) -> u16 {
        (self.baud as u32 * 1_032_444 / 100_000) as u16
    }

    /// REG_70 bits 6:0, tone 2's gain: the transmit deviation. Stock's: 0x2C for FSK 2400, 0x6C for FFSK.
    pub fn tone_gain(self) -> u8 {
        if self.mode == Mode::Fsk {
            0x2C
        } else {
            0x6C
        }
    }

    /// REG_5C: stock's value for the mode (FSK 0x5665, FFSK 0xA730; only bit 6, the CRC, is documented) with bit 6
    /// as asked.
    pub fn reg5c(self, crc: bool) -> u16 {
        let base: u16 = if self.mode == Mode::Fsk { 0x5665 } else { 0xA730 };
        (base & !(1 << 6)) | (crc as u16) << 6
    }
}

/// A run's set-up: the speed, the chip's CRC on or off (`nocrc`), and the framing ([`Opts`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cfg {
    pub speed: Speed,
    pub crc: bool,
    pub opt: Opts,
}

impl Cfg {
    /// The bench frame's framing ([`Opts::FRAME`]).
    pub const fn new(speed: Speed, crc: bool) -> Self {
        Cfg { speed, crc, opt: Opts::FRAME }
    }

    /// REG_5C: [`Speed::reg5c`], or with `base=` that mode's base instead, bit 6 the CRC as asked.
    pub fn reg5c(&self) -> u16 {
        match self.opt.base {
            None => self.speed.reg5c(self.crc),
            Some(b) => (b & !(1 << 6)) | (self.crc as u16) << 6,
        }
    }

    /// REG_58 without its enable bit: [`Speed::reg58`] with the preamble type (bits 5:4) asked.
    pub fn reg58(&self) -> u16 {
        (self.speed.reg58() & !0x30) | (self.opt.ptype as u16 & 3) << 4
    }
}

/// Stock's REG_5C bases: FSK's and FFSK's (`base=fsk|ffsk`).
pub const BASE_FSK: u16 = 0x5665;
pub const BASE_FFSK: u16 = 0xA730;

/// The framing an `fsk` run sets the modem up with (ferrite#192: ACARS's sync and preamble on the bench, without
/// `bk` pokes, which the run's set-up writes over at every retune, wake and frame): `sync=HEX` (2 or 4 bytes; REG_59
/// bit 3), `pre=N` preamble bytes (1-16, REG_59 7:4), `ptype=N` preamble type (0-3, REG_58 5:4), `inv` received data
/// inverted (REG_59 bit 10), `scr` scramble (REG_59 bit 13), `base=fsk|ffsk` REG_5C's other bits from that mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opts {
    pub sync: [u8; 4],
    pub sync4: bool,
    pub pre: u8,
    pub ptype: u8,
    pub inv: bool,
    pub scr: bool,
    pub base: Option<u16>,
}

impl Opts {
    /// The bench frame's: [`SYNC`], [`PREAMBLE`] bytes of type 00, no inversion or scramble, the speed's REG_5C.
    pub const FRAME: Opts = Opts { sync: SYNC, sync4: true, pre: PREAMBLE, ptype: 0, inv: false, scr: false, base: None };

    /// One option word into `self`; false if it isn't one.
    pub fn word(&mut self, w: &[u8]) -> bool {
        let is = |s: &[u8]| w.eq_ignore_ascii_case(s);
        let val = |k: &[u8]| (w.len() > k.len() && w[..k.len()].eq_ignore_ascii_case(k)).then(|| &w[k.len()..]);
        let num = |v: &[u8]| (!v.is_empty() && v.len() <= 2 && v.iter().all(u8::is_ascii_digit)).then(|| v.iter().fold(0u8, |n, &c| n * 10 + (c - b'0')));
        if is(b"inv") {
            self.inv = true;
        } else if is(b"scr") {
            self.scr = true;
        } else if is(b"base=fsk") {
            self.base = Some(BASE_FSK);
        } else if is(b"base=ffsk") {
            self.base = Some(BASE_FFSK);
        } else if let Some(v) = val(b"pre=") {
            match num(v).filter(|n| (1..=16).contains(n)) {
                Some(n) => self.pre = n,
                None => return false,
            }
        } else if let Some(v) = val(b"ptype=") {
            match num(v).filter(|&n| n <= 3) {
                Some(n) => self.ptype = n,
                None => return false,
            }
        } else if let Some(v) = val(b"sync=") {
            if (v.len() != 4 && v.len() != 8) || !v.iter().all(u8::is_ascii_hexdigit) {
                return false;
            }
            let h = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
            let mut s = [0u8; 4];
            for (i, p) in v.chunks(2).enumerate() {
                s[i] = h(p[0]) << 4 | h(p[1]);
            }
            (self.sync, self.sync4) = (s, v.len() == 8);
        } else {
            return false;
        }
        true
    }
}

/// What `fsk` asks the Radio task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// `fsk`: the counts and the last frame.
    Show,
    /// `fsk off`: the modem off, the slot retuned as normal.
    Off,
    /// `fsk rx SPEED [nocrc]`: receive on the active slot's chip, counts from zero.
    Rx(Cfg),
    /// `fsk tx SPEED N [nocrc]`: N frames at the next PTT key-up (Dry TX on only).
    Tx(Cfg, u16),
}

/// What `fsk` answers: the run (None: off), `fsk tx`'s frame count (None: receiving) and whether it still waits for
/// its key-up, the chip listening or sending now, and the counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    pub cfg: Option<Cfg>,
    pub tx: Option<u16>,
    pub armed: bool,
    pub chip: Option<u8>,
    pub stats: Stats,
}

/// The counts of a run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Sync words found (REG_02 bit 1), and of those found inverted (REG_0B bit 7).
    pub syncs: u16,
    pub sync_neg: u16,
    /// Frames ended (REG_02 bit 13); with the chip's CRC on, good and bad (REG_0B bit 4).
    pub frames: u16,
    pub crc_ok: u16,
    pub crc_bad: u16,
    /// Frames whose bytes passed our own check ([`check`]: our CRC and the PRBS exact), whatever the chip said.
    pub clean: u16,
    /// Frames whose bytes fit better with each FIFO word's low byte first (else high first, as sent), and starting
    /// one byte late ([`Check::shifted`]).
    pub lo_first: u16,
    pub shifted: u16,
    /// Frames with fewer than [`WORDS`] words read (the FIFO overran between ticks).
    pub short: u16,
    /// PRBS bit errors over every frame read whole.
    pub bit_errors: u32,
    /// The last frame's bytes as read (high byte of each word first), how many, and its counter (as checked).
    pub last: [u8; LEN],
    pub last_n: u8,
    pub last_counter: u16,
    /// `fsk tx`: frames sent at the key-up so far.
    pub sent: u16,
}

/// The frame being read from the FIFO.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RxFrame {
    words: [u16; WORDS],
    got: u8,
}

impl RxFrame {
    /// Words still to read for a whole frame.
    pub fn left(&self) -> usize {
        WORDS - self.got as usize
    }

    pub fn push(&mut self, w: u16) {
        if (self.got as usize) < WORDS {
            self.words[self.got as usize] = w;
            self.got += 1;
        }
    }
}

impl Stats {
    /// A sync word found: a new frame starts.
    pub fn sync(&mut self, neg: bool, f: &mut RxFrame) {
        self.syncs = self.syncs.saturating_add(1);
        self.sync_neg = self.sync_neg.saturating_add(neg as u16);
        f.got = 0;
    }

    /// The frame ended: `crc` the chip's verdict (None with its CRC off). Counted, kept as the last, and cleared.
    pub fn done(&mut self, f: &mut RxFrame, crc: Option<bool>) {
        self.frames = self.frames.saturating_add(1);
        match crc {
            Some(true) => self.crc_ok = self.crc_ok.saturating_add(1),
            Some(false) => self.crc_bad = self.crc_bad.saturating_add(1),
            None => {}
        }
        let n = f.got as usize;
        for (i, w) in f.words[..n].iter().enumerate() {
            self.last[2 * i..2 * i + 2].copy_from_slice(&w.to_be_bytes());
        }
        self.last_n = (2 * n) as u8;
        if n < WORDS {
            self.short = self.short.saturating_add(1);
        } else {
            let c = check(&f.words);
            self.last_counter = c.counter;
            self.bit_errors = self.bit_errors.saturating_add(c.errors as u32);
            self.lo_first = self.lo_first.saturating_add(c.lo_first as u16);
            self.shifted = self.shifted.saturating_add(c.shifted as u16);
            self.clean = self.clean.saturating_add((c.crc && c.errors == 0) as u16);
        }
        f.got = 0;
    }
}

/// The PRBS9 bytes every frame carries (x^9 + x^5 + 1 from all ones, [`Prbs9`] as `txtest`'s), bits packed most
/// significant first.
pub fn prbs() -> [u8; PRBS_LEN] {
    let mut p = Prbs9::default();
    core::array::from_fn(|_| (0..8).fold(0u8, |b, _| b << 1 | p.bit()))
}

/// CRC-16/CCITT-FALSE: polynomial 0x1021, initial 0xFFFF, no reflection, no final xor (check "123456789" 0x29B1).
pub fn crc16(data: &[u8]) -> u16 {
    data.iter().fold(0xFFFFu16, |mut c, &b| {
        c ^= (b as u16) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { c << 1 ^ 0x1021 } else { c << 1 };
        }
        c
    })
}

/// Frame `n`'s data bytes: the counter, the PRBS, our CRC.
pub fn payload(n: u16) -> [u8; LEN] {
    let mut p = [0u8; LEN];
    p[..2].copy_from_slice(&n.to_be_bytes());
    p[2..2 + PRBS_LEN].copy_from_slice(&prbs());
    let c = crc16(&p[..LEN - 2]);
    p[LEN - 2..].copy_from_slice(&c.to_be_bytes());
    p
}

/// The frame as FIFO words (REG_5F), each word's high byte first (as REG_5A holds sync byte 0 in its high byte; the
/// bench confirms the order on the air).
pub fn words(p: &[u8; LEN]) -> [u16; WORDS] {
    core::array::from_fn(|i| u16::from_be_bytes([p[2 * i], p[2 * i + 1]]))
}

/// What [`check`] found in a frame's words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Check {
    pub counter: u16,
    /// PRBS bits wrong.
    pub errors: u16,
    /// Our CRC matched.
    pub crc: bool,
    /// The bytes fit better taking each word's low byte first.
    pub lo_first: bool,
    /// The FIFO's bytes start one late: the frame's first byte (the counter's high byte) isn't in it, and the
    /// counter's high byte is taken as 0 (ferrite#192: FFSK 1200/2400 at 2400 baud, Nov 25, 2026, gives the frame
    /// from its second byte on, low byte first, every bit right).
    pub shifted: bool,
}

/// A received frame checked against [`payload`], both byte orders and both starts tried; the one with fewer PRBS
/// errors counts (the PRBS bytes are in the frame either way).
pub fn check(w: &[u16; WORDS]) -> Check {
    let want = prbs();
    let one = |lo: bool, sh: bool| {
        let got: [u8; LEN] = core::array::from_fn(|i| {
            let x = w[i / 2].to_be_bytes();
            x[(i & 1) ^ lo as usize]
        });
        // the frame as sent: shifted, byte 0 is taken as 0 (the FIFO's last byte, after our CRC, is the chip's)
        let b: [u8; LEN] = core::array::from_fn(|i| if !sh { got[i] } else if i == 0 { 0 } else { got[i - 1] });
        let errors = b[2..2 + PRBS_LEN].iter().zip(want).map(|(a, e)| (a ^ e).count_ones() as u16).sum();
        let crc = crc16(&b[..LEN - 2]) == u16::from_be_bytes([b[LEN - 2], b[LEN - 1]]);
        Check { counter: u16::from_be_bytes([b[0], b[1]]), errors, crc, lo_first: lo, shifted: sh }
    };
    let mut best = one(false, false);
    for (lo, sh) in [(true, false), (false, true), (true, true)] {
        let c = one(lo, sh);
        if c.errors < best.errors {
            best = c;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::format;
    use std::string::String;

    /// TERM-43: our CRC is CRC-16/CCITT-FALSE (its check value), and the frame is the counter, the PRBS and the CRC,
    /// byte for byte what `tools/bench-blade/fskmodem.py --selftest` prints for frame 1.
    #[test]
    fn frame_matches_the_pc_side() {
        assert_eq!(crc16(b"123456789"), 0x29B1);
        let p = payload(1);
        let hex: String = p.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, PC_FRAME_1);
        assert_eq!(&p[..2], &[0, 1]);
        assert_eq!(crc16(&p[..LEN - 2]), u16::from_be_bytes([p[LEN - 2], p[LEN - 1]]));
    }

    /// The frame 1 bytes from `fskmodem.py --selftest`.
    const PC_FRAME_1: &str = "000107be2e64129da3cf9b15238dab89888042309cab0de9b9142b4fd9256b99";

    /// TERM-43: a frame read back whole is clean, a flipped bit counts one error and fails our CRC, and words read
    /// low byte first are found so.
    #[test]
    fn check_counts_errors_and_byte_order() {
        let w = words(&payload(7));
        assert_eq!(check(&w), Check { counter: 7, errors: 0, crc: true, lo_first: false, shifted: false });
        let mut bad = w;
        bad[3] ^= 0x0100;
        let c = check(&bad);
        assert_eq!((c.errors, c.crc), (1, false));
        let swapped = w.map(u16::swap_bytes);
        assert_eq!(check(&swapped), Check { counter: 7, errors: 0, crc: true, lo_first: true, shifted: false });
    }

    /// TERM-43 (ferrite#192): the FIFO as the chip gave it on Nov 25, 2026 (`fsk rx acars`, frame 19 of the bench
    /// file, AM and FM alike): low byte first, from the frame's second byte on: clean, counter 19.
    #[test]
    fn check_finds_the_shifted_frame() {
        let got = [
            0x0713, 0x2EBE, 0x1264, 0xA39D, 0x9BCF, 0x2315, 0xAB8D, 0x8889, 0x4280, 0x9C30, 0x0DAB, 0xB9E9, 0x2B14, 0xD94F, 0x6525, 0xAAFB,
        ];
        assert_eq!(check(&got), Check { counter: 19, errors: 0, crc: true, lo_first: true, shifted: true });
    }

    /// TERM-43: the counts: a sync, the words, the end with the chip's CRC; a frame cut short counts as short.
    #[test]
    fn stats_count_frames() {
        let mut s = Stats::default();
        let mut f = RxFrame::default();
        s.sync(false, &mut f);
        for w in words(&payload(3)) {
            f.push(w);
        }
        assert_eq!(f.left(), 0);
        s.done(&mut f, Some(true));
        s.sync(true, &mut f);
        f.push(0x1234);
        s.done(&mut f, Some(false));
        assert_eq!((s.syncs, s.sync_neg, s.frames, s.crc_ok, s.crc_bad, s.clean, s.short), (2, 1, 2, 1, 1, 1, 1));
        assert_eq!((s.last_n, s.last_counter, &s.last[..2]), (2, 3, &[0x12, 0x34][..]));
    }

    /// TERM-43: the speeds `fsk` takes and their registers: stock's for FSK 2400 and FFSK 1200/1800.
    #[test]
    fn speeds_and_registers() {
        let s = |w: &str| Speed::parse(w.as_bytes());
        let fsk2400 = s("2400").unwrap();
        assert_eq!((fsk2400.reg58() | 1, fsk2400.rate_word(), fsk2400.tone_gain(), fsk2400.reg5c(true)), (0x00C9, 0x60CA, 0x2C, 0x5665));
        let ffsk = s("FFSK1800").unwrap();
        assert_eq!((ffsk.reg58() | 1, ffsk.rate_word(), ffsk.tone_gain(), ffsk.reg5c(false)), (0x3FC3, 0x3065, 0x6C, 0xA730));
        assert_eq!(s("ffsk2400").unwrap().reg58(), 0x73C8);
        // ferrite#192: `acars` is FFSK 1200/2400's mode and filter at 2400 baud (stock's FSK 2400 rate word)
        let acars = s("ACARS").unwrap();
        assert_eq!((acars.reg58(), acars.rate_word(), acars.tone_gain(), acars.documented()), (0x73C8, 0x60CA, 0x6C, false));
        assert_eq!(s("1200").unwrap().reg58(), 0x00C0);
        assert_eq!(s("4800").map(|s| (s.reg58(), s.documented())), Some((0x00C8, false)));
        assert!(fsk2400.documented() && ffsk.documented());
        for bad in ["299", "9601", "fsk", "", "12000"] {
            assert_eq!(s(bad), None, "{bad}");
        }
    }

    /// TERM-43 (ferrite#192): the framing options, and REG_58/5C with them.
    #[test]
    fn options_set_the_framing() {
        let mut o = Opts::FRAME;
        for w in ["sync=01A3a33f", "pre=1", "ptype=2", "inv", "scr", "base=fsk"] {
            assert!(o.word(w.as_bytes()), "{w}");
        }
        assert_eq!(o, Opts { sync: [0x01, 0xA3, 0xA3, 0x3F], sync4: true, pre: 1, ptype: 2, inv: true, scr: true, base: Some(BASE_FSK) });
        assert!(o.word(b"sync=8c5c"));
        assert_eq!((o.sync[..2].to_vec(), o.sync4), (std::vec![0x8C, 0x5C], false));
        for bad in ["pre=0", "pre=17", "ptype=4", "sync=123", "sync=12345g78", "nocrc", "base=x", "pre="] {
            assert!(!Opts::FRAME.clone().word(bad.as_bytes()), "{bad}");
        }
        let acars = Speed::parse(b"acars").unwrap();
        let c = Cfg { opt: o, ..Cfg::new(acars, false) };
        assert_eq!((c.reg58(), c.reg5c()), (0x73E8, 0x5625));
        assert_eq!((Cfg::new(acars, true).reg58(), Cfg::new(acars, true).reg5c()), (0x73C8, 0xA770));
    }
}
