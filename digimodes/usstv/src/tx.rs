//! The transmitter: a picture to audio samples, phase-continuous, VIS first.

use crate::color::to_ycc;
use crate::mode::{vis_tones, Chan, Layout, Mode, Seg, BLACK_HZ, WHITE_HZ, WIDTH};
use crate::math::sin_at;

/// What the encoder reads: the picture, `WIDTH` wide and `mode.lines` tall, one pixel at a time (keep the line in
/// hand: the encoder asks for the same line many times in a row).
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
    seg_start: u64,
    seg_end: u64,
    n: u64,
    phase: u32,
}

impl Encoder {
    /// `amp`: the peak, full scale 32767.
    pub fn new(mode: &'static Mode, rate: u32, amp: i16) -> Encoder {
        let vis = vis_tones(mode.vis);
        let mut e = Encoder { mode, rate: rate as u64, amp: amp as i32, vis, part: Part::Vis, i: 0, seg: vis[0], seg_start: 0, seg_end: vis[0].us() as u64 * 1000, n: 0, phase: 0 };
        e.seg = vis[0];
        e
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
            self.seg_end += self.seg.us() as u64 * 1000;
            return true;
        }
    }

    /// The next sample, or None when the picture has gone.
    pub fn next(&mut self, pic: &mut impl Picture) -> Option<i16> {
        let t = self.n * 1_000_000_000 / self.rate;
        while t >= self.seg_end {
            if !self.advance() {
                return None;
            }
        }
        let hz_milli: u64 = match self.seg {
            Seg::Tone(hz, _) => hz as u64 * 1000,
            Seg::Scan(ch, us) => {
                let x = ((t - self.seg_start) * WIDTH as u64 / (us as u64 * 1000)).min(WIDTH as u64 - 1) as usize;
                let v = self.value(pic, ch, x) as u64;
                BLACK_HZ as u64 * 1000 + v * (WHITE_HZ - BLACK_HZ) as u64 * 1000 / 255
            }
        };
        let inc = (hz_milli << 32) / (self.rate * 1000);
        let s = (sin_at(self.phase) * self.amp) >> 15;
        self.phase = self.phase.wrapping_add(inc as u32);
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
