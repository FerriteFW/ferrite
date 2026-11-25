//! ACARS (chapter 27b, on trial): the VHF air-ground messages' frame after its SOH, parsed into fixed fields
//! ([`parse`]), blocks of one message joined and the last few messages kept in RAM ([`Log`]), and what the common
//! labels mean ([`label`]). Pure logic, no heap; the radio's modem (the BK4829's FSK at 2400 bit/s, ferrite#192)
//! is not connected yet.
//!
//! A frame, as it comes after the bit sync, the two SYNs and SOH, each character 7-bit ASCII with odd parity:
//!
//! ```text
//! mode (1) | registration (7, dot-padded) | technical ack (1: NAK or a letter) | label (2) | block ID (1)
//! | STX, text | ETX (the last block) or ETB (more follow) | CRC (2, low byte first) | DEL
//! ```
//!
//! With no text, ETX (or ETB) comes straight after the block ID. A block ID that is a digit says the aircraft sent
//! it (a downlink); its text starts with the message number (4, `M01A`: the last letter is the block's place in
//! the message) and the flight (6, `AC0123`). A letter is a ground station's (an uplink), with neither.
//!
//! The check is CRC-16/KERMIT (polynomial 0x1021 reflected, so 0x8408 shifting right; initial 0; no final XOR)
//! over every byte from the mode to the ETX or ETB as received, parity bits included. [`tests`] builds a frame
//! with [`encode`] and checks it, and the standard check value of "123456789" (0x2189).

/// The control characters a frame uses.
pub const SOH: u8 = 0x01;
pub const STX: u8 = 0x02;
pub const ETX: u8 = 0x03;
pub const NAK: u8 = 0x15;
pub const SYN: u8 = 0x16;
pub const ETB: u8 = 0x17;
pub const DEL: u8 = 0x7F;

/// The most text one block carries (with the message number and flight on a downlink).
pub const TEXT_MAX: usize = 220;
/// Mode, registration, ack, label and block ID: the fixed part after SOH.
const HEAD: usize = 12;

/// CRC-16/KERMIT, bit at a time (a frame is under 250 bytes, a few a minute: no table in the image).
pub fn crc16(data: &[u8]) -> u16 {
    let mut c = 0u16;
    for &b in data {
        c ^= b as u16;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0x8408 } else { c >> 1 };
        }
    }
    c
}

/// The byte with bit 7 set so it has an odd number of ones (as sent).
pub const fn with_parity(c: u8) -> u8 {
    let c = c & 0x7F;
    if c.count_ones().is_multiple_of(2) { c | 0x80 } else { c }
}

/// Why bytes aren't a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Shorter than the fixed part, its end and the CRC.
    Short,
    /// After the block ID came neither STX nor ETX/ETB.
    NoStart,
    /// No ETX or ETB after the text.
    NoEnd,
    /// The CRC's two bytes are missing.
    NoCrc,
}

/// One block, parsed: every field fixed-size, the text with its parity bits taken off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub mode: u8,
    /// The aircraft's registration as sent, dot-padded on the left (`.C-FTJO`); [`Frame::reg`] trims it.
    pub reg_raw: [u8; 7],
    /// NAK (none) or the letter of the block acknowledged.
    pub ack: u8,
    /// Two characters; `_` and DEL is the general response, shown as `_d` ([`label_text`]).
    pub label: [u8; 2],
    pub block: u8,
    /// A downlink's message number (`M01A`) and flight (`AC0123`); zeros when the frame has none.
    pub msgno: [u8; 4],
    pub flight: [u8; 6],
    text: [u8; TEXT_MAX],
    len: u8,
    /// It ended with ETB: more blocks of the message follow.
    pub more: bool,
    pub crc_ok: bool,
    /// The CRC as received (two copies of one block heard have the same).
    pub crc: u16,
    /// Bytes from the mode to the end whose parity was even (wrong).
    pub parity: u8,
}

impl Frame {
    const EMPTY: Frame = Frame {
        mode: 0,
        reg_raw: [0; 7],
        ack: 0,
        label: [0; 2],
        block: 0,
        msgno: [0; 4],
        flight: [0; 6],
        text: [0; TEXT_MAX],
        len: 0,
        more: false,
        crc_ok: false,
        crc: 0,
        parity: 0,
    };

    /// From a ground station (block ID not a digit).
    pub fn up(&self) -> bool {
        !self.block.is_ascii_digit()
    }

    /// The free text (after a downlink's message number and flight).
    pub fn text(&self) -> &[u8] {
        &self.text[..self.len as usize]
    }

    pub fn reg(&self) -> &[u8] {
        trim_reg(&self.reg_raw)
    }
}

/// A registration without its leading dots (and any NUL padding).
pub fn trim_reg(r: &[u8]) -> &[u8] {
    let s = r.iter().position(|&c| c != b'.').unwrap_or(r.len());
    let e = r.iter().rposition(|&c| c != 0 && c != b'.').map_or(s, |e| e + 1);
    &r[s..e.max(s)]
}

/// A field as text: up to the first NUL.
pub fn field(f: &[u8]) -> &[u8] {
    &f[..f.iter().position(|&c| c == 0).unwrap_or(f.len())]
}

/// A label as shown: DEL as `d` (`_d`, the general response).
pub fn label_text(l: [u8; 2]) -> [u8; 2] {
    l.map(|c| if c == DEL { b'd' } else { c })
}

/// The bytes after SOH, up to and including the CRC (a DEL after it, or anything else, is ignored).
pub fn parse(b: &[u8]) -> Result<Frame, Error> {
    if b.len() < HEAD + 1 + 2 {
        return Err(Error::Short);
    }
    let c = |x: u8| x & 0x7F;
    let mut f = Frame::EMPTY;
    f.mode = c(b[0]);
    for (d, &s) in f.reg_raw.iter_mut().zip(&b[1..8]) {
        *d = c(s);
    }
    f.ack = c(b[8]);
    f.label = [c(b[9]), c(b[10])];
    f.block = c(b[11]);
    let (start, end) = match c(b[HEAD]) {
        STX => (HEAD + 1, (HEAD + 1..b.len()).find(|&i| matches!(c(b[i]), ETX | ETB)).ok_or(Error::NoEnd)?),
        ETX | ETB => (HEAD, HEAD),
        _ => return Err(Error::NoStart),
    };
    if b.len() < end + 3 {
        return Err(Error::NoCrc);
    }
    f.more = c(b[end]) == ETB;
    f.parity = b[..=end].iter().filter(|x| x.count_ones().is_multiple_of(2)).count().min(255) as u8;
    f.crc = u16::from_le_bytes([b[end + 1], b[end + 2]]);
    f.crc_ok = crc16(&b[..=end]) == f.crc;
    let mut t = &b[start..end];
    if !f.up() && start > HEAD {
        let n = t.len().min(4);
        for (d, &s) in f.msgno.iter_mut().zip(&t[..n]) {
            *d = c(s);
        }
        t = &t[n..];
        let n = t.len().min(6);
        for (d, &s) in f.flight.iter_mut().zip(&t[..n]) {
            *d = c(s);
        }
        t = &t[n..];
    }
    let n = t.len().min(TEXT_MAX);
    for (d, &s) in f.text.iter_mut().zip(&t[..n]) {
        *d = c(s);
    }
    f.len = n as u8;
    Ok(f)
}

/// What [`encode`] sends.
#[derive(Clone, Copy, Debug, Default)]
pub struct Fields<'a> {
    pub mode: u8,
    /// Up to 7 characters; dots fill the left.
    pub reg: &'a [u8],
    pub ack: u8,
    pub label: [u8; 2],
    pub block: u8,
    /// A downlink's message number and flight, sent first in the text when the text isn't empty.
    pub msgno: &'a [u8],
    pub flight: &'a [u8],
    pub text: &'a [u8],
    /// ETB instead of ETX.
    pub more: bool,
}

/// A frame's bytes after SOH, as sent (parity, CRC, DEL), into `out`; returns its length (0: `out` too short).
/// For the samples (the simulator, `acars feed`) and the tests.
pub fn encode(f: &Fields, out: &mut [u8]) -> usize {
    let body = f.msgno.len().min(4) + f.flight.len().min(6) + f.text.len();
    let has_text = body > 0;
    let need = HEAD + 1 + if has_text { body + 1 } else { 0 } + 3;
    if out.len() < need {
        return 0;
    }
    let mut n = 0;
    let mut put = |c: u8| {
        out[n] = with_parity(c);
        n += 1;
    };
    put(f.mode);
    let r = &f.reg[..f.reg.len().min(7)];
    for _ in r.len()..7 {
        put(b'.');
    }
    r.iter().for_each(|&c| put(c));
    put(f.ack);
    put(f.label[0]);
    put(f.label[1]);
    put(f.block);
    if has_text {
        put(STX);
        f.msgno[..f.msgno.len().min(4)].iter().chain(&f.flight[..f.flight.len().min(6)]).chain(f.text).for_each(|&c| put(c));
    }
    put(if f.more { ETB } else { ETX });
    let crc = crc16(&out[..n]);
    out[n..n + 2].copy_from_slice(&crc.to_le_bytes());
    out[n + 2] = DEL;
    n + 3
}

// ---- labels -------------------------------------------------------------------------------------------------------

/// What a label is: a tag for the list (up to 4 characters) and a short meaning. Written for Ferrite from the
/// labels commonly heard; airlines use many more of their own.
pub struct Label {
    pub code: [u8; 2],
    pub tag: &'static str,
    pub meaning: &'static str,
}

const fn l(code: &[u8; 2], tag: &'static str, meaning: &'static str) -> Label {
    Label { code: *code, tag, meaning }
}

/// The labels Ferrite names, in code order for the binary search.
pub static LABELS: [Label; 32] = [
    l(b"10", "AIRL", "Airline: free text"),
    l(b"5U", "WX", "Weather request"),
    l(b"5Z", "AIRL", "Airline designated"),
    l(b"80", "OOOI", "OOOI/position"),
    l(b"A1", "ATC", "Oceanic clearance"),
    l(b"A3", "ATC", "Departure clearance"),
    l(b"A9", "ATIS", "ATIS report"),
    l(b"AA", "ATC", "FANS CPDLC"),
    l(b"B1", "ATC", "Oceanic clearance request"),
    l(b"B3", "ATC", "Departure clearance request"),
    l(b"B6", "ADS", "ADS-C report"),
    l(b"BA", "ATC", "FANS CPDLC"),
    l(b"C1", "PRT", "To the cockpit printer"),
    l(b"H1", "TEXT", "Message to/from crew"),
    l(b"Q0", "TEST", "Link test"),
    l(b"Q1", "OOOI", "Departure/arrival report"),
    l(b"Q2", "ETA", "ETA report"),
    l(b"Q3", "TIME", "Clock update"),
    l(b"Q7", "DLAY", "Delay report"),
    l(b"QA", "OUT", "Out, with fuel"),
    l(b"QB", "OFF", "Off report"),
    l(b"QC", "ON", "On report"),
    l(b"QD", "IN", "In, with fuel"),
    l(b"QE", "OUT", "Out, with destination"),
    l(b"QF", "OFF", "Off, with destination"),
    l(b"QM", "IN", "Arrival information"),
    l(b"QN", "DIV", "Diversion report"),
    l(b"RA", "PRT", "To the cockpit printer"),
    l(b"SA", "LINK", "Media advisory"),
    l(b"SQ", "SQ", "Ground station squitter"),
    l(b"_j", "ACK", "No information"),
    l(b"_\x7f", "ACK", "General response"),
];

/// A numbered label not in [`LABELS`] (airlines define their own).
static AIRLINE: Label = l(b"00", "AIRL", "Airline defined");
static UNKNOWN: Label = l(b"??", "MSG", "Unknown label");

/// What label `code` (as received, DEL and all) is.
pub fn label(code: [u8; 2]) -> &'static Label {
    match LABELS.binary_search_by(|x| x.code.cmp(&code)) {
        Ok(i) => &LABELS[i],
        Err(_) if code[0].is_ascii_digit() => &AIRLINE,
        Err(_) => &UNKNOWN,
    }
}

// ---- messages -----------------------------------------------------------------------------------------------------

/// A message kept: a block, or blocks joined. Its text is in the [`Log`]'s pool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Msg {
    pub reg: [u8; 7],
    pub flight: [u8; 6],
    pub msgno: [u8; 4],
    pub label: [u8; 2],
    pub mode: u8,
    pub ack: u8,
    /// The first block's ID.
    pub block: u8,
    /// [`CRC_BAD`], [`UP`], [`CUT`], [`GAP`].
    pub flags: u8,
    /// Blocks joined (1 for a single block).
    pub blocks: u8,
    /// Characters with bad parity, all blocks.
    pub parity: u8,
    /// The frequency it came on, as the caller numbers them.
    pub freq: u8,
    /// When it came (the caller's seconds; ages wrap).
    pub at_s: u32,
    at: u16,
    len: u16,
}

/// The CRC failed: kept and shown, marked.
pub const CRC_BAD: u8 = 1;
/// From a ground station.
pub const UP: u8 = 2;
/// Text cut: joined blocks past [`JOIN_MAX`].
pub const CUT: u8 = 4;
/// Blocks missing: a block's letter skipped one, or the last block never came ([`JOIN_S`]).
pub const GAP: u8 = 8;

impl Msg {
    fn of(f: &Frame, freq: u8, now_s: u32) -> Msg {
        let flags = if f.crc_ok { 0 } else { CRC_BAD } | if f.up() { UP } else { 0 };
        Msg {
            reg: f.reg_raw,
            flight: f.flight,
            msgno: f.msgno,
            label: f.label,
            mode: f.mode,
            ack: f.ack,
            block: f.block,
            flags,
            blocks: 1,
            parity: f.parity,
            freq,
            at_s: now_s,
            at: 0,
            len: 0,
        }
    }

    pub fn up(&self) -> bool {
        self.flags & UP != 0
    }

    pub fn crc_ok(&self) -> bool {
        self.flags & CRC_BAD == 0
    }

    pub fn reg(&self) -> &[u8] {
        trim_reg(&self.reg)
    }

    /// The flight, trimmed (empty: none).
    pub fn flight(&self) -> &[u8] {
        field(&self.flight).trim_ascii()
    }

    /// The airline's two-letter code from the flight (IATA: letters or a digit with a letter), if it has one.
    pub fn airline(&self) -> Option<[u8; 2]> {
        let f = self.flight();
        let ok = f.len() >= 3 && f[..2].iter().all(|c| c.is_ascii_alphanumeric()) && !f[..2].iter().all(u8::is_ascii_digit);
        ok.then(|| [f[0], f[1]])
    }

    /// Same message, another block: same aircraft, and the same message number (but its block letter) on a
    /// downlink, the same label and mode on an uplink.
    fn joins(&self, n: &Msg) -> bool {
        self.reg == n.reg
            && self.up() == n.up()
            && if self.up() { (self.label, self.mode) == (n.label, n.mode) } else { self.msgno[..3] == n.msgno[..3] && self.flight == n.flight }
    }
}

/// Joined text kept for one message: two blocks; more is cut ([`CUT`]).
pub const JOIN_MAX: usize = 2 * TEXT_MAX;
/// Messages being joined at once.
pub const PARTS: usize = 2;
/// A message whose next block hasn't come in this long is kept as it is, marked [`GAP`].
pub const JOIN_S: u32 = 30;
/// A block heard again (the same CRC) within this long is the same block resent: kept once.
pub const DUP_S: u32 = 60;
/// The CRCs remembered for that.
const RECENT: usize = 8;

#[derive(Clone, Copy)]
struct Part {
    m: Msg,
    text: [u8; JOIN_MAX],
    len: u16,
    /// The last block's time, and the block letter expected next (a downlink's message number's last character).
    last_s: u32,
    next: u8,
    used: bool,
}

/// The last `N` messages, newest first, their text in a `POOL`-byte ring (a message whose text is written over goes
/// with it), and the messages being joined.
pub struct Log<const N: usize, const POOL: usize> {
    msgs: [Msg; N],
    head: u8,
    count: u8,
    pool: [u8; POOL],
    wr: u16,
    parts: [Part; PARTS],
    /// The last [`RECENT`] CRCs and when they came ([`DUP_S`]).
    recent: [(u16, u32); RECENT],
    recent_at: u8,
    /// Moves on with every change (the screens draw again).
    pub version: u32,
    /// Frames since power-on, those with a bad CRC, and copies dropped.
    pub frames: u16,
    pub bad: u16,
    pub dups: u16,
}

impl<const N: usize, const POOL: usize> Default for Log<N, POOL> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize, const POOL: usize> Log<N, POOL> {
    const PART0: Part = Part { m: Msg { reg: [0; 7], flight: [0; 6], msgno: [0; 4], label: [0; 2], mode: 0, ack: 0, block: 0, flags: 0, blocks: 0, parity: 0, freq: 0, at_s: 0, at: 0, len: 0 }, text: [0; JOIN_MAX], len: 0, last_s: 0, next: 0, used: false };

    pub const fn new() -> Self {
        assert!(N <= 255 && POOL <= u16::MAX as usize && POOL >= JOIN_MAX);
        Log {
            msgs: [Self::PART0.m; N],
            head: 0,
            count: 0,
            pool: [0; POOL],
            wr: 0,
            parts: [Self::PART0; PARTS],
            recent: [(0, 0); RECENT],
            recent_at: 0,
            version: 0,
            frames: 0,
            bad: 0,
            dups: 0,
        }
    }

    pub fn len(&self) -> u16 {
        self.count as u16
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Message `i`, 0 the newest, and its text.
    pub fn get(&self, i: u16) -> Option<(Msg, &[u8])> {
        if i >= self.count as u16 {
            return None;
        }
        let m = self.msgs[(self.head as usize + N - 1 - i as usize) % N];
        Some((m, &self.pool[m.at as usize..(m.at + m.len) as usize]))
    }

    pub fn clear(&mut self) {
        let v = self.version;
        *self = Self::new();
        self.version = v.wrapping_add(1);
    }

    /// A frame heard at `now_s` on frequency `freq`: kept, joined to the message it continues, or dropped as a copy.
    pub fn push(&mut self, f: &Frame, freq: u8, now_s: u32) {
        self.tick(now_s);
        self.frames = self.frames.saturating_add(1);
        if !f.crc_ok {
            self.bad = self.bad.saturating_add(1);
        } else if self.recent.iter().any(|&(c, t)| c == f.crc && t != 0 && now_s.wrapping_sub(t) < DUP_S) {
            self.dups = self.dups.saturating_add(1);
            return;
        } else {
            self.recent[self.recent_at as usize % RECENT] = (f.crc, now_s | 1);
            self.recent_at = self.recent_at.wrapping_add(1);
        }
        let m = Msg::of(f, freq, now_s);
        // a block with a bad CRC stands alone: its fields can't be trusted to join it
        if f.crc_ok {
            if let Some(k) = self.parts.iter().position(|p| p.used && p.m.joins(&m)) {
                let p = &mut self.parts[k];
                if !p.m.up() && m.msgno[3] != p.next {
                    p.m.flags |= GAP;
                }
                p.m.blocks = p.m.blocks.saturating_add(1);
                p.m.parity = p.m.parity.saturating_add(m.parity);
                (p.last_s, p.next) = (now_s, m.msgno[3].wrapping_add(1));
                let n = f.text().len().min(JOIN_MAX - p.len as usize);
                if n < f.text().len() {
                    p.m.flags |= CUT;
                }
                p.text[p.len as usize..p.len as usize + n].copy_from_slice(&f.text()[..n]);
                p.len += n as u16;
                if !f.more {
                    self.finish(k);
                }
                return;
            }
            if f.more {
                let k = match self.parts.iter().position(|p| !p.used) {
                    Some(k) => k,
                    None => {
                        let k = if self.parts[0].last_s.wrapping_sub(self.parts[1].last_s) as i32 <= 0 { 0 } else { 1 };
                        self.parts[k].m.flags |= GAP;
                        self.finish(k);
                        k
                    }
                };
                let p = &mut self.parts[k];
                *p = Part { m, text: [0; JOIN_MAX], len: f.text().len() as u16, last_s: now_s, next: m.msgno[3].wrapping_add(1), used: true };
                p.text[..f.text().len()].copy_from_slice(f.text());
                return;
            }
        }
        self.keep(m, f.text());
    }

    /// Time passes: a message whose next block is overdue is kept as it is ([`JOIN_S`]).
    pub fn tick(&mut self, now_s: u32) {
        for k in 0..PARTS {
            if self.parts[k].used && now_s.wrapping_sub(self.parts[k].last_s) >= JOIN_S {
                self.parts[k].m.flags |= GAP;
                self.finish(k);
            }
        }
    }

    fn finish(&mut self, k: usize) {
        let p = &mut self.parts[k];
        p.used = false;
        let (m, len) = (p.m, p.len as usize);
        // the pool is borrowed below: copy the text out a piece at a time (no second buffer on the stack)
        let at = self.room(len);
        let p = &self.parts[k];
        self.pool[at..at + len].copy_from_slice(&p.text[..len]);
        self.put(Msg { at: at as u16, len: len as u16, ..m });
    }

    fn keep(&mut self, m: Msg, text: &[u8]) {
        let at = self.room(text.len());
        self.pool[at..at + text.len()].copy_from_slice(text);
        self.put(Msg { at: at as u16, len: text.len() as u16, ..m });
    }

    /// Where `len` bytes of text go in the pool: at the write point, or at its start when they don't fit before the
    /// end; the messages whose text that covers are dropped (the oldest, as the pool is written in turn).
    fn room(&mut self, len: usize) -> usize {
        let mut a = self.wr as usize;
        if a + len > POOL {
            a = 0;
        }
        let b = a + len;
        if len > 0 {
            let mut drop = 0;
            for k in 0..self.count as usize {
                let m = &self.msgs[(self.head as usize + N - self.count as usize + k) % N];
                if m.len > 0 && (m.at as usize) < b && a < (m.at + m.len) as usize {
                    drop = k + 1;
                }
            }
            self.count -= drop as u8;
        }
        self.wr = b as u16;
        a
    }

    fn put(&mut self, m: Msg) {
        self.msgs[self.head as usize] = m;
        self.head = ((self.head as usize + 1) % N) as u8;
        self.count = (self.count + 1).min(N as u8);
        self.version = self.version.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn frame(f: &Fields) -> Vec<u8> {
        let mut b = [0u8; 400];
        let n = encode(f, &mut b);
        assert!(n > 0);
        b[..n].to_vec()
    }

    const DOWN: Fields = Fields {
        mode: b'2',
        reg: b"C-FIUA",
        ack: NAK,
        label: *b"H1",
        block: b'3',
        msgno: b"M01A",
        flight: b"AC0115",
        text: b"POSN49123W123158,0712,350,CYVR,KSFO",
        more: false,
    };

    /// ACARS-2.2: the CRC variant: CRC-16/KERMIT's check value, and a built frame's CRC sits low byte first after ETX.
    #[test]
    fn crc_is_kermit() {
        assert_eq!(crc16(b"123456789"), 0x2189);
        let b = frame(&DOWN);
        let end = b.iter().position(|&c| c & 0x7F == ETX).unwrap();
        assert_eq!(u16::from_le_bytes([b[end + 1], b[end + 2]]), crc16(&b[..=end]));
        // KERMIT is reflected with no final XOR: the CRC over the frame and its CRC is 0
        assert_eq!(crc16(&b[..end + 3]), 0);
        assert_eq!(b[end + 3], DEL);
    }

    /// ACARS-2.2: every character sent with odd parity.
    #[test]
    fn every_byte_has_odd_parity() {
        for c in 0..128u8 {
            assert_eq!(with_parity(c).count_ones() % 2, 1);
            assert_eq!(with_parity(c) & 0x7F, c);
        }
        let b = frame(&DOWN);
        let end = b.len() - 3;
        assert!(b[..end].iter().all(|c| c.count_ones() % 2 == 1));
    }

    /// ACARS-2.1, ACARS-2.5: a downlink's fields, its message number and flight.
    #[test]
    fn parses_a_downlink() {
        let f = parse(&frame(&DOWN)).unwrap();
        assert!(f.crc_ok && !f.up() && !f.more);
        assert_eq!((f.mode, f.reg(), &f.reg_raw, f.ack), (b'2', &b"C-FIUA"[..], b".C-FIUA", NAK));
        assert_eq!((&f.label, f.block, &f.msgno, &f.flight), (b"H1", b'3', b"M01A", b"AC0115"));
        assert_eq!(f.text(), b"POSN49123W123158,0712,350,CYVR,KSFO");
        assert_eq!(f.parity, 0);
    }

    /// ACARS-2.5: an uplink (a letter block ID) has no message number or flight.
    #[test]
    fn parses_an_uplink() {
        let up = Fields { block: b'A', msgno: b"", flight: b"", label: *b"RA", text: b"METAR CYVR 240700Z", ack: b'2', ..DOWN };
        let f = parse(&frame(&up)).unwrap();
        assert!(f.crc_ok && f.up());
        assert_eq!((f.msgno, f.flight, f.ack), ([0; 4], [0; 6], b'2'));
        assert_eq!(f.text(), b"METAR CYVR 240700Z");
    }

    /// ACARS-2.1: no text, ETX straight after the block ID.
    #[test]
    fn no_text_has_etx_after_the_block_id() {
        let q0 = Fields { label: *b"Q0", msgno: b"", flight: b"", text: b"", ..DOWN };
        let b = frame(&q0);
        assert_eq!(b[HEAD] & 0x7F, ETX);
        let f = parse(&b).unwrap();
        assert!(f.crc_ok && f.text().is_empty() && f.msgno == [0; 4]);
        assert_eq!(label(f.label).meaning, "Link test");
    }

    /// ACARS-10: `_` and DEL is the general response, shown `_d`.
    #[test]
    fn general_response_label_is_shown_as_underscore_d() {
        let f = parse(&frame(&Fields { label: [b'_', DEL], text: b"", ..DOWN })).unwrap();
        assert_eq!(label_text(f.label), *b"_d");
        assert_eq!(label(f.label).meaning, "General response");
    }

    /// ACARS-2.5: a downlink too short for its number and flight keeps what came.
    #[test]
    fn short_downlink_text_fills_what_it_has() {
        let f = parse(&frame(&Fields { msgno: b"M02", flight: b"", text: b"", ..DOWN })).unwrap();
        assert_eq!((&f.msgno, f.flight, f.text()), (b"M02\0", [0; 6], &b""[..]));
    }

    /// ACARS-2.2: a bad CRC or parity is counted; the frame is still read.
    #[test]
    fn a_bad_crc_or_parity_is_counted_not_refused() {
        let mut b = frame(&DOWN);
        b[20] ^= 0x01; // one bit: parity and CRC both fail
        let f = parse(&b).unwrap();
        assert!(!f.crc_ok);
        assert_eq!(f.parity, 1);
        let mut b = frame(&DOWN);
        let end = b.len() - 3;
        b[end + 1] ^= 0xFF; // the CRC itself
        let f = parse(&b).unwrap();
        assert!(!f.crc_ok && f.parity == 0);
    }

    /// ACARS-2.1: bytes without a frame's shape are refused.
    #[test]
    fn broken_frames_are_errors() {
        let b = frame(&DOWN);
        assert_eq!(parse(&b[..5]), Err(Error::Short));
        let mut x = b.clone();
        x[HEAD] = with_parity(b'X');
        assert_eq!(parse(&x), Err(Error::NoStart));
        let end = b.len() - 4; // the ETX
        assert_eq!(parse(&b[..end]), Err(Error::NoEnd));
        assert_eq!(parse(&b[..end + 2]), Err(Error::NoCrc));
        assert!(parse(&b[..end + 3]).is_ok(), "the DEL is optional");
    }

    /// ACARS-2.1: a block's text is at most 220 characters.
    #[test]
    fn text_past_220_is_cut() {
        let long = [b'X'; 300];
        let f = parse(&frame(&Fields { msgno: b"", flight: b"", block: b'B', text: &long, ..DOWN })).unwrap();
        assert!(f.crc_ok);
        assert_eq!(f.text().len(), TEXT_MAX);
    }

    /// ACARS-10: the label table, sorted for its search; other numbered labels are airlines'.
    #[test]
    fn labels_are_sorted_and_found() {
        assert!(LABELS.windows(2).all(|w| w[0].code < w[1].code));
        assert!(LABELS.iter().all(|l| !l.tag.is_empty() && l.tag.len() <= 4 && l.meaning.len() <= 28));
        assert_eq!(label(*b"H1").meaning, "Message to/from crew");
        assert_eq!(label(*b"5Z").tag, "AIRL");
        assert_eq!(label(*b"47").meaning, "Airline defined");
        assert_eq!(label(*b"ZZ").meaning, "Unknown label");
    }

    /// ACARS-4, ACARS-6: the registration without its dots; the airline code from the flight.
    #[test]
    fn registrations_and_airlines() {
        assert_eq!(trim_reg(b"..N1234"), b"N1234");
        assert_eq!(trim_reg(b"......."), b"");
        let m = Msg { flight: *b"AC0115", ..Msg::default() };
        assert_eq!(m.airline(), Some(*b"AC"));
        assert_eq!(Msg { flight: *b"2L0412", ..m }.airline(), Some(*b"2L"));
        assert_eq!(Msg { flight: *b"123456", ..m }.airline(), None);
        assert_eq!(Msg { flight: [0; 6], ..m }.airline(), None);
    }

    type L = Log<8, 1024>;

    fn push(l: &mut L, f: &Fields, now: u32) {
        l.push(&parse(&frame(f)).unwrap(), 0, now);
    }

    /// ACARS-8: the log, newest first.
    #[test]
    fn log_keeps_newest_first() {
        let mut l = L::new();
        push(&mut l, &DOWN, 10);
        push(&mut l, &Fields { reg: b"C-GWSZ", text: b"SECOND", ..DOWN }, 11);
        assert_eq!(l.len(), 2);
        let (m, t) = l.get(0).unwrap();
        assert_eq!((m.reg(), t, m.at_s), (&b"C-GWSZ"[..], &b"SECOND"[..], 11));
        assert_eq!(l.get(1).unwrap().1, DOWN.text);
        assert!(l.get(2).is_none());
    }

    /// ACARS-2.3: a block heard again within 60 s is kept once; long after, it is a new message.
    #[test]
    fn a_resent_block_is_kept_once() {
        let mut l = L::new();
        push(&mut l, &DOWN, 10);
        push(&mut l, &DOWN, 20);
        assert_eq!((l.len(), l.dups), (1, 1));
        push(&mut l, &DOWN, 10 + DUP_S + 1);
        assert_eq!(l.len(), 2, "long after, it's a new message");
    }

    /// ACARS-2.4: a downlink's blocks joined by message number, another message between them.
    #[test]
    fn blocks_join_by_message_number() {
        let mut l = L::new();
        let a = Fields { msgno: b"M05A", text: b"FIRST BLOCK ", more: true, ..DOWN };
        let b = Fields { msgno: b"M05B", block: b'4', text: b"SECOND", more: false, ..DOWN };
        push(&mut l, &a, 10);
        assert_eq!(l.len(), 0, "waiting for the rest");
        // another aircraft's message in between
        push(&mut l, &Fields { reg: b"N37502", flight: b"UA0947", ..DOWN }, 11);
        push(&mut l, &b, 12);
        assert_eq!(l.len(), 2);
        let (m, t) = l.get(0).unwrap();
        assert_eq!((t, m.blocks, m.flags, m.block, &m.msgno), (&b"FIRST BLOCK SECOND"[..], 2, 0, b'3', b"M05A"));
    }

    /// ACARS-2.4: a skipped block letter, or a last block that never came, marks it.
    #[test]
    fn a_missing_block_is_marked() {
        let mut l = L::new();
        push(&mut l, &Fields { msgno: b"M05A", text: b"ONE", more: true, ..DOWN }, 10);
        push(&mut l, &Fields { msgno: b"M05C", text: b"THREE", ..DOWN }, 11);
        let (m, t) = l.get(0).unwrap();
        assert_eq!((t, m.flags & GAP), (&b"ONETHREE"[..], GAP));
        // and one whose last block never comes is kept after JOIN_S
        push(&mut l, &Fields { msgno: b"M06A", text: b"ALONE", more: true, ..DOWN }, 20);
        l.tick(20 + JOIN_S - 1);
        assert_eq!(l.len(), 1);
        l.tick(20 + JOIN_S);
        let (m, t) = l.get(0).unwrap();
        assert_eq!((t, m.flags & GAP), (&b"ALONE"[..], GAP));
    }

    /// ACARS-2.4: joined text past 440 characters is cut and marked.
    #[test]
    fn joined_text_past_join_max_is_cut() {
        let mut l = L::new();
        let big = [b'Y'; 210];
        for (i, k) in b"ABC".iter().copied().enumerate() {
            let no = [b'M', b'0', b'7', k];
            push(&mut l, &Fields { msgno: &no, text: &big, more: i < 2, ..DOWN }, 10 + i as u32);
        }
        let (m, t) = l.get(0).unwrap();
        assert_eq!((t.len(), m.blocks, m.flags & CUT), (JOIN_MAX, 3, CUT));
    }

    /// ACARS-2.2: a block with a bad CRC is kept alone, never joined.
    #[test]
    fn a_bad_block_stands_alone() {
        let mut l = L::new();
        let mut b = frame(&Fields { more: true, ..DOWN });
        b[30] ^= 1;
        l.push(&parse(&b).unwrap(), 0, 10);
        let (m, _) = l.get(0).unwrap();
        assert!(!m.crc_ok());
        assert_eq!((l.bad, l.frames), (1, 1));
    }

    /// ACARS-2.4: an uplink's blocks joined by aircraft, label and mode.
    #[test]
    fn uplinks_join_by_label() {
        let mut l = L::new();
        let up = Fields { block: b'A', msgno: b"", flight: b"", label: *b"C1", text: b"PART ONE ", more: true, ..DOWN };
        push(&mut l, &up, 10);
        push(&mut l, &Fields { block: b'B', text: b"PART TWO", more: false, ..up }, 11);
        let (m, t) = l.get(0).unwrap();
        assert!(m.up());
        assert_eq!((t, m.blocks), (&b"PART ONE PART TWO"[..], 2));
    }

    /// ACARS-2.4: two messages joined at once; a third finishes the older.
    #[test]
    fn three_joins_at_once_finish_the_oldest() {
        let mut l = L::new();
        for (k, r) in [&b"C-AAAA"[..], b"C-BBBB", b"C-CCCC"].into_iter().enumerate() {
            push(&mut l, &Fields { reg: r, msgno: b"M01A", text: b"X", more: true, ..DOWN }, 10 + k as u32);
        }
        let (m, _) = l.get(0).unwrap();
        assert_eq!((m.reg(), m.flags & GAP), (&b"C-AAAA"[..], GAP));
    }

    /// ACARS-8: the text pool drops the oldest messages first; 16 messages at most.
    #[test]
    fn the_pool_drops_the_oldest_text() {
        let mut l = L::new();
        let t = [b'Z'; 200];
        for k in 0..12u32 {
            let r = [b'C', b'-', b'F', b'A', b'A', b'A' + k as u8];
            push(&mut l, &Fields { reg: &r, text: &t, ..DOWN }, 100 + k * 100);
        }
        // 1024 bytes hold five 200-byte texts; every one kept reads back whole
        assert!(l.len() >= 4 && l.len() <= 5, "{}", l.len());
        for i in 0..l.len() {
            let (m, tx) = l.get(i).unwrap();
            assert_eq!(tx, &t[..]);
            assert_eq!(m.reg[6], b'A' + 11 - i as u8);
        }
        // and N caps it with short texts
        for k in 0..20u32 {
            push(&mut l, &Fields { text: b"", reg: &[b'N', b'0' + (k % 10) as u8, b'0' + (k / 10) as u8], ..DOWN }, 5000 + k * 100);
        }
        assert_eq!(l.len(), 8);
    }

    /// ACARS-8: clearing (`acars clear`) empties the log and redraws.
    #[test]
    fn clear_moves_the_version() {
        let mut l = L::new();
        push(&mut l, &DOWN, 1);
        let v = l.version;
        l.clear();
        assert!(l.is_empty() && l.version != v);
    }

    /// ACARS-2.1: any bytes (a property test): never a panic, and what parses stays inside its fields.
    #[test]
    fn any_bytes_never_panic() {
        let mut seed = 0x1234_5678_9ABC_DEF0u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let base = frame(&Fields { more: true, ..DOWN });
        let mut l = L::new();
        for i in 0..20_000u32 {
            let mut b: Vec<u8> = if i % 2 == 0 { base.clone() } else { (0..next() % 300).map(|_| next() as u8).collect() };
            for _ in 0..next() % 4 {
                if !b.is_empty() {
                    let k = next() as usize % b.len();
                    b[k] = next() as u8;
                }
            }
            b.truncate(next() as usize % 320);
            if let Ok(f) = parse(&b) {
                assert!(f.text().len() <= TEXT_MAX);
                l.push(&f, 0, i / 3);
            }
        }
        for i in 0..l.len() {
            assert!(l.get(i).unwrap().1.len() <= JOIN_MAX);
        }
    }
}
