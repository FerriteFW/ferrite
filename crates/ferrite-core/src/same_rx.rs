//! SAME headers from the BK4829's own modem (manual chapter 27, DEC-19; WX-4, WX-11): the chip's FSK receiver in
//! its NOAA SAME mode (520.83 baud, 2083.3/1562.5 Hz; BK4819 application note, REG_58 receive mode 000 with
//! bandwidth 010, the rate in Tone 2's word) finds the sync word and hands over 16-bit FIFO words. This turns the
//! words into the header's text and the text into a [`SameHeader`], for the Radio task (the firmware) and the
//! host tests alike.
//!
//! - **Sync:** the modem is told to sync on `ZCZC`, the header's first four characters, so the data starts at the
//!   dash after them. SAME sends each byte least significant bit first; how the BK4829 packs bits into FIFO
//!   bytes and bytes into words isn't documented, so [`SYNC`] assumes most significant bit first (the usual
//!   FSK modem order: `Z` reads the same either way, `C` 0x43 arrives as 0xC2) and [`Assembler`] works out the
//!   rest from the first eight bytes, which are always `-ORG-EEE` (`-WXR-TOR`): byte order in the word, and bit
//!   order in the byte. Bench-confirm (roadmap 6b): which orientation the chip gives, and whether its data is
//!   inverted (REG_59 bit 10).
//! - **One burst is enough:** a header is sent three times about a second apart; Battery Save may sleep through
//!   one or two (WX-11), so the first burst that parses is taken, and the same text again within
//!   [`REPEAT_S`] is the same alert, not a new one.

use crate::same::{self, SameHeader};

/// The modem's sync word, `ZCZC` as the chip should see it (most significant bit first, see the module notes).
pub const SYNC: [u8; 4] = [0x5A, 0xC2, 0x5A, 0xC2];
/// The packet length the modem is told to expect, bytes after the sync: the longest header (31 areas) is
/// `-ORG-EEE` + 31 x `-PSSCCC` + `+TTTT-JJJHHMM-LLLLLLLL-`, 255 characters.
pub const MAX_LEN: usize = 256;
/// SAME's rate, millibaud (520 5/6 baud).
pub const BAUD_MILLI: u32 = 520_833;
/// The same text again within this long is a repeat of the alert just raised (a header's three bursts, and the
/// EAS rule that a header doesn't repeat inside a few minutes).
pub const REPEAT_S: u32 = 300;

/// Byte order in a FIFO word and bit order in a byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Orient {
    /// The first byte of a word is its low byte.
    pub low_first: bool,
    /// Each byte arrives bit-reversed (least significant bit first on air read most significant first).
    pub reversed: bool,
}

impl Orient {
    pub const ALL: [Orient; 4] = [
        Orient { low_first: false, reversed: true },
        Orient { low_first: true, reversed: true },
        Orient { low_first: false, reversed: false },
        Orient { low_first: true, reversed: false },
    ];

    /// One FIFO word's two characters.
    pub fn bytes(self, w: u16) -> [u8; 2] {
        let (a, b) = ((w >> 8) as u8, w as u8);
        let (a, b) = if self.low_first { (b, a) } else { (a, b) };
        if self.reversed {
            [a.reverse_bits(), b.reverse_bits()]
        } else {
            [a, b]
        }
    }
}

/// Where a header being received stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// More words wanted.
    More,
    /// The header's text is complete: [`Assembler::text`].
    Done,
    /// Not a header (unprintable, too long, or no orientation fits): start over at the next sync.
    Bad,
}

/// The FIFO words of one burst, from the sync on, into the header's text. 268 bytes.
pub struct Assembler {
    /// `ZCZC` and what followed.
    buf: [u8; 4 + MAX_LEN],
    len: u16,
    /// The first four words, until the orientation is known.
    first: [u16; 4],
    n_first: u8,
    orient: Option<Orient>,
    /// Dashes since the `+` (the header ends at the third), or None before it.
    after_plus: Option<u8>,
}

impl Default for Assembler {
    fn default() -> Self {
        Self::new()
    }
}

impl Assembler {
    pub const fn new() -> Self {
        Assembler { buf: [0; 4 + MAX_LEN], len: 4, first: [0; 4], n_first: 0, orient: None, after_plus: None }
    }

    /// A new burst (the modem found the sync word).
    pub fn start(&mut self) {
        *self = Assembler::new();
        self.buf[..4].copy_from_slice(b"ZCZC");
    }

    /// The orientation found, once eight bytes are in.
    pub fn orient(&self) -> Option<Orient> {
        self.orient
    }

    /// The header's text so far, `ZCZC-...`.
    pub fn text(&self) -> &[u8] {
        &self.buf[..self.len as usize]
    }

    /// One FIFO word.
    pub fn push(&mut self, w: u16) -> Step {
        let Some(o) = self.orient else {
            // DEC-19: words after a burst refused for its orientation (no sync since, so no `start`): still refused,
            // not a write past `first` (a panic in the Radio task, found by fuzz_same_recovers).
            if self.n_first as usize >= self.first.len() {
                return Step::Bad;
            }
            self.first[self.n_first as usize] = w;
            self.n_first += 1;
            if self.n_first < 4 {
                return Step::More;
            }
            // `-ORG-EEE`: a dash, three capitals, a dash, three capitals or digits.
            let fits = |o: Orient| {
                let mut b = [0u8; 8];
                for (i, w) in self.first.iter().enumerate() {
                    b[2 * i..2 * i + 2].copy_from_slice(&o.bytes(*w));
                }
                b[0] == b'-' && b[4] == b'-' && b[1..4].iter().all(u8::is_ascii_uppercase) && b[5..8].iter().all(u8::is_ascii_alphanumeric)
            };
            let Some(o) = Orient::ALL.into_iter().find(|&o| fits(o)) else { return Step::Bad };
            self.orient = Some(o);
            let first = self.first;
            let mut step = Step::More;
            for w in first {
                step = self.put(o, w);
            }
            return step;
        };
        self.put(o, w)
    }

    fn put(&mut self, o: Orient, w: u16) -> Step {
        for c in o.bytes(w) {
            if !(0x20..0x7F).contains(&c) {
                return Step::Bad;
            }
            if self.len as usize == self.buf.len() {
                return Step::Bad;
            }
            self.buf[self.len as usize] = c;
            self.len += 1;
            match (c, &mut self.after_plus) {
                (b'+', p @ None) => *p = Some(0),
                (b'-', Some(n)) => {
                    *n += 1;
                    if *n == 3 {
                        return Step::Done; // +TTTT-JJJHHMM-LLLLLLLL-
                    }
                }
                _ => {}
            }
        }
        Step::More
    }
}

/// Headers taken: the first burst that parses raises the alert, the same text again within [`REPEAT_S`] doesn't.
#[derive(Default)]
pub struct Repeats {
    /// A checksum of the last header taken, and when (seconds).
    last: Option<(u32, u32)>,
}

impl Repeats {
    pub const fn new() -> Self {
        Repeats { last: None }
    }

    /// A finished burst's text at `now_s`: the header, if it parses and isn't a repeat.
    pub fn take(&mut self, text: &[u8], now_s: u32) -> Option<SameHeader> {
        let h = same::parse(text)?;
        let sum = text.iter().fold(0x811C_9DC5u32, |a, &c| (a ^ c as u32).wrapping_mul(0x0100_0193));
        if self.last.is_some_and(|(s, t)| s == sum && now_s.wrapping_sub(t) < REPEAT_S) {
            return None;
        }
        self.last = Some((sum, now_s));
        Some(h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec::Vec;

    const HDR: &[u8] = b"ZCZC-WXR-TOR-039173-039051-139069+0030-1591829-KCLE/NWS-";

    /// DEC-19: a burst refused for its orientation stays refused for the words after it until the next sync
    /// (`start`), and a good header still comes out after that: it used to panic on the fifth word.
    #[test]
    fn words_after_a_refused_burst() {
        let mut a = Assembler::new();
        a.start();
        let mut last = Step::More;
        for _ in 0..4 {
            last = a.push(0x0101);
        }
        assert_eq!(last, Step::Bad);
        for w in 0..20u16 {
            assert_eq!(a.push(w), Step::Bad);
        }
        a.start();
        let o = Orient::ALL[0];
        let r = words(HDR, o).into_iter().map(|w| a.push(w)).find(|&s| s != Step::More);
        assert_eq!(r, Some(Step::Done));
        assert_eq!(a.text(), HDR);
    }

    /// The data after `ZCZC` as the chip would hand it over in orientation `o`.
    fn words(text: &[u8], o: Orient) -> Vec<u16> {
        let data = &text[4..];
        let mut padded = data.to_vec();
        if padded.len() % 2 == 1 {
            padded.push(0xAB); // the trailing byte of the next thing on air
        }
        padded
            .chunks(2)
            .map(|p| {
                let (a, b) = if o.reversed { (p[0].reverse_bits(), p[1].reverse_bits()) } else { (p[0], p[1]) };
                if o.low_first {
                    (b as u16) << 8 | a as u16
                } else {
                    (a as u16) << 8 | b as u16
                }
            })
            .collect()
    }

    fn run(ws: &[u16]) -> (Step, Vec<u8>, Option<Orient>) {
        let mut a = Assembler::new();
        a.start();
        let mut step = Step::More;
        for &w in ws {
            step = a.push(w);
            if step != Step::More {
                break;
            }
        }
        (step, a.text().to_vec(), a.orient())
    }

    #[test]
    fn every_orientation() {
        for o in Orient::ALL {
            let (step, text, got) = run(&words(HDR, o));
            assert_eq!(step, Step::Done, "{o:?}");
            assert_eq!(text, HDR, "{o:?}");
            assert_eq!(got, Some(o));
            assert!(same::parse(&text).is_some());
        }
    }

    #[test]
    fn sync_is_zczc_msb_first() {
        // SAME's bytes go least significant bit first; read most significant first, ZCZC is SYNC.
        let on_air: Vec<u8> = b"ZCZC".iter().map(|c| c.reverse_bits()).collect();
        assert_eq!(on_air, SYNC);
    }

    #[test]
    fn rejects_noise_and_long_text() {
        assert_eq!(run(&[0x1234, 0xFFFF, 0x0000, 0x5555]).0, Step::Bad);
        // Printable but never ending: too long.
        let mut long = b"ZCZC-WXR-TOR".to_vec();
        long.extend(std::iter::repeat_n(b'-', MAX_LEN));
        let o = Orient::ALL[0];
        assert_eq!(run(&words(&long, o)).0, Step::Bad);
        // A bit error that makes an unprintable character: '1' (data byte 12, word 6's high byte, sent
        // bit-reversed) loses its 0x20 bit.
        let mut ws = words(HDR, o);
        ws[6] ^= 1 << (15 - 5);
        assert_eq!(run(&ws).0, Step::Bad);
    }

    #[test]
    fn three_bursts_one_alert() {
        let mut r = Repeats::new();
        assert!(r.take(HDR, 100).is_some());
        assert!(r.take(HDR, 101).is_none(), "the second burst");
        assert!(r.take(HDR, 102).is_none(), "the third");
        assert!(r.take(b"ZCZC-WXR-SVR-039173+0030-1591829-KCLE/NWS-", 103).is_some(), "another header");
        assert!(r.take(HDR, 100 + REPEAT_S + 5).is_some(), "the same text much later is new");
        assert!(r.take(b"ZCZC-WXR-TOR-03917", 900).is_none(), "doesn't parse");
    }

    /// A default assembler is a new one: started, it takes a header whole.
    #[test]
    fn default_assembler_is_new() {
        let mut a = Assembler::default();
        a.start();
        let mut step = Step::More;
        for w in words(HDR, Orient::ALL[0]) {
            step = a.push(w);
            if step != Step::More {
                break;
            }
        }
        assert_eq!(step, Step::Done);
        assert_eq!(a.text(), HDR);
    }
}
