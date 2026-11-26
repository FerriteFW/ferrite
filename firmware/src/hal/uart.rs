//! Serial ports. Receive is interrupt driven into a ring; transmit is polled, but for the cable's link, which the
//! transmit interrupt drains from its ring ([`tx_ring`], decision 290). Mirrors fw/hal/hal_uart.
//!   UART_BT   USART1 PA9/PA10   Bluetooth module
//!   UART_GPS  USART3 PB10/PB11  GPS module
//!   UART_CPS  UART4  PC10/PC11  programming cable (console)
use super::gpio::{pin, Mode, Pin, GPIOA, GPIOB, GPIOC};
use super::regs::*;
use super::{clock, irq, modify, rd, wr};
use crate::ring::Ring;
use crate::report::{UartStats, UART_COUNT};
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU8, Ordering::Relaxed};

/// Above the time driver and the executors; the handlers only fill the rings.
const PRIO: u8 = 3;

struct Hw {
    base: u32,
    en_reg: u32,
    en_bit: u32,
    irq: u16,
    tx: Pin,
    rx: Pin,
}

// A `static`: the receive interrupt reads it, and only named statics stay in the fast flash (memory.x).
static HW: [Hw; UART_COUNT] = [
    Hw { base: USART1, en_reg: CRM_APB2EN, en_bit: CRM_APB2_USART1, irq: irq::USART1, tx: pin(GPIOA, 9), rx: pin(GPIOA, 10) },
    Hw { base: USART3, en_reg: CRM_APB1EN, en_bit: CRM_APB1_USART3, irq: irq::USART3, tx: pin(GPIOB, 10), rx: pin(GPIOB, 11) },
    Hw { base: UART4, en_reg: CRM_APB1EN, en_bit: CRM_APB1_UART4, irq: irq::UART4, tx: pin(GPIOC, 10), rx: pin(GPIOC, 11) },
];

// Bluetooth and the cable: room for a whole second link frame while the first is being applied (LINK-41.1: two
// `WRITE_MANY` frames in flight; 256 + the port's 64-byte ring held 320 of the 498 bytes two frames take).
static BUF_BT: [AtomicU8; 512] = [const { AtomicU8::new(0) }; 512];
static BUF_GPS: [AtomicU8; 1024] = [const { AtomicU8::new(0) }; 1024];
static BUF_CPS: [AtomicU8; 512] = [const { AtomicU8::new(0) }; 512];
static RX: [Ring<'static>; UART_COUNT] = [Ring::new(&BUF_BT), Ring::new(&BUF_GPS), Ring::new(&BUF_CPS)];
static RX_BYTES: [AtomicU32; UART_COUNT] = [const { AtomicU32::new(0) }; UART_COUNT];
static LINE_ERRORS: [AtomicU32; UART_COUNT] = [const { AtomicU32::new(0) }; UART_COUNT];
static TX_BYTES: [AtomicU32; UART_COUNT] = [const { AtomicU32::new(0) }; UART_COUNT];
/// The line errors by kind (STS bits 1 framing, 3 overrun; noise is the rest of `LINE_ERRORS`): a wrong rate shows as
/// framing errors, a receiver too slow to empty the data register as overruns.
static FRAMING: [AtomicU32; UART_COUNT] = [const { AtomicU32::new(0) }; UART_COUNT];
static OVERRUN: [AtomicU32; UART_COUNT] = [const { AtomicU32::new(0) }; UART_COUNT];
/// TERM-13.1 (issue #236): bytes were lost on this port (its ring full, or an overrun) and the gap isn't marked
/// yet: the next byte that finds room for two goes in behind [`ferrite_core::link::LOST`]. The cable and Bluetooth
/// only ([`MARKS`]); the GPS's NMEA has its own checksums.
static LOST: [AtomicBool; UART_COUNT] = [const { AtomicBool::new(false) }; UART_COUNT];
static MARKS: [bool; UART_COUNT] = [true, false, true];
/// The rate each port was last started at (0: never), for the bench's `bt stat`.
static RATE: [AtomicU32; UART_COUNT] = [const { AtomicU32::new(0) }; UART_COUNT];

pub fn init(id: usize, baud: u32) {
    let u = &HW[id];
    irq::disable(u.irq);
    modify(u.en_reg, 0, u.en_bit);
    wr(u.base + USART_CTRL1, 0);
    RX[id].reset();
    LOST[id].store(false, Relaxed);
    u.tx.output(true, Mode::AfPp); // idle high before the peripheral takes the pin
    u.rx.config(Mode::InPu);
    wr(u.base + USART_BAUDR, clock::tree().baudr(baud)); // both APBs alike (PWR-24.9)
    RATE[id].store(baud, Relaxed);
    rd(u.base + USART_STS);
    rd(u.base + USART_DT);
    wr(u.base + USART_CTRL1, USART_CTRL1_UEN | USART_CTRL1_TEN | USART_CTRL1_REN | USART_CTRL1_RDBFIEN);
    irq::enable(u.irq, PRIO);
}

/// Whether this port reaches `baud` within 2 % from its bus clock (the divisor is 16 at least).
/// Both APBs run alike, so the port doesn't matter; the check is at the current speed and at Low Power Mode's
/// (PWR-24.9: a port set up now is re-timed for it).
pub fn rate_ok(_id: usize, baud: u32) -> bool {
    clock::tree().baud_ok(baud) && clock::LOW.baud_ok(baud)
}

/// SND-14: the TX pin a GPIO output held high, the UART still enabled (its receive works; what it shifts out
/// reaches no pin). Stock holds PC10 so outside programming mode: a speaker mic takes its bias from it. Call after
/// [`flush`]; [`init`] puts the pin back on the UART, so call again after it. Never low: that would cut the bias.
pub fn tx_pin_quiet(id: usize) {
    HW[id].tx.output(true, Mode::OutPp);
}

/// The TX pin back on the UART (idle high: no edge on the way).
pub fn tx_pin_live(id: usize) {
    HW[id].tx.output(true, Mode::AfPp);
}

/// Until the last byte written has left the pin (before a rate change).
pub fn flush(id: usize) {
    let base = HW[id].base;
    if rd(base + USART_CTRL1) & USART_CTRL1_UEN != 0 {
        while rd(base + USART_STS) & USART_STS_TDC == 0 {}
    }
}

/// Every port that is on, until its last byte has left (before a speed switch, PWR-24.9).
pub fn flush_all() {
    for id in 0..UART_COUNT {
        flush(id);
    }
}

/// [`flush_all`] for a speed switch, interrupts off: the bytes coming in meanwhile taken as the interrupt would
/// (decision 362).
pub(super) fn flush_all_rx() {
    for u in &HW {
        if rd(u.base + USART_CTRL1) & USART_CTRL1_UEN != 0 {
            while rd(u.base + USART_STS) & USART_STS_TDC == 0 {
                poll_rx();
            }
        }
    }
}

/// Decision 362: each port's waiting byte taken, as its interrupt would, while a speed switch holds interrupts off.
pub(super) fn poll_rx() {
    for (id, u) in HW.iter().enumerate() {
        if rd(u.base + USART_CTRL1) & USART_CTRL1_UEN != 0 {
            take_rx(id, u.base, rd(u.base + USART_STS));
        }
    }
}

/// PWR-24.9: each port that is on at its rate from tree `t` (interrupts are off; nothing is being sent:
/// [`flush_all_rx`] came first). Decision 362: also at the switch's start, from the crystal ([`CRYSTAL`]), so a
/// byte arriving while the PLL locks comes in whole; a rate the crystal can't reach (the cable's 921600) keeps
/// its divisor, and a byte then may be lost.
///
/// [`CRYSTAL`]: ferrite_core::clocktree::CRYSTAL
pub(super) fn retime(t: ferrite_core::clocktree::Tree) {
    for (id, u) in HW.iter().enumerate() {
        let baud = RATE[id].load(Relaxed);
        if baud != 0 && (t != ferrite_core::clocktree::CRYSTAL || t.baud_ok(baud)) && rd(u.base + USART_CTRL1) & USART_CTRL1_UEN != 0 {
            // Written with the port on: turning it off for this could pull the TX pin low (a break on the cable).
            wr(u.base + USART_BAUDR, t.baudr(baud));
        }
    }
}

/// A new rate for a port that is on, written with it on (turning it off could pull the TX pin low: a break on the
/// cable). Call [`flush`] first; a byte arriving across the change may be lost. The reader image's `fdump`.
pub fn set_rate(id: usize, baud: u32) {
    wr(HW[id].base + USART_BAUDR, clock::tree().baudr(baud));
    RATE[id].store(baud, Relaxed);
}

/// Two stop bits (true) or one on a port that is on (USART_CTRL2 STOPBN, bits 12-13): the reader's link tests.
pub fn set_stop2(id: usize, two: bool) {
    const USART_CTRL2: u32 = 0x10;
    let a = HW[id].base + USART_CTRL2;
    wr(a, (rd(a) & !(3 << 12)) | if two { 2 << 12 } else { 0 });
}

/// Drops what has come and not been taken.
pub fn clear_rx(id: usize) {
    RX[id].clear();
    LOST[id].store(false, Relaxed);
}

/// Off: both pins analog so a powered-down module is not fed, and the port's clock gated (PWR-23).
pub fn deinit(id: usize) {
    let u = &HW[id];
    irq::disable(u.irq);
    wr(u.base + USART_CTRL1, 0);
    // Analog: no input buffer left on a floating pin.
    u.tx.config(Mode::Analog);
    u.rx.config(Mode::Analog);
    RX[id].clear();
    LOST[id].store(false, Relaxed);
    modify(u.en_reg, u.en_bit, 0);
}

pub fn write(id: usize, data: &[u8]) {
    let base = HW[id].base;
    if rd(base + USART_CTRL1) & USART_CTRL1_UEN == 0 {
        return;
    }
    TX_BYTES[id].fetch_add(data.len() as u32, Relaxed);
    for &b in data {
        while rd(base + USART_STS) & USART_STS_TDBE == 0 {}
        wr(base + USART_DT, b as u32);
    }
}

/// The transmit data register is free: a byte written now goes without waiting. Bluetooth's 9600 baud takes about
/// 1 ms a byte, too long to wait for in a polled loop that shares its executor (TERM-4), so it sends only then.
pub fn tx_free(id: usize) -> bool {
    let base = HW[id].base;
    rd(base + USART_CTRL1) & USART_CTRL1_UEN != 0 && rd(base + USART_STS) & USART_STS_TDBE != 0
}

/// The rate the port was last started at (0: never).
pub fn rate(id: usize) -> u32 {
    RATE[id].load(Relaxed)
}

/// Framing and overrun errors so far.
pub fn errors(id: usize) -> (u32, u32) {
    (FRAMING[id].load(Relaxed), OVERRUN[id].load(Relaxed))
}

pub fn getc(id: usize) -> Option<u8> {
    RX[id].get()
}

/// Bytes received and not yet taken (the cable's flow control watches it).
pub fn rx_count(id: usize) -> u16 {
    RX[id].count()
}

pub fn stats(id: usize) -> UartStats {
    UartStats {
        rx_bytes: RX_BYTES[id].load(Relaxed),
        tx_bytes: TX_BYTES[id].load(Relaxed),
        dropped: RX[id].dropped(),
        line_errors: LINE_ERRORS[id].load(Relaxed),
    }
}

/// The ring the transmit interrupt drains ([`tx_ring`]; null: [`send_async`]'s bytes, or polled writes).
static TX_RING: [AtomicPtr<Ring<'static>>; UART_COUNT] = [const { AtomicPtr::new(core::ptr::null_mut()) }; UART_COUNT];
/// Why nothing may leave now, a bit each ([`HOLD_LINK`], [`HOLD_KEYED`]): the interrupt stops at the next byte.
static TX_HOLD: [AtomicU8; UART_COUNT] = [const { AtomicU8::new(0) }; UART_COUNT];
/// The Link task's own reasons (TX-43 as it sees it, SND-14's speaker mic, a byte of its own to write).
pub const HOLD_LINK: u8 = 1;
/// TX-43 (decision 290): the Radio task holds the cable the moment it keys (without Dry TX).
pub const HOLD_KEYED: u8 = 2;

/// Decision 290: the transmit interrupt sends `ring` byte by byte until it is empty, so the port keeps sending while
/// the thread-mode tasks are busy (the UI task's redraws took up to 200 ms, and LINK-13's audio waited behind them).
/// Called again whenever bytes may be waiting: it only (re)arms the interrupt. Nothing goes while held ([`tx_hold`]).
pub fn tx_ring(id: usize, ring: &'static Ring<'static>) {
    let base = HW[id].base;
    if rd(base + USART_CTRL1) & USART_CTRL1_UEN == 0 || TX_HOLD[id].load(Relaxed) != 0 || ring.count() == 0 {
        return;
    }
    TX_RING[id].store(ring as *const Ring<'static> as *mut Ring<'static>, Relaxed);
    modify(base + USART_CTRL1, 0, USART_CTRL1_TDBEIEN);
}

/// Held for `why` (`on`) or let go: while any reason holds, the transmit interrupt sends nothing more (the byte in
/// the shift register finishes) until let go and [`tx_ring`] is called again.
pub fn tx_hold(id: usize, why: u8, on: bool) {
    if on {
        TX_HOLD[id].fetch_or(why, Relaxed);
        modify(HW[id].base + USART_CTRL1, USART_CTRL1_TDBEIEN, 0);
    } else {
        TX_HOLD[id].fetch_and(!why, Relaxed);
    }
}

/// The bytes [`send_async`] has still to send: from, to (addresses).
static TX_FROM: [AtomicU32; UART_COUNT] = [const { AtomicU32::new(0) }; UART_COUNT];
static TX_TO: [AtomicU32; UART_COUNT] = [const { AtomicU32::new(0) }; UART_COUNT];

/// Sends `data` from the interrupt, byte by byte, and returns at once (the reader's `fdumpz` compresses the next
/// chunk meanwhile). Static data only; it must stay as it is until [`tx_busy`] says false: the caller's promise.
pub fn send_async(id: usize, data: &'static [u8]) {
    while tx_busy(id) {}
    let base = HW[id].base;
    TX_BYTES[id].fetch_add(data.len() as u32, Relaxed);
    TX_RING[id].store(core::ptr::null_mut(), Relaxed);
    TX_FROM[id].store(data.as_ptr() as u32, Relaxed);
    TX_TO[id].store(data.as_ptr() as u32 + data.len() as u32, Relaxed);
    modify(base + USART_CTRL1, 0, USART_CTRL1_TDBEIEN);
}

/// A [`send_async`] still has bytes to hand to the port.
pub fn tx_busy(id: usize) -> bool {
    rd(HW[id].base + USART_CTRL1) & USART_CTRL1_TDBEIEN != 0
}

/// The interrupt handler of port `id`.
pub fn isr(id: usize) {
    let base = HW[id].base;
    let sts = rd(base + USART_STS);
    if rd(base + USART_CTRL1) & USART_CTRL1_TDBEIEN != 0 && sts & USART_STS_TDBE != 0 {
        let ring = TX_RING[id].load(Relaxed);
        if !ring.is_null() {
            // SAFETY: only `&'static Ring`s are stored ([`tx_ring`]).
            // A held port takes nothing from the ring: what waits stays for later.
            match if TX_HOLD[id].load(Relaxed) != 0 { None } else { unsafe { &*ring }.get() } {
                Some(b) => {
                    wr(base + USART_DT, b as u32);
                    TX_BYTES[id].fetch_add(1, Relaxed);
                }
                None => modify(base + USART_CTRL1, USART_CTRL1_TDBEIEN, 0),
            }
        } else {
        let from = TX_FROM[id].load(Relaxed);
        if from < TX_TO[id].load(Relaxed) {
            // SAFETY: send_async's caller keeps the bytes in place until tx_busy is false.
            wr(base + USART_DT, unsafe { *(from as *const u8) } as u32);
            TX_FROM[id].store(from + 1, Relaxed);
        } else {
            modify(base + USART_CTRL1, USART_CTRL1_TDBEIEN, 0);
        }
        }
    }
    take_rx(id, base, sts);
}

/// The receive half of [`isr`]: a byte (or a line error) in the port, into its ring.
#[inline(always)]
fn take_rx(id: usize, base: u32, sts: u32) {
    if sts & (USART_STS_RDBF | USART_STS_ERR) != 0 {
        let b = rd(base + USART_DT) as u8; // STS then DT read clears the flags
        if sts & USART_STS_ERR != 0 {
            LINE_ERRORS[id].fetch_add(1, Relaxed);
            if sts & USART_STS_FERR != 0 {
                FRAMING[id].fetch_add(1, Relaxed);
            }
            if sts & USART_STS_ROERR != 0 {
                OVERRUN[id].fetch_add(1, Relaxed);
                LOST[id].store(MARKS[id], Relaxed); // a byte the port had no time to hand over
            }
        }
        if sts & USART_STS_RDBF != 0 {
            RX_BYTES[id].fetch_add(1, Relaxed);
            // TERM-13.1: the gap marked once there is room for the mark and this byte; until then this byte is
            // lost too (counted by the ring), and a byte lost with nothing marked starts a gap.
            let r = &RX[id];
            if !LOST[id].load(Relaxed) {
                if !r.put(b) && MARKS[id] {
                    LOST[id].store(true, Relaxed);
                }
            } else if r.room() >= 2 {
                r.put(ferrite_core::link::LOST);
                r.put(b);
                LOST[id].store(false, Relaxed);
            } else {
                r.drop_one();
            }
        }
    }
}
