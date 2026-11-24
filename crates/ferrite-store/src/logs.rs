//! The records that grow with use (`docs/firmware-design.md`, "SPI flash layout"; manual INBOX-6 – INBOX-8,
//! APRS-11, GPS-10):
//!
//! - [`Ring`]: the event log (EVENT_LOG, 992 records) and the APRS messages (APRS's last six sectors, 186
//!   records). 128-byte records, 31 to a sector, record `seq` at slot `seq % capacity`; entering a sector erases
//!   it, so the oldest 31 go at once (INBOX-8). Appends only program erased bytes; marking an event or message
//!   read, and a message's tries, acks and failure, only clear bits in bytes the CRC leaves out, so none of it
//!   erases anything. A power cut mid-append leaves a torn record after the newest: mount blanks it through the
//!   scratch sector (or erases a sector a cut erase left as noise).
//! - [`Waypoints`]: up to 256 records of 32 bytes in WAYPOINTS, kept packed oldest first, so the list (newest
//!   first) is the slots backwards. Adding programs the next slot; deleting moves the later ones down a slot,
//!   sector by sector through the scratch sector (a cut can leave one waypoint twice, never lose one).
//! - [`Contacts`]: up to 128 records of 32 bytes in CONTACTS, kept in callsign order ([`Packed`], as the
//!   waypoints): saving one moves the later ones up a slot, last sector first.
//! - The APRS settings and the quick messages: one record in two copies (APRS's first two sectors), kept like
//!   the settings ([`crate::settings`]).
//!
//! Times are the store's own seconds ([`crate::FlashStore::set_time`]): UTC once a clock (GPS, or set by hand) is
//! set; before that they count on from the newest record stored, so "ago" never runs backwards, but the time the
//! radio was off isn't counted (there is no RTC: decision 76). So each record says whether its time was UTC
//! ([`CLOCK`], INBOX-6.1): an age is given only when it is real, a UTC record read while the clock is UTC or a
//! record from this power-on; any other is [`AGO_UNKNOWN`], never a guessed date.

use crate::io::Flash;
use crate::{crc16, scratch, u16_at, u32_at, Nor, Result, SECTOR};
use core::cell::Cell;
use ferrite_app::data::{Contact, Event, Message, MsgState, Pos, Src, Station, Sym, Waypoint, WpSrc, AGO_UNKNOWN};
use ferrite_ui::text::Buf;

pub const REC: u32 = 128;
/// Records in a sector: the last 128 bytes stay free for the scratch copy's footer.
pub const PER: u32 = 31;
const MAGIC: [u8; 2] = [0xE7, 0x01];
/// Offsets in a record: seq (0..4), time (4..8), magic (8..10), the read byte (10, outside the CRC), body from 11,
/// CRC-16 at 126 over 0..10 and 11..body end.
const READ: usize = 10;
const BODY: usize = 11;
const CRC_AT: usize = 126;
/// INBOX-6.1: 0 when the record was stamped with the clock UTC (GPS, or set by hand); erased (0xFF, and every
/// record from before this byte) when not. Outside the CRC, in events and messages alike.
pub const CLOCK: usize = 125;

/// What a record's age is reckoned from (INBOX-6.1).
#[derive(Clone, Copy)]
pub struct Clock {
    /// The store's seconds now.
    pub now: u32,
    /// The store's clock is UTC now.
    pub utc: bool,
    /// This power-on's own frame now: the clock at mount plus the uptime (what non-UTC stamps count in).
    pub boot_now: u32,
}

impl Clock {
    /// The age of a record stamped `t`, `utc` or not, from this power-on (`this_boot`) or not.
    pub fn ago(self, t: u32, utc: bool, this_boot: bool) -> u32 {
        match (utc, this_boot) {
            (true, _) if self.utc => self.now.saturating_sub(t),
            (false, true) => self.boot_now.saturating_sub(t),
            _ => AGO_UNKNOWN,
        }
    }
}

/// A ring of 128-byte records over whole sectors.
pub struct Ring {
    start: u32,
    sectors: u32,
    /// Where the CRC stops (bytes after it, up to 126, may be programmed later).
    body_end: usize,
    newest: Cell<Option<u32>>,
    oldest: Cell<u32>,
    /// The first seq appended this power-on (INBOX-6.1).
    boot: Cell<u32>,
}

impl Ring {
    pub const fn new(start: u32, end: u32, body_end: usize) -> Self {
        Ring { start, sectors: (end - start) / SECTOR, body_end, newest: Cell::new(None), oldest: Cell::new(0), boot: Cell::new(0) }
    }

    pub fn cap(&self) -> u32 {
        self.sectors * PER
    }

    fn addr(&self, seq: u32) -> u32 {
        let s = seq % self.cap();
        self.start + s / PER * SECTOR + s % PER * REC
    }

    fn crc(&self, b: &[u8; REC as usize]) -> u16 {
        let mut t = [0u8; CRC_AT];
        t[..READ].copy_from_slice(&b[..READ]);
        let n = self.body_end - BODY;
        t[READ..READ + n].copy_from_slice(&b[BODY..self.body_end]);
        crc16(&t[..READ + n])
    }

    fn valid(&self, b: &[u8; REC as usize], seq: u32) -> bool {
        u32_at(b, 0) == seq && b[8..10] == MAGIC && self.crc(b) == u16_at(b, CRC_AT)
    }

    /// Records stored.
    pub fn len(&self) -> u16 {
        self.newest.get().map_or(0, |n| (n - self.oldest.get() + 1) as u16)
    }

    pub fn is_empty(&self) -> bool {
        self.newest.get().is_none()
    }

    /// The newest record's time, if any.
    pub fn newest_time<N: Nor>(&self, f: &Flash<N>) -> Option<u32> {
        self.read(f, 0).map(|b| u32_at(&b, 4))
    }

    /// Finds the records, and repairs what a power cut left (see the module comment).
    pub async fn mount<N: Nor>(&self, f: &Flash<N>) -> Result {
        let (mut newest, mut oldest): (Option<u32>, u32) = (None, u32::MAX);
        for sec in 0..self.sectors {
            let base = self.start + sec * SECTOR;
            // Scanned in a block of its own: the record buffer isn't kept across the repairs' waits.
            let (good, bad) = {
                let mut b = [0u8; REC as usize];
                let (mut good, mut bad) = (0, 0u32);
                for k in 0..PER {
                    f.raw_read(base + k * REC, &mut b)?;
                    if b == [0xFF; REC as usize] {
                        continue;
                    }
                    let seq = u32_at(&b, 0);
                    if seq % self.cap() == sec * PER + k && self.valid(&b, seq) {
                        good += 1;
                        newest = Some(newest.map_or(seq, |n: u32| n.max(seq)));
                        oldest = oldest.min(seq);
                    } else {
                        bad |= 1 << k;
                    }
                }
                (good, bad)
            };
            if bad != 0 && good == 0 {
                f.erase(base).await?;
            } else if bad != 0 {
                scratch::rewrite(f, base, &|o, piece| {
                    for (i, x) in piece.iter_mut().enumerate() {
                        let k = (o + i as u32) / REC;
                        if k < PER && bad & 1 << k != 0 {
                            *x = 0xFF;
                        }
                    }
                })
                .await?;
            }
        }
        // Records from an older lap than the newest's window (a cut between erasing and appending can't leave
        // any, but a noisy chip might): not counted.
        if let Some(n) = newest {
            oldest = oldest.max(n.saturating_sub(self.cap() - 1));
        }
        self.newest.set(newest);
        self.oldest.set(if newest.is_some() { oldest } else { 0 });
        self.boot.set(newest.map_or(0, |n| n + 1));
        Ok(())
    }

    /// Record `i`, newest first.
    pub fn read<N: Nor>(&self, f: &Flash<N>, i: u16) -> Option<[u8; REC as usize]> {
        let seq = self.seq(i)?;
        let mut b = [0u8; REC as usize];
        f.read(self.addr(seq), &mut b).ok()?;
        self.valid(&b, seq).then_some(b)
    }

    /// The age of record `b` (INBOX-6.1).
    pub fn ago(&self, b: &[u8; REC as usize], c: Clock) -> u32 {
        c.ago(u32_at(b, 4), b[CLOCK] == 0, u32_at(b, 0) >= self.boot.get())
    }

    /// Record `i`'s sequence number, newest first.
    pub fn seq(&self, i: u16) -> Option<u32> {
        (i < self.len()).then(|| self.newest.get().unwrap_or(0) - i as u32)
    }

    /// The record with this sequence number, if still stored.
    pub fn by_seq<N: Nor>(&self, f: &Flash<N>, seq: u32) -> Option<[u8; REC as usize]> {
        let n = self.newest.get()?;
        if seq > n || seq < self.oldest.get() {
            return None;
        }
        self.read(f, (n - seq) as u16)
    }

    /// Appends a record: `rec`'s seq, magic and CRC are filled in here, its time and body by the caller.
    pub async fn append<N: Nor>(&self, f: &Flash<N>, rec: &mut [u8; REC as usize]) -> Result<u32> {
        let seq = self.newest.get().map_or(0, |n| n + 1);
        let a = self.addr(seq);
        let base = a - a % SECTOR;
        if (seq % self.cap()).is_multiple_of(PER) {
            if !f.blank(base, SECTOR)? {
                f.erase(base).await?;
            }
            // The sector held the lap before's records seq - cap .. that sector's end.
            let lap_end = (seq - (seq % self.cap()) % PER + PER).saturating_sub(self.cap());
            self.oldest.set(self.oldest.get().max(lap_end));
        } else if !f.blank(a, REC)? {
            // A torn append this power-on (the flash failed under it): blank that slot only.
            let off = a - base;
            scratch::rewrite(f, base, &|o, piece| {
                for (i, x) in piece.iter_mut().enumerate() {
                    if (off..off + REC).contains(&(o + i as u32)) {
                        *x = 0xFF;
                    }
                }
            })
            .await?;
        }
        rec[..4].copy_from_slice(&seq.to_le_bytes());
        rec[8..10].copy_from_slice(&MAGIC);
        let c = self.crc(rec);
        rec[CRC_AT..].copy_from_slice(&c.to_le_bytes());
        f.program(a, &rec[..]).await?;
        if self.newest.get().is_none() {
            self.oldest.set(seq);
        }
        self.newest.set(Some(seq));
        Ok(seq)
    }

    /// Clears bits of one byte outside the CRC (`at` is 10 or past the body) of the record with this seq.
    pub async fn mark<N: Nor>(&self, f: &Flash<N>, seq: u32, at: usize, value: u8) -> Result {
        let Some(b) = self.by_seq(f, seq) else { return Ok(()) };
        if b[at] & value != b[at] {
            f.program(self.addr(seq) + at as u32, &[b[at] & value]).await?;
        }
        Ok(())
    }

    /// Every record gone.
    pub async fn clear<N: Nor>(&self, f: &Flash<N>) -> Result {
        for sec in 0..self.sectors {
            let base = self.start + sec * SECTOR;
            if !f.blank(base, SECTOR)? {
                f.erase_blanked(base).await?;
            }
        }
        self.forget();
        Ok(())
    }

    /// The area was erased behind the ring's back (the reset).
    pub fn forget(&self) {
        self.newest.set(None);
        self.oldest.set(0);
        self.boot.set(0);
    }
}

fn put<const N: usize>(b: &mut [u8], s: &Buf<N>) {
    let n = s.len().min(b.len());
    b[..n].copy_from_slice(&s.as_bytes()[..n]);
}

fn text<const N: usize>(b: &[u8]) -> Buf<N> {
    let n = b.iter().position(|&c| c == 0 || c == 0xFF).unwrap_or(b.len()).min(N);
    Buf::from(&b[..n])
}

// ---- events ----

/// Where an event's body ends (bytes 118..126 stay erased).
pub const EVENT_END: usize = 118;
/// What a queued event carries: the read byte and the body (11..118).
pub const EVENT_JOB: usize = 1 + EVENT_END - BODY;

/// An event as the Storage task queues it ([`crate::store::Job::Log`]): no seq or time yet.
pub fn event_job(e: &Event) -> [u8; EVENT_JOB] {
    let mut r = [0u8; REC as usize];
    r[READ] = if e.unread { 0xFF } else { 0 };
    r[11] = Src::ALL.iter().position(|s| *s == e.src).unwrap_or(0) as u8;
    r[12] = e.slot;
    r[13] = e.pos.is_some() as u8;
    r[14..18].copy_from_slice(&e.freq.to_le_bytes());
    r[18..22].copy_from_slice(&e.id.to_le_bytes());
    r[22..24].copy_from_slice(&e.x.to_le_bytes());
    r[24..26].copy_from_slice(&e.y.to_le_bytes());
    let p = e.pos.unwrap_or_default();
    r[26..30].copy_from_slice(&p.lat.to_le_bytes());
    r[30..34].copy_from_slice(&p.lon.to_le_bytes());
    put(&mut r[34..54], &e.name);
    put(&mut r[54..118], &e.text);
    let mut j = [0u8; EVENT_JOB];
    j[0] = r[READ];
    j[1..].copy_from_slice(&r[BODY..EVENT_END]);
    j
}

/// The record for a queued event, stamped `t`, with the clock UTC or not (INBOX-6.1).
pub fn event_rec(j: &[u8; EVENT_JOB], t: u32, utc: bool) -> [u8; REC as usize] {
    let mut r = [0xFFu8; REC as usize];
    r[4..8].copy_from_slice(&t.to_le_bytes());
    r[CLOCK] = if utc { 0 } else { 0xFF };
    r[READ] = j[0];
    r[BODY..EVENT_END].copy_from_slice(&j[1..]);
    r
}

/// Event record `b`, `ago_s` its age ([`Ring::ago`]).
pub fn event_decode(b: &[u8; REC as usize], ago_s: u32) -> Event {
    let pos = (b[13] & 1 != 0).then(|| Pos { lat: u32_at(b, 26) as i32, lon: u32_at(b, 30) as i32 });
    Event {
        src: Src::ALL[(b[11] as usize).min(Src::ALL.len() - 1)],
        ago_s,
        freq: u32_at(b, 14),
        slot: b[12],
        unread: b[READ] == 0xFF,
        id: u32_at(b, 18),
        x: u16_at(b, 22),
        y: u16_at(b, 24),
        pos,
        name: text(&b[34..54]),
        text: text(&b[54..118]),
    }
}

// ---- APRS messages ----

/// Where a message's body ends; 96 (tries, one bit cleared per try) and 97 (bit 0 cleared: acked, bit 1:
/// failed, bit 2: rejected, APRS-34, bit 3: cancelled, APRS-13.1, bit 4: deleted, APRS-46.1, either way) are
/// programmed later. A message to us keeps its sender's message number at
/// 89..94 (zero-padded; all zero without one, as in records from before APRS-32), and the addressee it was sent to
/// at 98..108 (APRS-6.5: outside the CRC, so records from before it stay valid and read it as unknown, erased).
pub const MSG_END: usize = 96;
const TO: core::ops::Range<usize> = 98..108;
const TRIES: usize = 96;
const FINAL: usize = 97;
const NUMBER: core::ops::Range<usize> = 89..94;

pub fn msg_rec(inbound: bool, peer: &Buf<10>, txt: &Buf<67>, id: &Buf<5>, to: &Buf<10>, t: u32, utc: bool) -> [u8; REC as usize] {
    let mut r = [0xFFu8; REC as usize];
    r[4..8].copy_from_slice(&t.to_le_bytes());
    r[CLOCK] = if utc { 0 } else { 0xFF };
    r[BODY..MSG_END].fill(0);
    r[11] = inbound as u8;
    put(&mut r[12..22], peer);
    put(&mut r[22..89], txt);
    put(&mut r[NUMBER], id);
    if !to.is_empty() {
        r[TO].fill(0);
        put(&mut r[TO], to);
    }
    r
}

/// APRS-32: whether record `b` is the message to us from `peer` with number `id` and text `txt` (a retry heard
/// again after a power cycle).
pub fn msg_same(b: &[u8; REC as usize], peer: &Buf<10>, id: &Buf<5>, txt: &Buf<67>) -> bool {
    b[11] != 0 && !id.is_empty() && text::<5>(&b[NUMBER]) == *id && text::<10>(&b[12..22]) == *peer && text::<67>(&b[22..89]) == *txt
}

/// Message record `b`, `ago_s` its age ([`Ring::ago`]).
pub fn msg_decode(b: &[u8; REC as usize], ago_s: u32) -> Message {
    let inbound = b[11] != 0;
    let tries = (!b[TRIES]).count_ones() as u8;
    let state = if inbound {
        if b[READ] == 0xFF { MsgState::Unread } else { MsgState::Read }
    } else if b[FINAL] & 1 == 0 {
        MsgState::Acked
    } else if b[FINAL] & 4 == 0 {
        MsgState::Rejected
    } else if b[FINAL] & 2 == 0 {
        MsgState::Failed
    } else if b[FINAL] & 8 == 0 {
        MsgState::Cancelled // APRS-13.1
    } else if tries >= 2 {
        MsgState::Retrying(tries)
    } else {
        MsgState::Pending
    };
    Message { inbound, peer: text(&b[12..22]), text: text(&b[22..89]), ago_s, state, to: if inbound { text(&b[TO]) } else { Buf::new() } }
}

/// The byte and value that move an outgoing message to `state` (bits only cleared).
pub fn msg_state_byte(state: MsgState) -> Option<(usize, u8)> {
    match state {
        MsgState::Acked => Some((FINAL, !1)),
        MsgState::Failed => Some((FINAL, !2)),
        MsgState::Rejected => Some((FINAL, !4)),
        MsgState::Cancelled => Some((FINAL, !8)),
        MsgState::Retrying(n) => Some((TRIES, 0xFFu8.checked_shl(n.min(8) as u32).unwrap_or(0))),
        MsgState::Pending => Some((TRIES, 0xFE)),
        MsgState::Read => Some((READ, 0)),
        MsgState::Unread => None,
    }
}

/// Whether record `b` is our message to `peer` with `txt`, still waiting for its end.
pub fn msg_open(b: &[u8; REC as usize], peer: &Buf<10>, txt: &Buf<67>) -> bool {
    let m = msg_decode(b, 0);
    m.peer == *peer && m.text == *txt && msg_is_open(b)
}

/// APRS-46.1: whether record `b` was deleted (it stays in the ring, skipped, until the ring wraps over it).
pub fn msg_deleted(b: &[u8; REC as usize]) -> bool {
    b[FINAL] & 0x10 == 0
}

/// APRS-46.1: the marks that delete a message: (byte, value) pairs, bits only cleared. One of ours still open
/// ends as cancelled too, so a later state change (an ack heard late) can't land on it; one to us is read.
pub fn msg_delete_marks(b: &[u8; REC as usize]) -> [(usize, u8); 2] {
    if b[11] != 0 {
        [(FINAL, !0x10), (READ, 0)]
    } else {
        [(FINAL, !0x18), (FINAL, !0x18)]
    }
}

/// Whether record `b` is one of ours still being tried (pending or retrying).
pub fn msg_is_open(b: &[u8; REC as usize]) -> bool {
    let m = msg_decode(b, 0);
    !m.inbound && matches!(m.state, MsgState::Pending | MsgState::Retrying(_))
}

// ---- the heard list (APRS-4) ----

/// Where a heard row's body ends: flags (11), call (12..22), symbol (22), position (23..31), course and speed
/// (31..35), altitude (35..39), the digipeater (39..49), comment (49..89), times heard (89..91), FX.25 (91), the Mic-E
/// device (92, 0xFF none, APRS-5.2; the CRC covers it, so rows written before it fail their check).
pub const HEARD_END: usize = 93;
/// What a queued row carries: the body (11..92).
pub const HEARD_JOB: usize = HEARD_END - BODY;

/// A station as the Storage task queues it ([`crate::store::Job::Heard`]): no seq or time yet.
pub fn heard_job(s: &Station) -> [u8; HEARD_JOB] {
    let mut r = [0u8; REC as usize];
    r[11] = s.pos.is_some() as u8 | (s.course.is_some() as u8) << 1 | (s.alt_m.is_some() as u8) << 2 | (s.fx25.is_some() as u8) << 3;
    put(&mut r[12..22], &s.call);
    r[22] = s.sym as u8;
    let p = s.pos.unwrap_or_default();
    r[23..27].copy_from_slice(&p.lat.to_le_bytes());
    r[27..31].copy_from_slice(&p.lon.to_le_bytes());
    let (c, v) = s.course.unwrap_or_default();
    r[31..33].copy_from_slice(&c.to_le_bytes());
    r[33..35].copy_from_slice(&v.to_le_bytes());
    r[35..39].copy_from_slice(&s.alt_m.unwrap_or(0).to_le_bytes());
    // The station's via is the digipeater with a `*` (APRS-3): the call alone is kept.
    let via = s.via.as_bytes();
    put::<10>(&mut r[39..49], &Buf::from(via.strip_suffix(b"*").unwrap_or(via)));
    put(&mut r[49..89], &s.comment);
    r[89..91].copy_from_slice(&s.times.to_le_bytes());
    r[91] = s.fx25.unwrap_or(0);
    r[92] = s.device.unwrap_or(0xFF);
    let mut j = [0u8; HEARD_JOB];
    j.copy_from_slice(&r[BODY..HEARD_END]);
    j
}

/// The record for a queued row, stamped `t`, with the clock UTC or not (INBOX-6.1).
pub fn heard_rec(j: &[u8; HEARD_JOB], t: u32, utc: bool) -> [u8; REC as usize] {
    let mut r = [0xFFu8; REC as usize];
    r[4..8].copy_from_slice(&t.to_le_bytes());
    r[BODY..HEARD_END].copy_from_slice(j);
    r[CLOCK] = if utc { 0 } else { 0xFF };
    r
}

/// Heard row `b`'s station callsign.
pub fn heard_call(b: &[u8; REC as usize]) -> Buf<10> {
    text(&b[12..22])
}

/// Heard row `b`, `ago_s` its age ([`Ring::ago`]); the via marked used again.
pub fn heard_decode(b: &[u8; REC as usize], ago_s: u32) -> Station {
    let f = b[11];
    let mut via = Buf::<24>::new();
    let v = text::<10>(&b[39..49]);
    if !v.is_empty() {
        via.push(v.as_bytes());
        via.push(b"*");
    }
    Station {
        call: text(&b[12..22]),
        sym: Sym::ALL[(b[22] as usize).min(Sym::ALL.len() - 1)],
        pos: (f & 1 != 0).then(|| Pos { lat: u32_at(b, 23) as i32, lon: u32_at(b, 27) as i32 }),
        ago_s,
        via,
        course: (f & 2 != 0).then(|| (u16_at(b, 31), u16_at(b, 33))),
        alt_m: (f & 4 != 0).then(|| u32_at(b, 35) as i32),
        times: u16_at(b, 89),
        comment: text(&b[49..89]),
        msg: Buf::new(),
        msg_min: None,
        fx25: (f & 8 != 0).then_some(b[91]),
        device: (b[92] != 0xFF).then_some(b[92]),
    }
}

// ---- waypoints ----

pub const WP_LEN: u32 = 32;
/// Waypoints in a sector (the last 32 bytes stay free for the scratch footer).
pub const WP_PER: u32 = 127;
/// GPS-10: 16 sectors of 127 (decision 333).
pub const WP_MAX: u16 = 2032;

/// Name (16) · latitude (4) · longitude (4) · source (1) · 0xFF (5) · CRC-16 of the 30 bytes before (2).
pub fn wp_encode(w: &Waypoint) -> [u8; WP_LEN as usize] {
    let mut b = [0xFFu8; WP_LEN as usize];
    b[..16].fill(0);
    put(&mut b[..16], &w.name);
    b[16..20].copy_from_slice(&w.pos.lat.to_le_bytes());
    b[20..24].copy_from_slice(&w.pos.lon.to_le_bytes());
    b[24] = w.src as u8;
    let c = crc16(&b[..30]);
    b[30..32].copy_from_slice(&c.to_le_bytes());
    b
}

pub fn wp_decode(b: &[u8; WP_LEN as usize]) -> Option<Waypoint> {
    if *b == [0xFF; WP_LEN as usize] || crc16(&b[..30]) != u16_at(b, 30) {
        return None;
    }
    let src = [WpSrc::Marked, WpSrc::Aprs, WpSrc::Link, WpSrc::Grid][(b[24] as usize).min(3)];
    Some(Waypoint { name: text(&b[..16]), pos: Pos { lat: u32_at(b, 16) as i32, lon: u32_at(b, 20) as i32 }, src })
}

/// 32-byte records packed from slot 0 over whole sectors, 127 to a sector (the last 32 bytes stay free for the
/// scratch footer); a record is held when its CRC-16 (bytes 30..32, over 0..30) is good. The waypoints keep them
/// oldest first, the contacts in callsign order.
pub struct Packed {
    start: u32,
    max: u16,
    count: Cell<u16>,
}

impl Packed {
    pub const fn new(start: u32, max: u16) -> Self {
        Packed { start, max, count: Cell::new(0) }
    }

    fn addr(&self, slot: u16) -> u32 {
        let s = slot as u32;
        self.start + s / WP_PER * SECTOR + s % WP_PER * WP_LEN
    }

    pub fn len(&self) -> u16 {
        self.count.get()
    }

    pub fn is_empty(&self) -> bool {
        self.count.get() == 0
    }

    pub fn full(&self) -> bool {
        self.count.get() >= self.max
    }

    fn valid(b: &[u8; WP_LEN as usize]) -> bool {
        *b != [0xFF; WP_LEN as usize] && crc16(&b[..30]) == u16_at(b, 30)
    }

    /// Counts the packed records, and blanks whatever follows them: a torn one (a cut add), or old data in the
    /// area (a chip that held something else there), so the next adds program erased bytes.
    pub async fn mount<N: Nor>(&self, f: &Flash<N>) -> Result {
        let n = {
            let mut b = [0u8; WP_LEN as usize];
            let mut n = 0u16;
            while n < self.max {
                f.raw_read(self.addr(n), &mut b)?;
                if !Self::valid(&b) {
                    break;
                }
                n += 1;
            }
            n
        };
        self.count.set(n);
        let sectors = (self.max as u32).div_ceil(WP_PER);
        for sec in 0..sectors {
            let base = self.start + sec * SECTOR;
            // The first slot of this sector that must be blank.
            let from = (n as u32).saturating_sub(sec * WP_PER).min(WP_PER);
            if from == WP_PER || f.blank(base + from * WP_LEN, SECTOR - from * WP_LEN)? {
                continue;
            }
            if from == 0 {
                f.erase(base).await?;
            } else {
                scratch::rewrite(f, base, &|o, piece| {
                    for (i, x) in piece.iter_mut().enumerate() {
                        if o + i as u32 >= from * WP_LEN {
                            *x = 0xFF;
                        }
                    }
                })
                .await?;
            }
        }
        Ok(())
    }

    /// The record in `slot`.
    pub fn get<N: Nor>(&self, f: &Flash<N>, slot: u16) -> Option<[u8; WP_LEN as usize]> {
        if slot >= self.count.get() {
            return None;
        }
        let mut b = [0u8; WP_LEN as usize];
        f.read(self.addr(slot), &mut b).ok()?;
        Self::valid(&b).then_some(b)
    }

    /// Puts `rec` in `slot` (0..=len): the later ones move up a slot, last sector first, so a cut can leave one
    /// twice, never lose one.
    pub async fn insert<N: Nor>(&self, f: &Flash<N>, slot: u16, rec: &[u8; WP_LEN as usize]) -> Result<bool> {
        let n = self.count.get();
        if n >= self.max || slot > n {
            return Ok(false);
        }
        if slot == n {
            f.program(self.addr(n), rec).await?;
        } else {
            for sec in (slot as u32 / WP_PER..=n as u32 / WP_PER).rev() {
                self.shift(f, sec, slot, n + 1, Some(rec)).await?;
            }
        }
        self.count.set(n + 1);
        Ok(true)
    }

    /// Replaces the record in `slot` (one sector rewritten: before or after, whatever a cut).
    pub async fn set<N: Nor>(&self, f: &Flash<N>, slot: u16, rec: &[u8; WP_LEN as usize]) -> Result<bool> {
        if slot >= self.count.get() {
            return Ok(false);
        }
        let at = (slot as u32 % WP_PER * WP_LEN) as usize;
        scratch::rewrite(f, self.start + slot as u32 / WP_PER * SECTOR, &|o, piece| {
            for (i, x) in piece.iter_mut().enumerate() {
                if let Some(&b) = (o as usize + i).checked_sub(at).and_then(|k| rec.get(k)) {
                    *x = b;
                }
            }
        })
        .await?;
        Ok(true)
    }

    /// Takes the record out of `slot`: the later ones move down a slot, first sector first.
    pub async fn remove<N: Nor>(&self, f: &Flash<N>, slot: u16) -> Result<bool> {
        let n = self.count.get();
        if slot >= n {
            return Ok(false);
        }
        for sec in slot as u32 / WP_PER..=(n as u32 - 1) / WP_PER {
            self.shift(f, sec, slot, n - 1, None).await?;
        }
        self.count.set(n - 1);
        Ok(true)
    }

    /// Rewrites sector `sec` for a list of `len` records after an insert at `at` (`rec` Some: slot `at` gets it,
    /// later slots the one below) or a removal at `at` (None: later slots take the one above).
    async fn shift<N: Nor>(&self, f: &Flash<N>, sec: u32, at: u16, len: u16, rec: Option<&[u8; WP_LEN as usize]>) -> Result {
        scratch::rewrite(f, self.start + sec * SECTOR, &|o, piece| {
            for (r, chunk) in piece.chunks_mut(WP_LEN as usize).enumerate() {
                let k = o / WP_LEN + r as u32;
                let slot = sec * WP_PER + k;
                if k >= WP_PER || slot < at as u32 {
                    continue;
                }
                if slot >= len as u32 {
                    chunk.fill(0xFF);
                } else if let Some(rec) = rec {
                    if slot == at as u32 {
                        chunk.copy_from_slice(rec);
                    } else {
                        let _ = f.raw_read(self.addr(slot as u16 - 1), chunk);
                    }
                } else {
                    let _ = f.raw_read(self.addr(slot as u16 + 1), chunk);
                }
            }
        })
        .await
    }

    pub fn forget(&self) {
        self.count.set(0);
    }
}

/// The waypoints, packed oldest first from slot 0.
pub struct Waypoints(pub Packed);

impl Waypoints {
    pub const fn new(start: u32) -> Self {
        Waypoints(Packed::new(start, WP_MAX))
    }

    pub fn len(&self) -> u16 {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub async fn mount<N: Nor>(&self, f: &Flash<N>) -> Result {
        self.0.mount(f).await
    }

    /// Waypoint `i`, newest first.
    pub fn get<N: Nor>(&self, f: &Flash<N>, i: u16) -> Option<Waypoint> {
        wp_decode(&self.0.get(f, self.0.len().checked_sub(i)?.checked_sub(1)?)?)
    }

    pub async fn add<N: Nor>(&self, f: &Flash<N>, w: &Waypoint) -> Result<bool> {
        self.0.insert(f, self.0.len(), &wp_encode(w)).await
    }

    /// Replaces the newest waypoint equal to `old` with `new` in its slot (LINK-55): its sector rewritten once.
    pub async fn replace<N: Nor>(&self, f: &Flash<N>, old: &Waypoint, new: &Waypoint) -> Result<bool> {
        match (0..self.0.len()).rev().find(|&s| self.0.get(f, s).and_then(|b| wp_decode(&b)).as_ref() == Some(old)) {
            Some(k) => self.0.set(f, k, &wp_encode(new)).await,
            None => Ok(false),
        }
    }

    /// Deletes the newest waypoint equal to `w`: the later ones move down a slot.
    pub async fn delete<N: Nor>(&self, f: &Flash<N>, w: &Waypoint) -> Result<bool> {
        let n = self.0.len();
        match (0..n).rev().find(|&s| self.0.get(f, s).and_then(|b| wp_decode(&b)).as_ref() == Some(w)) {
            Some(k) => self.0.remove(f, k).await,
            None => Ok(false),
        }
    }

    pub fn forget(&self) {
        self.0.forget();
    }
}

// ---- contacts ----

/// APRS-37: 8 sectors of 127 (decision 333).
pub const CONTACT_MAX: u16 = 1016;
const _: () = assert!(CONTACT_MAX == ferrite_app::screens::contacts::CONTACT_MAX && WP_MAX == ferrite_app::link::lists::WAYPOINTS_MAX);

/// Callsign (10) · name (12) · 0xFF (8) · CRC-16 of the 30 bytes before (2).
pub fn contact_encode(c: &Contact) -> [u8; WP_LEN as usize] {
    let mut b = [0xFFu8; WP_LEN as usize];
    b[..22].fill(0);
    put(&mut b[..10], &c.call);
    put(&mut b[10..22], &c.name);
    let k = crc16(&b[..30]);
    b[30..32].copy_from_slice(&k.to_le_bytes());
    b
}

pub fn contact_decode(b: &[u8; WP_LEN as usize]) -> Contact {
    Contact { call: text(&b[..10]), name: text(&b[10..22]) }
}

/// The contacts, packed in callsign order (APRS-37).
pub struct Contacts(pub Packed);

impl Contacts {
    pub const fn new(start: u32) -> Self {
        Contacts(Packed::new(start, CONTACT_MAX))
    }

    pub fn get<N: Nor>(&self, f: &Flash<N>, i: u16) -> Option<Contact> {
        self.0.get(f, i).map(|b| contact_decode(&b))
    }

    /// Where `call` is (Ok) or would go (Err), by binary search.
    pub fn find<N: Nor>(&self, f: &Flash<N>, call: &[u8]) -> core::result::Result<u16, u16> {
        let (mut lo, mut hi) = (0u16, self.0.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            let Some(c) = self.get(f, mid) else { return Err(lo) };
            match c.call.as_bytes().cmp(call) {
                core::cmp::Ordering::Less => lo = mid + 1,
                core::cmp::Ordering::Greater => hi = mid,
                core::cmp::Ordering::Equal => return Ok(mid),
            }
        }
        Err(lo)
    }

    /// Adds `c`, or renames the contact with its callsign.
    pub async fn put<N: Nor>(&self, f: &Flash<N>, c: &Contact) -> Result<bool> {
        match self.find(f, c.call.as_bytes()) {
            Ok(k) if self.get(f, k).as_ref() == Some(c) => Ok(true),
            Ok(k) => self.0.set(f, k, &contact_encode(c)).await,
            Err(k) => self.0.insert(f, k, &contact_encode(c)).await,
        }
    }

    pub async fn delete<N: Nor>(&self, f: &Flash<N>, call: &[u8]) -> Result<bool> {
        match self.find(f, call) {
            Ok(k) => self.0.remove(f, k).await,
            Err(_) => Ok(false),
        }
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::block;
    use crate::sim::SimFlash;
    use ferrite_core::layout::{CONTACTS, EVENT_LOG, WAYPOINTS};

    fn rec(body: u8) -> [u8; REC as usize] {
        let mut r = [0xFFu8; REC as usize];
        r[4..8].copy_from_slice(&1_800_000_000u32.to_le_bytes());
        r[BODY] = body;
        r
    }

    /// INBOX-8: an append the flash failed under this power-on leaves a torn slot; the next append blanks it and
    /// takes it, so no seq is skipped, before or after a remount. A seq not stored reads as nothing.
    #[test]
    fn a_torn_append_is_blanked_and_its_slot_reused() {
        let f = Flash::new(SimFlash::new());
        let r = Ring::new(EVENT_LOG.start, EVENT_LOG.end, EVENT_END);
        block(r.mount(&f)).unwrap();
        assert!(r.is_empty() && r.newest_time(&f).is_none());
        assert_eq!(block(r.append(&f, &mut rec(1))), Ok(0));
        let n = f.nor().ops;
        f.nor().cut_at = Some(n + 5);
        assert!(block(r.append(&f, &mut rec(2))).is_err());
        f.nor().power_on();
        assert!(!f.blank(r.addr(1), REC).unwrap(), "the slot is torn");
        assert_eq!(block(r.append(&f, &mut rec(3))), Ok(1), "the same slot, the next seq");
        assert!(!r.is_empty());
        assert_eq!((r.len(), r.read(&f, 0).unwrap()[BODY], r.by_seq(&f, 0).unwrap()[BODY]), (2, 3, 1));
        assert!(r.by_seq(&f, 2).is_none(), "not stored yet");
        let again = Ring::new(EVENT_LOG.start, EVENT_LOG.end, EVENT_END);
        block(again.mount(&f)).unwrap();
        assert_eq!((again.len(), again.read(&f, 0).unwrap()[BODY], again.newest_time(&f)), (2, 3, Some(1_800_000_000)));
    }

    /// APRS-37, GPS-10: saving a contact exactly as stored writes nothing; deleting one not saved, or a waypoint not
    /// stored, changes nothing and says so.
    #[test]
    fn unchanged_or_missing_records_write_nothing() {
        let f = Flash::new(SimFlash::new());
        let (c, w) = (Contacts::new(CONTACTS.start), Waypoints::new(WAYPOINTS.start));
        block(c.0.mount(&f)).unwrap();
        block(w.mount(&f)).unwrap();
        assert!(c.0.is_empty() && w.is_empty());
        let k = Contact { call: Buf::from(b"VE3KPW-9"), name: Buf::from(b"Pat") };
        assert_eq!(block(c.put(&f, &k)), Ok(true));
        let ops = f.nor().ops;
        assert_eq!(block(c.put(&f, &k)), Ok(true));
        assert_eq!(f.nor().ops, ops, "the same contact again: nothing written");
        assert_eq!(block(c.delete(&f, b"VA3RTX")), Ok(false));
        let wp = Waypoint { name: Buf::from(b"Camp"), pos: Pos { lat: 1, lon: 2 }, src: WpSrc::Link };
        assert_eq!(block(w.delete(&f, &wp)), Ok(false));
        assert_eq!(block(w.add(&f, &wp)), Ok(true));
        assert!(!w.is_empty() && !c.0.is_empty());
        assert_eq!(f.nor().ops - ops, 32, "only the waypoint's own record programmed");
    }
}
