//! The transmitter: a picture to audio samples, phase-continuous, VIS first.

use crate::color::to_ycc;
use crate::mode::{vis_tones, Chan, Layout, Mode, Seg, BLACK_HZ, WHITE_HZ, WIDTH};
use crate::math::sin_at;

/// What the encoder reads: the picture, `WIDTH` wide and `mode.lines` tall (PD120: 496, two lines a frame as a real
/// sender's, which the receiver averages into its 248), one pixel at a time (keep the line in hand: the encoder asks
/// for the same line many times in a row).
pub trait Picture {
    fn rgb(&mut self, x: usize, y: usize) -> (u8, u8, u8);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Vis,
    Lead,
    Frame(u16),
    Done,
}

pub struct Encoder {
    mode: &'static Mode,
    rate: u64,
    amp: i32,
    vis: [Seg; 13],
    part: Part,
    i: usize,
    seg: Seg,
    /// Time in µs x rate: a sample is `1_000_000` of them, a segment `us * rate`; no division per sample.
    t: u64,
    seg_start: u64,
    seg_end: u64,
    /// A scan's pixels per time unit.
    px_per: f32,
    /// Phase steps per Hz (2^32 / rate).
    inc_per_hz: f32,
    n: u64,
    phase: u32,
}

impl Encoder {
    /// `amp`: the peak, full scale 32767.
    pub fn new(mode: &'static Mode, rate: u32, amp: i16) -> Encoder {
        let vis = vis_tones(mode.vis);
        Encoder {
            mode,
            rate: rate as u64,
            amp: amp as i32,
            vis,
            part: Part::Vis,
            i: 0,
            seg: vis[0],
            t: 0,
            seg_start: 0,
            seg_end: vis[0].us() as u64 * rate as u64,
            px_per: 0.0,
            inc_per_hz: 4_294_967_296.0 / rate as f32,
            n: 0,
            phase: 0,
        }
    }

    /// How long the whole transmission is, in samples.
    pub fn total_samples(&self) -> u64 {
        (crate::mode::VIS_US as u64 + self.mode.image_us() as u64) * self.rate / 1_000_000
    }

    /// Samples sent so far.
    pub fn sent(&self) -> u64 {
        self.n
    }

    /// The frame being sent (0 during the VIS).
    pub fn frame(&self) -> u16 {
        if let Part::Frame(f) = self.part { f } else { 0 }
    }

    fn segs(&self) -> &[Seg] {
        match self.part {
            Part::Vis => &self.vis,
            Part::Lead => self.mode.lead,
            Part::Frame(f) => self.mode.frame(f),
            Part::Done => &[],
        }
    }

    /// The next segment; false at the end.
    fn advance(&mut self) -> bool {
        loop {
            self.i = self.i.wrapping_add(1);
            if self.i >= self.segs().len() {
                self.i = 0;
                self.part = match self.part {
                    Part::Vis => Part::Lead,
                    Part::Lead => Part::Frame(0),
                    Part::Frame(f) if f + 1 < self.mode.frames => Part::Frame(f + 1),
                    _ => Part::Done,
                };
                if self.part == Part::Done {
                    return false;
                }
                if self.segs().is_empty() {
                    self.i = usize::MAX; // wraps to 0 on the next pass
                    continue;
                }
            }
            self.seg = self.segs()[self.i];
            self.seg_start = self.seg_end;
            self.seg_end += self.seg.us() as u64 * self.rate;
            self.px_per = WIDTH as f32 / (self.seg.us() as u64 * self.rate) as f32;
            return true;
        }
    }

    /// The next sample, or None when the picture has gone.
    pub fn next(&mut self, pic: &mut impl Picture) -> Option<i16> {
        while self.t >= self.seg_end {
            if !self.advance() {
                return None;
            }
        }
        let hz = match self.seg {
            Seg::Tone(hz, _) => hz as f32,
            Seg::Scan(ch, _) => {
                let x = (((self.t - self.seg_start) as f32 * self.px_per) as usize).min(WIDTH - 1);
                let v = self.value(pic, ch, x) as f32;
                BLACK_HZ as f32 + v * (WHITE_HZ - BLACK_HZ) as f32 / 255.0
            }
        };
        let s = (sin_at(self.phase) * self.amp) >> 15;
        self.phase = self.phase.wrapping_add((hz * self.inc_per_hz) as u32);
        self.t += 1_000_000;
        self.n += 1;
        Some(s as i16)
    }

    fn value(&self, pic: &mut impl Picture, ch: Chan, x: usize) -> u8 {
        let Part::Frame(f) = self.part else { return 0 };
        let last = self.mode.lines as usize - 1;
        let (a, b) = match self.mode.layout {
            Layout::Rgb => (f as usize, f as usize),
            Layout::Robot if f % 2 == 0 => (f as usize, (f as usize + 1).min(last)),
            Layout::Robot => (f as usize - 1, f as usize),
            // PD120: a frame's two lines are the picture's 2f and 2f + 1 (the receiver gives out their average)
            Layout::Pd if self.mode.lines == self.mode.frames => (2 * f as usize, 2 * f as usize + 1),
            Layout::Pd => {
                let l = self.mode.lines_of(f);
                (l.start as usize, l.end as usize - 1)
            }
        };
        let own = if self.mode.layout == Layout::Robot { f as usize } else { a };
        match ch {
            Chan::R => pic.rgb(x, own).0,
            Chan::G => pic.rgb(x, own).1,
            Chan::B => pic.rgb(x, own).2,
            Chan::Y => {
                let (r, g, b) = pic.rgb(x, own);
                to_ycc(r, g, b).0
            }
            Chan::Y2 => {
                let (r, g, b) = pic.rgb(x, b);
                to_ycc(r, g, b).0
            }
            Chan::Ry | Chan::By => {
                let (r1, g1, b1) = pic.rgb(x, a);
                let (r2, g2, b2) = pic.rgb(x, b);
                let avg = |p: u8, q: u8| ((p as u16 + q as u16) / 2) as u8;
                let (_, cr, cb) = to_ycc(avg(r1, r2), avg(g1, g2), avg(b1, b2));
                if ch == Chan::Ry { cr } else { cb }
            }
        }
    }
}
