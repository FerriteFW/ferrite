//! POCSAG through the BK4829's own FSK modem (DEC-4, DEC-23, decision 453): the modem finds the sync codeword and
//! hands over each batch's 64 bytes as 32 FIFO words; this turns them into codewords and pages. Pure, so the host
//! tests run it; the Radio task feeds it ([`PagerRx::sync`], [`PagerRx::word`], [`PagerRx::end`]).

use crate::decoders::Page;
use upocsag::decode::Decoder;

/// A batch after its sync codeword: 16 codewords of 4 bytes.
pub const BATCH_BYTES: u16 = 64;
/// The modem's frame: the first batch, then 29 more each behind its own sync codeword (the modem doesn't find a
/// sync with no preamble before it quickly enough after a frame ends, bench Nov 24, 2026). The longest the modem
/// takes (2048 bytes); [`PagerRx`] stops at the first missing sync, where the transmission ended.
pub const FRAME_BYTES: u16 = BATCH_BYTES + 29 * (BATCH_BYTES + 4);
/// A sync codeword with more bits wrong than this isn't one.
const SYNC_ERRS: u32 = 4;
/// The sync codeword, as the modem's four sync bytes (byte 0 first on the air).
pub const SYNC_BYTES: [u8; 4] = [0x7C, 0xD2, 0x15, 0xD8];
/// The speeds 0 4 2 5 offers (DEC-23): the chip's FSK 1.2K and 2.4K. 512 is not a mode of the modem, and ham
/// paging (DAPNET) is 1200.
pub const BAUDS: [u16; 2] = [1200, 2400];

/// One batch's words in, codewords to the decoder, pages out.
pub struct PagerRx {
    dec: Decoder<80>,
    half: Option<u16>,
    /// The batch's codewords so far: decoded once it is whole, in the polarity its idle codewords show (an
    /// inverted codeword is a codeword too, so a lone one can't tell; the modem's REG_0B flag didn't, on the bench).
    batch: [u32; 16],
    n: u8,
    /// Words of the sync codeword between batches still to come (2, 1), the first's, and whether the frame has
    /// ended (no sync where one belongs: the rest of the frame is noise).
    sync_left: u8,
    sync_hi: u16,
    over: bool,
    /// The last batch's polarity, for a batch with no idle codeword either way.
    neg: bool,
}

impl Default for PagerRx {
    fn default() -> Self {
        Self::new(1200)
    }
}

impl PagerRx {
    pub const fn new(baud: u16) -> Self {
        let mut dec = Decoder::new();
        dec.baud = baud;
        PagerRx { dec, half: None, batch: [0; 16], n: 0, sync_left: 0, sync_hi: 0, over: false, neg: false }
    }

    pub fn baud(&self) -> u16 {
        self.dec.baud
    }

    /// The modem found a sync codeword: a batch starts (one cut short is decoded as far as it came).
    pub fn sync(&mut self, mut found: impl FnMut(Page)) {
        self.flush(&mut found);
        self.dec.sync();
        (self.half, self.sync_left, self.over) = (None, 0, false);
    }

    /// The next FIFO word as the modem gives it: the first byte off the air in its low half (bench, Nov 24, 2026:
    /// the idle codeword 7A 89 C1 97 came as 0x897A, 0x97C1), so a codeword is two words, each byte-swapped.
    pub fn word(&mut self, w: u16, mut found: impl FnMut(Page)) {
        if self.over {
            return;
        }
        if self.sync_left > 0 {
            // the sync codeword between two batches, either way up
            self.sync_left -= 1;
            if self.sync_left == 1 {
                self.sync_hi = w;
                return;
            }
            let cw = (self.sync_hi.swap_bytes() as u32) << 16 | w.swap_bytes() as u32;
            if (cw ^ upocsag::SYNC).count_ones().min((!cw ^ upocsag::SYNC).count_ones()) > SYNC_ERRS {
                self.over = true; // the transmission ended: what follows is noise
                self.dec.end(|m| found(Page::from(m)));
            } else {
                self.dec.sync();
            }
            return;
        }
        let Some(hi) = self.half.take() else {
            self.half = Some(w);
            return;
        };
        self.batch[self.n as usize] = (hi.swap_bytes() as u32) << 16 | w.swap_bytes() as u32;
        self.n += 1;
        if self.n as usize == self.batch.len() {
            self.flush(&mut found);
            self.sync_left = 2;
        }
    }

    /// The batch's codewords to the decoder, the right way up.
    fn flush(&mut self, found: &mut impl FnMut(Page)) {
        let words = &self.batch[..self.n as usize];
        let idle = |cw: u32| upocsag::bch::correct(cw).is_some_and(|(c, _)| c == upocsag::IDLE);
        let (plain, inv) = (words.iter().filter(|&&c| idle(c)).count(), words.iter().filter(|&&c| idle(!c)).count());
        if plain != inv {
            self.neg = inv > plain;
        }
        for i in 0..self.n as usize {
            let cw = if self.neg { !self.batch[i] } else { self.batch[i] };
            // all zeros (or ones) is what a stalled modem's FIFO reads back, and it passes the check as address 0:
            // taken as idle, never as a page
            let cw = if cw == 0 || cw == u32::MAX { upocsag::IDLE } else { cw };
            self.dec.word(cw, |m| found(Page::from(m)));
        }
        self.n = 0;
    }

    /// The frame ran past the transmission's end: no sync codeword where the next batch's belongs.
    pub fn over(&self) -> bool {
        self.over
    }

    /// The transmission ended (no sync for a batch's time): the page in progress, if any.
    pub fn end(&mut self, mut found: impl FnMut(Page)) {
        self.flush(&mut found);
        self.half = None;
        self.dec.end(|m| found(Page::from(m)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    /// The words a modem would hand over for `codewords` (after the preamble), batch by batch at each sync; `inv`
    /// flips every bit, as a sender of the other polarity.
    fn run(codewords: &[u32], inv: bool, baud: u16) -> Vec<Page> {
        let mut rx = PagerRx::new(baud);
        let mut out = Vec::new();
        let mut first = true;
        // the modem: its sync at the first sync codeword, then the rest of the frame as words, syncs included,
        // and noise after the transmission
        let noise = [0x1234_5678u32, 0x9ABC_DEF0, 0x0F0F_0F0F, 0xDEAD_BEEF];
        for &cw in codewords.iter().chain(noise.iter().cycle().take(40)) {
            if cw == upocsag::SYNC && first {
                first = false;
                rx.sync(|p| out.push(p));
                continue;
            }
            let cw = if inv { !cw } else { cw };
            // as the FIFO hands them over: each word's first air byte low
            rx.word(((cw >> 16) as u16).swap_bytes(), |p| out.push(p));
            rx.word((cw as u16).swap_bytes(), |p| out.push(p));
        }
        rx.end(|p| out.push(p));
        out
    }

    fn page(address: u32, function: u8, content: upocsag::encode::Content) -> Vec<u32> {
        let mut w = Vec::new();
        upocsag::encode::encode(address, function, content, |x| w.push(x));
        w
    }

    /// DEC-23: a page over two batches comes out whole, at either speed, in either polarity.
    #[test]
    fn batches_to_pages_either_polarity() {
        let text = b"FERRITE PAGER TEST 1200";
        let w = page(1_234_567, 3, upocsag::encode::Content::Alpha(text));
        assert!(w.iter().filter(|&&x| x == upocsag::SYNC).count() >= 2, "the address in frame 7: two batches");
        for inv in [false, true] {
            for baud in BAUDS {
                let p = run(&w, inv, baud);
                assert_eq!(p.len(), 1, "inv {inv}");
                assert_eq!((p[0].capcode, p[0].function, p[0].baud), (1_234_567, 3, baud));
                assert_eq!(&p[0].text[..p[0].len as usize], text);
            }
        }
        let n = page(42, 0, upocsag::encode::Content::Numeric(b"5551234"));
        let p = run(&n, false, 1200);
        assert_eq!((p[0].capcode, p[0].numeric, &p[0].text[..p[0].len as usize]), (42, true, &b"5551234"[..]));
    }

    /// A stalled FIFO's zero words make no page.
    #[test]
    fn zero_words_make_no_page() {
        let mut rx = PagerRx::new(1200);
        let mut out = Vec::new();
        rx.sync(|p| out.push(p));
        for _ in 0..64 {
            rx.word(0, |p| out.push(p));
        }
        rx.end(|p| out.push(p));
        assert!(out.is_empty());
    }

    /// Two bit errors in a codeword are corrected; a batch cut short ends the page with what came.
    #[test]
    fn errors_corrected() {
        let mut w = page(1_234_567, 3, upocsag::encode::Content::Alpha(b"HELLO WORLD"));
        let i = w.iter().position(|&x| x >> 31 == 1).expect("a message codeword");
        w[i] ^= 0x0000_0101;
        let p = run(&w, false, 1200);
        assert_eq!(&p[0].text[..p[0].len as usize], b"HELLO WORLD");
    }
}
