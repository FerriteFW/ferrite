//! The decoder slot. The radio runs one decoder at a time, so the active one lives in a single static of this
//! enum and every decoder shares its memory: the slot is as large as the largest. FT8/FT4 (uft48) and WSPR
//! (uwspr), about 12.75 KB each, join when their audio path is in (milestone 6); the budget below already holds
//! them.

use udsc::Receiver as Dsc;
use uafsk::Demodulator as Afsk;

/// Audio rate the DSP task feeds the decoders.
pub const RATE_HZ: u32 = 24_000;

/// RAM the slot may take: FT8's or WSPR's footprint.
pub const BUDGET: usize = 13 * 1024;

#[allow(clippy::large_enum_variant)] // the slot is as large as its largest decoder by design
pub enum Decoder {
    Off,
    /// APRS and other 1200-baud AFSK packet (uaprs parses the frames).
    Afsk(Afsk),
    /// Marine DSC, channel 70.
    Dsc(Dsc),
    /// SSTV pictures (chapter 27a), while the SSTV screen is open.
    Sstv(usstv::Decoder),
    /// ACARS (ACARS-11), while the ACARS screen is open and the radio tuned for it.
    Acars(crate::acarsrx::Demod),
}

/// A screen that takes the slot while it's open, the other decoders waiting (SSTV-1, ACARS-11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Claim {
    None,
    Sstv,
    Acars,
}

/// What a decoder found, handed to the event bus.
pub enum Found<'a> {
    Frame(&'a [u8]),
    /// An FX.25 block, collected but not corrected (APRS-44): too slow for the DSP task, so the APRS task
    /// corrects it ([`uafsk::Fx25Block::decode`]).
    Fx25(&'a uafsk::Fx25Block),
    /// A frame heard but not decoded (APRS-56): its FCS failed on every slicer.
    Miss(&'a uafsk::Miss),
    Call(&'a udsc::Call<'a>),
    /// SSTV-2: a picture started (its VIS code), a line of it, or its end (whole or not).
    SstvStart(u8),
    SstvLine(u16, &'a [u16; usstv::mode::WIDTH]),
    SstvEnd(bool),
    /// ACARS-11: a frame's bytes after SOH, up to its CRC.
    Acars(&'a [u8]),
}

/// The SSTV decoder's lines, to the slot's caller.
struct SstvOut<'f, F: FnMut(Found)>(&'f mut F);

impl<F: FnMut(Found)> usstv::Sink for SstvOut<'_, F> {
    fn start(&mut self, mode: &'static usstv::Mode) {
        (self.0)(Found::SstvStart(mode.vis));
    }
    fn line(&mut self, y: u16, px: &[u16; usstv::mode::WIDTH]) {
        (self.0)(Found::SstvLine(y, px));
    }
    fn end(&mut self, complete: bool) {
        (self.0)(Found::SstvEnd(complete));
    }
}

impl Decoder {
    pub fn afsk() -> Self {
        Decoder::Afsk(Afsk::new(uafsk::fsk::Config::new(RATE_HZ)))
    }


    pub fn dsc() -> Self {
        Decoder::Dsc(Dsc::new(uafsk::fsk::Config::new(RATE_HZ)))
    }

    pub fn sstv() -> Self {
        Decoder::Sstv(usstv::Decoder::new(RATE_HZ))
    }

    /// Which screen has the slot now.
    pub fn claim(&self) -> Claim {
        match self {
            Decoder::Sstv(_) => Claim::Sstv,
            Decoder::Acars(_) => Claim::Acars,
            _ => Claim::None,
        }
    }

    /// Puts a new decoder in the slot: SSTV's or ACARS's while its screen has it (SSTV-1, ACARS-11), else `kind`. In a frame of its own:
    /// a decoder is built by value, as large as the slot, and built inline in the DSP task's poll it made that frame
    /// 12.9 KB on every poll, on top of what the DSP's interrupt preempts. Black faulted receiving SSTV (the stack ran
    /// into the statics, Nov 17, 2026); here the temporary (9.6 KB for SSTV's) is on the stack only while the slot
    /// changes, not on every block of audio.
    #[inline(never)]
    pub fn set(&mut self, claim: Claim, kind: crate::msg::DecoderKind) {
        use crate::msg::DecoderKind;
        *self = Decoder::Off; // the old one dropped first (nothing of it is kept)
        match (claim, kind) {
            (Claim::Sstv, _) => *self = Decoder::sstv(),
            (Claim::Acars, _) => *self = Decoder::Acars(crate::acarsrx::Demod::new(RATE_HZ)),
            (Claim::None, DecoderKind::Off) => {}
            (Claim::None, DecoderKind::Afsk) => *self = Decoder::afsk(),
            (Claim::None, DecoderKind::Dsc) => *self = Decoder::dsc(),
        }
    }

    /// SSTV-2.5: a mode started by hand (its VIS code; 0 stops the picture being received).
    pub fn sstv_force(&mut self, vis: u8, mut found: impl FnMut(Found)) {
        if let Decoder::Sstv(d) = self {
            let mut out = SstvOut(&mut found);
            match usstv::Mode::by_vis(vis) {
                Some(m) => d.force(m, &mut out),
                None => d.stop(&mut out),
            }
        }
    }

    /// The AFSK demodulator's carrier detect: 1200-baud data heard now (APRS-56.4); false for the others.
    pub fn busy(&self) -> bool {
        matches!(self, Decoder::Afsk(d) if d.busy())
    }

    /// One block of audio.
    pub fn push(&mut self, block: &[i16], mut found: impl FnMut(Found)) {
        match self {
            Decoder::Off => {}
            Decoder::Afsk(d) => {
                block.iter().for_each(|&x| d.push(x, |f| found(Found::Frame(f))));
                // The demodulator's one block buffer: handed over (the DSP task copies it into APRS_RX's slot,
                // which a plain frame's size already sets), then freed for the next tag.
                if let Some(b) = d.pending_fx25() {
                    found(Found::Fx25(b));
                    d.release_fx25();
                }
                if let Some(m) = d.take_miss() {
                    found(Found::Miss(&m)); // a miss's window (32 bits) is longer than a block: one at most
                }
            }
            Decoder::Dsc(d) => block.iter().for_each(|&x| d.push(x, |c| found(Found::Call(c)))),
            Decoder::Sstv(d) => d.feed(block, &mut SstvOut(&mut found)),
            Decoder::Acars(d) => d.feed(block, |f| found(Found::Acars(f))),
        }
    }
}

const _: () = assert!(core::mem::size_of::<Decoder>() <= BUDGET);

/// A pager message's characters kept: the event log's text field holds 64.
pub const PAGE_TEXT: usize = 64;
/// The largest capcode: POCSAG addresses are 21 bits.
pub const CAPCODE_MAX: u32 = (1 << 21) - 1;

/// A decoded page, small enough to cross from the DSP task to the UI task (DEC-4, DEC-7): 72 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    /// The 21-bit address (capcode).
    pub capcode: u32,
    pub baud: u16,
    /// Function bits, 0-3.
    pub function: u8,
    /// Read as numeric characters (else 7-bit text).
    pub numeric: bool,
    pub len: u8,
    pub text: [u8; PAGE_TEXT],
}

impl Page {
    /// A message's text, read the way it most likely is: function 0 as numeric digits (the usual convention),
    /// others as 7-bit characters when nine in ten of them are printable (control characters and the padding
    /// senders end with dropped), else as digits after all (systems differ: upocsag's `Message`). A tone-only
    /// page has no text.
    pub fn from(m: &upocsag::Message) -> Page {
        let mut p = Page { capcode: m.address, baud: m.baud, function: m.function, numeric: false, len: 0, text: [0; PAGE_TEXT] };
        let (mut n, mut ok) = (0usize, 0usize);
        for c in m.alpha() {
            n += 1;
            ok += (0x20..0x7F).contains(&c) as usize;
        }
        let mut put = |c: u8| {
            if (p.len as usize) < PAGE_TEXT {
                p.text[p.len as usize] = c;
                p.len += 1;
            }
        };
        if m.function != 0 && n > 0 && ok * 10 >= n * 9 {
            m.alpha().filter(|c| (0x20..0x7F).contains(c)).for_each(&mut put);
        } else {
            p.numeric = true;
            m.numeric().for_each(&mut put);
        }
        while p.len > 0 && p.text[p.len as usize - 1] == b' ' {
            p.len -= 1; // numeric pages end padded with spaces
        }
        p
    }

    pub fn text(&self) -> &[u8] {
        &self.text[..self.len as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page through the codec and back, as both kinds.
    #[test]
    fn page_text() {
        extern crate std;
        use std::vec::Vec;
        for (function, content, want, numeric) in [
            (3, upocsag::encode::Content::Alpha(b"STN 4 RESPOND HWY 417"), &b"STN 4 RESPOND HWY 417"[..], false),
            (0, upocsag::encode::Content::Numeric(b"5551234"), &b"5551234"[..], true),
        ] {
            let mut words = Vec::new();
            upocsag::encode::encode(1_234_567, function, content, |w| words.push(w));
            let mut d: upocsag::decode::Decoder = upocsag::decode::Decoder::new();
            d.baud = 1200;
            let mut got = None;
            for w in words {
                if w == upocsag::SYNC {
                    d.sync(); // a batch starts
                } else {
                    d.word(w, |m| got = Some(Page::from(m)));
                }
            }
            d.end(|m| got = got.or(Some(Page::from(m))));
            let p = got.expect("decoded");
            assert_eq!((p.capcode, p.function, p.baud, p.numeric), (1_234_567, function, 1200, numeric));
            assert_eq!(p.text(), want);
        }
    }

    #[test]
    fn slot_size() {
        let n = core::mem::size_of::<Decoder>();
        std::println!("decoder slot: {n} B");
        assert!(n <= BUDGET);
    }

    /// The other slots: Off takes audio and does nothing; DSC and AFSK decode nothing from silence.
    #[test]
    fn other_slots_on_silence() {
        let mut hits = 0;
        for mut d in [Decoder::Off, Decoder::dsc(), Decoder::afsk()] {
            d.push(&[0; 2400], |_| hits += 1);
        }
        assert_eq!(hits, 0);
    }

    /// DEC-4: a text page longer than the event log's field keeps its first [`PAGE_TEXT`] characters.
    #[test]
    fn long_page_cut_to_the_field() {
        extern crate std;
        use std::vec::Vec;
        let text: Vec<u8> = (0..90u8).map(|i| b'A' + i % 26).collect();
        let mut words = Vec::new();
        upocsag::encode::encode(42, 3, upocsag::encode::Content::Alpha(&text), |w| words.push(w));
        let mut d: upocsag::decode::Decoder = upocsag::decode::Decoder::new();
        d.baud = 1200;
        let mut got = None;
        for w in words {
            if w == upocsag::SYNC {
                d.sync();
            } else {
                d.word(w, |m| got = Some(Page::from(m)));
            }
        }
        d.end(|m| got = got.or(Some(Page::from(m))));
        let p = got.expect("decoded");
        assert!(!p.numeric);
        assert_eq!(p.text(), &text[..PAGE_TEXT]);
    }
}
