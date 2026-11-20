//! The KISS transmit window (Winlink Packet, and any host that sends more than one AX.25 frame at a time).
//!
//! Eight slots, AX.25's modulo-8 window, in BSS: the APRS task already lives in the UI task's stack frame for
//! the whole power-on, and eight frames are about 2.7 KB. The UI task queues, the DSP task sends. A slot is
//! free, queued, on air, or done (sent, waiting for the ACKMODE echo). Order stays oldest at the head: done,
//! then the one on air, then queued.

use super::dsp::Frame;
use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU8, Ordering::Relaxed};
use critical_section::Mutex;
use ferrite_app::aprs_live::FRAME_MAX;
use ferrite_core::audio::uafsk::hdlc::{FlagBits, Framer};

const WINDOW: usize = 8;
const FREE: u8 = 0;
const QUEUED: u8 = 1;
const ON_AIR: u8 = 2;
const DONE: u8 = 3;

const EMPTY: Frame = Frame { len: 0, txdelay_10ms: 0, fx25: 0, level: 1, b: [0; FRAME_MAX] };

struct Ring {
    frame: [Frame; WINDOW],
    state: [u8; WINDOW],
    head: u8,
    len: u8,
}

impl Ring {
    const fn new() -> Self {
        Ring { frame: [EMPTY; WINDOW], state: [FREE; WINDOW], head: 0, len: 0 }
    }

    fn tail(&self) -> usize {
        (self.head as usize + self.len as usize) % WINDOW
    }

    fn push(&mut self, frame: Frame) -> bool {
        if self.len as usize == WINDOW {
            return false;
        }
        let i = self.tail();
        self.frame[i] = frame;
        self.state[i] = QUEUED;
        self.len += 1;
        true
    }

    fn free_head(&mut self) {
        self.state[self.head as usize] = FREE;
        self.head = (self.head + 1) % WINDOW as u8;
        self.len -= 1;
    }

    fn has_queued(&self) -> bool {
        let mut i = self.head;
        for _ in 0..self.len {
            if self.state[i as usize] == QUEUED {
                return true;
            }
            i = (i + 1) % WINDOW as u8;
        }
        false
    }

    /// The oldest queued frame, now on air.
    fn take_queued(&mut self) -> Option<Frame> {
        let mut i = self.head;
        for _ in 0..self.len {
            if self.state[i as usize] == QUEUED {
                self.state[i as usize] = ON_AIR;
                return Some(self.frame[i as usize]);
            }
            i = (i + 1) % WINDOW as u8;
        }
        None
    }

    fn drop_oldest_queued(&mut self) -> bool {
        if self.len == 0 || self.state[self.head as usize] != QUEUED {
            return false;
        }
        self.free_head();
        true
    }

    /// Queued frames behind a transmission already keyed. The one on air, and any done, stay.
    fn drop_queued(&mut self) {
        while self.len > 0 {
            let i = (self.head as usize + self.len as usize - 1) % WINDOW;
            if self.state[i] != QUEUED {
                break;
            }
            self.state[i] = FREE;
            self.len -= 1;
        }
    }

    fn finish_on_air(&mut self) -> bool {
        let mut i = self.head;
        for _ in 0..self.len {
            if self.state[i as usize] == ON_AIR {
                self.state[i as usize] = DONE;
                return true;
            }
            i = (i + 1) % WINDOW as u8;
        }
        false
    }

    fn pop_done(&mut self) -> Option<Frame> {
        if self.len == 0 || self.state[self.head as usize] != DONE {
            return None;
        }
        let frame = self.frame[self.head as usize];
        self.free_head();
        Some(frame)
    }

    fn abandon_on_air(&mut self) {
        if self.len > 0 && self.state[self.head as usize] == ON_AIR {
            self.free_head();
        }
    }

    fn clear(&mut self) {
        *self = Self::new();
    }
}

static RING: Mutex<RefCell<Ring>> = Mutex::new(RefCell::new(Ring::new()));
/// The continuation frame's bytes, borrowed by the modulator. Rewritten only after that borrow ends.
static HOLD: Mutex<RefCell<Frame>> = Mutex::new(RefCell::new(EMPTY));

/// Set by the UI task immediately before it hands the DSP task the first frame of a KISS key-up.
static BURST: AtomicBool = AtomicBool::new(false);
/// Closing flags for the last frame of the key-up (at least 3). The UI task writes it from the host's TX tail.
static TAIL: AtomicU16 = AtomicU16::new(3);
/// Continuation frames whose audio has started, for the UI task to count as sent.
static EXTRA: AtomicU8 = AtomicU8::new(0);
/// KISS frames whose bits have finished, so the UI task can extend the play timeout.
static FINISHED: AtomicU8 = AtomicU8::new(0);

pub fn pending() -> bool {
    critical_section::with(|cs| RING.borrow_ref(cs).len > 0)
}

pub fn has_queued() -> bool {
    critical_section::with(|cs| RING.borrow_ref(cs).has_queued())
}

/// A data frame from the host. `false` when the window is full: the frame is dropped, and the host retries.
pub fn push(frame: Frame) -> bool {
    critical_section::with(|cs| RING.borrow_ref_mut(cs).push(frame))
}

/// The oldest queued frame, marked on air. The UI task keys on this one.
pub fn take_for_air() -> Option<Frame> {
    critical_section::with(|cs| RING.borrow_ref_mut(cs).take_queued())
}

/// KISS-13: the channel stayed busy, so the oldest queued frame is dropped and the one behind it can try.
pub fn drop_oldest_queued() -> bool {
    critical_section::with(|cs| RING.borrow_ref_mut(cs).drop_oldest_queued())
}

pub fn drop_queued() {
    critical_section::with(|cs| RING.borrow_ref_mut(cs).drop_queued());
}

/// The key-up was refused: the frame never went out, so it is not echoed.
pub fn abandon_on_air() {
    critical_section::with(|cs| RING.borrow_ref_mut(cs).abandon_on_air());
}

pub fn clear() {
    critical_section::with(|cs| RING.borrow_ref_mut(cs).clear());
}

/// A frame the DSP task has finished sending, oldest first. The UI task echoes it when ACKMODE is on.
pub fn pop_done() -> Option<Frame> {
    critical_section::with(|cs| RING.borrow_ref_mut(cs).pop_done())
}

pub fn arm_burst() {
    BURST.store(true, Relaxed);
}

pub fn take_burst() -> bool {
    BURST.swap(false, Relaxed)
}

pub fn set_tail(flags: u16) {
    TAIL.store(flags.max(1), Relaxed);
}

pub fn take_extra() -> u8 {
    EXTRA.swap(0, Relaxed)
}

pub fn finished() -> u8 {
    FINISHED.load(Relaxed)
}

fn finish_on_air() {
    let found = critical_section::with(|cs| RING.borrow_ref_mut(cs).finish_on_air());
    if found {
        FINISHED.fetch_add(1, Relaxed);
        crate::chan::TERM_WAKE.signal(());
    }
}

/// Copy the next queued frame into [`HOLD`] and mark it on air. The slice borrows [`HOLD`].
fn stage_next() -> Option<&'static [u8]> {
    critical_section::with(|cs| {
        let frame = RING.borrow_ref_mut(cs).take_queued()?;
        let mut hold = HOLD.borrow_ref_mut(cs);
        *hold = frame;
        let len = hold.len as usize;
        let ptr = hold.b.as_ptr();
        // SAFETY: `ptr` is HOLD's frame bytes. stage_next is the only writer, and it runs again only after
        // the Framer borrowing this slice has been dropped.
        Some(unsafe { core::slice::from_raw_parts(ptr, len) })
    })
}

/// Bits of one KISS key-up. The first frame carries the TX delay and one closing flag. Each frame queued
/// behind it follows with no delay of its own, so that flag opens it. Then the TX tail, and if a frame
/// arrived during the tail it goes out before the key-up ends.
pub struct KissBits<'a> {
    src: Src<'a>,
}

enum Src<'a> {
    Frame(Framer<'a>),
    Tail(FlagBits),
    End,
}

impl<'a> KissBits<'a> {
    pub fn starting(frame: &'a [u8], preamble: u16) -> Self {
        KissBits { src: Src::Frame(Framer::new(frame, preamble, 1)) }
    }
}

impl Iterator for KissBits<'_> {
    type Item = u8;
    fn next(&mut self) -> Option<u8> {
        loop {
            match &mut self.src {
                Src::Frame(f) => {
                    if let Some(b) = f.next() {
                        return Some(b);
                    }
                }
                Src::Tail(t) => {
                    if let Some(b) = t.next() {
                        return Some(b);
                    }
                }
                Src::End => return None,
            }
            let from_frame = matches!(self.src, Src::Frame(_));
            // Drop the Framer before stage_next rewrites the buffer it may borrow.
            self.src = Src::End;
            if from_frame {
                finish_on_air();
            }
            if let Some(bytes) = stage_next() {
                self.src = Src::Frame(Framer::new(bytes, 0, 1));
                EXTRA.fetch_add(1, Relaxed);
                continue;
            }
            if from_frame {
                let extra = TAIL.load(Relaxed).saturating_sub(1);
                if extra > 0 {
                    self.src = Src::Tail(FlagBits::new(extra));
                    continue;
                }
            }
            return None;
        }
    }
}
