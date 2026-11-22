//! The receiver: audio samples in, image lines out. A quadrature discriminator gives the frequency each sample; the
//! VIS header (read from 1 ms averages) names the mode and when the image starts; each frame's pixels are averaged
//! into [`WIDTH`] bins as they arrive; the sync pulses' times, fitted with a straight line, correct the line
//! period for the sender's clock (slant) and keep the frames in step.

use crate::color::{rgb565, to_rgb};
use crate::math::{atan2, clamp_u8};
use crate::mode::{Chan, Layout, Mode, Seg, BLACK_HZ, LEADER_HZ, SYNC_HZ, VIS_ONE_HZ, VIS_ZERO_HZ, WHITE_HZ, WIDTH};
use core::f32::consts::TAU;

/// Where the decoded image goes.
pub trait Sink {
    fn start(&mut self, mode: &'static Mode);
    /// Line `y` (0 at the top), RGB565.
    fn line(&mut self, y: u16, px: &[u16; WIDTH]);
    /// The image ended: all its frames (`complete`), or the signal went.
    fn end(&mut self, complete: bool);
}

const BOX: usize = 16;
/// The end of a scan that runs straight into a tone (a sync, a porch, Robot 36's separator) is left out, this long
/// (µs): the filters smear the tone back into its last pixels, a green edge on every line where the sync's 1200 Hz
/// follows Robot 36's colour (Nov 22, 2026: the right edge 53 levels off on a clean Robot 36 loop; 63 to 24 with
/// this, and Martin M1 7.2 to 3.3, Scottie S1 9.3 to 3.5, PD90 11.9 to 9.0). Those pixels repeat the last one before
/// (`Acc::levels` fills gaps from the left).
const SYNC_GUARD_US: f32 = 600.0;

/// A second-order section (RBJ's cookbook, transposed direct form II).
#[derive(Clone, Copy)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}

impl Biquad {
    /// Low-pass (`high` false) or high-pass at `hz`, quality `q`; sine and cosine from the table.
    fn new(rate: u32, hz: u32, q: f32, high: bool) -> Biquad {
        let step = (((hz as u64) << 32) / rate as u64) as u32;
        let (sn, cs) = (crate::math::sin_at(step) as f32 / 32767.0, crate::math::sin_at(step.wrapping_add(1 << 30)) as f32 / 32767.0);
        let alpha = sn / (2.0 * q);
        let a0 = 1.0 + alpha;
        let (b0, b1) = if high { ((1.0 + cs) / 2.0, -(1.0 + cs)) } else { ((1.0 - cs) / 2.0, 1.0 - cs) };
        Biquad { b: [b0 / a0, b1 / a0, b0 / a0], a: [-2.0 * cs / a0, (1.0 - alpha) / a0], z: [0.0; 2] }
    }

    #[inline]
    fn step(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// The discriminator: the frequency of each sample.
pub struct Demod {
    rate: f32,
    osc: (f32, f32),
    rot: (f32, f32),
    n: u32,
    l1: usize,
    l2: usize,
    b1: [(f32, f32); BOX],
    b2: [(f32, f32); BOX],
    s1: (f32, f32),
    s2: (f32, f32),
    i1: usize,
    i2: usize,
    prev: (f32, f32),
    /// The band the tones are in (1-2.5 kHz), ahead of the mixer: a receiver whose audio clips (black's ADC at 3 kHz
    /// deviation, Nov 16, 2026) gives square waves, whose harmonics fell where the running sums don't reject them.
    band: [Biquad; 3],
}

impl Demod {
    pub fn new(rate: u32) -> Demod {
        // the mixer's step from the sine table (0.3 Hz at most off): no series at run time
        let step = (LEADER_HZ as u64 * (1u64 << 32) / rate as u64) as u32;
        let (c, s) = (crate::math::sin_at(step.wrapping_add(1 << 30)) as f32 / 32767.0, crate::math::sin_at(step) as f32 / 32767.0);
        // two running sums: nulls at 3 and 4 kHz from the centre, where the mixer's image falls
        let l1 = ((rate + 1500) / 3000).clamp(2, BOX as u32) as usize;
        let l2 = ((rate + 2000) / 4000).clamp(2, BOX as u32) as usize;
        Demod { rate: rate as f32, osc: (1.0, 0.0), rot: (c, s), n: 0, l1, l2, b1: [(0.0, 0.0); BOX], b2: [(0.0, 0.0); BOX], s1: (0.0, 0.0), s2: (0.0, 0.0), i1: 0, i2: 0, prev: (0.0, 0.0), band: [
            Biquad::new(rate, 1000, 0.707, true),
            Biquad::new(rate, 2500, 0.541, false),
            Biquad::new(rate, 2500, 1.307, false),
        ] }
    }

    /// The filters' delay, samples.
    pub fn delay(&self) -> f32 {
        (self.l1 - 1) as f32 / 2.0 + (self.l2 - 1) as f32 / 2.0
    }

    /// One sample in, its frequency (Hz) out.
    pub fn step(&mut self, x: i16) -> f32 {
        let mut x = x as f32;
        for b in &mut self.band {
            x = b.step(x);
        }
        let z = (x * self.osc.0, -x * self.osc.1);
        let (c, s) = self.osc;
        self.osc = (c * self.rot.0 - s * self.rot.1, c * self.rot.1 + s * self.rot.0);
        self.n += 1;
        if self.n.is_multiple_of(1024) {
            // keep the oscillator on the unit circle
            let m = self.osc.0 * self.osc.0 + self.osc.1 * self.osc.1;
            let k = 1.5 - 0.5 * m;
            self.osc = (self.osc.0 * k, self.osc.1 * k);
        }
        let o = self.b1[self.i1];
        self.b1[self.i1] = z;
        self.i1 = (self.i1 + 1) % self.l1;
        self.s1 = (self.s1.0 + z.0 - o.0, self.s1.1 + z.1 - o.1);
        let y = self.s1;
        let o = self.b2[self.i2];
        self.b2[self.i2] = y;
        self.i2 = (self.i2 + 1) % self.l2;
        self.s2 = (self.s2.0 + y.0 - o.0, self.s2.1 + y.1 - o.1);
        let z = self.s2;
        let p = self.prev;
        self.prev = z;
        // z times the conjugate of the last: its angle is the phase step
        let d = (z.0 * p.0 + z.1 * p.1, z.1 * p.0 - z.0 * p.1);
        LEADER_HZ as f32 + atan2(d.1, d.0) * self.rate / TAU
    }
}

/// A pixel's level from its frequency.
fn level(f: f32) -> u8 {
    clamp_u8((f - BLACK_HZ as f32) * 255.0 / (WHITE_HZ - BLACK_HZ) as f32)
}

fn near(f: f32, hz: u16, tol: f32) -> bool {
    crate::math::abs(f - hz as f32) < tol
}

const BINS: usize = 512;

/// Per-frame pixel sums: four channels ([`slot`]) by [`WIDTH`].
struct Acc {
    sum: [[u16; WIDTH]; 4],
    n: [[u8; WIDTH]; 4],
}

fn slot(c: Chan) -> usize {
    match c {
        Chan::R | Chan::Y => 0,
        Chan::G | Chan::Ry => 1,
        Chan::B | Chan::By => 2,
        Chan::Y2 => 3,
    }
}

impl Acc {
    fn clear(&mut self) {
        self.sum = [[0; WIDTH]; 4];
        self.n = [[0; WIDTH]; 4];
    }

    /// A channel's levels, gaps filled from the left (or the right at the start).
    fn levels(&self, s: usize, out: &mut [u8; WIDTH]) {
        let mut last = None;
        for x in 0..WIDTH {
            if self.n[s][x] > 0 {
                let v = (self.sum[s][x] / self.n[s][x] as u16) as u8;
                out[x] = v;
                if last.is_none() {
                    out[..x].fill(v);
                }
                last = Some(v);
            } else if let Some(v) = last {
                out[x] = v;
            }
        }
        if last.is_none() {
            out.fill(0);
        }
    }
}

/// A straight line through (frame, sync centre) points, kept as running means and co-moments of the centre's
/// deviation from the mode's nominal period: small numbers, so f32 holds them (no soft-float doubles on the radio).
#[derive(Clone, Copy, Default)]
struct Fit {
    n: f32,
    mx: f32,
    my: f32,
    sxx: f32,
    sxy: f32,
}

impl Fit {
    fn add(&mut self, x: f32, y: f32) {
        self.n += 1.0;
        let dx = x - self.mx;
        self.mx += dx / self.n;
        let dy = y - self.my;
        self.my += dy / self.n;
        self.sxx += dx * (x - self.mx);
        self.sxy += dx * (y - self.my);
    }

    /// The deviation per frame (slope) and at frame 0; with one point (or all at one frame) the mean, flat.
    fn line(&self) -> (f32, f32) {
        let b = if self.sxx > 0.0 { self.sxy / self.sxx } else { 0.0 };
        (b, self.my - b * self.mx)
    }
}

struct Img {
    mode: &'static Mode,
    frame: u16,
    /// The frame's start: a whole sample index and a fraction.
    start: u32,
    frac: f32,
    /// Samples a frame, and per µs (from the fit).
    period: f32,
    per_us: f32,
    /// The current segment and where it starts and ends in the frame, samples.
    seg: usize,
    seg_start: f32,
    seg_end: f32,
    /// The sync's window in the frame, samples, and what was seen in it.
    win: (f32, f32),
    sync_n: u32,
    sync_sum: f32,
    /// The fit of the sync centres (samples from `t0`) against the frame: see [`Fit`].
    t0: u32,
    fit: Fit,
    missed: u16,
    /// A forced mode before its first sync: the run of sync-low samples so far.
    hunt: Option<u32>,
    /// Robot 36: the separator's frequency this frame (1500 Hz even, 2300 odd), and frames in a row that
    /// disagreed with the count.
    sep: (f32, u32),
    sep_bad: u8,
}

/// The receiver.
pub struct Decoder {
    rate: u32,
    demod: Demod,
    n: u32,
    bin_len: u32,
    bin_acc: f32,
    bin_cnt: u32,
    bins: [i16; BINS],
    nb: u32,
    low_run: u32,
    vis_at: Option<u32>,
    img: Option<Img>,
    acc: Acc,
    /// Robot 36: the even frame's luma and R-Y, waiting for the odd one.
    keep_y: [u8; WIDTH],
    keep_ry: [u8; WIDTH],
    /// The image last given: its mode and how many lines it had.
    pub last: Option<(&'static Mode, u16, bool)>,
}

impl Decoder {
    pub fn new(rate: u32) -> Decoder {
        Decoder {
            rate,
            demod: Demod::new(rate),
            n: 0,
            bin_len: rate / 1000,
            bin_acc: 0.0,
            bin_cnt: 0,
            bins: [0; BINS],
            nb: 0,
            low_run: 0,
            vis_at: None,
            img: None,
            acc: Acc { sum: [[0; WIDTH]; 4], n: [[0; WIDTH]; 4] },
            keep_y: [0; WIDTH],
            keep_ry: [0; WIDTH],
            last: None,
        }
    }

    /// An image is being received: its mode and frame.
    pub fn receiving(&self) -> Option<(&'static Mode, u16)> {
        self.img.as_ref().map(|i| (i.mode, i.frame))
    }

    /// Stops the image being received (it ends incomplete).
    pub fn stop(&mut self, sink: &mut impl Sink) {
        if self.img.take().is_some() {
            sink.end(false);
        }
    }

    /// Starts a mode without its VIS (the sender left it out, or it was missed): the image starts at the next sync.
    pub fn force(&mut self, mode: &'static Mode, sink: &mut impl Sink) {
        self.stop(sink);
        // a frame start placed now; the first syncs found put it right (the fit takes over after four)
        self.begin(mode, self.n, sink);
        if let Some(i) = self.img.as_mut() {
            i.hunt = Some(0);
        }
    }

    pub fn feed(&mut self, samples: &[i16], sink: &mut impl Sink) {
        for &x in samples {
            let f = self.demod.step(x);
            self.sample(f, sink);
            self.n = self.n.wrapping_add(1);
        }
    }

    fn sample(&mut self, f: f32, sink: &mut impl Sink) {
        // 1 ms bins for the VIS
        self.bin_acc += f;
        self.bin_cnt += 1;
        if self.bin_cnt == self.bin_len {
            let m = self.bin_acc / self.bin_cnt as f32;
            (self.bin_acc, self.bin_cnt) = (0.0, 0);
            self.bin(m, sink);
        }
        if self.img.is_some() {
            self.pixel(f, sink);
        }
    }

    fn bin(&mut self, m: f32, sink: &mut impl Sink) {
        let b = self.nb;
        self.bins[b as usize % BINS] = m as i16;
        self.nb = b.wrapping_add(1);
        let at = |k: u32| self.bins[k as usize % BINS] as f32;
        if near(m, SYNC_HZ, 120.0) {
            self.low_run += 1;
        } else {
            self.low_run = 0;
        }
        // a start bit: 20 ms of 1200 Hz (the break is only 10) after mostly leader
        if self.low_run == 20 && self.vis_at.is_none() && b >= 140 {
            let s = b - 19;
            let lead = (s - 120..s - 20).filter(|&k| near(at(k), LEADER_HZ, 150.0)).count();
            let busy = self.img.as_ref().is_some_and(|i| i.missed < 3);
            if lead >= 70 && !busy {
                self.vis_at = Some(s);
            }
        }
        if let Some(s) = self.vis_at {
            if b.wrapping_sub(s) >= 299 {
                self.vis_at = None;
                let bit = |k: u32| {
                    let c = s + 30 * (k + 1);
                    let m = (c + 5..c + 25).map(at).sum::<f32>() / 20.0;
                    m < (VIS_ONE_HZ + VIS_ZERO_HZ) as f32 / 2.0
                };
                let mut code = 0u8;
                for k in 0..7 {
                    code |= (bit(k) as u8) << k;
                }
                let parity_ok = (code.count_ones() % 2 == 1) == bit(7);
                if let (true, Some(mode)) = (parity_ok, Mode::by_vis(code)) {
                    // the start bit's edge is about half a bin before its first whole bin
                    let edge = s * self.bin_len - self.bin_len / 2;
                    let start = edge + 300 * self.bin_len;
                    self.stop(sink);
                    self.begin(mode, start, sink);
                }
            }
        }
    }

    fn begin(&mut self, mode: &'static Mode, image_start: u32, sink: &mut impl Sink) {
        let per_us = self.rate as f32 / 1e6;
        let start = image_start + (mode.lead_us() as f32 * per_us) as u32;
        let period = mode.frame_us() as f32 * per_us;
        let mut img = Img { mode, frame: 0, start, frac: 0.0, period, per_us, seg: 0, seg_start: 0.0, seg_end: 0.0, win: (0.0, 0.0), sync_n: 0, sync_sum: 0.0, t0: start, fit: Fit::default(), missed: 0, hunt: None, sep: (0.0, 0), sep_bad: 0 };
        Self::layout(&mut img);
        self.acc.clear();
        self.img = Some(img);
        sink.start(mode);
    }

    /// The frame's first segment and its sync window, at the current period.
    fn layout(i: &mut Img) {
        let segs = i.mode.frame(i.frame);
        i.seg = 0;
        i.seg_start = 0.0;
        i.seg_end = segs[0].us() as f32 * i.per_us;
        let (at, len) = i.mode.sync();
        let w = (len as f32 / 2.0).min(5000.0);
        i.win = ((at as f32 - w) * i.per_us, (at as f32 + len as f32 + w) * i.per_us);
        i.sync_n = 0;
        i.sync_sum = 0.0;
        i.sep = (0.0, 0);
    }

    fn pixel(&mut self, f: f32, sink: &mut impl Sink) {
        let Some(i) = self.img.as_mut() else { return };
        if let Some(run) = i.hunt {
            // a forced mode: the first sync places the frames
            let (at, len) = i.mode.sync();
            let low = f < (SYNC_HZ + BLACK_HZ) as f32 / 2.0;
            if low {
                i.hunt = Some(run + 1);
            } else if run as f32 >= 0.7 * len as f32 * i.per_us {
                let centre = self.n as f32 - run as f32 / 2.0;
                let start = centre - (at as f32 + len as f32 / 2.0) * i.per_us + i.period;
                (i.start, i.frac, i.t0, i.hunt) = (start as u32, start - (start as u32) as f32, start as u32, None);
                Self::layout(i);
                self.acc.clear();
            } else {
                i.hunt = Some(0);
            }
            return;
        }
        let d = self.n.wrapping_sub(i.start) as i32;
        if d < 0 && d > -(1 << 30) {
            return; // before the frame (the VIS's stop bit)
        }
        let rel = d as f32 - i.frac;
        if rel < 0.0 {
            return;
        }
        if rel >= i.period {
            return self.end_frame(sink);
        }
        let segs = i.mode.frame(i.frame);
        while rel >= i.seg_end && i.seg + 1 < segs.len() {
            i.seg += 1;
            i.seg_start = i.seg_end;
            i.seg_end += segs[i.seg].us() as f32 * i.per_us;
        }
        if i.mode.layout == Layout::Robot && i.seg == 3 {
            i.sep = (i.sep.0 + f, i.sep.1 + 1);
        }
        if let Seg::Scan(ch, _) = segs[i.seg] {
            let x = ((rel - i.seg_start) / (i.seg_end - i.seg_start) * WIDTH as f32) as usize;
            let next = segs.get(i.seg + 1).or_else(|| i.mode.frame(i.frame.wrapping_add(1)).first());
            let into_tone = matches!(next, Some(Seg::Tone(..)));
            let guarded = into_tone && rel >= i.seg_end - SYNC_GUARD_US * i.per_us;
            if x < WIDTH && !guarded {
                let s = slot(ch);
                self.acc.sum[s][x] = self.acc.sum[s][x].saturating_add(level(f) as u16);
                self.acc.n[s][x] = self.acc.n[s][x].saturating_add(1);
            }
        }
        if rel >= i.win.0 && rel < i.win.1 && f < (SYNC_HZ + BLACK_HZ) as f32 / 2.0 {
            i.sync_n += 1;
            i.sync_sum += rel;
        }
    }

    fn end_frame(&mut self, sink: &mut impl Sink) {
        let Some(mut i) = self.img.take() else { return };
        let (at, len) = i.mode.sync();
        let nominal = i.mode.frame_us() as f32 * self.rate as f32 / 1e6;
        // the frame's start, samples from t0 (it can be before t0 after a Robot 36 recount)
        let rel_start = i.start.wrapping_sub(i.t0) as i32 as f32 + i.frac;
        // the sync's centre, if most of it was seen: its deviation from the nominal period goes into the fit
        if i.sync_n as f32 >= 0.6 * len as f32 * i.per_us {
            let x = i.frame as f32;
            let y = rel_start + i.sync_sum / i.sync_n as f32 - nominal * x;
            // a point far off the fit (noise in the window) is left out
            let (b, a) = i.fit.line();
            if i.fit.n < 4.0 || crate::math::abs(y - (a + b * x)) < 0.02 * nominal {
                i.fit.add(x, y);
                i.missed = 0;
            } else {
                i.missed += 1;
            }
        } else {
            i.missed += 1;
        }
        // Robot 36: two frames in a row whose separator says the other parity: the count is a frame out (a forced
        // start, or a missed frame); move it on and fit again (the line's constant takes up the offset)
        if i.mode.layout == Layout::Robot && i.sep.1 > 0 {
            let m = i.sep.0 / i.sep.1 as f32;
            let odd = m > LEADER_HZ as f32;
            if crate::math::abs(m - LEADER_HZ as f32) > 200.0 && odd != (i.frame % 2 == 1) {
                i.sep_bad += 1;
                if i.sep_bad >= 2 && i.frame + 1 < i.mode.frames {
                    i.frame += 1;
                    i.fit = Fit::default();
                    i.sep_bad = 0;
                }
            } else {
                i.sep_bad = 0;
            }
        }
        self.emit(&i, sink);
        let next = i.frame + 1;
        // a forced mode's lock that keeps missing (Scottie's starting sync taken for a frame's): hunt again
        if i.missed >= 3 && i.fit.n < 4.0 && next < i.mode.frames {
            (i.frame, i.hunt, i.missed) = (next, Some(0), 0);
            Self::layout(&mut i);
            self.acc.clear();
            self.img = Some(i);
            return;
        }
        if next >= i.mode.frames || i.missed > 12 {
            self.last = Some((i.mode, i.mode.lines_of(i.frame).end, next >= i.mode.frames));
            sink.end(next >= i.mode.frames);
            return;
        }
        // where the next frame starts: from the fit, or one period on
        let centre = (at as f32 + len as f32 / 2.0) * self.rate as f32 / 1e6;
        let rel_next = if i.fit.n >= 4.0 {
            let (mut b, a) = i.fit.line();
            if crate::math::abs(b) > 0.015 * nominal {
                b = 0.0;
            }
            let period = nominal + b;
            i.period = period;
            i.per_us = period / i.mode.frame_us() as f32;
            nominal * next as f32 + a + b * next as f32 - centre * period / nominal
        } else if i.fit.n >= 1.0 {
            // too few to fit: the mean deviation, the nominal period
            nominal * next as f32 + i.fit.my - centre
        } else {
            rel_start + i.period
        };
        let whole = libm_floor(rel_next);
        i.frame = next;
        i.start = i.t0.wrapping_add(whole as i32 as u32);
        i.frac = rel_next - whole;
        Self::layout(&mut i);
        self.acc.clear();
        self.img = Some(i);
    }

    fn emit(&mut self, i: &Img, sink: &mut impl Sink) {
        let mut c = [[0u8; WIDTH]; 4];
        for (s, out) in c.iter_mut().enumerate() {
            self.acc.levels(s, out);
        }
        let mut px = [0u16; WIDTH];
        let f = i.frame;
        match i.mode.layout {
            Layout::Rgb => {
                for x in 0..WIDTH {
                    px[x] = rgb565(c[0][x], c[1][x], c[2][x]);
                }
                sink.line(f, &px);
            }
            Layout::Robot if f.is_multiple_of(2) => {
                self.keep_y = c[0];
                self.keep_ry = c[1];
            }
            Layout::Robot => {
                for (y, luma) in [(f - 1, &self.keep_y), (f, &c[0])] {
                    for x in 0..WIDTH {
                        let (r, g, b) = to_rgb(luma[x], self.keep_ry[x], c[2][x]);
                        px[x] = rgb565(r, g, b);
                    }
                    sink.line(y, &px);
                }
            }
            Layout::Pd => {
                let lines = i.mode.lines_of(f);
                if lines.len() == 1 {
                    for x in 0..WIDTH {
                        let y = ((c[0][x] as u16 + c[3][x] as u16) / 2) as u8;
                        let (r, g, b) = to_rgb(y, c[1][x], c[2][x]);
                        px[x] = rgb565(r, g, b);
                    }
                    sink.line(lines.start, &px);
                } else {
                    for (k, s) in [0usize, 3].into_iter().enumerate() {
                        for x in 0..WIDTH {
                            let (r, g, b) = to_rgb(c[s][x], c[1][x], c[2][x]);
                            px[x] = rgb565(r, g, b);
                        }
                        sink.line(lines.start + k as u16, &px);
                    }
                }
            }
        }
    }
}

/// floor for f32 without libm.
fn libm_floor(x: f32) -> f32 {
    let t = x as i32 as f32;
    if t > x { t - 1.0 } else { t }
}
