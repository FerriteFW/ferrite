//! The board's pins and buses as uradio's bus traits, so the chip drivers run on them unchanged.

use crate::board::{FLASH_CS, LCD_CS, LCD_DATA_PORT, LCD_DC, LCD_RST, LCD_WR};
use crate::hal::gpio::{self, pin, Mode, Pin};
use crate::hal::{i2c, spi, time};
use uradio_bus::{Delay, I2c, InPin, IoPin, Nack, OutPin, Spi};

/// A push-pull output.
pub struct Out(pub Pin);
impl OutPin for Out {
    fn set(&mut self, high: bool) {
        self.0.write(high)
    }
}

/// An input (configured by board::init or the owner).
pub struct In(pub Pin);
impl InPin for In {
    fn is_high(&mut self) -> bool {
        self.0.read()
    }
}

/// A half-duplex data line: driven push-pull, or released with the pull-up (the BK4829's SDATA).
pub struct Io(pub Pin);
impl OutPin for Io {
    fn set(&mut self, high: bool) {
        self.0.write(high)
    }
}
impl InPin for Io {
    fn is_high(&mut self) -> bool {
        self.0.read()
    }
}
impl IoPin for Io {
    fn drive(&mut self, on: bool) {
        if on {
            self.0.output(self.0.out_level(), Mode::OutPp)
        } else {
            self.0.config(Mode::InPu)
        }
    }
}

/// Busy-wait microseconds (cycle counter); only for the short waits inside a chip transaction.
pub struct BusyDelay;
impl Delay for BusyDelay {
    fn delay_us(&mut self, us: u32) {
        time::delay_us(us)
    }
    /// TERM-53: the BK4829 bus's half period in ns, on the cycle counter.
    fn delay_ns(&mut self, ns: u32) {
        time::delay_ns(ns)
    }
}

/// The SPI flash on SPI2, chip select PB12.
pub struct Flash;
impl Flash {
    pub fn new() -> Self {
        spi::spi2_init();
        FLASH_CS.output(true, Mode::OutPp);
        Flash
    }
}
impl Spi for Flash {
    /// SAFE-1.2: a microsecond after each chip-select edge, as stock waits (its delay around every flash command):
    /// a flash chip needs its select high for a while between commands (tSHSL, 50-100 ns by maker) or it may miss
    /// the next one, a write enable among them. Back to back it was tens of ns at 120-240 MHz (to Nov 19, 2026).
    fn select(&mut self, on: bool) {
        FLASH_CS.write(!on);
        time::delay_us(1);
    }
    fn transfer(&mut self, byte: u8) -> u8 {
        spi::spi2_xfer(byte)
    }
}

/// The SI4732's bit-banged I2C.
pub struct SiBus(pub i2c::I2c);
impl I2c for SiBus {
    fn write(&mut self, addr: u8, data: &[u8]) -> Result<(), Nack> {
        if self.0.write(addr, data) { Ok(()) } else { Err(Nack) }
    }
    fn read(&mut self, addr: u8, buf: &mut [u8]) -> Result<(), Nack> {
        if self.0.read(addr, buf) { Ok(()) } else { Err(Nack) }
    }
}

/// The display's 8080 bus: data on PD8-PD15, write strobe PD0, chip select PD1, D/C PD3, reset PD2. Chip select is
/// low only during a call (a command, its data, a band of pixels) and high between them, as stock (which raises it
/// after every byte): a deselected panel ignores the strobe and D/C, so a glitch on the flex between draws (a
/// transmission close by, ESD) can't write it (DISP-32, #182). Held low always until Nov 25, 2026.
pub struct Lcd;
impl Lcd {
    pub fn new() -> Self {
        LCD_CS.output(true, Mode::OutPpFast);
        LCD_WR.output(true, Mode::OutPpFast);
        LCD_DC.output(true, Mode::OutPpFast);
        LCD_RST.output(true, Mode::OutPp);
        for n in 8..16 {
            pin(LCD_DATA_PORT, n).config(Mode::OutPpFast);
        }
        Lcd
    }
}

/// Bytes sent to the LCD (pixel data and fills; commands not counted), so the cable log's draw lines can split bus
/// time from drawing time. Counted once per `data` or `repeat` call, not per byte: the byte loop stays as it was.
/// Until Oct 13, 2026 it counted only in the bring-up build, so the normal build's log always said "0 KB".
pub static LCD_BYTES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

#[inline(always)]
fn add_bytes(n: u32) {
    use core::sync::atomic::Ordering::Relaxed;
    LCD_BYTES.store(LCD_BYTES.load(Relaxed).wrapping_add(n), Relaxed);
}

/// LCD_WR is PD0, on the data lines' port: one write to GPIOD's set/clear register puts the byte on PD8-PD15 and
/// pulls WR low together (PD1-PD7 untouched: PD4-PD7 are the keypad sense pull-ups).
const WR_LOW: u8 = 1 << 0;

#[inline(always)]
fn byte(b: u8, reps: u32) {
    // The ST7789 latches on WR rising, so the data may change as WR falls: one write sets both. Then WR stays low
    // for unrolled nops and high for a few more, keeping the 8080 write cycle (66 ns), WR low/high (15 ns) and data
    // setup/hold (10 ns) at 120 MHz. Oct 12, 2026: separate writes and looped nops cost about 1 us a byte (120
    // cycles); at 120 MHz with no padding one-colour fills were lost.
    // Each write is read back before the wait, so the strobe's low and high times have a floor whatever the
    // write buffer does (unrolled nops alone left the screen blank on the radio, Oct 12, 2026; 6 low / 4 high left grey specks
    // around redrawn boxes, so 10 / 8).
    // Above 120 MHz (a `clock-*` bench build) the nops repeat, so the strobe never gets shorter than at 120.
    gpio::write_high_byte_clear(LCD_DATA_PORT, b, WR_LOW);
    gpio::sync(LCD_DATA_PORT);
    for _ in 0..reps {
        // SAFETY: nops only.
        unsafe { core::arch::asm!("nop", "nop", "nop", "nop", "nop", "nop", "nop", "nop", "nop", "nop", options(nomem, nostack, preserves_flags)) };
    }
    LCD_WR.set();
    gpio::sync(LCD_DATA_PORT);
    for _ in 0..reps {
        unsafe { core::arch::asm!("nop", "nop", "nop", "nop", "nop", "nop", "nop", "nop", options(nomem, nostack, preserves_flags)) };
    }
}

/// The LCD strobe's nops are counted at 120 MHz: once at that speed or below, more often above. Read from the clock
/// the core runs at now, once a call (not a byte): the build's speed was fixed at compile time until Nov 3, 2026, and
/// a redraw during satellite prediction's 240 MHz (PWR-26.8) halved the strobe and left green bars after the drawn
/// area (black). At 24 MHz (PWR-24.9) the strobe is only longer.
#[inline(always)]
fn nop_reps() -> u32 {
    crate::hal::clock::tree().lcd_nops()
}

// The trees' fastest is 240 MHz (`clock::TOP`): the strobe's count is 1 or 2, each a constant in the byte loops.
const _: () = assert!(crate::hal::clock::TOP.lcd_nops() <= 2 && crate::hal::clock::NORMAL.lcd_nops() <= 2);

/// Chip select low before a call's first strobe: read back, then a few nops (33 ns at 120 MHz, as long above it), so
/// its setup before the strobe falls (15 ns) holds at any clock. Once a call, not a byte.
#[inline(always)]
fn select(reps: u32) {
    LCD_CS.clr();
    gpio::sync(LCD_DATA_PORT);
    for _ in 0..reps {
        // SAFETY: nops only.
        unsafe { core::arch::asm!("nop", "nop", "nop", "nop", options(nomem, nostack, preserves_flags)) };
    }
}

/// Chip select high after a call: `byte` already waited the strobe's high time after the last rising edge, which
/// covers the hold (15 ns).
#[inline(always)]
fn deselect() {
    LCD_CS.set();
}

impl st7789::Bus8080 for Lcd {
    fn command(&mut self, cmd: u8) {
        let r = nop_reps();
        LCD_DC.clr();
        select(r);
        byte(cmd, r);
        deselect();
        LCD_DC.set();
    }
    fn data(&mut self, data: &[u8]) {
        add_bytes(data.len() as u32);
        let r = nop_reps();
        select(r);
        // The count as a constant (1 at 120 MHz and below, 2 at 240, the fastest tree: `lcd_nops`) lets the
        // strobe's nop loops fold away: read at run time each byte paid a loop's counter and branch twice, 385 ns
        // a byte against 240 (black, Nov 4, 2026: the slot switch's redraw 135-166 ms against Oct 23's 95-106)
        if r <= 1 {
            for &b in data {
                byte(b, 1)
            }
        } else {
            for &b in data {
                byte(b, 2)
            }
        }
        deselect();
    }
    /// Straight from the pixels: copying them into a byte buffer first made a band's send about 0.4 us a byte
    /// against a fill's 0.24 (Oct 23, 2026). The strobe is `byte`'s, unchanged.
    fn pixels(&mut self, px: &[u16]) {
        add_bytes(2 * px.len() as u32);
        let r = nop_reps();
        select(r);
        if r <= 1 {
            for &c in px {
                byte((c >> 8) as u8, 1);
                byte(c as u8, 1);
            }
        } else {
            for &c in px {
                byte((c >> 8) as u8, 2);
                byte(c as u8, 2);
            }
        }
        deselect();
    }
    fn repeat(&mut self, color: u16, count: u32) {
        let (hi, lo) = ((color >> 8) as u8, color as u8);
        add_bytes(count.wrapping_mul(2));
        let r = nop_reps();
        select(r);
        if r <= 1 {
            for _ in 0..count {
                byte(hi, 1);
                byte(lo, 1);
            }
        } else {
            for _ in 0..count {
                byte(hi, 2);
                byte(lo, 2);
            }
        }
        deselect();
    }
    fn reset(&mut self, active: bool) {
        LCD_RST.write(!active)
    }
}
