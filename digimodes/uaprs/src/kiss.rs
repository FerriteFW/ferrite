//! KISS (Chepponis and Karn, 1987): AX.25 frames between a TNC and a host over a serial line. Each frame is
//! `FEND, command, data, FEND`, with FEND and FESC in the data escaped. Command 0x00 (port 0) carries a frame.
//! Command 12 is ACKMODE (BPQ, Dire Wolf, Winlink Packet): the host asks for it, and the TNC echoes each frame
//! back with command 12 once that frame has gone out, so the host starts its retry timer then.

pub const FEND: u8 = 0xC0;
pub const FESC: u8 = 0xDB;
pub const TFEND: u8 = 0xDC;
pub const TFESC: u8 = 0xDD;
/// Command 12: ACKMODE. A byte of 1 turns it on, 0 off. A report uses this command and the frame that went out.
pub const ACKMODE: u8 = 12;

/// Data frame on port `port` (0 to 15): the KISS bytes, one at a time through `out`.
pub fn encode(port: u8, frame: &[u8], out: impl FnMut(u8)) {
    encode_cmd(port, 0, frame, out);
}

/// As [`encode`], with a command nibble. An ACKMODE report is command [`ACKMODE`].
pub fn encode_cmd(port: u8, cmd: u8, frame: &[u8], mut out: impl FnMut(u8)) {
    out(FEND);
    out((port & 15) << 4 | cmd & 15);
    for &b in frame {
        match b {
            FEND => {
                out(FESC);
                out(TFEND);
            }
            FESC => {
                out(FESC);
                out(TFESC);
            }
            b => out(b),
        }
    }
    out(FEND);
}

/// A received KISS frame: port, command (0 = data; 1 to 6 are TNC settings such as TXDELAY) and its bytes.
#[derive(Debug, PartialEq, Eq)]
pub struct Received<'a> {
    pub port: u8,
    pub command: u8,
    pub data: &'a [u8],
}

/// Serial bytes in, frames out. `N` bytes of buffer for the type byte and the frame: the default, 330, holds the
/// type byte and the longest AX.25 frame without FCS (328: 10 addresses, control, PID, 256 bytes of information).
pub struct Decoder<const N: usize = 330> {
    buf: [u8; N],
    len: u16,
    in_frame: bool,
    escape: bool,
    overflow: bool,
}

impl<const N: usize> Default for Decoder<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Decoder<N> {
    pub const fn new() -> Self {
        const { assert!(N <= u16::MAX as usize, "buffer up to 65535 bytes") };
        Decoder { buf: [0; N], len: 0, in_frame: false, escape: false, overflow: false }
    }

    /// Feed one byte. Returns a frame when one ends; frames that overflowed the buffer are dropped.
    pub fn push(&mut self, b: u8) -> Option<Received<'_>> {
        if b == FEND {
            // A frame cut off in the middle of an escape is dropped.
            let done = self.in_frame && self.len > 0 && !self.overflow && !self.escape;
            let len = self.len as usize;
            self.in_frame = true;
            self.len = 0;
            self.escape = false;
            self.overflow = false;
            if done {
                // The first byte is the type: port in the high nibble, command in the low.
                return Some(Received { port: self.buf[0] >> 4, command: self.buf[0] & 15, data: &self.buf[1..len] });
            }
            return None;
        }
        if !self.in_frame {
            return None;
        }
        let b = if self.escape {
            self.escape = false;
            match b {
                TFEND => FEND,
                TFESC => FESC,
                // The spec: any other byte after FESC "is an error; no action is taken and frame assembly
                // continues". The byte is dropped.
                _ => return None,
            }
        } else if b == FESC {
            self.escape = true;
            return None;
        } else {
            b
        };
        if self.len as usize == N {
            self.overflow = true;
        } else {
            self.buf[self.len as usize] = b;
            self.len += 1;
        }
        None
    }
}

/// The TNC settings a host can send (KISS commands 1 to 5, and 12), in KISS's units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    /// Keying delay before data, 10 ms units (command 1). Default 50 (500 ms).
    pub tx_delay: u8,
    /// Persistence P (command 2): transmit in a free slot with probability (P + 1) / 256. Default 63 (0.25).
    pub persistence: u8,
    /// Slot time, 10 ms units (command 3). Default 10 (100 ms).
    pub slot_time: u8,
    /// Time to keep transmitting after the data, 10 ms units (command 4, obsolete but sent by some hosts).
    pub tx_tail: u8,
    /// Command 5, kept as sent. [`Access`] never acts on it: a half-duplex radio always listens before it
    /// transmits.
    pub full_duplex: bool,
    /// Command 12, ACKMODE. Off until a host asks: nothing is echoed after a transmission.
    pub ack_mode: bool,
}

impl Default for Params {
    fn default() -> Self {
        Params { tx_delay: 50, persistence: 63, slot_time: 10, tx_tail: 0, full_duplex: false, ack_mode: false }
    }
}

impl Received<'_> {
    /// For a single-port TNC: port 0's frames, and Return (0xFF). The spec: "In systems with only one HDLC
    /// port, it is by definition Port 0"; a frame for another port is ignored.
    pub fn for_single_port(&self) -> bool {
        self.port == 0 || (self.port == 15 && self.command == 15)
    }
}

/// What a received KISS frame asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Command<'a> {
    /// Transmit this AX.25 frame (without FCS).
    Data(&'a [u8]),
    /// A setting was changed in [`Params`].
    Set,
    /// Command 6, SetHardware: device-specific bytes.
    Hardware(&'a [u8]),
    /// 0xFF: leave KISS mode.
    Exit,
    /// Unknown command, or a setting without its value.
    Unknown,
}

impl Params {
    /// Applies a received frame: settings update `self`; data and the rest are returned to act on.
    pub fn apply<'a>(&mut self, r: &Received<'a>) -> Command<'a> {
        if r.port == 15 && r.command == 15 {
            return Command::Exit; // type byte 0xFF
        }
        let v = r.data.first().copied();
        match (r.command, v) {
            (0, _) => return Command::Data(r.data),
            (6, _) => return Command::Hardware(r.data),
            (1, Some(v)) => self.tx_delay = v,
            (2, Some(v)) => self.persistence = v,
            (3, Some(v)) => self.slot_time = v,
            (4, Some(v)) => self.tx_tail = v,
            (5, Some(v)) => self.full_duplex = v != 0,
            (ACKMODE, Some(v)) => self.ack_mode = v != 0,
            _ => return Command::Unknown,
        }
        Command::Set
    }

    /// TX delay and tail as HDLC flags at `baud` (for uafsk's `TxConfig` preamble and postamble); at least 1
    /// each, since a frame needs its flags.
    pub fn flags(&self, baud: u32) -> (u16, u16) {
        // 10 ms units x baud / 8 bits per flag / 100.
        let n = |t: u8| ((t as u32 * baud).div_ceil(800)).max(1) as u16;
        (n(self.tx_delay), n(self.tx_tail))
    }

    /// Closing flags of a 1200-baud KISS transmission: the modem's usual three, or the host's TX tail when
    /// command 4 asked for more. A tail of 0 stays three flags, which is what a frame goes out with today.
    pub fn closing_flags(&self) -> u16 {
        const MIN: u16 = 3;
        if self.tx_tail == 0 { MIN } else { self.flags(1200).1.max(MIN) }
    }

    /// Flags around one frame of a key-up at 1200 baud. The first frame gets the TX delay; a later frame gets
    /// none, so the previous frame's closing flag opens it and the window stays one transmission. Only the last
    /// frame carries [`Params::closing_flags`]; the others end on a single flag.
    pub fn frame_flags(&self, first: bool, last: bool) -> (u16, u16) {
        let pre = if first { self.flags(1200).0 } else { 0 };
        let post = if last { self.closing_flags() } else { 1 };
        (pre, post)
    }
}

/// Channel access by p-persistence, as KISS TNCs do it: while the channel is busy, wait; when it is clear, send
/// with probability (P + 1) / 256, otherwise wait a slot time and try again.
#[derive(Clone, Copy, Debug, Default)]
pub struct Access {
    /// When the next try may happen, milliseconds (wrapping clock).
    next: Option<u32>,
}

impl Access {
    /// Call while a frame is waiting: may it be sent now? `now_ms` is any wrapping millisecond clock, `busy`
    /// the channel (the demodulator's carrier detect and the squelch), `random` a fresh random byte each call.
    /// The host's full duplex (command 5) never skips the channel check: the radio is half duplex, and a frame
    /// from an app is never sent over someone else's.
    pub fn may_send(&mut self, p: &Params, now_ms: u32, busy: bool, random: u8) -> bool {
        if busy {
            self.next = None;
            return false;
        }
        if let Some(t) = self.next {
            if (now_ms.wrapping_sub(t) as i32) < 0 {
                return false; // still waiting out a slot
            }
        }
        if random <= p.persistence {
            self.next = None;
            true
        } else {
            self.next = Some(now_ms.wrapping_add(p.slot_time as u32 * 10));
            false
        }
    }
}
