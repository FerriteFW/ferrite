//! APRS Traffic (APRS-59, decision 463): the last packets heard, every kind, newest first, for the Traffic page
//! and the station screen's Message line (APRS-28). RAM only: one fixed byte ring of variable-length records,
//! never formatted text; a record is decoded only when it is drawn. Nothing goes to SPI flash, and power-off
//! empties it.
//!
//! A record: a header byte (the kind in bits 0-3, heard through a digipeater in bit 4, the count less one in
//! bits 5-7), the time in [`UNIT_S`] units (u16, little-endian), the source as a call field, then by kind a
//! call field (a message's addressee, a third-party packet's sender) and a text field (a message's text, an
//! ack's number, an object's name).
//!
//! A call field is a callsign packed in 5 bytes when it can be (`0x80 | ssid`, then the base of up to six
//! capitals and digits in base 37, u32 little-endian; ssid 0 none, n + 1 for "-n"), else its bytes as they came:
//! the length (under 0x80) and up to [`CALL_MAX`] bytes. A text field is its length and its bytes.

use core::cell::RefCell;
use ferrite_ui::text::Buf;

/// The ring's bytes: 128 beacons (8 bytes each), 68 short messages (15 bytes for "6" to TEST), about 32 of 30
/// characters.
pub const TRAFFIC_BYTES: usize = 1024;
/// The time's unit: a u16 of them covers 72 hours.
pub const UNIT_S: u32 = 4;
/// Records older than this many units go (before the u16 wraps and they look new again): about 68 hours.
const MAX_AGE_U: u16 = 0xF000;
/// A call field's raw bytes at most (an addressee is up to 9 characters).
pub const CALL_MAX: usize = 9;
/// A message's text kept (the station screen's Message line shows 32), an ack's number, an object's name.
pub const TEXT_MAX: usize = 32;
const ID_MAX: usize = 5;
const NAME_MAX: usize = 9;
/// The longest record: header, time, two raw call fields, a text field.
const REC_MAX: usize = 3 + 2 * (1 + CALL_MAX) + 1 + TEXT_MAX;
/// The count saturates here (shown as "x8").
pub const COUNT_MAX: u8 = 8;

/// What a packet was (APRS-59.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Kind {
    Message,
    Ack,
    Rej,
    Position,
    Status,
    Weather,
    Object,
    Item,
    Telemetry,
    Query,
    ThirdParty,
    Other,
}

impl Kind {
    const ALL: [Kind; 12] =
        [Kind::Message, Kind::Ack, Kind::Rej, Kind::Position, Kind::Status, Kind::Weather, Kind::Object, Kind::Item, Kind::Telemetry, Kind::Query, Kind::ThirdParty, Kind::Other];

    /// The word the Traffic page shows.
    pub fn name(self) -> &'static str {
        ["message", "ack", "rej", "position", "status", "weather", "object", "item", "telemetry", "query", "3rd party", "other"][self as usize]
    }

    /// A call field after the source: a message's addressee (acks, rejects and directed queries too), a
    /// third-party packet's own sender.
    fn has_call(self) -> bool {
        matches!(self, Kind::Message | Kind::Ack | Kind::Rej | Kind::Query | Kind::ThirdParty)
    }

    /// A text field, at most this long: a message's text, an ack's number, an object's name.
    fn text_max(self) -> usize {
        match self {
            Kind::Message | Kind::Query => TEXT_MAX,
            Kind::Ack | Kind::Rej => ID_MAX,
            Kind::Object | Kind::Item => NAME_MAX,
            _ => 0,
        }
    }
}

/// A record decoded, for drawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rec {
    pub kind: Kind,
    /// Heard this many times in a row (the same kind from the same station, with the same text), at most
    /// [`COUNT_MAX`].
    pub count: u8,
    /// Every copy came through a digipeater (none heard direct).
    pub via: bool,
    pub ago_s: u32,
    pub src: Buf<10>,
    /// The addressee (or a third-party packet's sender); empty for other kinds.
    pub to: Buf<10>,
    /// The text, the ack's number or the object's name; empty for other kinds.
    pub text: Buf<32>,
}

/// Base 37: 0 ends the base, then the digits and the capitals.
fn digit37(c: u8) -> Option<u32> {
    match c {
        b'0'..=b'9' => Some((c - b'0') as u32 + 1),
        b'A'..=b'Z' => Some((c - b'A') as u32 + 11),
        _ => None,
    }
}

/// A callsign packed in 5 bytes, when it is up to six capitals and digits with no SSID or a plain number up to
/// 99 (written as it would be read back: "-05" and "-" stay raw).
pub fn pack_call(c: &[u8]) -> Option<[u8; 5]> {
    let (base, ssid) = match c.iter().position(|&b| b == b'-') {
        Some(k) => (&c[..k], Some(&c[k + 1..])),
        None => (c, None),
    };
    if base.is_empty() || base.len() > 6 {
        return None;
    }
    let code = match ssid {
        None => 0,
        Some(s) if !s.is_empty() && s.len() <= 2 && s.iter().all(u8::is_ascii_digit) && !(s.len() == 2 && s[0] == b'0') => {
            s.iter().fold(0u8, |n, &d| n * 10 + (d - b'0')) + 1
        }
        Some(_) => return None,
    };
    let mut v = 0u32;
    for k in 0..6 {
        v = v * 37 + if k < base.len() { digit37(base[k])? } else { 0 };
    }
    let [a, b, c2, d] = v.to_le_bytes();
    Some([0x80 | code, a, b, c2, d])
}

/// A packed callsign's text.
fn unpack_call(f: [u8; 5]) -> Buf<10> {
    let mut v = u32::from_le_bytes([f[1], f[2], f[3], f[4]]);
    let mut d = [0u8; 6];
    for k in (0..6).rev() {
        d[k] = (v % 37) as u8;
        v /= 37;
    }
    let mut b = Buf::<10>::new();
    for &x in d.iter().take_while(|&&x| x != 0) {
        b.push(&[if x <= 10 { b'0' + x - 1 } else { b'A' + x - 11 }]);
    }
    let code = f[0] & 0x7F;
    if code > 0 {
        let n = code - 1;
        b.push(b"-");
        if n >= 10 {
            b.push(&[b'0' + n / 10]);
        }
        b.push(&[b'0' + n % 10]);
    }
    b
}

/// The ring (inside a RefCell in [`Traffic`]).
struct Ring {
    b: [u8; TRAFFIC_BYTES],
    /// The oldest record's offset, the bytes in use, the records, the newest record's offset.
    start: u16,
    used: u16,
    n: u16,
    last: u16,
    /// The time now, in [`UNIT_S`] units (wrapping).
    now: u16,
}

impl Ring {
    fn at(&self, off: usize) -> u8 {
        self.b[off % TRAFFIC_BYTES]
    }

    /// A field's length at `off` (its first byte and what follows).
    fn field_len(&self, off: usize) -> usize {
        let f = self.at(off);
        if f & 0x80 != 0 { 5 } else { 1 + f as usize }
    }

    /// The record's length at `off`.
    fn rec_len(&self, off: usize) -> usize {
        let kind = Kind::ALL.get((self.at(off) & 0x0F) as usize).copied().unwrap_or(Kind::Other);
        let mut k = 3;
        k += self.field_len(off + k);
        if kind.has_call() {
            k += self.field_len(off + k);
        }
        if kind.text_max() > 0 {
            k += self.field_len(off + k);
        }
        k
    }

    fn time(&self, off: usize) -> u16 {
        u16::from_le_bytes([self.at(off + 1), self.at(off + 2)])
    }

    fn drop_oldest(&mut self) {
        let len = self.rec_len(self.start as usize);
        self.start = ((self.start as usize + len) % TRAFFIC_BYTES) as u16;
        self.used -= len as u16;
        self.n -= 1;
    }

    fn set_now(&mut self, now: u16) {
        self.now = now;
        while self.n > 0 && now.wrapping_sub(self.time(self.start as usize)) > MAX_AGE_U {
            self.drop_oldest();
        }
    }

    /// The record `r` (its first `len` bytes), heard now: the newest bumped when it is the same packet again,
    /// else added after it, the oldest dropped for room.
    fn push(&mut self, r: &[u8]) {
        let len = r.len();
        if self.n > 0 {
            let last = self.last as usize;
            let same = self.rec_len(last) == len && self.at(last) & 0x0F == r[0] & 0x0F && (3..len).all(|k| self.at(last + k) == r[k]);
            if same {
                let h = self.at(last);
                let count = (h >> 5) + 1;
                let count = if count < COUNT_MAX { count } else { COUNT_MAX - 1 };
                // via only while no copy came direct
                let h = (h & 0x0F) | (h & r[0] & 0x10) | (count << 5);
                self.b[last % TRAFFIC_BYTES] = h;
                self.b[(last + 1) % TRAFFIC_BYTES] = r[1];
                self.b[(last + 2) % TRAFFIC_BYTES] = r[2];
                return;
            }
        }
        while TRAFFIC_BYTES - (self.used as usize) < len {
            self.drop_oldest();
        }
        let at = (self.start as usize + self.used as usize) % TRAFFIC_BYTES;
        for (k, &x) in r.iter().enumerate() {
            self.b[(at + k) % TRAFFIC_BYTES] = x;
        }
        self.last = at as u16;
        self.used += len as u16;
        self.n += 1;
    }

    /// The offset of record `i`, newest first.
    fn offset(&self, i: u16) -> Option<usize> {
        let k = self.n.checked_sub(i + 1)?;
        let mut off = self.start as usize;
        for _ in 0..k {
            off += self.rec_len(off);
        }
        Some(off % TRAFFIC_BYTES)
    }

    /// A call field at `off` (its text) and its length.
    fn call_at(&self, off: usize) -> (Buf<10>, usize) {
        let f = self.at(off);
        if f & 0x80 != 0 {
            (unpack_call([f, self.at(off + 1), self.at(off + 2), self.at(off + 3), self.at(off + 4)]), 5)
        } else {
            let mut b = Buf::<10>::new();
            for k in 0..f as usize {
                b.push(&[self.at(off + 1 + k)]);
            }
            (b, 1 + f as usize)
        }
    }

    fn decode(&self, off: usize) -> Rec {
        let h = self.at(off);
        let kind = Kind::ALL.get((h & 0x0F) as usize).copied().unwrap_or(Kind::Other);
        let (src, mut k) = self.call_at(off + 3);
        k += 3;
        let mut to = Buf::new();
        if kind.has_call() {
            let (c, n) = self.call_at(off + k);
            to = c;
            k += n;
        }
        let mut text = Buf::new();
        if kind.text_max() > 0 {
            for j in 0..self.at(off + k) as usize {
                text.push(&[self.at(off + k + 1 + j)]);
            }
        }
        let ago_s = self.now.wrapping_sub(self.time(off)) as u32 * UNIT_S;
        Rec { kind, count: (h >> 5) + 1, via: h & 0x10 != 0, ago_s, src, to, text }
    }
}

/// A call field written at `r[k..]`: packed when it can be, else its bytes (at most [`CALL_MAX`]). The length.
fn put_call(r: &mut [u8; REC_MAX], k: usize, c: &[u8]) -> usize {
    match pack_call(c) {
        Some(p) => {
            r[k..k + 5].copy_from_slice(&p);
            5
        }
        None => put_text(r, k, c, CALL_MAX),
    }
}

fn put_text(r: &mut [u8; REC_MAX], k: usize, t: &[u8], max: usize) -> usize {
    let t = &t[..t.len().min(max)];
    r[k] = t.len() as u8;
    r[k + 1..k + 1 + t.len()].copy_from_slice(t);
    1 + t.len()
}

/// The packets heard lately (APRS-59): the APRS task's, kept in RAM.
pub struct Traffic {
    ring: RefCell<Ring>,
}

impl Default for Traffic {
    fn default() -> Self {
        Self::new()
    }
}

impl Traffic {
    pub const fn new() -> Self {
        Traffic { ring: RefCell::new(Ring { b: [0; TRAFFIC_BYTES], start: 0, used: 0, n: 0, last: 0, now: 0 }) }
    }

    /// The clock moved on (seconds since power-on): records past [`MAX_AGE_U`] go.
    pub fn set_now(&self, now_s: u32) {
        self.ring.borrow_mut().set_now((now_s / UNIT_S) as u16);
    }

    /// A packet heard now (APRS-59.2): `to` and `text` are kept for the kinds that have them, cut to their
    /// fields.
    #[inline(never)]
    pub fn push(&self, kind: Kind, via: bool, src: &[u8], to: &[u8], text: &[u8]) {
        let mut ring = self.ring.borrow_mut();
        let mut r = [0u8; REC_MAX];
        let [t0, t1] = ring.now.to_le_bytes();
        r[0] = kind as u8 | if via { 0x10 } else { 0 };
        r[1] = t0;
        r[2] = t1;
        let mut k = 3;
        k += put_call(&mut r, k, src);
        if kind.has_call() {
            k += put_call(&mut r, k, to);
        }
        if kind.text_max() > 0 {
            k += put_text(&mut r, k, text, kind.text_max());
        }
        ring.push(&r[..k]);
    }

    /// The records kept.
    pub fn len(&self) -> u16 {
        self.ring.borrow().n
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Record `i`, newest first.
    pub fn get(&self, i: u16) -> Option<Rec> {
        let ring = self.ring.borrow();
        let off = ring.offset(i)?;
        Some(ring.decode(off))
    }

    /// APRS-28: the newest message `call` was heard sending (to anyone) that is still kept.
    pub fn last_message(&self, call: &[u8]) -> Option<Rec> {
        let ring = self.ring.borrow();
        let mut off = ring.start as usize;
        let mut found = None;
        for _ in 0..ring.n {
            if ring.at(off) & 0x0F == Kind::Message as u8 && ring.call_at(off + 3).0.as_bytes() == call {
                found = Some(off % TRAFFIC_BYTES);
            }
            off += ring.rec_len(off);
        }
        found.map(|o| ring.decode(o))
    }

    /// Clear the heard list, the reset: everything goes.
    pub fn clear(&self) {
        let mut ring = self.ring.borrow_mut();
        (ring.start, ring.used, ring.n, ring.last) = (0, 0, 0, 0);
    }

    /// The bytes in use (tests, the size report).
    pub fn used(&self) -> u16 {
        self.ring.borrow().used
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn all(t: &Traffic) -> Vec<Rec> {
        (0..t.len()).map(|i| t.get(i).unwrap()).collect()
    }

    /// APRS-59.2: a callsign packs in 5 bytes and reads back the same; one that wouldn't read back the same
    /// (lower case, a long base, "-05", a letter SSID) is kept as its bytes.
    #[test]
    fn calls_pack_and_read_back() {
        for c in [&b"VE7GPL-1"[..], b"A", b"W1AW", b"VA7FER-15", b"ZZZZZZ-99", b"999999", b"TEST", b"EMAIL-2", b"N0CALL-0"] {
            let p = pack_call(c).unwrap_or_else(|| panic!("{}", core::str::from_utf8(c).unwrap()));
            assert_eq!(unpack_call(p).as_bytes(), c);
        }
        for c in [&b"ve7gpl"[..], b"ABCDEFG", b"VE7GPL-05", b"VE7GPL-A", b"VE7GPL-", b"-1", b"", b"BLN1WX-100", b"SPACE X"] {
            assert_eq!(pack_call(c), None, "{}", core::str::from_utf8(c).unwrap());
        }
    }

    /// APRS-59.1, APRS-59.2: every kind kept with its fields: a message's addressee and text (cut to 32), an
    /// ack's or reject's number, an object's name, a third-party packet's sender; the rest the source alone. A
    /// raw addressee (not a callsign) is kept as it came. Newest first, with their ages.
    #[test]
    fn every_kind_kept() {
        let t = Traffic::new();
        let long = b"a long message to someone else, longer than 32 characters";
        type Case<'a> = (Kind, &'a [u8], &'a [u8], &'a [u8]);
        let cases: [Case; 13] = [
            (Kind::Message, b"VE7GPL-1", b"TEST", b"6"),
            (Kind::Message, b"VE7GPL-2", b"BLN1wx", long),
            (Kind::Ack, b"VE7GPL-3", b"VA7FER-2", b"12"),
            (Kind::Rej, b"VE7GPL-4", b"VA7FER-2", b"123456"),
            (Kind::Position, b"VE7GPL-5", b"", b""),
            (Kind::Status, b"VE7GPL-6", b"", b""),
            (Kind::Weather, b"VE7GPL-7", b"", b""),
            (Kind::Object, b"VE7GPL-8", b"", b"LEADER   X"),
            (Kind::Item, b"VE7GPL-9", b"", b"AID#2"),
            (Kind::Telemetry, b"VE7GPL-10", b"", b""),
            (Kind::Query, b"VE7GPL-11", b"VA7FER", b"?APRSP"),
            (Kind::ThirdParty, b"VE7IGT-10", b"KA9ZZZ-9", b""),
            (Kind::Other, b"VE7GPL-12", b"ignored", b"ignored"),
        ];
        for (k, (kind, src, to, text)) in cases.iter().enumerate() {
            t.set_now(k as u32 * 8);
            t.push(*kind, k % 2 == 1, src, to, text);
        }
        t.set_now(200);
        let got = all(&t);
        assert_eq!(got.len(), cases.len());
        for (k, (kind, src, to, text)) in cases.iter().enumerate() {
            let r = got[cases.len() - 1 - k];
            assert_eq!((r.kind, r.src.as_bytes(), r.count, r.via), (*kind, *src, 1, k % 2 == 1), "{k}");
            assert_eq!(r.ago_s, 200 - k as u32 * 8, "{k}");
            let want_to: &[u8] = if kind.has_call() { to } else { b"" };
            assert_eq!(r.to.as_bytes(), want_to, "{k}");
            let want_text = &text[..text.len().min(kind.text_max())];
            assert_eq!(r.text.as_bytes(), want_text, "{k}");
        }
        assert_eq!(got[cases.len() - 2].text.as_bytes(), &long[..32], "cut to 32");
        assert_eq!(got[cases.len() - 4].text.as_bytes(), b"12345", "an ack's number: 5");
        // the sizes: 9 for a beacon, 5 more for a packed addressee, the text and its length
        let t2 = Traffic::new();
        t2.push(Kind::Position, false, b"VE7GPL-5", b"", b"");
        assert_eq!(t2.used(), 8);
        t2.push(Kind::Message, false, b"VE7GPL-1", b"TEST", b"6");
        assert_eq!(t2.used(), 8 + 15);
    }

    /// APRS-59.3: the same kind from the same station again (with the same text) bumps the newest record's count
    /// and time; "via" stays only while no copy came direct; the count stops at 8; anything else is a new record.
    #[test]
    fn repeats_coalesce() {
        let t = Traffic::new();
        t.push(Kind::Position, true, b"VE7GPL-5", b"", b"");
        t.set_now(40);
        t.push(Kind::Position, true, b"VE7GPL-5", b"", b"");
        assert_eq!(t.len(), 1);
        let r = t.get(0).unwrap();
        assert_eq!((r.count, r.via, r.ago_s), (2, true, 0));
        t.push(Kind::Position, false, b"VE7GPL-5", b"", b"");
        assert!(!t.get(0).unwrap().via, "heard direct once");
        t.push(Kind::Position, true, b"VE7GPL-5", b"", b"");
        assert!(!t.get(0).unwrap().via, "stays direct");
        for _ in 0..20 {
            t.push(Kind::Position, true, b"VE7GPL-5", b"", b"");
        }
        assert_eq!((t.len(), t.get(0).unwrap().count), (1, COUNT_MAX));
        t.push(Kind::Status, true, b"VE7GPL-5", b"", b"");
        t.push(Kind::Message, true, b"VE7GPL-5", b"TEST", b"1");
        t.push(Kind::Message, true, b"VE7GPL-5", b"TEST", b"1");
        t.push(Kind::Message, true, b"VE7GPL-5", b"TEST", b"2");
        t.push(Kind::Message, true, b"VE7GPL-6", b"TEST", b"2");
        t.push(Kind::Position, true, b"VE7GPL-5", b"", b"");
        let got = all(&t);
        assert_eq!(got.iter().map(|r| (r.kind, r.count)).collect::<Vec<_>>(), [
            (Kind::Position, 1),
            (Kind::Message, 1),
            (Kind::Message, 1),
            (Kind::Message, 2),
            (Kind::Status, 1),
            (Kind::Position, COUNT_MAX)
        ]);
    }

    /// APRS-59.4: full, the oldest records drop; the ring wraps through its end (records split across it read
    /// back whole); records older than about 68 hours go before the time wraps.
    #[test]
    fn wraps_and_drops_the_oldest() {
        let t = Traffic::new();
        for k in 0..500u32 {
            t.set_now(k * 4);
            let text = std::format!("message number {k}");
            t.push(Kind::Message, false, std::format!("VE7A-{}", k % 16).as_bytes(), b"VA7FER-2", text.as_bytes());
            assert!(t.used() as usize <= TRAFFIC_BYTES);
        }
        let n = t.len();
        assert!(n > 25 && n < 40, "{n}");
        for i in 0..n {
            let r = t.get(i).unwrap();
            let k = 499 - i as u32;
            assert_eq!(r.text.as_bytes(), std::format!("message number {k}").as_bytes());
            assert_eq!(r.src.as_bytes(), std::format!("VE7A-{}", k % 16).as_bytes());
            assert_eq!(r.ago_s, i as u32 * 4);
        }
        assert_eq!(t.last_message(b"VE7A-3").unwrap().text.as_bytes(), b"message number 499");
        // 68 hours on: all gone, none shown as new
        t.set_now(499 * 4 + 0xF001 * UNIT_S);
        assert_eq!(t.len(), 0);
        t.push(Kind::Status, false, b"VE7GPL", b"", b"");
        assert_eq!(all(&t).len(), 1);
        t.clear();
        assert!(t.is_empty());
    }

    /// APRS-28: the newest message from a station, to anyone, among those kept: an ack or a beacon after it
    /// leaves it; none once it's dropped.
    #[test]
    fn last_message_is_the_newest_kept() {
        let t = Traffic::new();
        assert!(t.last_message(b"VE7GPL-1").is_none());
        t.push(Kind::Message, false, b"VE7GPL-1", b"VA7FER", b"3");
        t.set_now(60);
        t.push(Kind::Message, false, b"VE7GPL-1", b"TEST", b"6");
        t.push(Kind::Ack, false, b"VE7GPL-1", b"VE7XYZ", b"5");
        t.push(Kind::Position, false, b"VE7GPL-1", b"", b"");
        t.push(Kind::Message, false, b"VE7GPL-2", b"CQ", b"other");
        t.set_now(100);
        let r = t.last_message(b"VE7GPL-1").unwrap();
        assert_eq!((r.to.as_bytes(), r.text.as_bytes(), r.ago_s), (&b"TEST"[..], &b"6"[..], 40));
        for _ in 0..200 {
            t.push(Kind::Status, false, b"VE7GPL-9", b"", b"");
            t.push(Kind::Position, false, b"VE7GPL-9", b"", b"");
        }
        assert!(t.last_message(b"VE7GPL-1").is_none(), "dropped with the oldest");
    }
}
