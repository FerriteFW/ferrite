//! The receiver: audio samples in, image lines out. A quadrature discriminator gives the frequency each sample; the
//! VIS header (read from 1 ms averages) names the mode and when the image starts; each frame's pixels are averaged
//! into [`WIDTH`] bins as they arrive; the sync pulses' times, fitted with a straight line, correct the line
//! period for the sender's clock (slant) and keep the frames in step.

use crate::color::{rgb565, to_rgb};
use crate::math::{atan2, clamp_u8, cos, sin};
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
}

impl Demod {
    pub fn new(rate: u32) -> Demod {
        let w = TAU * LEADER_HZ as f32 / rate as f32;
        // two running sums: nulls at 3 and 4 kHz from the centre, where the mixer's image falls
        let l1 = ((rate + 1500) / 3000).clamp(2, BOX as u32) as usize;
        let l2 = ((rate + 2000) / 4000).clamp(2, BOX as u32) as usize;
        Demod { rate: rate as f32, osc: (1.0, 0.0), rot: (cos(w), sin(w)), n: 0, l1, l2, b1: [(0.0, 0.0); BOX], b2: [(0.0, 0.0); BOX], s1: (0.0, 0.0), s2: (0.0, 0.0), i1: 0, i2: 0, prev: (0.0, 0.0) }
    }

    /// The filters' delay, samples.
    pub fn delay(&self) -> f32 {
        (self.l1 - 1) as f32 / 2.0 + (self.l2 - 1) as f32 / 2.0
    }

    /// One sample in, its frequency (Hz) out.
    pub fn step(&mut self, x: i16) -> f32 {
        let x = x as f32;
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
    /// The fit: sums over (frame, sync centre) points, centres relative to `t0`.
    t0: f64,
    fit: [f64; 5],
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
        let mut img = Img { mode, frame: 0, start, frac: 0.0, period, per_us, seg: 0, seg_start: 0.0, seg_end: 0.0, win: (0.0, 0.0), sync_n: 0, sync_sum: 0.0, t0: start as f64, fit: [0.0; 5], missed: 0, hunt: None, sep: (0.0, 0), sep_bad: 0 };
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
                (i.start, i.frac, i.t0, i.hunt) = (start as u32, start - (start as u32) as f32, start as u32 as f64, None);
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
            if x < WIDTH {
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
        // the sync's centre, if most of it was seen
        let abs_start = i.start as f64 + i.frac as f64;
        if i.sync_n as f32 >= 0.6 * len as f32 * i.per_us {
            let c = abs_start + (i.sync_sum / i.sync_n as f32) as f64 - i.t0;
            let x = i.frame as f64;
            let nominal = i.mode.frame_us() as f64 * self.rate as f64 / 1e6;
            let [n, sx, sy, sxx, sxy] = i.fit;
            // a point far off the fit (noise in the window) is left out
            let good = if n >= 4.0 {
                let b = (n * sxy - sx * sy) / (n * sxx - sx * sx);
                let a = (sy - b * sx) / n;
                (c - (a + b * x)).abs() < 0.02 * nominal
            } else {
                true
            };
            if good {
                i.fit = [n + 1.0, sx + x, sy + c, sxx + x * x, sxy + x * c];
                i.missed = 0;
            } else {
                i.missed += 1;
            }
        } else {
            i.missed += 1;
        }
        // Robot 36: two frames in a row whose separator says the other parity: the count is a frame out (a forced
        // start, or a missed frame); move it on and fit again
        if i.mode.layout == Layout::Robot && i.sep.1 > 0 {
            let m = i.sep.0 / i.sep.1 as f32;
            let odd = m > LEADER_HZ as f32;
            if crate::math::abs(m - LEADER_HZ as f32) > 200.0 && odd != (i.frame % 2 == 1) {
                i.sep_bad += 1;
                if i.sep_bad >= 2 && i.frame + 1 < i.mode.frames {
                    i.frame += 1;
                    i.fit = [0.0; 5];
                    i.t0 = abs_start - i.frame as f64 * i.period as f64;
                    i.sep_bad = 0;
                }
            } else {
                i.sep_bad = 0;
            }
        }
        self.emit(&i, sink);
        let next = i.frame + 1;
        // a forced mode's lock that keeps missing (Scottie's starting sync taken for a frame's): hunt again
        if i.missed >= 3 && i.fit[0] < 4.0 && next < i.mode.frames {
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
        let nominal = i.mode.frame_us() as f64 * self.rate as f64 / 1e6;
        let centre = (at as f64 + len as f64 / 2.0) * self.rate as f64 / 1e6;
        let [n, sx, sy, sxx, sxy] = i.fit;
        let next_start = if n >= 4.0 {
            let mut b = (n * sxy - sx * sy) / (n * sxx - sx * sx);
            if !(b > nominal * 0.985 && b < nominal * 1.015) {
                b = nominal;
            }
            let a = (sy - b * sx) / n;
            i.period = b as f32;
            i.per_us = (b / i.mode.frame_us() as f64) as f32;
            i.t0 + a + b * next as f64 - centre * b / nominal
        } else if n >= 1.0 {
            // too few to fit: the mean offset from the nominal period
            let a = (sy - nominal * sx) / n;
            i.t0 + a + nominal * next as f64 - centre
        } else {
            abs_start + i.period as f64
        };
        i.frame = next;
        i.start = next_start as u32;
        i.frac = (next_start - i.start as f64) as f32;
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
