//! # st7789
//!
//! A driver for the ST7789V display controller on an 8080 8-bit parallel bus (the RT-950 Pro's: 240 x 320
//! portrait, RGB565, bit-banged). `no_std`, no allocation, no frame buffer: the caller draws through windows,
//! a strip at a time.
//!
//! - Start-up waits up to 150 ms between commands: [`STARTUP`] lists the steps, waits included, for a firmware to
//!   await; [`St7789::init`] runs them with a busy-wait delay.
//! - Waterfalls scroll in the panel's own memory ([`St7789::scroll_area`], [`St7789::scroll_to`]): a new row is
//!   one window write.

#![no_std]

use uradio_bus::Delay;

/// The 8080 bus: a command byte (D/C low), then data bytes (D/C high). Chip select and the write strobe are the
/// implementation's business; `repeat` sends one 16-bit colour many times (a fill) and has a loop default.
pub trait Bus8080 {
    fn command(&mut self, cmd: u8);
    fn data(&mut self, data: &[u8]);

    fn repeat(&mut self, color: u16, count: u32) {
        let b = color.to_be_bytes();
        for _ in 0..count {
            self.data(&b);
        }
    }

    /// Pixels (RGB565), high byte first. The default sends them through a small buffer; a bus that writes bytes
    /// one at a time sends them straight from `px`.
    fn pixels(&mut self, px: &[u16]) {
        let mut buf = [0u8; 64];
        for chunk in px.chunks(32) {
            for (i, &c) in chunk.iter().enumerate() {
                buf[2 * i..2 * i + 2].copy_from_slice(&c.to_be_bytes());
            }
            self.data(&buf[..2 * chunk.len()]);
        }
    }

    /// Hardware reset line: true = held in reset (low).
    fn reset(&mut self, active: bool);
}

pub const SWRESET: u8 = 0x01;
pub const SLPIN: u8 = 0x10;
pub const SLPOUT: u8 = 0x11;
pub const PTLON: u8 = 0x12;
pub const NORON: u8 = 0x13;
pub const PTLAR: u8 = 0x30;
pub const IDMOFF: u8 = 0x38;
pub const IDMON: u8 = 0x39;
pub const INVOFF: u8 = 0x20;
pub const INVON: u8 = 0x21;
pub const DISPOFF: u8 = 0x28;
pub const DISPON: u8 = 0x29;
pub const CASET: u8 = 0x2A;
pub const RASET: u8 = 0x2B;
pub const RAMWR: u8 = 0x2C;
pub const VSCRDEF: u8 = 0x33;
pub const MADCTL: u8 = 0x36;
pub const VSCSAD: u8 = 0x37;
pub const COLMOD: u8 = 0x3A;

/// A start-up step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Hold or release the reset line.
    Reset(bool),
    /// A command and its data.
    Command(u8, &'static [u8]),
    /// Wait, milliseconds.
    Wait(u32),
}

/// Panel orientation and colour: MADCTL 0xC0 (row and column order flipped: the RT-950 Pro's mounting, from
/// stock), RGB.
pub const MADCTL_RT950: u8 = 0xC0;

/// The start-up sequence (datasheet order, stock's register settings): hardware reset, software reset, sleep out, 16-bit
/// colour, orientation, inversion on (IPS panel), normal mode. Display on is separate ([`St7789::on`]), so the
/// caller can clear the screen first.
pub const STARTUP: &[Step] = &[
    Step::Reset(false),
    Step::Wait(1),
    Step::Reset(true),
    Step::Wait(10),
    Step::Reset(false),
    Step::Wait(120),
    Step::Command(SWRESET, &[]),
    Step::Wait(150),
    Step::Command(SLPOUT, &[]),
    Step::Wait(120),
    Step::Command(COLMOD, &[0x55]),
    Step::Command(MADCTL, &[MADCTL_RT950]),
    Step::Command(INVON, &[]),
    Step::Command(NORON, &[]),
];

/// The mode registers [`STARTUP`] sets that decide how drawn pixels look: 16-bit colour, orientation, inversion on.
/// [`St7789::modes`] sends them again (DISP-32): a panel that lost one (an upset on its flex, a lone stray command)
/// shows every colour inverted, mirrored or garbled until they are set again. Six bytes, no waits, nothing on the
/// screen changes when they already hold.
pub const MODES: &[Step] = &[Step::Command(COLMOD, &[0x55]), Step::Command(MADCTL, &[MADCTL_RT950]), Step::Command(INVON, &[])];

/// The panel.
pub struct St7789<B: Bus8080> {
    bus: B,
    width: u16,
    height: u16,
}

impl<B: Bus8080> St7789<B> {
    /// A panel of `width` x `height` pixels (240 x 320 portrait on the RT-950 Pro). Sends nothing.
    pub fn new(bus: B, width: u16, height: u16) -> Self {
        St7789 { bus, width, height }
    }

    /// Run one start-up step (waits are the caller's: return them).
    pub fn step(&mut self, s: Step) -> Option<u32> {
        match s {
            Step::Reset(on) => self.bus.reset(on),
            Step::Command(c, d) => {
                self.bus.command(c);
                if !d.is_empty() {
                    self.bus.data(d);
                }
            }
            Step::Wait(ms) => return Some(ms),
        }
        None
    }

    /// The whole start-up, waiting in place (about 400 ms), then the screen cleared to `color` and turned on.
    pub fn init(&mut self, delay: &mut impl Delay, color: u16) {
        for &s in STARTUP {
            if let Some(ms) = self.step(s) {
                delay.delay_us(ms * 1000);
            }
        }
        self.fill(0, 0, self.width, self.height, color);
        self.on(true);
    }

    /// DISP-32: sends [`MODES`] again (colour format, orientation, inversion); idle, partial and the scroll area
    /// are left as they are.
    pub fn modes(&mut self) {
        for &s in MODES {
            self.step(s);
        }
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    /// Selects the rectangle `x, y, w, h` (clipped to the panel; nothing if empty) for the pixels that follow;
    /// returns the pixel count.
    pub fn window(&mut self, x: u16, y: u16, w: u16, h: u16) -> u32 {
        if x >= self.width || y >= self.height || w == 0 || h == 0 {
            return 0;
        }
        let (x1, y1) = ((x + w).min(self.width) - 1, (y + h).min(self.height) - 1);
        self.bus.command(CASET);
        self.bus.data(&[(x >> 8) as u8, x as u8, (x1 >> 8) as u8, x1 as u8]);
        self.bus.command(RASET);
        self.bus.data(&[(y >> 8) as u8, y as u8, (y1 >> 8) as u8, y1 as u8]);
        self.bus.command(RAMWR);
        (x1 - x + 1) as u32 * (y1 - y + 1) as u32
    }

    /// Pixels (RGB565) for the window, row by row; more may follow in later calls.
    pub fn pixels(&mut self, px: &[u16]) {
        self.bus.pixels(px);
    }

    /// `count` pixels of one colour for the window (a drawing layer's runs of background).
    pub fn repeat(&mut self, color: u16, count: u32) {
        self.bus.repeat(color, count);
    }

    /// Fills a rectangle with one colour.
    pub fn fill(&mut self, x: u16, y: u16, w: u16, h: u16, color: u16) {
        let n = self.window(x, y, w, h);
        if n > 0 {
            self.bus.repeat(color, n);
        }
    }

    /// Writes a whole rectangle of pixels (`px.len()` must be its area after clipping; the rest is ignored).
    pub fn blit(&mut self, x: u16, y: u16, w: u16, h: u16, px: &[u16]) {
        let n = self.window(x, y, w, h) as usize;
        self.pixels(&px[..n.min(px.len())]);
    }

    /// The scrolling area: `top` fixed lines, then the scrolling lines, then `bottom` fixed lines (along the
    /// panel's 320-line axis, which is vertical in portrait).
    pub fn scroll_area(&mut self, top: u16, bottom: u16) {
        let scroll = self.height.saturating_sub(top + bottom);
        self.bus.command(VSCRDEF);
        self.bus.data(&[(top >> 8) as u8, top as u8, (scroll >> 8) as u8, scroll as u8, (bottom >> 8) as u8, bottom as u8]);
    }

    /// The first memory line shown at the top of the scrolling area.
    pub fn scroll_to(&mut self, line: u16) {
        self.bus.command(VSCSAD);
        self.bus.data(&[(line >> 8) as u8, line as u8]);
    }

    pub fn on(&mut self, on: bool) {
        self.bus.command(if on { DISPON } else { DISPOFF });
    }

    pub fn invert(&mut self, on: bool) {
        self.bus.command(if on { INVON } else { INVOFF });
    }

    /// Sleep in or out. The panel needs 120 ms before the next sleep command or drawing after sleep out: the
    /// caller waits.
    pub fn sleep(&mut self, on: bool) {
        self.bus.command(if on { SLPIN } else { SLPOUT });
    }

    /// Idle mode: 8 colours (each channel's top bit), for lower power.
    pub fn idle(&mut self, on: bool) {
        self.bus.command(if on { IDMON } else { IDMOFF });
    }

    /// Partial mode on panel rows `first..=last` (memory rows; the rest show blank), or back to normal (`None`).
    pub fn partial(&mut self, rows: Option<(u16, u16)>) {
        match rows {
            Some((a, b)) => {
                self.bus.command(PTLAR);
                self.bus.data(&[(a >> 8) as u8, a as u8, (b >> 8) as u8, b as u8]);
                self.bus.command(PTLON);
            }
            None => self.bus.command(NORON),
        }
    }

    pub fn release(self) -> B {
        self.bus
    }
}
