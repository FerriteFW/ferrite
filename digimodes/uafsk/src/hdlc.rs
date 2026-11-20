//! HDLC framing as AX.25 packet radio uses it: frames between `0x7E` flags, bits sent least significant first, a
//! zero stuffed after five ones in a row, and a CRC-16/X.25 frame check sequence (FCS) at the end.
//!
//! These work on bits *after* NRZI decoding (receive) or *before* NRZI encoding (transmit); the modem does NRZI.

use udsp::crc::{crc16_x25, crc16_x25_update, CRC16_X25_GOOD};

/// Longest frame accepted, FCS included: AX.25 allows 10 addresses (70 bytes), control and PID (2), 256 bytes of
/// information and the FCS (2).
pub const MAX_FRAME: usize = 330;
/// Shortest: two addresses (14), control (1), FCS (2).
pub const MIN_FRAME: usize = 17;

/// Most low-confidence symbols kept for repair.
pub const MAX_REPAIR: usize = 4;

/// Bits in, checked frames out. RAM: the frame buffer, 330 bytes.
pub struct Deframer {
    buf: [u8; MAX_FRAME],
    len: usize,
    byte: u8,
    nbits: u8,
    ones: u8,
    in_frame: bool,
    /// Repair: how many of the least confident symbols to try flipping (0 = off).
    k: u8,
    /// The previous symbol: confidence (u16::MAX: not a candidate) and the data bit it decided.
    pend: (u16, u16),
    /// The least confident symbols so far: (confidence, first data bit), sorted, unused = u16::MAX confidence.
    low: [(u16, u16); MAX_REPAIR],
    repaired: bool,
    /// A frame that ended on a flag, aligned and long enough, but failed its FCS (and any repair): its length,
    /// FCS included, until [`Deframer::take_bad`] (0: none).
    bad: u16,
}

impl Deframer {
    pub const fn new() -> Self {
        Deframer {
            buf: [0; MAX_FRAME],
            len: 0,
            byte: 0,
            nbits: 0,
            ones: 0,
            in_frame: false,
            k: 0,
            pend: (u16::MAX, 0),
            low: [(u16::MAX, 0); MAX_REPAIR],
            repaired: false,
            bad: 0,
        }
    }

    /// Try flipping up to `k` (0 to [`MAX_REPAIR`]) least confident symbols of a frame that fails its FCS.
    pub fn set_repair(&mut self, k: u8) {
        self.k = k.min(MAX_REPAIR as u8);
    }

    /// The frame that just failed its FCS (and any repair), FCS included, once: read it straight after the bit
    /// that ended it (the next frame's bits overwrite it). For counting and logging what wasn't decoded.
    #[inline(always)]
    pub fn take_bad(&mut self) -> Option<&[u8]> {
        let n = core::mem::take(&mut self.bad) as usize;
        (n != 0).then(|| &self.buf[..n])
    }

    /// The last frame returned needed a repair.
    pub fn repaired(&self) -> bool {
        self.repaired
    }

    /// As [`Deframer::push_fcs`], with the confidence of the tone decision behind this bit (larger is surer).
    #[inline(always)]
    pub fn push_soft(&mut self, bit: u8, conf: u32) -> Option<(&[u8], u16)> {
        if self.k > 0 && self.in_frame {
            self.soft(bit, conf);
        }
        self.push_fcs(bit)
    }

    /// Repair bookkeeping for one bit: the previous symbol (whose tone decided the previous bit and this one)
    /// becomes a candidate, unless either bit is a stuffed zero (confidence u16::MAX: never a candidate).
    #[inline(never)]
    fn soft(&mut self, bit: u8, conf: u32) {
        if bit == 0 && self.ones == 5 {
            self.pend.0 = u16::MAX;
            return;
        }
        let (c, i) = self.pend;
        let k = self.k as usize;
        if c < self.low[k - 1].0 {
            let mut j = k - 1;
            while j > 0 && self.low[j - 1].0 > c {
                self.low[j] = self.low[j - 1];
                j -= 1;
            }
            self.low[j] = (c, i);
        }
        self.pend = (conf.min(u16::MAX as u32 - 1) as u16, (self.len * 8 + self.nbits as usize) as u16);
    }

    /// The frame in `buf[..len]` failed its FCS: flip combinations of the least confident symbols (each two
    /// adjacent data bits), fewest first, keeping only flips that leave the bit stuffing as it was.
    fn repair(&mut self, len: usize) -> bool {
        let nbits = len * 8;
        let mut cand = [0u16; MAX_REPAIR];
        let mut delta = [0u16; MAX_REPAIR];
        let mut n = 0;
        for &(c, i) in &self.low[..self.k as usize] {
            let i = i as usize;
            if c == u16::MAX || i + 1 >= nbits || run(&self.buf, nbits, i) >= 5 || run(&self.buf, nbits, i + 1) >= 5 {
                continue;
            }
            cand[n] = i as u16;
            // The CRC register's change from flipping data bits i and i+1: the CRC is linear.
            let mut d: u16 = 0;
            for step in i..nbits {
                if step <= i + 1 {
                    d ^= 1;
                }
                d = if d & 1 != 0 { (d >> 1) ^ 0x8408 } else { d >> 1 };
            }
            delta[n] = d;
            n += 1;
        }
        let syn = crc16_x25_update(0xFFFF, &self.buf[..len]) ^ CRC16_X25_GOOD;
        for size in 1..=n {
            for mask in 1u32..(1 << n) {
                if mask.count_ones() as usize != size {
                    continue;
                }
                let x = (0..n).filter(|&j| mask >> j & 1 != 0).fold(0, |a, j| a ^ delta[j]);
                if x != syn {
                    continue;
                }
                for j in (0..n).filter(|&j| mask >> j & 1 != 0) {
                    flip(&mut self.buf, cand[j] as usize);
                    flip(&mut self.buf, cand[j] as usize + 1);
                }
                let ok = (0..n).filter(|&j| mask >> j & 1 != 0).all(|j| {
                    let i = cand[j] as usize;
                    run(&self.buf, nbits, i) < 5 && run(&self.buf, nbits, i + 1) < 5
                });
                if ok && ax25_addresses_ok(&self.buf[..len - 2]) {
                    return true;
                }
                // The first combination that fixes the FCS is the likeliest; if it breaks the stuffing or the
                // addresses, the frame stays bad (trying on would only raise the odds of a false match).
                for j in (0..n).filter(|&j| mask >> j & 1 != 0) {
                    flip(&mut self.buf, cand[j] as usize);
                    flip(&mut self.buf, cand[j] as usize + 1);
                }
                return false;
            }
        }
        false
    }

    /// Feed one decoded bit (0 or 1). Returns a frame, without its FCS, when one ends with a correct FCS.
    pub fn push(&mut self, bit: u8) -> Option<&[u8]> {
        self.push_fcs(bit).map(|(frame, _)| frame)
    }

    /// As [`Deframer::push`], with the frame's FCS (a cheap key for spotting the same frame twice).
    #[inline(always)]
    pub fn push_fcs(&mut self, bit: u8) -> Option<(&[u8], u16)> {
        if bit != 0 {
            // Counts up to 7 at most: a steady tone sends ones for ever.
            self.ones = (self.ones + 1).min(7);
            if self.ones > 6 {
                // Seven or more ones: abort (or idle line). Wait for the next flag.
                self.in_frame = false;
                return None;
            }
        } else {
            match self.ones {
                5 => {
                    // A stuffed zero: drop it.
                    self.ones = 0;
                    return None;
                }
                6 => return self.flag(),
                _ => self.ones = 0,
            }
        }
        if self.in_frame {
            self.byte = (self.byte >> 1) | (bit << 7);
            self.nbits += 1;
            if self.nbits == 8 {
                if self.len == MAX_FRAME {
                    self.in_frame = false;
                    return None;
                }
                self.buf[self.len] = self.byte;
                self.len += 1;
                self.nbits = 0;
            }
        }
        None
    }

    /// A flag (0 1 1 1 1 1 1 0) ends any frame. Kept out of line: the per-bit path stays small.
    #[inline(never)]
    fn flag(&mut self) -> Option<(&[u8], u16)> {
        // The data bits added since the last byte are the flag's leading 0 and six 1s, so an aligned frame has
        // exactly 7 of them.
        self.ones = 0;
        let done = self.in_frame && self.nbits == 7 && self.len >= MIN_FRAME;
        let len = self.len;
        let mut good = done && crc16_x25_update(0xFFFF, &self.buf[..len]) == CRC16_X25_GOOD;
        self.repaired = done && !good && self.k > 0 && self.repair(len);
        good |= self.repaired;
        self.bad = if done && !good { len as u16 } else { 0 };
        self.start();
        if good {
            let fcs = u16::from_le_bytes([self.buf[len - 2], self.buf[len - 1]]);
            return Some((&self.buf[..len - 2], fcs));
        }
        None
    }

    fn start(&mut self) {
        self.in_frame = true;
        self.len = 0;
        self.nbits = 0;
        self.pend = (u16::MAX, 0);
        self.low = [(u16::MAX, 0); MAX_REPAIR];
    }
}

/// AX.25 address field sanity: 2 to 10 addresses of 7 bytes, the extension bit (bit 0) clear except on the last
/// address's final byte, callsign characters shifted ASCII A-Z, 0-9 or space. Repaired frames must pass it.
pub fn ax25_addresses_ok(f: &[u8]) -> bool {
    let mut n = 0;
    loop {
        let Some(a) = f.get(n * 7..n * 7 + 7) else { return false };
        if !a[..6].iter().all(|&c| c & 1 == 0 && matches!(c >> 1, b'A'..=b'Z' | b'0'..=b'9' | b' ')) {
            return false;
        }
        n += 1;
        if a[6] & 1 == 1 {
            return n >= 2;
        }
        if n == 10 {
            return false;
        }
    }
}

fn bit_at(buf: &[u8], i: usize) -> bool {
    buf[i / 8] >> (i % 8) & 1 != 0
}

fn flip(buf: &mut [u8], i: usize) {
    buf[i / 8] ^= 1 << (i % 8);
}

/// Length of the run of ones through data bit `i` (0 if it is a zero), counting at most 6 each way.
fn run(buf: &[u8], nbits: usize, i: usize) -> usize {
    if !bit_at(buf, i) {
        return 0;
    }
    let mut n = 1;
    let mut j = i;
    while j > 0 && n < 7 && bit_at(buf, j - 1) {
        j -= 1;
        n += 1;
    }
    let mut j = i + 1;
    while j < nbits && n < 13 && bit_at(buf, j) {
        j += 1;
        n += 1;
    }
    n
}

impl Default for Deframer {
    fn default() -> Self {
        Self::new()
    }
}

/// A frame (FCS added here) as bits: `pre` flags, the stuffed frame, `post` flags.
pub struct Framer<'a> {
    frame: &'a [u8],
    fcs: [u8; 2],
    pre: u16,
    post: u16,
    /// Position: flags before, then frame bytes and FCS (index), then flags after.
    stage: u8,
    index: usize,
    bit: u8,
    ones: u8,
}

const FLAG: u8 = 0x7E;

/// `n` HDLC flags (0x7E), least significant bit first, the same bits [`Framer`] emits for its preamble and
/// postamble. A KISS window sends the TX tail as these after the last frame's closing flag.
pub struct FlagBits {
    left: u16,
    bit: u8,
}

impl FlagBits {
    pub const fn new(n: u16) -> Self {
        FlagBits { left: n, bit: 0 }
    }
}

impl Iterator for FlagBits {
    type Item = u8;
    fn next(&mut self) -> Option<u8> {
        if self.left == 0 {
            return None;
        }
        let b = (FLAG >> self.bit) & 1;
        self.bit += 1;
        if self.bit == 8 {
            self.bit = 0;
            self.left -= 1;
        }
        Some(b)
    }
}

impl<'a> Framer<'a> {
    pub fn new(frame: &'a [u8], pre: u16, post: u16) -> Self {
        Framer { frame, fcs: crc16_x25(frame).to_le_bytes(), pre, post, stage: 0, index: 0, bit: 0, ones: 0 }
    }

    fn data_byte(&self) -> Option<u8> {
        if self.index < self.frame.len() {
            Some(self.frame[self.index])
        } else {
            self.fcs.get(self.index - self.frame.len()).copied()
        }
    }
}

impl Iterator for Framer<'_> {
    type Item = u8;
    fn next(&mut self) -> Option<u8> {
        loop {
            match self.stage {
                0 | 2 => {
                    let count = if self.stage == 0 { self.pre } else { self.post };
                    if self.index as u16 >= count {
                        self.stage += 1;
                        self.index = 0;
                        self.bit = 0;
                        if self.stage == 3 {
                            return None;
                        }
                        continue;
                    }
                    let b = (FLAG >> self.bit) & 1;
                    self.bit += 1;
                    if self.bit == 8 {
                        self.bit = 0;
                        self.index += 1;
                    }
                    return Some(b);
                }
                1 => {
                    if self.ones == 5 {
                        self.ones = 0;
                        return Some(0); // stuffed zero
                    }
                    let Some(byte) = self.data_byte() else {
                        self.stage = 2;
                        self.index = 0;
                        self.bit = 0;
                        continue;
                    };
                    let b = (byte >> self.bit) & 1;
                    self.ones = if b == 1 { self.ones + 1 } else { 0 };
                    self.bit += 1;
                    if self.bit == 8 {
                        self.bit = 0;
                        self.index += 1;
                    }
                    return Some(b);
                }
                _ => return None,
            }
        }
    }
}
