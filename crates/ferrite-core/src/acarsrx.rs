//! ACARS from the receiver's audio (ACARS-11, ferrite#192): 2400 bit/s MSK on 1200 and 2400 Hz tones to the frame's
//! bytes after SOH, for [`crate::acars::parse`]. The BK4829's own modem can't take ACARS (it needs an alternating
//! preamble ACARS never sends: bench, Nov 25, 2026), so it is done here, in software, on the decoders' audio.
//!
//! 1. **The bit clock:** the audio mixed down by 1800 Hz (between the tones), a box low-pass of 1.2 bits, and the
//!    phase step from one sample to the next (the cross product: positive above 1800 Hz, so 2400 Hz). Each chain
//!    smooths that over part of a bit, and its clock (a phase accumulator, a turn a bit) is pulled to the bit edges
//!    where the smoothed sign changes: gently, but hard for the first [`FAST_EDGES`] changes after [`QUIET_BITS`]
//!    without one (the pre-key is one steady tone: without the fast pull the clock was still finding the edges
//!    when the sync came, and half the lost bursts were that).
//! 2. **Bits from the phase:** MSK's phase moves a quarter turn a bit, up on 2400 Hz and down on 1200 Hz, so at each
//!    bit edge the phase, less k quarter turns, sits at one of two opposite points, and which says the data bit
//!    (the tones' NRZI, read without summing them: a tone misread costs one bit, not the rest of the frame). The
//!    reference point is a running average of the edges' phase, its sign taken off. Which of the two points is 1
//!    isn't known: the sync says.
//! 3. **Chains:** [`CHAINS`] of them, two smoothings by two clock pulls, each with its own frame reader. Chosen on
//!    black's recordings of the bladeRF (`bench-results/20261125-acars-rx/msklab5.py`, Nov 25, 2026): 0 bursts of
//!    30 with an error, against 13 for tone decisions and NRZI with the gentle clock alone.
//! 4. **Sync:** `*`, SYN, SYN as 24 data bits (each character 7 bits and odd parity, least significant first), either
//!    polarity; then SOH, or the hunt goes on. A reader reading a body keeps watching too (ACARS-11.2): the sync
//!    within [`SYNC_AFTER`] bits of a pre-key (at least [`PREKEY_BITS`] of the sync's polarity's ones) means the body
//!    was cut short and a new burst starts. A body can't hold that run: 7 bits and odd parity allow at most 14 equal
//!    bits in a row, and the CRC's two free bytes come last.
//! 5. **The frame:** characters after SOH up to ETX or ETB and the two CRC bytes ([`crate::acars`]'s shape: STX
//!    or the end straight after the 12-character head); anything longer than [`FRAME_MAX`] is dropped.
//! 6. **One copy:** every chain that gets the frame finishes within a few bits of the others; the first copy waits
//!    [`HOLD_BITS`] for one whose CRC passes, and that (or the first) is given once.

use crate::acars::{crc16, ETB, ETX, STX, TEXT_MAX};
use udsp::trig::{cos_sin, phase_step};

/// The low-pass after the mixer, thousandths of a bit.
const LP_MBITS: u32 = 1200;
/// The chains: smoothings (thousandths of a bit) by clock pulls (a right shift of the phase error). `static`s: the
/// DSP task reads them (an unnamed constant table would sit in the firmware's slow flash).
static SMOOTH_MBITS: [u32; 2] = [700, 1100];
static PULL_SHIFT: [u8; 2] = [3, 2];
pub const CHAINS: usize = 4;
const _: () = assert!(CHAINS == 2 * 2, "CHAINS is SMOOTH_MBITS x PULL_SHIFT");
/// The longest box, samples (1.2 bits at 48 kHz).
const BOX_MAX: usize = 24;
/// The clock's hard pull: for this many sign changes after [`QUIET_BITS`] bits without one.
const FAST_EDGES: u8 = 8;
const QUIET_BITS: u8 = 12;
/// The mixed-down samples kept, for each chain to read the edge's (its smoothing's delay back).
const HIST: usize = 16;
/// The phase reference's averaging: a right shift (a quarter of each new edge).
const REF_SHIFT: u8 = 2;
/// The head after SOH: mode, registration (7), acknowledgement, label (2), block ID.
const HEAD: usize = 12;
/// The longest frame after SOH: the head, STX, the text, ETX or ETB, the CRC.
pub const FRAME_MAX: usize = HEAD + 1 + TEXT_MAX + 1 + 2;
/// `*`, SYN, SYN as received, the first character in the low byte.
const SYNC: u32 = 0x2A | 0x16 << 8 | 0x16 << 16;
const SOH: u8 = 0x01;
/// The pre-key a reader needs before a sync mid-body: equal data bits in a row (the pre-key is 16 or more characters
/// of ones; a body has at most 14 in a row).
const PREKEY_BITS: u8 = 32;
/// How soon after the pre-key's run the sync must end, bits: the run ends two bits into `+`, the sync 30 bits later.
const SYNC_AFTER: u8 = 40;
/// How long the first copy of a frame waits for a slicer's copy with a good CRC, in bits.
pub const HOLD_BITS: u32 = 48;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Hunt,
    Soh,
    Body,
}

/// One slicer's frame reader.
#[derive(Clone, Copy)]
struct Deframer {
    /// The last 24 data bits, the newest in bit 23 (kept in every state).
    shift: u32,
    /// The run of equal data bits: its length (saturating) and its bit.
    run: u8,
    run_bit: bool,
    /// Bits since the last run of [`PREKEY_BITS`] or more, and that run's bit (saturating).
    since_pk: u8,
    pk_bit: bool,
    /// The stream's data is inverted (the sync came inverted).
    inv: bool,
    state: State,
    /// Bits of the character being read, and it so far.
    nbit: u8,
    cur: u8,
    /// Characters after SOH, and where the frame's ETX or ETB is (0: not yet).
    n: u16,
    end: u16,
    buf: [u8; FRAME_MAX],
}

impl Deframer {
    const NEW: Deframer = Deframer { shift: 0, run: 0, run_bit: false, since_pk: u8::MAX, pk_bit: false, inv: false, state: State::Hunt, nbit: 0, cur: 0, n: 0, end: 0, buf: [0; FRAME_MAX] };

    /// A data bit (either polarity: the sync says which); true when a frame is complete in `buf[..n]`.
    fn bit(&mut self, d: bool) -> bool {
        self.shift = (self.shift >> 1) | (d as u32) << 23;
        if d == self.run_bit {
            self.run = self.run.saturating_add(1);
        } else {
            (self.run, self.run_bit) = (1, d);
        }
        if self.run >= PREKEY_BITS {
            (self.since_pk, self.pk_bit) = (0, d);
        } else {
            self.since_pk = self.since_pk.saturating_add(1);
        }
        let inv = self.shift != SYNC;
        if inv && self.shift != SYNC ^ 0xFF_FFFF {
            if self.state == State::Hunt {
                return false;
            }
        } else if self.state == State::Hunt || self.since_pk <= SYNC_AFTER && self.pk_bit != inv {
            // a sync: in the hunt, or after a pre-key of its polarity mid-body (the body was cut: a new burst)
            (self.inv, self.state, self.nbit, self.cur) = (inv, State::Soh, 0, 0);
            return false;
        }
        match self.state {
            State::Hunt => false,
            State::Soh | State::Body => {
                self.cur |= ((d ^ self.inv) as u8) << self.nbit;
                self.nbit += 1;
                if self.nbit < 8 {
                    return false;
                }
                let c = self.cur;
                (self.nbit, self.cur) = (0, 0);
                if self.state == State::Soh {
                    if c == SOH {
                        (self.state, self.n, self.end) = (State::Body, 0, 0);
                    } else {
                        self.hunt();
                    }
                    return false;
                }
                self.char(c)
            }
        }
    }

    /// A character after SOH; true when the frame is complete.
    fn char(&mut self, c: u8) -> bool {
        let i = self.n as usize;
        if i >= FRAME_MAX {
            self.hunt();
            return false;
        }
        self.buf[i] = c;
        self.n += 1;
        let ch = c & 0x7F;
        if self.end == 0 && i >= HEAD {
            if i == HEAD && !matches!(ch, STX | ETX | ETB) {
                self.hunt(); // neither text nor the end where the head ends: not a frame
                return false;
            }
            if matches!(ch, ETX | ETB) {
                self.end = i as u16;
            }
        }
        if self.end != 0 && self.n == self.end + 3 {
            self.hunt();
            return true;
        }
        false
    }

    fn hunt(&mut self) {
        self.state = State::Hunt;
    }

    /// The complete frame's bytes (after `bit` said so: `n` and `end` stay until the next SOH).
    fn frame(&self) -> &[u8] {
        &self.buf[..self.n as usize]
    }

    fn crc_ok(&self) -> bool {
        let e = self.end as usize;
        crc16(&self.buf[..=e]) == u16::from_le_bytes([self.buf[e + 1], self.buf[e + 2]])
    }
}

/// A moving sum over `n` samples.
#[derive(Clone, Copy)]
struct Boxcar {
    n: u8,
    pos: u8,
    ring: [i32; BOX_MAX],
    sum: i32,
}

impl Boxcar {
    fn new(n: usize) -> Self {
        Boxcar { n: n.clamp(1, BOX_MAX) as u8, pos: 0, ring: [0; BOX_MAX], sum: 0 }
    }

    #[inline(always)]
    fn push(&mut self, x: i32) -> i32 {
        let p = self.pos as usize;
        self.sum = self.sum + x - self.ring[p];
        self.ring[p] = x;
        self.pos = if p + 1 == self.n as usize { 0 } else { p as u8 + 1 };
        self.sum
    }
}

/// Samples in `mbits` thousandths of a bit at `rate`.
const fn samples(rate: u32, mbits: u32) -> usize {
    ((rate * mbits + 1_200_000) / 2_400_000) as usize
}

/// One chain: the smoothing, the bit clock (0 at the bit edges), the smoothed sign, the clock's pull, the bits
/// since the last sign change and the hard pulls left, and the phase reference with the bit count (its quarter
/// turns).
#[derive(Clone, Copy)]
struct Chain {
    smooth: Boxcar,
    /// The smoothing's delay, samples: the edge's mixed-down sample is this far back.
    delay: u8,
    pull: u8,
    clock: u32,
    above: bool,
    quiet: u8,
    fast: u8,
    r: (i32, i32),
    k: u8,
}

impl Chain {
    /// The discriminator's sample; true at each bit edge.
    #[inline(always)]
    fn push(&mut self, d: i32, step: u32) -> bool {
        let above = self.smooth.push(d) > 0;
        if above != self.above {
            self.above = above;
            if self.quiet > QUIET_BITS {
                self.fast = FAST_EDGES;
            }
            self.quiet = 0;
            // the pull moves the clock towards 0 by a fraction, never across a half turn
            let e = self.clock as i32;
            let pull = if self.fast > 0 {
                self.fast -= 1;
                e - (e >> 2)
            } else {
                e >> self.pull
            };
            self.clock = self.clock.wrapping_sub(pull as u32);
        }
        let before = self.clock;
        self.clock = self.clock.wrapping_add(step);
        if self.clock < before {
            self.quiet = self.quiet.saturating_add(1);
            return true;
        }
        false
    }

    /// The edge's mixed-down sample: its quarter turns taken off, its side of the reference (the data bit, either
    /// polarity), the reference moved towards it.
    #[inline(always)]
    fn decide(&mut self, z: (i32, i32)) -> bool {
        // z times (-j)^k: k quarter turns back
        let w = match self.k & 3 {
            0 => z,
            1 => (z.1, -z.0),
            2 => (-z.0, -z.1),
            _ => (-z.1, z.0),
        };
        self.k = self.k.wrapping_add(1);
        let dot = w.0 as i64 * self.r.0 as i64 + w.1 as i64 * self.r.1 as i64;
        let one = dot < 0;
        let s = if one { (-w.0, -w.1) } else { w };
        self.r = (self.r.0 + ((s.0 - self.r.0) >> REF_SHIFT), self.r.1 + ((s.1 - self.r.1) >> REF_SHIFT));
        one
    }
}

/// The ACARS receiver: samples in, each frame's bytes after SOH out (to [`crate::acars::parse`]).
pub struct Demod {
    nco: u32,
    nco_step: u32,
    bit_step: u32,
    lp: [Boxcar; 2],
    last: (i32, i32),
    /// The last [`HIST`] mixed-down samples, the newest at `at`.
    hist: [(i32, i32); HIST],
    at: u8,
    chains: [Chain; CHAINS],
    readers: [Deframer; CHAINS],
    /// The copy waiting for a better one: its bytes, length, CRC verdict, and the bit count it goes out at.
    held: [u8; FRAME_MAX],
    held_n: u16,
    held_ok: bool,
    held_until: u32,
    /// Bits clocked by chain 0 (the hold's clock).
    bits: u32,
}

impl Demod {
    /// At `rate_hz` (the DSP task's is 24 kHz; 12 to 48 kHz).
    pub fn new(rate_hz: u32) -> Self {
        let r = rate_hz as u64 * 1000;
        let chains = core::array::from_fn(|i| {
            let n = samples(rate_hz, SMOOTH_MBITS[i / PULL_SHIFT.len()]);
            Chain {
                smooth: Boxcar::new(n),
                delay: ((n.clamp(1, BOX_MAX) - 1) / 2).min(HIST - 1) as u8,
                pull: PULL_SHIFT[i % PULL_SHIFT.len()],
                clock: 0,
                above: false,
                quiet: 0,
                fast: 0,
                r: (0, 0),
                k: 0,
            }
        });
        Demod {
            nco: 0,
            nco_step: phase_step(1_800_000, r),
            bit_step: phase_step(2_400_000, r),
            lp: [Boxcar::new(samples(rate_hz, LP_MBITS)); 2],
            last: (0, 0),
            hist: [(0, 0); HIST],
            at: 0,
            chains,
            readers: [Deframer::NEW; CHAINS],
            held: [0; FRAME_MAX],
            held_n: 0,
            held_ok: false,
            held_until: 0,
            bits: 0,
        }
    }

    /// One sample; `found` gets each frame once.
    pub fn push(&mut self, x: i16, mut found: impl FnMut(&[u8])) {
        let (c, s) = cos_sin((self.nco >> 20) as usize);
        self.nco = self.nco.wrapping_add(self.nco_step);
        let x = x as i32;
        let z = (self.lp[0].push((x * c) >> 15), self.lp[1].push(-((x * s) >> 15)));
        // the phase step: Im(z_n conj z_n-1), positive above 1800 Hz
        let d = ((z.1 as i64 * self.last.0 as i64 - z.0 as i64 * self.last.1 as i64) >> 16) as i32;
        self.last = z;
        self.at = (self.at + 1) % HIST as u8;
        self.hist[self.at as usize] = z;
        for k in 0..CHAINS {
            if !self.chains[k].push(d, self.bit_step) {
                continue;
            }
            let back = (self.at as usize + HIST - self.chains[k].delay as usize) % HIST;
            let bit = self.chains[k].decide(self.hist[back]);
            if k == 0 {
                self.bits = self.bits.wrapping_add(1);
            }
            let r = &mut self.readers[k];
            if !r.bit(bit) {
                continue;
            }
            let ok = r.crc_ok();
            if self.held_n == 0 || ok && !self.held_ok {
                let f = r.frame();
                self.held[..f.len()].copy_from_slice(f);
                if self.held_n == 0 {
                    self.held_until = self.bits.wrapping_add(HOLD_BITS);
                }
                (self.held_n, self.held_ok) = (f.len() as u16, ok);
            }
        }
        if self.held_n != 0 && self.bits.wrapping_sub(self.held_until) < u32::MAX / 2 {
            found(&self.held[..self.held_n as usize]);
            self.held_n = 0;
        }
    }

/// Several samples.
    pub fn feed(&mut self, xs: &[i16], mut found: impl FnMut(&[u8])) {
        for &x in xs {
            self.push(x, &mut found);
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::acars::{encode, parse, with_parity, Fields, DEL, NAK, SYN};
    use std::vec::Vec;

    /// A burst as sent: the pre-key's ones, `+`, `*`, SYN, SYN, SOH, the frame, as tone decisions (true: 1200 Hz).
    pub(crate) fn tones(frame: &[u8], prekey: usize) -> Vec<bool> {
        let mut bytes: Vec<u8> = std::vec![0xFF; prekey];
        bytes.extend([with_parity(b'+'), with_parity(b'*'), SYN, SYN, SOH]);
        bytes.extend_from_slice(frame);
        bytes.push(DEL);
        let mut prev = true;
        let mut out = Vec::new();
        for b in bytes {
            for i in 0..8 {
                let d = b >> i & 1 != 0;
                out.push(d != prev); // a change of data bit is 1200 Hz
                prev = d;
            }
        }
        out
    }

    /// MSK audio at `rate`: continuous phase, 1200 or 2400 Hz a bit, amplitude `amp`, with deterministic noise of
    /// `noise` (peak) and silence around it.
    pub(crate) fn audio(t: &[bool], rate: u32, amp: f64, noise: f64) -> Vec<i16> {
        let spb = rate as f64 / 2400.0;
        let n = (t.len() as f64 * spb) as usize;
        let mut ph = 0.0f64;
        let mut rng = 12345u32;
        let pad = rate as usize / 20;
        let mut out = std::vec![0i16; pad];
        for i in 0..n {
            let f = if t[((i as f64) / spb) as usize] { 1200.0 } else { 2400.0 };
            ph += 2.0 * core::f64::consts::PI * f / rate as f64;
            rng = rng.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let r = ((rng >> 16) as f64 / 32768.0 - 1.0) * noise;
            out.push((amp * ph.sin() + r) as i16);
        }
        out.extend(std::vec![0i16; pad]);
        out
    }

    fn sample(text: &[u8], buf: &mut [u8]) -> usize {
        encode(&Fields { mode: b'2', reg: b".C-FTJO", ack: NAK, label: *b"H1", block: b'1', msgno: b"M01A", flight: b"AC0123", text, more: false }, buf)
    }

    fn decode(x: &[i16], rate: u32) -> Vec<Vec<u8>> {
        let mut d = Demod::new(rate);
        let mut got = Vec::new();
        d.feed(x, |f| got.push(f.to_vec()));
        got
    }

    /// ACARS-11: a burst at the DSP task's rate decodes once, byte for byte (the CRC and the fields check), with the
    /// pre-key, the `+*` bit sync and SYN SYN SOH before it; in noise too.
    #[test]
    fn a_burst_decodes_once() {
        let mut b = [0u8; FRAME_MAX + 1];
        let n = sample(b"HELLO FERRITE 0123456789", &mut b);
        for noise in [0.0, 4000.0] {
            let got = decode(&audio(&tones(&b[..n], 16), 24_000, 12_000.0, noise), 24_000);
            assert_eq!(got.len(), 1, "noise {noise}: one frame");
            assert_eq!(&got[0][..], &b[..n - 1], "noise {noise}: the bytes up to the CRC");
            let f = parse(&got[0]).unwrap();
            assert!(f.crc_ok);
            assert_eq!((f.reg(), &f.flight, f.text()), (&b"C-FTJO"[..], b"AC0123", &b"HELLO FERRITE 0123456789"[..]));
        }
    }

    /// ACARS-11: the longest text (220 characters) and a block with none (ETX after the head); bursts back to back;
    /// both NRZI starting points (the data inverted or not).
    #[test]
    fn long_short_and_back_to_back() {
        let mut long = [0u8; FRAME_MAX + 1];
        let text: Vec<u8> = (0..TEXT_MAX - 10).map(|i| b'A' + (i % 26) as u8).collect();
        let nl = sample(&text, &mut long);
        let mut short = [0u8; FRAME_MAX + 1];
        let ns = encode(&Fields { mode: b'2', reg: b"N123AB", ack: NAK, label: *b"Q0", block: b'A', ..Default::default() }, &mut short);
        let mut t = tones(&long[..nl], 16);
        t.extend(tones(&short[..ns], 4));
        let mut inv = tones(&long[..nl], 12);
        inv.iter_mut().take(3).for_each(|x| *x = !*x); // a different start: the rest of the data inverted
        t.extend(inv);
        let got = decode(&audio(&t, 24_000, 10_000.0, 1500.0), 24_000);
        assert_eq!(got.len(), 3);
        assert_eq!(&got[0][..], &long[..nl - 1]);
        assert_eq!(&got[1][..], &short[..ns - 1]);
        assert_eq!(&got[2][..], &long[..nl - 1]);
        assert!(parse(&got[1]).unwrap().crc_ok);
    }

    /// ACARS-11: noise, a steady tone and a burst cut short give nothing.
    #[test]
    fn nothing_from_noise_or_a_cut_burst() {
        let mut b = [0u8; FRAME_MAX + 1];
        let n = sample(b"CUT SHORT", &mut b);
        let t = tones(&b[..n], 16);
        let cut = &t[..t.len() - 40];
        let mut x = audio(cut, 24_000, 12_000.0, 2000.0);
        x.extend(audio(&std::vec![false; 2400], 24_000, 12_000.0, 0.0));
        x.extend(audio(&[], 24_000, 0.0, 12_000.0));
        let mut rng = 7u32;
        x.extend((0..48_000).map(|_| {
            rng = rng.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (rng >> 17) as i16 - 16384
        }));
        assert!(decode(&x, 24_000).is_empty());
    }

    /// ACARS-11 (ferrite#73: a receive path must never stick): after up to a second of any audio (noise at any
    /// level, a steady tone, clipping, DC, bursts cut short or with bits flipped) the receiver decodes the next good
    /// burst: every slicer's reader is back to hunting within a frame's length, and a held copy goes out within
    /// HOLD_BITS. Each case's work is bounded by its samples (no loop over the input but the per-sample one).
    #[test]
    fn fuzz_recovers_after_garbage() {
        use ferrite_fuzzkit::{bytes_to_samples, samples_to_bytes, Fuzz};
        let mut b = [0u8; FRAME_MAX + 1];
        let n = sample(b"FERRITE AFTER GARBAGE", &mut b);
        let good = b[..n].to_vec();
        let burst = audio(&tones(&good, 16), 24_000, 12_000.0, 1500.0);
        Fuzz::new("acars_recovers", 0xACA6, 60).slow_ms(5_000).run(
            |r| {
                let mut a: Vec<i16> = Vec::new();
                while a.len() < 24_000 && !r.one_in(6) {
                    let k = r.range(1, 8000);
                    match r.below(5) {
                        0 => {
                            // a burst cut short, its bits flipped here and there
                            let mut t = tones(&good, r.below(16));
                            let cut = r.range(1, t.len());
                            t.truncate(cut);
                            for _ in 0..r.below(6) {
                                let i = r.below(t.len());
                                t[i] = !t[i];
                            }
                            a.extend(audio(&t, 24_000, r.range(500, 20_000) as f64, r.below(8000) as f64));
                        }
                        1 => {
                            let amp = r.range(1, 32768) as i32;
                            a.extend((0..k).map(|_| ((r.u16() as i16 as i32 * amp) >> 15) as i16));
                        }
                        2 => {
                            let half = r.range(2, 40);
                            a.extend((0..k).map(|i| if (i / half).is_multiple_of(2) { i16::MAX } else { i16::MIN }));
                        }
                        3 => a.extend(core::iter::repeat_n(r.u16() as i16, k)),
                        _ => a.extend(core::iter::repeat_n(0, k)),
                    }
                }
                samples_to_bytes(&a)
            },
            |g| {
                let mut d = Demod::new(24_000);
                d.feed(&bytes_to_samples(g), |f| assert!(f.len() <= FRAME_MAX));
                // a frame's time of quiet: a reader reading a burst cut short reads on until FRAME_MAX characters
                d.feed(&std::vec![0i16; (FRAME_MAX + 4) * 8 * 10], |f| assert!(f.len() <= FRAME_MAX));
                let mut got = Vec::new();
                d.feed(&burst, |f| got.push(f.to_vec()));
                assert!(got.iter().any(|f| f[..] == good[..n - 1]), "a good burst lost after garbage ({} frames)", got.len());
            },
        );
    }

    /// ACARS-11.2, found by fuzz_recovers_after_garbage: a burst cut short (a fade, a collision) leaves each reader
    /// reading its body until FRAME_MAX characters (about 0.8 s); a burst starting inside that time decodes, its
    /// pre-key and sync seen mid-body, wherever the first was cut.
    #[test]
    fn burst_right_after_a_cut_one() {
        let mut b = [0u8; FRAME_MAX + 1];
        let n = sample(b"FERRITE AFTER GARBAGE", &mut b);
        let mut lost = Vec::new();
        let first = tones(&b[..n], 8);
        for cut in (100..first.len()).step_by(5) {
            let mut x = audio(&first[..cut], 24_000, 12_000.0, 1000.0);
            x.extend(audio(&tones(&b[..n], 16), 24_000, 12_000.0, 1000.0));
            if !decode(&x, 24_000).iter().any(|f| f[..] == b[..n - 1]) {
                lost.push(cut);
            }
        }
        assert!(lost.is_empty(), "the second burst lost after the first cut at these bits: {lost:?}");
    }

    /// ACARS-11.2: a body is never abandoned for a sync inside its own text: bodies with the longest runs of equal
    /// bits the characters allow (DEL, SOH, 0x7E and 0x01 pairs) and `+*` SYN SYN SOH in the text, and 200 bodies of
    /// random characters (but ETX and ETB), all decode, alone and back to back.
    #[test]
    fn no_false_sync_mid_body() {
        let mut texts: Vec<Vec<u8>> = std::vec![
            b"\x7f\x7f\x7f\x7f+*\x16\x16\x01FAKE".to_vec(),
            b"\x01\x01\x01\x01+*\x16\x16\x01FAKE".to_vec(),
            b"\x7e\x7f\x7e\x7f\x7f+*\x16\x16\x01\x7f\x7f".to_vec(),
            [b'+', b'*', SYN, SYN, 1].repeat(40),
            std::vec![0x7F; TEXT_MAX - 10],
        ];
        let mut rng = 0xACA5u32;
        for _ in 0..200 {
            rng = rng.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let len = (rng >> 16) as usize % (TEXT_MAX - 10);
            texts.push(
                (0..len)
                    .map(|_| {
                        rng = rng.wrapping_mul(1_103_515_245).wrapping_add(12345);
                        match (rng >> 16) as u8 & 0x7F {
                            ETX | ETB => b'.', // the frame's end
                            c => c,
                        }
                    })
                    .collect(),
            );
        }
        let (mut all, mut want) = (Vec::new(), Vec::new());
        for (i, text) in texts.iter().enumerate() {
            let mut b = [0u8; FRAME_MAX + 1];
            let n = sample(text, &mut b);
            let t = tones(&b[..n], 16);
            let got = decode(&audio(&t, 24_000, 12_000.0, 1500.0), 24_000);
            assert_eq!(got, [&b[..n - 1]], "text {i} alone");
            all.extend(t);
            want.push(b[..n - 1].to_vec());
        }
        let got = decode(&audio(&all, 24_000, 12_000.0, 1500.0), 24_000);
        assert_eq!(got, want, "back to back");
    }
}
