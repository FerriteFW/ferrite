//! The SSTV modes: each line (a "frame": PD modes send two image lines in one) as a list of tones and scans.

/// What a scan carries: RGB for Martin and Scottie; luma and the two colour differences (BT.601, studio swing,
/// as MMSSTV) for Robot and PD. `Y2`: a PD frame's second line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chan {
    R,
    G,
    B,
    Y,
    Y2,
    /// R-Y (Cr).
    Ry,
    /// B-Y (Cb).
    By,
}

/// One part of a frame: a fixed tone, or a scan of one channel across the image, each with its length in µs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seg {
    Tone(u16, u32),
    Scan(Chan, u32),
}

impl Seg {
    pub const fn us(self) -> u32 {
        match self {
            Seg::Tone(_, us) | Seg::Scan(_, us) => us,
        }
    }
}

/// How a mode's frames make image lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// One RGB line a frame.
    Rgb,
    /// Robot 36: Y and one colour difference a frame, R-Y on even frames and B-Y on odd; each pair of frames makes
    /// two lines sharing both.
    Robot,
    /// PD: Y, R-Y, B-Y, Y2 a frame: two lines sharing the colour.
    Pd,
}

/// Frequencies (Hz): sync, black and white; VIS bits.
pub const SYNC_HZ: u16 = 1200;
pub const BLACK_HZ: u16 = 1500;
pub const WHITE_HZ: u16 = 2300;
pub const LEADER_HZ: u16 = 1900;
pub const VIS_ONE_HZ: u16 = 1100;
pub const VIS_ZERO_HZ: u16 = 1300;

/// Every image this crate gives out is this wide; a wider scan (PD120's 640) is averaged into it.
pub const WIDTH: usize = 320;
/// And at most this tall.
pub const MAX_LINES: usize = 256;

#[derive(Debug, PartialEq, Eq)]
pub struct Mode {
    pub name: &'static str,
    /// The VIS code (7 bits).
    pub vis: u8,
    /// Frames sent after the VIS.
    pub frames: u16,
    /// Image lines given out: `frames`, or `2 * frames` for PD90, or `frames` again for PD120 (its two lines a
    /// frame are averaged into one, 496 into 248).
    pub lines: u16,
    pub layout: Layout,
    /// Sent once before the first frame (Scottie's starting sync).
    pub lead: &'static [Seg],
    /// A frame; Robot 36's odd frames use `odd`.
    pub even: &'static [Seg],
    pub odd: &'static [Seg],
}

use Chan::*;
use Seg::*;

const R36_EVEN: [Seg; 6] = [Tone(SYNC_HZ, 9000), Tone(BLACK_HZ, 3000), Scan(Y, 88_000), Tone(BLACK_HZ, 4500), Tone(LEADER_HZ, 1500), Scan(Ry, 44_000)];
const R36_ODD: [Seg; 6] = [Tone(SYNC_HZ, 9000), Tone(BLACK_HZ, 3000), Scan(Y, 88_000), Tone(WHITE_HZ, 4500), Tone(LEADER_HZ, 1500), Scan(By, 44_000)];
const M1: [Seg; 8] = [
    Tone(SYNC_HZ, 4862),
    Tone(BLACK_HZ, 572),
    Scan(G, 146_432),
    Tone(BLACK_HZ, 572),
    Scan(B, 146_432),
    Tone(BLACK_HZ, 572),
    Scan(R, 146_432),
    Tone(BLACK_HZ, 572),
];
const S1_LEAD: [Seg; 1] = [Tone(SYNC_HZ, 9000)];
const S1: [Seg; 7] = [Tone(BLACK_HZ, 1500), Scan(G, 138_240), Tone(BLACK_HZ, 1500), Scan(B, 138_240), Tone(SYNC_HZ, 9000), Tone(BLACK_HZ, 1500), Scan(R, 138_240)];
const PD90: [Seg; 6] = [Tone(SYNC_HZ, 20_000), Tone(BLACK_HZ, 2080), Scan(Y, 170_240), Scan(Ry, 170_240), Scan(By, 170_240), Scan(Y2, 170_240)];
const PD120: [Seg; 6] = [Tone(SYNC_HZ, 20_000), Tone(BLACK_HZ, 2080), Scan(Y, 121_600), Scan(Ry, 121_600), Scan(By, 121_600), Scan(Y2, 121_600)];

pub static ROBOT36: Mode = Mode { name: "Robot 36", vis: 8, frames: 240, lines: 240, layout: Layout::Robot, lead: &[], even: &R36_EVEN, odd: &R36_ODD };
pub static MARTIN1: Mode = Mode { name: "Martin M1", vis: 44, frames: 256, lines: 256, layout: Layout::Rgb, lead: &[], even: &M1, odd: &M1 };
pub static SCOTTIE1: Mode = Mode { name: "Scottie S1", vis: 60, frames: 256, lines: 256, layout: Layout::Rgb, lead: &S1_LEAD, even: &S1, odd: &S1 };
pub static PD90_MODE: Mode = Mode { name: "PD90", vis: 99, frames: 128, lines: 256, layout: Layout::Pd, lead: &[], even: &PD90, odd: &PD90 };
pub static PD120_MODE: Mode = Mode { name: "PD120", vis: 95, frames: 248, lines: 248, layout: Layout::Pd, lead: &[], even: &PD120, odd: &PD120 };

/// Every mode, in the order the radio lists them.
pub static MODES: [&Mode; 5] = [&ROBOT36, &MARTIN1, &SCOTTIE1, &PD90_MODE, &PD120_MODE];

impl Mode {
    pub fn by_vis(vis: u8) -> Option<&'static Mode> {
        MODES.iter().copied().find(|m| m.vis == vis)
    }

    pub fn frame(&self, f: u16) -> &'static [Seg] {
        if f % 2 == 1 { self.odd } else { self.even }
    }

    /// One frame's length, µs (even and odd are the same length).
    pub fn frame_us(&self) -> u32 {
        self.even.iter().map(|s| s.us()).sum()
    }

    pub fn lead_us(&self) -> u32 {
        self.lead.iter().map(|s| s.us()).sum()
    }

    /// The whole transmission after the VIS, µs.
    pub fn image_us(&self) -> u32 {
        self.lead_us() + self.frame_us() * self.frames as u32
    }

    /// The sync in a frame: its start, µs from the frame's start, and its length.
    pub fn sync(&self) -> (u32, u32) {
        let mut at = 0;
        for s in self.even {
            if let Tone(SYNC_HZ, us) = *s {
                return (at, us);
            }
            at += s.us();
        }
        (0, 0)
    }

    /// The image lines frame `f` gives (PD90: two; Robot 36: two on odd frames, none on even).
    pub fn lines_of(&self, f: u16) -> core::ops::Range<u16> {
        match self.layout {
            Layout::Rgb => f..f + 1,
            Layout::Robot if f % 2 == 1 => f - 1..f + 1,
            Layout::Robot => f..f,
            Layout::Pd if self.lines == self.frames => f..f + 1,
            Layout::Pd => 2 * f..2 * f + 2,
        }
    }
}

/// The VIS header (leader, break, leader, start bit, 7 data bits LSB first, even parity, stop bit) as tones.
pub fn vis_tones(vis: u8) -> [Seg; 13] {
    let mut t = [Tone(LEADER_HZ, 300_000), Tone(SYNC_HZ, 10_000), Tone(LEADER_HZ, 300_000), Tone(SYNC_HZ, 30_000), Tone(0, 0), Tone(0, 0), Tone(0, 0), Tone(0, 0), Tone(0, 0), Tone(0, 0), Tone(0, 0), Tone(0, 0), Tone(SYNC_HZ, 30_000)];
    let bit = |b: bool| Tone(if b { VIS_ONE_HZ } else { VIS_ZERO_HZ }, 30_000);
    for k in 0..7 {
        t[4 + k] = bit(vis >> k & 1 != 0);
    }
    t[11] = bit((vis & 0x7F).count_ones() % 2 == 1);
    t
}

/// The VIS header's length, µs: from the first leader to the end of the stop bit.
pub const VIS_US: u32 = 300_000 + 10_000 + 300_000 + 30_000 * 10;
