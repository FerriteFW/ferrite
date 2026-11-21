//! The radio's APRS task logic (manual chapter 25, roadmap 5b-5d): AX.25 frames from the AFSK decoder in, frames
//! for the modulator out. The heard list (APRS-3, APRS-4: its details from flash but for the most recent, APRS-4.5), messages to us acked (APRS-6), ours tried
//! on APRS-13's schedule until an ack, beacons by the beacon mode (APRS-24) and after voice (APRS-27), the
//! fill-in digipeater (APRS-12, APRS-26: off unless asked), the path (0 5 9 1), APRS-9's 30 s spacing and
//! APRS-20's wait for the band relay; a message heard again stored once (APRS-32), queries answered (APRS-33,
//! APRS-35), rejections (APRS-34). Pure logic: the firmware's UI task feeds it frames and ticks it once a second
//! (the store and the App are there); the host tests run the same code on recorded audio. The simulators keep
//! their made-up traffic (`sample::aprs`).
//!
//! Frames are the AX.25 bytes without the FCS, as `uafsk`'s demodulator gives them and its modulator takes them.

use crate::aprs::{REPLY_HOLD_S, beacon_due, retry_wait, AprsCfg, LastBeacon, HEARD_MAX, INFO_MAX, TRIES};
use crate::data::{Beacon, Fix, MsgState, Pos, Station, Sym, AGO_UNKNOWN};
use core::cell::Cell;
use ferrite_core::freq::Freq10;
use ferrite_core::liveview::{fnv, FNV0};
use ferrite_ui::text::Buf;
use ufmt::uwrite;
/// The APRS library, for the firmware's KISS TNC (chapter 26).
pub use uaprs;
use uaprs::ax25::{Address, Frame};
use uaprs::parse::{self, MessageKind, Packet};

/// The largest frame: 10 addresses, control, PID and [`INFO_MAX`] bytes of information (`uafsk`'s HDLC limit
/// less the FCS).
pub const FRAME_MAX: usize = 328;

/// Our messages being tried at once, the acks owed, and the frames remembered for the digipeater's duplicate
/// check.
const OUT: usize = 4;
const ACKS: usize = 4;
const DUPES: usize = 16;
/// APRS-32: the messages to us remembered, 8 bytes each (a key of the sender, the number and the text, and when
/// first heard), so a repeat is acked again without a second inbox entry or ring; 24 entries, 192 bytes.
const SEEN: usize = 24;
/// APRS-32: a message without a number heard again within this long is the same one (a second digipeater's copy).
const SEEN_PLAIN_S: u32 = 30;
/// APRS-35: general queries (`?APRS?`) answered at most this often, after a random wait of
/// [`GENERAL_WAIT_S`] plus up to [`GENERAL_SPREAD_S`]; an answer not sent within [`QUERY_TTL_S`] of its time is
/// dropped.
const GENERAL_GAP_S: u32 = 300;
const GENERAL_WAIT_S: u32 = 5;
const GENERAL_SPREAD_S: u32 = 56;
const QUERY_TTL_S: u32 = 120;
/// APRS-9: automatic transmissions at least this far apart; a message's first try (PTT's or Send's), acks,
/// digipeats and KISS frames are exempt.
const AUTO_GAP_S: u32 = 30;
/// The digipeater: a frame heard again within this long isn't repeated, and at most one digipeat this often
/// (decided Oct 7, 2026, chapter 25).
const DUPE_S: u32 = 30;
const DIGI_GAP_S: u32 = 5;
/// GPS-33: an automatic beacon whose fix is older than its interval waits this long for a fresh one (the GPS
/// manager wakes the module for it), then is skipped until the next.
const FIX_WAIT_S: u32 = 120;
/// The destination ("tocall") that names the software: APZ is experimental, FER for Ferrite.
const TOCALL: [u8; 6] = *b"APZFER";
/// APRS-54.3: an uncompressed position report (APRS 1.0.1 chapter 8) has at most this many characters after its
/// symbol code: the course and speed, the altitude and the comment together.
pub const POSITION_TAIL_MAX: usize = 43;
/// What comes before them in ours (no timestamp): `=`, the latitude, the table, the longitude, the code.
const POSITION_HEAD: usize = 20;
/// APRS-54.4: the beacon power in tenths of a watt, Low, Mid and High, as measured (`research/tx-power-oct16.md`,
/// the black radio into an inline meter): on 2 m 0.46, 2.46 and 9.0 W; on 70 cm 0.98, 4.9 and 9.8 W, Alex's
/// nominal 1, 5 and 10 W. 1.25 m keys Low only (TX-9) and has no trusted measurement: the nominal 1 W.
const WATTS10_VHF: [u8; 3] = [5, 25, 90];
const WATTS10_NOMINAL: [u8; 3] = [10, 50, 100];

/// A station heard, as the heard list keeps it in RAM (APRS-4.5): what the list and its counts need. The rest
/// (path, course, altitude, comment, FX.25) is in [`Det`] for the stations heard most recently, else in the
/// station's last stored row, which the store reads from SPI flash when the station is shown ([`AprsLive::row_of`]).
#[derive(Clone, Copy)]
struct Heard {
    call: Buf<10>,
    sym: Sym,
    pos: Option<Pos>,
    at_s: u32,
    times: u16,
    /// APRS-4: the minute (seconds / 60, wrapping) its last stored row went this power-on ([`NO_ROW`]: not yet),
    /// and that row's position and comment, hashed ([`row_key`]: one byte each).
    row_at: u16,
    row_key: u16,
    /// The seq of its last stored row in the store's HEARD ring ([`NO_SEQ`]: none known).
    seq: u32,
    /// Heard this power-on ([`HEARD_NOW`]); its time unknown ([`HEARD_UNKNOWN`]: restored from a row whose age
    /// the store doesn't know, INBOX-6.1); heard direct ([`HEARD_DIRECT`], APRS-33's ?APRSD).
    flags: u8,
}

/// A station's details, for the stations heard most recently ([`DET`] of them, newest first; APRS-4.5): those
/// its frames gave (`known`, a bit per field), the rest from its stored row.
#[derive(Clone, Copy)]
struct Det {
    call: Buf<10>,
    known: u8,
    /// The digipeater it came through (the last used one), empty when heard direct.
    via: Buf<10>,
    course: Option<(u16, u16)>,
    alt_m: Option<i32>,
    /// APRS-5.2: the radio its Mic-E codes named (with the course: a position report's).
    device: Option<u8>,
    comment: Buf<40>,
    /// APRS-44: its last frame came as FX.25: the bytes corrected + 1 (0: plain AX.25).
    fx25: u8,
}

/// [`Det::known`]'s bits, and [`AprsLive::row_of`]'s mask of the fields the stored row fills.
pub const DET_VIA: u8 = 1;
pub const DET_COURSE: u8 = 2;
pub const DET_COMMENT: u8 = 4;
pub const DET_FX25: u8 = 8;
pub const DET_ALL: u8 = 15;

/// APRS-4.5: the stations whose details stay in RAM (the rest are read from their rows).
pub const DET: usize = 8;

const NO_ROW: u16 = u16::MAX;
const NO_SEQ: u32 = u32::MAX;
const HEARD_NOW: u8 = 1;
const HEARD_UNKNOWN: u8 = 2;
const HEARD_DIRECT: u8 = 4;

/// APRS-4: a station with a new position or comment gets a new row at most this often.
pub const ROW_CHANGE_S: u32 = 10 * 60;
/// APRS-4: a station still heard gets a row this often, so its time stays fresh across a restart.
pub const ROW_REFRESH_S: u32 = 30 * 60;

/// APRS-4: whether a station heard now (`now_s`) gets a stored row: the first time this power-on (`last` None),
/// when its position or comment changed and [`ROW_CHANGE_S`] have passed since its last row, or every
/// [`ROW_REFRESH_S`] while it's heard.
pub fn row_due(last: Option<u32>, changed: bool, now_s: u32) -> bool {
    match last {
        None => true,
        Some(t) => {
            let age = now_s.wrapping_sub(t);
            (changed && age >= ROW_CHANGE_S) || age >= ROW_REFRESH_S
        }
    }
}

/// What a row keeps track of, to tell a change (APRS-4): the position (high byte) and the comment (low byte),
/// each hashed to a byte; the comment's byte is left as it was when the comment isn't known (`None`: not in RAM).
fn row_key(pos: Option<Pos>, comment: Option<&Buf<40>>, old: u16) -> u16 {
    let fnv = |b: &mut dyn Iterator<Item = u8>| {
        let h = b.fold(0x811C_9DC5u32, |h, x| (h ^ x as u32).wrapping_mul(0x0100_0193));
        (h ^ h >> 8 ^ h >> 16 ^ h >> 24) as u8
    };
    let p = pos.map_or([0; 8], |p| {
        let mut b = [0; 8];
        b[..4].copy_from_slice(&p.lat.to_le_bytes());
        b[4..].copy_from_slice(&p.lon.to_le_bytes());
        b
    });
    let c = comment.map_or(old as u8, |c| fnv(&mut c.as_bytes().iter().copied()));
    (fnv(&mut p.iter().copied()) as u16) << 8 | c as u16
}

#[derive(Clone, Copy)]
struct Out {
    peer: Buf<10>,
    text: Buf<67>,
    /// The callsign it is sent as (APRS-13.4: empty until the first turn, which takes the current one).
    from: Buf<10>,
    id: u8,
    tries: u8,
    next_s: u32,
}

/// APRS-33: what a directed query gets.
#[derive(Clone, Copy)]
enum Answer {
    /// Our position report (`?APRSP`, `?APRS?`).
    Position,
    /// Our status report (`?APRSS`).
    Status,
    /// A message back, without a number (`?APRSD`, `?APRSH`, `?APRST`, `?PING?`, `?VER`).
    Text(Buf<67>),
}

#[derive(Clone, Copy)]
struct Query {
    to: Buf<10>,
    answer: Answer,
    at_s: u32,
}

/// APRS-35: a general query (`?APRS?`) to answer at `due_s`, with its footprint (centre in millionths of a
/// degree, radius in metres) if it gave one.
#[derive(Clone, Copy)]
struct General {
    due_s: u32,
    foot: Option<((i32, i32), u32)>,
}

#[derive(Clone, Copy)]
struct AckDue {
    peer: Buf<10>,
    id: Buf<5>,
}

/// What a received frame did, for the App (inbox, toasts) and the store (messages, the log).
#[derive(Clone, Copy, Default)]
pub struct Rx {
    /// A message to us (APRS-6): notify, save, log.
    pub inbound: Option<(Buf<10>, Buf<67>)>,
    /// Its message number (empty: none), for the store's check after a power cycle (APRS-32).
    pub inbound_id: Buf<5>,
    /// One of ours moved on: acked or rejected (APRS-34), for the store.
    pub state: Option<(Buf<10>, Buf<67>, MsgState)>,
    /// The heard list changed.
    pub changed: bool,
    /// APRS-4: the station at the front of the heard list ([`AprsLive::station`] 0) is due a stored row.
    pub row: bool,
    /// APRS-36: our own frame, heard back for the first time since we sent it, and the digipeater that sent it.
    pub echo: Option<Buf<10>>,
}

/// What the APRS task may send (APRS-22, APRS-29).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sending {
    /// Nothing: APRS mode off, no beacons (APRS-22 only decodes).
    Nothing,
    /// Beacons only: APRS mode off, automatic beacons on (APRS-29): no acks, messages or digipeats.
    Beacons,
    /// Everything: APRS mode on.
    All,
}

impl Sending {
    /// APRS-10.1: whether a frame of this kind ([`Tick::what`]) may still go: everything with APRS mode, only
    /// beacons with beacons alone, nothing otherwise. Checked again just before a waiting frame keys, so one
    /// queued while APRS held C never keys after C is given back.
    pub fn may_send(self, what: &str) -> bool {
        match self {
            Sending::All => true,
            Sending::Beacons => what == "Beacon",
            Sending::Nothing => false,
        }
    }
}

/// APRS-10, APRS-29 (decision 181): a frame waits for a clear moment on the APRS frequency (p-persistence on C's
/// squelch) at most this long, then it isn't sent.
pub const CLEAR_WAIT_MS: u32 = 5_000;

/// APRS-10, APRS-29: a frame waiting to go, sent in the first clear moment within [`CLEAR_WAIT_MS`] (the KISS
/// TNC's p-persistence, `uaprs::kiss::Access`, at its defaults: persistence 63, 100 ms slots).
#[derive(Clone, Copy, Debug, Default)]
pub struct ClearWait {
    since: Option<u32>,
    access: uaprs::kiss::Access,
}

/// [`ClearWait::poll`]'s answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clear {
    Wait,
    Send,
    /// Busy for the whole window: not sent.
    GiveUp,
}

impl ClearWait {
    /// A frame is waiting since `now_ms`.
    pub fn start(&mut self, now_ms: u32) {
        *self = ClearWait { since: Some(now_ms), access: uaprs::kiss::Access::default() };
    }

    pub fn waiting(&self) -> bool {
        self.since.is_some()
    }

    /// Each turn while a frame waits: `busy` C's squelch, `random` a fresh random byte.
    pub fn poll(&mut self, now_ms: u32, busy: bool, random: u8) -> Clear {
        let Some(t) = self.since else { return Clear::Wait };
        if self.access.may_send(&uaprs::kiss::Params::default(), now_ms, busy, random) {
            self.since = None;
            Clear::Send
        } else if now_ms.wrapping_sub(t) >= CLEAR_WAIT_MS {
            self.since = None;
            Clear::GiveUp
        } else {
            Clear::Wait
        }
    }
}

/// What a second did.
#[derive(Clone, Copy, Default)]
pub struct Tick {
    /// A frame to send: its length in the caller's buffer (0: none), what it is (for a refusal's toast), and
    /// whether APRS-9's spacing applies.
    pub len: usize,
    pub what: &'static str,
    /// Our messages that moved on (tried again, failed), for the store.
    pub states: [Option<(Buf<10>, Buf<67>, MsgState)>; 2],
    pub changed: bool,
}

/// APRS-53: the beacons the Sent page keeps, and the digipeaters kept for each.
pub const SENT_MAX: usize = 20;
const SENT_BY: usize = 2;

/// One sent beacon: the second it went out and who repeated it (calls zero-padded, an empty one unused).
#[derive(Clone, Copy, Default)]
struct SentRaw {
    at_s: u32,
    by: [[u8; 10]; SENT_BY],
}

/// APRS-53: the last [`SENT_MAX`] beacons we sent, in RAM only (lost at power-off): a ring of 24-byte entries.
/// The APRS task's and the simulator's.
pub struct SentLog {
    ring: [Cell<SentRaw>; SENT_MAX],
    /// The next slot to write, and how many are used.
    at: Cell<u8>,
    n: Cell<u8>,
}

impl Default for SentLog {
    fn default() -> Self {
        Self::new()
    }
}

impl SentLog {
    pub const fn new() -> Self {
        SentLog { ring: [const { Cell::new(SentRaw { at_s: 0, by: [[0; 10]; SENT_BY] }) }; SENT_MAX], at: Cell::new(0), n: Cell::new(0) }
    }

    /// APRS-53: a beacon went out at `now_s`; the oldest drops off a full ring.
    pub fn push(&self, now_s: u32) {
        let i = self.at.get() as usize;
        self.ring[i].set(SentRaw { at_s: now_s, ..SentRaw::default() });
        self.at.set(((i + 1) % SENT_MAX) as u8);
        self.n.set((self.n.get() + 1).min(SENT_MAX as u8));
    }

    /// APRS-53.1: the newest beacon heard back through `call`: kept unless it is there already or the entry has
    /// [`SENT_BY`]. True when it was added.
    pub fn heard_by(&self, call: &[u8]) -> bool {
        if self.n.get() == 0 || call.is_empty() || call.len() > 10 {
            return false;
        }
        let i = (self.at.get() as usize + SENT_MAX - 1) % SENT_MAX;
        let mut e = self.ring[i].get();
        if e.by.iter().any(|b| padded(b) == call) {
            return false;
        }
        let Some(slot) = e.by.iter_mut().find(|b| b[0] == 0) else { return false };
        slot[..call.len()].copy_from_slice(call);
        self.ring[i].set(e);
        true
    }

    pub fn len(&self) -> u16 {
        self.n.get() as u16
    }

    pub fn is_empty(&self) -> bool {
        self.n.get() == 0
    }

    /// Entry `i`, newest first, its age against `now_s`.
    pub fn get(&self, i: u16, now_s: u32) -> Option<crate::data::SentBeacon> {
        if i >= self.len() {
            return None;
        }
        let e = self.ring[(self.at.get() as usize + SENT_MAX - 1 - i as usize) % SENT_MAX].get();
        let mut s = crate::data::SentBeacon { ago_s: now_s.wrapping_sub(e.at_s), ..Default::default() };
        for (b, raw) in s.by.iter_mut().zip(e.by.iter()) {
            b.set(padded(raw));
        }
        Some(s)
    }
}

/// A call kept zero-padded, as its bytes.
fn padded(b: &[u8; 10]) -> &[u8] {
    &b[..b.iter().position(|&c| c == 0).unwrap_or(10)]
}

/// APRS-57.2: the stations remembered as replied to (the oldest forgotten for a new one).
const REPLIED: usize = 8;

/// The APRS task's state: the heard list ([`HEARD_MAX`] stations of about 40 bytes, and the details of the
/// [`DET`] heard most recently, APRS-4.5), our messages and acks, the digipeater's waiting frame.
pub struct AprsLive {
    heard: [Cell<Option<Heard>>; HEARD_MAX as usize],
    /// APRS-4.5: the details of the stations heard most recently, newest first.
    det: [Cell<Option<Det>>; DET],
    out: [Cell<Option<Out>>; OUT],
    next_id: Cell<u8>,
    acks: [Cell<Option<AckDue>>; ACKS],
    seen: [Cell<(u32, u32)>; SEEN],
    seen_at: Cell<u8>,
    /// APRS-33: a directed query's answer waiting, a general query's, and when the last general one went.
    query: Cell<Option<Query>>,
    general: Cell<Option<General>>,
    last_general: Cell<Option<u32>>,
    dupes: [Cell<(u16, u32)>; DUPES],
    dupe_at: Cell<u8>,
    /// A frame to digipeat, waiting for its turn, and the second it may go (0 5 7 3's wait).
    digi: Cell<Option<([u8; FRAME_MAX], u16)>>,
    digi_at: Cell<u32>,
    last_digi: Cell<Option<u32>>,
    last_auto: Cell<Option<u32>>,
    /// APRS-57: the auto reply waiting (to whom, the second it goes), and the stations replied to lately (when).
    reply_due: Cell<Option<(Buf<10>, u32)>>,
    replied: [Cell<Option<(Buf<10>, u32)>>; REPLIED],
    replied_at: Cell<u8>,
    beacon_last: Cell<Option<LastBeacon>>,
    beacon_now: Cell<bool>,
    /// APRS-50.1: automatic beacons sent since power-on (wrapping), for the beacon text's rate.
    beacons: Cell<u8>,
    /// APRS-27: a beacon owed after a voice transmission (kept to APRS-9's spacing, unlike "beacon now").
    after_voice: Cell<bool>,
    waiting: Cell<bool>,
    /// GPS-33: a due beacon waiting for a fresh fix since then.
    fix_wait: Cell<Option<u32>>,
    on_since: Cell<Option<u32>>,
    now_s: Cell<u32>,
    /// APRS-36: a frame of ours went out and hasn't been heard back yet; who repeated the last one heard back.
    echo: Cell<bool>,
    echo_by: Cell<Buf<10>>,
    /// APRS-53: the beacons sent, and the last beacon's information field (CRC-16), to know its copies.
    sent: SentLog,
    sent_key: Cell<u16>,
    /// What the last turn allowed (APRS-10.1): a frame heard is queued to digipeat only while everything may go.
    sending: Cell<Sending>,
    /// APRS-54: what our position reports' values come from: the battery (mV, 0 not read yet), the MCU's
    /// temperature (whole degrees C) and the APRS frequency (for the beacon power's watts).
    readings: Cell<Readings>,
}

impl Default for AprsLive {
    fn default() -> Self {
        Self::new()
    }
}

impl AprsLive {
    pub const fn new() -> Self {
        AprsLive {
            heard: [const { Cell::new(None) }; HEARD_MAX as usize],
            det: [const { Cell::new(None) }; DET],
            out: [const { Cell::new(None) }; OUT],
            next_id: Cell::new(1),
            acks: [const { Cell::new(None) }; ACKS],
            seen: [const { Cell::new((0, 0)) }; SEEN],
            seen_at: Cell::new(0),
            query: Cell::new(None),
            general: Cell::new(None),
            last_general: Cell::new(None),
            dupes: [const { Cell::new((0, 0)) }; DUPES],
            dupe_at: Cell::new(0),
            digi: Cell::new(None),
            digi_at: Cell::new(0),
            last_digi: Cell::new(None),
            last_auto: Cell::new(None),
            reply_due: Cell::new(None),
            replied: [const { Cell::new(None) }; REPLIED],
            replied_at: Cell::new(0),
            beacon_last: Cell::new(None),
            beacon_now: Cell::new(false),
            beacons: Cell::new(0),
            after_voice: Cell::new(false),
            waiting: Cell::new(false),
            fix_wait: Cell::new(None),
            on_since: Cell::new(None),
            now_s: Cell::new(0),
            echo: Cell::new(false),
            echo_by: Cell::new(Buf::new()),
            sent: SentLog::new(),
            sent_key: Cell::new(0),
            sending: Cell::new(Sending::Nothing),
            readings: Cell::new((0, None, 0)),
        }
    }

    // ---- the heard list (APRS-3, APRS-4) ----

    pub fn stations(&self) -> u16 {
        self.heard.iter().take_while(|h| h.get().is_some()).count() as u16
    }

    /// Station `i` (newest first) as RAM has it: the fields [`AprsLive::row_of`] names are left empty for the
    /// stored row to fill (the store's [`crate::data::Data::station`] does that).
    pub fn station(&self, i: u16) -> Option<Station> {
        let h = self.heard.get(i as usize)?.get()?;
        let d = self.det_of(&h.call);
        let known = d.map_or(0, |d| d.known);
        let mut via = Buf::<24>::new();
        if let Some(d) = d.filter(|d| !d.via.is_empty()) {
            via.push(d.via.as_bytes());
            via.push(b"*");
        }
        Some(Station {
            call: h.call,
            sym: h.sym,
            pos: h.pos,
            ago_s: if h.flags & HEARD_UNKNOWN != 0 { AGO_UNKNOWN } else { self.now_s.get().wrapping_sub(h.at_s) },
            via,
            course: d.filter(|_| known & DET_COURSE != 0).and_then(|d| d.course),
            alt_m: d.filter(|_| known & DET_COURSE != 0).and_then(|d| d.alt_m),
            times: h.times,
            comment: d.filter(|_| known & DET_COMMENT != 0).map_or(Buf::new(), |d| d.comment),
            msg: Buf::new(),
            msg_min: None,
            fx25: d.filter(|_| known & DET_FX25 != 0).and_then(|d| d.fx25.checked_sub(1)),
            device: d.filter(|_| known & DET_COURSE != 0).and_then(|d| d.device),
        })
    }

    /// APRS-4.5: station `i`'s stored row to fill what RAM doesn't hold: its seq in the store's HEARD ring and
    /// the fields to take from it ([`DET_VIA`] and the others). None when RAM has it all, or no row is known.
    pub fn row_of(&self, i: u16) -> Option<(u32, u8)> {
        let h = self.heard.get(i as usize)?.get()?;
        let missing = DET_ALL & !self.det_of(&h.call).map_or(0, |d| d.known);
        (missing != 0 && h.seq != NO_SEQ).then_some((h.seq, missing))
    }

    /// APRS-4.5: the store wrote `call`'s row as `seq`.
    pub fn row_stored(&self, call: &Buf<10>, seq: u32) {
        if let Some(c) = self.heard.iter().find(|c| c.get().is_some_and(|o| o.call == *call)) {
            let mut h = c.get().unwrap();
            h.seq = seq;
            c.set(Some(h));
        }
    }

    fn det_of(&self, call: &Buf<10>) -> Option<Det> {
        self.det.iter().find_map(|d| d.get().filter(|d| d.call == *call))
    }

    /// A station's details to the front of [`AprsLive::det`] (the oldest dropped when full): the fields it
    /// knows over the ones it had.
    fn det_put(&self, mut d: Det) {
        let old = self.det.iter().position(|c| c.get().is_some_and(|o| o.call == d.call));
        if let Some(o) = old.and_then(|i| self.det[i].get()) {
            if d.known & DET_COURSE == 0 && o.known & DET_COURSE != 0 {
                (d.course, d.alt_m, d.device) = (o.course, o.alt_m, o.device);
            }
            if d.known & DET_COMMENT == 0 && o.known & DET_COMMENT != 0 {
                d.comment = o.comment;
            }
            d.known |= o.known & (DET_COURSE | DET_COMMENT);
        }
        let end = old.unwrap_or(self.det.len() - 1);
        for i in (1..=end).rev() {
            self.det[i].set(self.det[i - 1].get());
        }
        self.det[0].set(Some(d));
    }

    /// APRS-44: the frame just given to [`AprsLive::receive`] (or heard plain already, a duplicate) came as FX.25
    /// with `corrected` bytes repaired: its station says so (the station screen's Path row).
    pub fn note_fx25(&self, frame: &[u8], corrected: u8) {
        let Some(f) = Frame::parse(frame) else { return };
        let mut call = Buf::<10>::new();
        f.src.emit(&mut |c| {
            call.push(&[c]);
        });
        if let Some(c) = self.det.iter().find(|c| c.get().is_some_and(|o| o.call == call)) {
            let mut d = c.get().unwrap();
            (d.fx25, d.known) = (corrected.saturating_add(1), d.known | DET_FX25);
            c.set(Some(d));
        }
    }

    /// APRS-4: a station from the stored rows at power-on (newest first, so each goes after the ones before),
    /// with its age then (`AGO_UNKNOWN`: unknown); `now_s` is this clock's now. A station already listed (heard
    /// since, or given again once GPS time is known) only takes a known age in place of an unknown one.
    pub fn restore(&self, s: &Station, seq: u32, now_s: u32) {
        let known = s.ago_s != AGO_UNKNOWN;
        let at_s = now_s.wrapping_sub(if known { s.ago_s } else { 0 });
        if let Some(c) = self.heard.iter().find(|c| c.get().is_some_and(|o| o.call == s.call)) {
            let mut h = c.get().unwrap();
            if known && h.flags & (HEARD_UNKNOWN | HEARD_NOW) == HEARD_UNKNOWN {
                (h.at_s, h.flags) = (at_s, 0);
                c.set(Some(h));
            }
            return;
        }
        let Some(c) = self.heard.iter().find(|c| c.get().is_none()) else { return };
        let direct = if s.via.is_empty() { HEARD_DIRECT } else { 0 };
        c.set(Some(Heard {
            call: s.call,
            sym: s.sym,
            pos: s.pos,
            at_s,
            times: s.times,
            row_at: NO_ROW,
            row_key: row_key(s.pos, Some(&s.comment), 0),
            seq,
            flags: direct | if known { 0 } else { HEARD_UNKNOWN },
        }));
    }

    pub fn clear_heard(&self) {
        self.heard.iter().for_each(|h| h.set(None));
        self.det.iter().for_each(|d| d.set(None));
    }

    /// The reset (BOOT-14): the heard list, our messages being tried, everything waiting to go and the beacon
    /// timing go (APRS-10.1).
    pub fn clear(&self) {
        self.clear_heard();
        self.clear_out();
        self.reply_due.set(None); // APRS-57
        self.beacon_now.set(false);
        self.waiting.set(false);
        self.acks.iter().for_each(|a| a.set(None));
        self.query.set(None);
        self.general.set(None);
        self.beacon_last.set(None);
        self.after_voice.set(false);
        self.fix_wait.set(None);
        self.digi.set(None);
    }

    /// A station heard now (`h`, with the details its frame gave in `d`): to the front, its old entry (if any)
    /// gone, the oldest dropped when full. True when it is due a stored row (APRS-4, [`row_due`]).
    fn hear(&self, mut h: Heard, d: Det) -> bool {
        let old = self.heard.iter().position(|c| c.get().is_some_and(|o| o.call == h.call));
        if let Some(o) = old.and_then(|i| self.heard[i].get()) {
            h.times = o.times.saturating_add(1);
            if h.pos.is_none() {
                (h.pos, h.sym) = (o.pos, o.sym);
            }
            (h.row_at, h.row_key, h.seq) = (o.row_at, o.row_key, o.seq);
        }
        self.det_put(d);
        let comment = self.det_of(&h.call).filter(|d| d.known & DET_COMMENT != 0).map(|d| d.comment);
        let key = row_key(h.pos, comment.as_ref(), h.row_key);
        let now_min = (h.at_s / 60) as u16;
        // Minutes since its last row, kept in 16 bits (wrapping every 45 days; a row at minute 0xFFFF reads as none:
        // one row more).
        let last = (h.row_at != NO_ROW).then(|| h.at_s.wrapping_sub(now_min.wrapping_sub(h.row_at) as u32 * 60));
        let due = row_due(last, key != h.row_key, h.at_s);
        if due {
            (h.row_at, h.row_key) = (now_min, key);
        }
        let end = old.unwrap_or(self.heard.len() - 1);
        for i in (1..=end).rev() {
            self.heard[i].set(self.heard[i - 1].get());
        }
        self.heard[0].set(Some(h));
        due
    }

    // ---- messages (APRS-6, APRS-13) ----

    /// Our message (the store's Send): its first try at the next tick, exempt from APRS-9's spacing. False when
    /// [`OUT`] are being tried already (APRS-13.3: the store checks [`AprsLive::room`] before it saves one).
    pub fn send(&self, to: &[u8], text: &[u8]) -> bool {
        let Some(slot) = self.out.iter().find(|o| o.get().is_none()) else { return false };
        let id = self.next_id.get();
        self.next_id.set(if id >= 99 { 1 } else { id + 1 });
        slot.set(Some(Out { peer: Buf::from(to), text: Buf::from(text), from: Buf::new(), id, tries: 0, next_s: self.now_s.get() }));
        true
    }

    /// APRS-13.3: room for one more of our messages.
    pub fn room(&self) -> bool {
        self.out.iter().any(|o| o.get().is_none())
    }

    /// APRS-13.1: our message to `to` with `text` is tried no more (stopped, or deleted). True when it was.
    pub fn unqueue(&self, to: &[u8], text: &[u8]) -> bool {
        let Some(c) = self.out.iter().find(|o| o.get().is_some_and(|o| o.peer.as_bytes() == to && o.text.as_bytes() == text)) else { return false };
        c.set(None);
        true
    }

    /// APRS-46: every message deleted: none of ours is tried any more.
    pub fn clear_out(&self) {
        self.out.iter().for_each(|o| o.set(None));
    }

    pub fn beacon_now(&self) {
        self.beacon_now.set(true);
    }

    /// APRS-54: the battery in millivolts (0: not read yet), the MCU's temperature in whole degrees (None: not
    /// read yet) and the APRS frequency, for the values in our position reports (the firmware's each turn).
    pub fn readings(&self, battery_mv: u16, mcu_c: Option<i8>, aprs_f: Freq10) {
        self.readings.set((battery_mv, mcu_c, aprs_f));
    }

    /// A voice transmission ended (APRS-27): a beacon at the next turn that APRS-9's spacing allows.
    pub fn after_voice(&self) {
        self.after_voice.set(true);
    }

    /// The beacon's state for the APRS card (APRS-17, APRS-20).
    pub fn beacon(&self, cfg: &AprsCfg) -> Beacon {
        let next_s = cfg.every_min().map(|m| match self.beacon_last.get() {
            Some(l) => (l.at_s + m as u32 * 60).saturating_sub(self.now_s.get()),
            None => 0,
        });
        Beacon { waiting: self.waiting.get(), next_s: next_s.filter(|_| self.on_since.get().is_some()), digi: self.echo_by.get() }
    }

    /// APRS-53: the beacon framed last went out (the APRS task calls this as it plays it): a Sent entry.
    pub fn beacon_sent(&self, now_s: u32) {
        self.now_s.set(now_s);
        self.sent.push(now_s);
    }

    /// APRS-53: the Sent page's entries, newest first.
    pub fn beacons_sent(&self) -> u16 {
        self.sent.len()
    }

    pub fn sent_beacon(&self, i: u16) -> Option<crate::data::SentBeacon> {
        self.sent.get(i, self.now_s.get())
    }

    // ---- receive ----

    /// A frame from the decoder. `me` is our callsign (empty: none set, nothing is acked or digipeated, APRS-8);
    /// `cfg` has the digipeater's settings (0 5 7, off by default: APRS-12, APRS-26).
    pub fn receive(&self, frame: &[u8], now_s: u32, me: &[u8], cfg: &AprsCfg) -> Rx {
        self.now_s.set(now_s);
        let mut rx = Rx::default();
        let Some(f) = Frame::parse(frame) else { return rx };
        if f.info.len() > INFO_MAX {
            return rx; // APRS-25
        }
        let my = Address::from_text(me);
        if my.is_some_and(|m| f.src.same(&m)) {
            // APRS-36: our own, digipeated back: the first time after a transmission, who repeated it
            let used = || f.digis().iter().rev().filter(|d| d.repeated);
            let station = used().find(|d| !d.call().starts_with(b"WIDE")).or_else(|| used().next());
            let mut by = Buf::<10>::new();
            if let Some(d) = station {
                d.emit(&mut |c| {
                    by.push(&[c]);
                });
            }
            // APRS-53.1: a copy of our last beacon (its information field, unchanged by digipeaters): every
            // digipeater heard repeating it is kept with its Sent entry
            if station.is_some() && f.info.iter().fold(0xFFFF, |h, &b| crc16(h, b)) == self.sent_key.get() {
                rx.changed |= self.sent.heard_by(by.as_bytes());
            }
            if station.is_some() && self.echo.get() {
                self.echo.set(false);
                self.echo_by.set(by);
                rx.echo = Some(by);
            }
            return rx;
        }
        let dupe = self.dupe(&f, now_s);
        if cfg.digi_on() && !dupe && self.sending.get() == Sending::All {
            if let Some(m) = my {
                if self.digipeat(&f, m, cfg.digi_mycall()) {
                    self.digi_at.set(now_s + cfg.digi_wait_s() as u32);
                }
            }
        }
        let mut call = Buf::<10>::new();
        f.src.emit(&mut |c| {
            call.push(&[c]);
        });
        let mut via = Buf::<10>::new();
        if let Some(d) = f.digis().iter().rev().find(|d| d.repeated) {
            d.emit(&mut |c| {
                via.push(&[c]);
            });
        }
        let direct = if via.is_empty() { HEARD_DIRECT } else { 0 };
        let mut h = Heard { call, sym: Sym::Ht, pos: None, at_s: now_s, times: 1, row_at: NO_ROW, row_key: 0, seq: NO_SEQ, flags: HEARD_NOW | direct };
        let mut d = Det { call, known: DET_VIA | DET_FX25, via, course: None, alt_m: None, device: None, comment: Buf::new(), fx25: 0 };
        let mut dest = [0u8; 6];
        let dc = f.dest.call();
        dest[..dc.len()].copy_from_slice(dc);
        match parse::parse(&dest[..dc.len()], f.info) {
            Ok(p @ (Packet::Position { pos, comment, .. } | Packet::Object { pos, comment, .. } | Packet::Item { pos, comment, .. })) => {
                d.device = match p {
                    Packet::Position { mic_e: Some(m), .. } => m.device,
                    _ => None,
                };
                h.pos = Some(Pos { lat: pos.lat.saturating_mul(10), lon: pos.lon.saturating_mul(10) });
                h.sym = sym(pos.table, pos.code);
                d.course = pos.course.map(|c| (c, pos.speed.map_or(0, |k| (k as u32 * 1852 / 1000) as u16)));
                // in i64: `/A=999999` (any 6 digits) times 3048 passes i32 (found by fuzzing, Oct 31)
                d.alt_m = pos.altitude.map(|ft| (ft as i64 * 3048 / 10_000) as i32);
                d.known |= DET_COURSE;
                // APRS-23: a weather report's fields stay at the start of the comment for the station screen.
                for b in comment.bytes().take(40) {
                    d.comment.push(&[b]);
                }
                if !d.comment.is_empty() {
                    d.known |= DET_COMMENT;
                }
                rx.row = self.hear(h, d);
                rx.changed = true;
            }
            Ok(Packet::Status { text, .. }) => {
                d.comment = Buf::from(&text[..text.len().min(40)]);
                d.known |= if d.comment.is_empty() { 0 } else { DET_COMMENT };
                rx.row = self.hear(h, d);
                rx.changed = true;
            }
            Ok(Packet::Message(m)) => {
                rx.row = self.hear(h, d);
                rx.changed = true;
                if my.is_some_and(|me| Address::from_text(m.addressee).is_some_and(|a| a.same(&me))) {
                    self.to_us(&call, m, &f, dupe, now_s, &mut rx);
                    if rx.inbound.is_some() {
                        self.auto_reply(&call, me, now_s, cfg); // APRS-57
                    }
                }
            }
            Ok(Packet::Other { kind: b'?', data }) => {
                rx.row = self.hear(h, d);
                rx.changed = true;
                if my.is_some() && !dupe {
                    self.general_query(data, &f, now_s);
                }
            }
            // APRS-58: third-party traffic (an IGate relaying from the internet, `}`): the relaying station is
            // heard, and a message inside to us (an ack among them) is taken as from its own sender, so messages and
            // acks from APRS-IS stations and services (SMSGTE, EMAIL-2, WXBOT) aren't lost and a message sent to
            // one isn't shown as failed when it was delivered (a review of Nov 20, 2026: these were dropped).
            Ok(Packet::ThirdParty(t)) => {
                rx.row = self.hear(h, d);
                rx.changed = true;
                let inner = third_party_call(t.src);
                if let (Some(mine), Some(from), Ok(Packet::Message(m))) = (my, inner, parse::parse(t.dest, t.info)) {
                    if Address::from_text(m.addressee).is_some_and(|a| a.same(&mine)) {
                        self.to_us(&from, m, &f, dupe, now_s, &mut rx);
                        if rx.inbound.is_some() {
                            self.auto_reply(&from, me, now_s, cfg); // APRS-57
                        }
                    }
                }
            }
            Ok(_) | Err(_) => {
                rx.row = self.hear(h, d);
                rx.changed = true;
            }
        }
        rx
    }

    /// APRS-57: a new message to us from `from`: the auto reply is set to go after its delay when it is on, has a
    /// text, the pattern takes `from`, and `from` isn't ourselves (any SSID), a service (APRS-39) or a station
    /// replied to in the last [`REPLY_HOLD_S`] (APRS-57.2: two radios replying to each other stop at one each).
    /// One waits at a time: a second station in the delay gets none.
    fn auto_reply(&self, from: &Buf<10>, me: &[u8], now_s: u32, cfg: &AprsCfg) {
        let base = |c: &[u8]| -> Buf<10> { Buf::from(c.split(|&b| b == b'-').next().unwrap_or(c)) };
        let ok = cfg.reply_on()
            && !cfg.reply_text().is_empty()
            && cfg.reply_matches(from.as_bytes())
            && !base(from.as_bytes()).as_bytes().eq_ignore_ascii_case(base(me).as_bytes())
            && crate::screens::contacts::service(from.as_bytes()).is_none()
            && self.reply_due.get().is_none()
            && !self.replied.iter().any(|r| r.get().is_some_and(|(c, at)| c == *from && now_s.wrapping_sub(at) < REPLY_HOLD_S));
        if ok {
            self.reply_due.set(Some((*from, now_s.wrapping_add(cfg.reply_delay_s() as u32))));
        }
    }

    /// APRS-57: the station the auto reply goes to now, once its delay is up (the caller sends it as one of our
    /// messages, APRS-13): marked replied to, so it gets no other for [`REPLY_HOLD_S`].
    pub fn reply_now(&self, now_s: u32) -> Option<Buf<10>> {
        let (to, at) = self.reply_due.get()?;
        if (now_s.wrapping_sub(at) as i32) < 0 {
            return None;
        }
        self.reply_due.set(None);
        let i = self.replied_at.get() as usize % REPLIED;
        self.replied[i].set(Some((to, now_s)));
        self.replied_at.set(((i + 1) % REPLIED) as u8);
        Some(to)
    }

    fn to_us(&self, from: &Buf<10>, m: parse::Message, f: &Frame, dupe: bool, now_s: u32, rx: &mut Rx) {
        match m.kind {
            MessageKind::Message => {
                // APRS-6, APRS-32: every copy with a number is acked (its sender retries because it missed ours).
                if let Some(i) = m.id {
                    self.owe_ack(from, &Buf::from(i));
                }
                // APRS 1.1 reply-ack: it also acknowledges one of ours.
                if let Some(a) = m.reply_ack {
                    self.acked(from, a, MsgState::Acked, rx);
                }
                // APRS-33: a query is answered (once for the copies digipeaters repeat), not kept as a message.
                if let Some(a) = self.directed(m.text, f) {
                    if let Some(answer) = a.filter(|_| !dupe && self.query.get().is_none()) {
                        self.query.set(Some(Query { to: *from, answer, at_s: now_s }));
                    }
                    return;
                }
                let text = Buf::<67>::from(&m.text[..m.text.len().min(67)]);
                if !self.seen_before(from, m.id, &text, now_s) {
                    rx.inbound = Some((*from, text));
                    rx.inbound_id = Buf::from(m.id.unwrap_or(b""));
                }
            }
            MessageKind::Ack => {
                if let Some(a) = m.id {
                    self.acked(from, a, MsgState::Acked, rx);
                }
            }
            // APRS-34: rejected ("rejNN", or "rejNN}AA" from a reply-ack station): ended at once, no more tries.
            MessageKind::Rej => {
                if let Some(a) = m.id {
                    self.acked(from, a, MsgState::Rejected, rx);
                }
            }
        }
    }

    /// APRS-32: whether this message to us was heard before: the same sender, number and text (a retry), or
    /// without a number the same sender and text within [`SEEN_PLAIN_S`]. A new one is remembered, the oldest
    /// entry making room.
    fn seen_before(&self, from: &Buf<10>, id: Option<&[u8]>, text: &Buf<67>, now_s: u32) -> bool {
        let k = fnv(fnv(fnv(fnv(FNV0, from.as_bytes()), &[0]), id.unwrap_or(b"")), &[0]);
        let mut k = fnv(k, text.as_bytes()) & !1 | id.is_some() as u32;
        if k >> 1 == 0 {
            k |= 2; // 0 is an empty entry
        }
        let hit = self.seen.iter().any(|c| {
            let (key, at) = c.get();
            key == k && (id.is_some() || now_s.wrapping_sub(at) < SEEN_PLAIN_S)
        });
        if !hit {
            let i = self.seen_at.get() as usize;
            self.seen[i].set((k, now_s));
            self.seen_at.set(((i + 1) % SEEN) as u8);
        }
        hit
    }

    /// APRS-33: a message to us that is a query: None when it isn't one, Some(None) for one we don't answer
    /// (`?APRSM`, `?APRSO`, `?WX?`, `?IGATE?`...), else the answer.
    fn directed(&self, text: &[u8], f: &Frame) -> Option<Option<Answer>> {
        let t = trim(text);
        let is = |q: &[u8]| t.eq_ignore_ascii_case(q);
        let starts = |q: &[u8]| t.len() >= q.len() && t[..q.len()].eq_ignore_ascii_case(q);
        if t.first() != Some(&b'?') {
            return None;
        }
        let mut b = Buf::<67>::new();
        Some(Some(if is(b"?APRSP") || is(b"?APRS?") {
            Answer::Position
        } else if is(b"?APRSS") {
            Answer::Status
        } else if is(b"?APRSD") {
            b.push(b"Directs=");
            for h in self.heard.iter().filter_map(|h| h.get()).filter(|h| h.flags & HEARD_DIRECT != 0) {
                if b.len() + 1 + h.call.len() > 67 {
                    break;
                }
                b.push(b" ");
                b.push(h.call.as_bytes());
            }
            Answer::Text(b)
        } else if starts(b"?APRSH") {
            let call = trim(&t[6..]);
            if call.is_empty() {
                return Some(None);
            }
            let up = |c: &[u8]| -> Buf<10> {
                let mut u = Buf::<10>::new();
                c.iter().for_each(|&x| u.push(&[x.to_ascii_uppercase()]));
                u
            };
            let call = up(call);
            match self.heard.iter().filter_map(|h| h.get()).find(|h| h.call.as_bytes().eq_ignore_ascii_case(call.as_bytes())) {
                Some(h) => {
                    let min = self.now_s.get().wrapping_sub(h.at_s) / 60;
                    b.push(h.call.as_bytes());
                    let _ = uwrite!(b, " heard {}x", h.times);
                    if h.flags & HEARD_UNKNOWN == 0 {
                        let _ = uwrite!(b, ", last {} min ago", min); // not for a row from before a restart (APRS-4)
                    }
                }
                None => {
                    b.push(call.as_bytes());
                    b.push(b" not heard");
                }
            }
            Answer::Text(b)
        } else if is(b"?APRST") || is(b"?PING?") {
            trace(f, &mut b);
            Answer::Text(b)
        } else if is(b"?VER") || is(b"?VER?") {
            b.push(b"Ferrite ");
            b.push(ferrite_core::VERSION.as_bytes());
            Answer::Text(b)
        } else if starts(b"?APRS") || is(b"?WX?") || is(b"?IGATE?") {
            return Some(None);
        } else {
            return None;
        }))
    }

    /// APRS-35: a general query heard (the information field after its `?`): `APRS?`, optionally with a
    /// footprint `lat,lon,radius` (degrees, miles); answered with our position after a random wait, at most once
    /// per [`GENERAL_GAP_S`]. Other general queries (`?WX?`, `?IGATE?`) aren't ours to answer.
    fn general_query(&self, data: &[u8], f: &Frame, now_s: u32) {
        let Some(rest) = data.strip_prefix(b"APRS?") else { return };
        if self.general.get().is_some() || self.last_general.get().is_some_and(|l| now_s.wrapping_sub(l) < GENERAL_GAP_S) {
            return;
        }
        let rest = trim(rest);
        let foot = if rest.is_empty() {
            None
        } else {
            match footprint(rest) {
                Some(p) => Some(p),
                None => return, // a footprint we can't read: not meant for everyone
            }
        };
        let h = fnv(fnv(FNV0, f.src.call()), f.info) ^ now_s;
        self.general.set(Some(General { due_s: now_s + GENERAL_WAIT_S + h % GENERAL_SPREAD_S, foot }));
    }

    fn acked(&self, from: &Buf<10>, id: &[u8], state: MsgState, rx: &mut Rx) {
        for c in &self.out {
            if let Some(o) = c.get().filter(|o| o.peer.as_bytes().eq_ignore_ascii_case(from.as_bytes()) && num(o.id).as_bytes() == id) {
                c.set(None);
                rx.state = Some((o.peer, o.text, state));
            }
        }
    }

    fn owe_ack(&self, peer: &Buf<10>, id: &Buf<5>) {
        let a = AckDue { peer: *peer, id: *id };
        if self.acks.iter().any(|c| c.get().is_some_and(|x| x.peer == a.peer && x.id == a.id)) {
            return;
        }
        if let Some(c) = self.acks.iter().find(|c| c.get().is_none()) {
            c.set(Some(a));
        }
    }

    /// The duplicate check: source and information seen within [`DUPE_S`].
    fn dupe(&self, f: &Frame, now_s: u32) -> bool {
        let mut h = 0xFFFFu16;
        let mut add = |b: u8| h = crc16(h, b);
        f.src.call().iter().for_each(|&b| add(b));
        add(f.src.ssid);
        f.info.iter().for_each(|&b| add(b));
        let seen = self.dupes.iter().any(|d| {
            let (k, t) = d.get();
            k == h && t != 0 && now_s.wrapping_sub(t) < DUPE_S
        });
        let i = self.dupe_at.get() as usize;
        self.dupes[i].set((h, now_s.max(1)));
        self.dupe_at.set(((i + 1) % DUPES) as u8);
        seen
    }

    /// A fill-in digipeater (APRS-26): the first unused path entry, when it is WIDE1-1 (or our callsign, with
    /// `mycall`), becomes our call, used. True when a frame is waiting to go.
    fn digipeat(&self, f: &Frame, me: Address, mycall: bool) -> bool {
        let wide1 = Address::from_text(b"WIDE1-1");
        let Some(i) = f.digis().iter().position(|d| !d.repeated) else { return false };
        let next = &f.digis()[i];
        if !(wide1.is_some_and(|w| next.same(&w)) || mycall && next.same(&me)) {
            return false;
        }
        let mut path = [me; uaprs::ax25::MAX_DIGIS];
        let n = f.digis().len();
        path[..n].copy_from_slice(f.digis());
        let mut used = me;
        used.repeated = true;
        path[i] = used;
        let mut buf = [0u8; FRAME_MAX];
        let Some(len) = Frame::new(f.src, f.dest, &path[..n], f.info).and_then(|g| g.write(&mut buf)) else { return false };
        self.digi.set(Some((buf, len as u16)));
        true
    }

    // ---- transmit ----

    /// A second of the APRS task. `send`: what may go (APRS mode: all, APRS-15; beacons alone: beacons, APRS-29;
    /// else nothing, APRS-22); `relay_ok`: slot C can transmit with the band relay
    /// where the active slot holds it (APRS-20); `me` our callsign (APRS-8); `fix` the GPS's (beacons, APRS-24).
    /// A frame to send is written to `out`; the caller keys slot C, plays it, unkeys.
    pub fn tick(&self, now_s: u32, send: Sending, relay_ok: bool, me: &[u8], fix: Option<Fix>, cfg: &AprsCfg, out: &mut [u8; FRAME_MAX]) -> Tick {
        self.now_s.set(now_s);
        self.sending.set(send);
        let mut t = Tick::default();
        // 0 5 7 1 turned off while a digipeat waits its turn: it doesn't go (APRS-10.1).
        if !cfg.digi_on() {
            self.digi.set(None);
        }
        // APRS-33: queries are answered only in APRS mode with automatic beacons on, and not long after they came.
        if send != Sending::All || !cfg.beacons_on() {
            self.query.set(None);
            self.general.set(None);
        }
        if self.query.get().is_some_and(|q| now_s.wrapping_sub(q.at_s) > QUERY_TTL_S) {
            self.query.set(None);
        }
        if self.general.get().is_some_and(|g| now_s > g.due_s + QUERY_TTL_S) {
            self.general.set(None);
        }
        if send == Sending::Beacons {
            // APRS-29: beacons alone answer nothing: acks owed and a digipeat waiting are dropped.
            self.acks.iter().for_each(|a| a.set(None));
            self.digi.set(None);
        }
        if send == Sending::Nothing {
            // Without APRS mode nothing is sent (APRS-22 only decodes): everything owed or asked for meanwhile is
            // dropped (acks, a digipeat, Beacon now, the beacon after voice), so turning the mode on later doesn't
            // send stale ones (APRS-10.1). Our messages wait for it (LINK-45).
            self.acks.iter().for_each(|a| a.set(None));
            self.digi.set(None);
            self.beacon_now.set(false);
            self.after_voice.set(false);
            self.on_since.set(None);
            self.fix_wait.set(None);
            t.changed = self.waiting.replace(false);
            return t;
        }
        if self.on_since.get().is_none() {
            self.on_since.set(Some(now_s));
        }
        // Ours that ran out of tries.
        for c in &self.out {
            if let Some(o) = c.get().filter(|o| o.tries >= TRIES && now_s >= o.next_s) {
                c.set(None);
                push_state(&mut t, (o.peer, o.text, MsgState::Failed));
                t.changed = true;
            }
        }
        let beacon = self.beacon_now.get() || beacon_due(cfg, self.beacon_last.get(), now_s, fix);
        let waiting = beacon && !relay_ok;
        t.changed |= self.waiting.replace(waiting) != waiting;
        let Some(my) = Address::from_text(me).filter(|_| relay_ok) else { return t };
        let auto_ok = self.last_auto.get().is_none_or(|l| now_s.wrapping_sub(l) >= AUTO_GAP_S);
        let mut info = [0u8; INFO_MAX];
        // The tocall, or a Mic-E report's latitude (APRS-52).
        let mut dest = TOCALL;
        // Acks first (they're owed), then a digipeat, then ours, then the beacon: one transmission a second.
        // Beacons alone (APRS-29): only the beacon.
        let all = send == Sending::All;
        if let Some(c) = self.acks.iter().find(|c| c.get().is_some()) {
            let a = c.get().unwrap();
            c.set(None);
            if let Some(n) = uaprs::build::ack(a.peer.as_bytes(), a.id.as_bytes(), &mut info) {
                return self.framed(my, &dest, &info[..n], "Ack", cfg, out, t);
            }
        }
        if let Some((buf, len)) = self.digi.take() {
            if now_s < self.digi_at.get() {
                self.digi.set(Some((buf, len))); // 0 5 7 3's wait
            } else if self.last_digi.get().is_none_or(|l| now_s.wrapping_sub(l) >= DIGI_GAP_S) {
                self.last_digi.set(Some(now_s));
                out[..len as usize].copy_from_slice(&buf[..len as usize]);
                return Tick { len: len as usize, what: "Digipeat", ..t };
            }
            // Over the rate: dropped, not delayed (the next station will carry it).
        }
        // APRS-13.4: a message is sent as one callsign throughout, since its ack comes back to that one. The
        // callsign changed since its first try: it ends as failed (one moved on per turn, as the store hears them).
        let me_call = Buf::<10>::from(me);
        for c in self.out.iter().filter(|_| all) {
            if let Some(mut o) = c.get() {
                if o.from.is_empty() {
                    o.from = me_call;
                    c.set(Some(o));
                } else if o.from != me_call && t.states.iter().any(|s| s.is_none()) {
                    c.set(None);
                    push_state(&mut t, (o.peer, o.text, MsgState::Failed));
                    t.changed = true;
                }
            }
        }
        for c in self.out.iter().filter(|_| all) {
            let Some(mut o) = c.get().filter(|o| o.tries < TRIES && now_s >= o.next_s && o.from == me_call) else { continue };
            if o.tries > 0 && !auto_ok {
                continue;
            }
            let id = num(o.id);
            let Some(n) = uaprs::build::message(o.peer.as_bytes(), o.text.as_bytes(), Some(id.as_bytes()), &mut info) else {
                // Text APRS can't carry (`|`, `~`, `{`): failed at once rather than five silent tries.
                c.set(None);
                push_state(&mut t, (o.peer, o.text, MsgState::Failed));
                t.changed = true;
                continue;
            };
            o.tries += 1;
            o.next_s = now_s + retry_wait(o.tries) as u32;
            c.set(Some(o));
            if o.tries > 1 {
                self.last_auto.set(Some(now_s));
                push_state(&mut t, (o.peer, o.text, MsgState::Retrying(o.tries)));
            }
            t.changed = true;
            return self.framed(my, &dest, &info[..n], "Message", cfg, out, t);
        }
        // APRS-33: a query's answer, an automatic transmission (APRS-9's spacing).
        if all && auto_ok {
            if let Some(q) = self.query.take() {
                let n = match q.answer {
                    Answer::Position => fresh(fix, cfg).and_then(|f| position_info(&f, cfg, self.readings.get(), &mut info, &mut dest)),
                    Answer::Status => status_info(&mut info),
                    Answer::Text(b) => uaprs::build::message(q.to.as_bytes(), b.as_bytes(), None, &mut info),
                };
                if let Some(n) = n {
                    self.last_auto.set(Some(now_s));
                    return self.framed(my, &dest, &info[..n], "Query reply", cfg, out, t);
                }
            }
            if let Some(g) = self.general.get().filter(|g| now_s >= g.due_s) {
                self.general.set(None);
                let f = fresh(fix, cfg).filter(|f| g.foot.is_none_or(|(c, r)| within(c, (f.pos.lat / 10, f.pos.lon / 10), r)));
                if let Some(n) = f.and_then(|f| position_info(&f, cfg, self.readings.get(), &mut info, &mut dest)) {
                    self.last_auto.set(Some(now_s));
                    self.last_general.set(Some(now_s));
                    return self.framed(my, &dest, &info[..n], "Query reply", cfg, out, t);
                }
            }
        }
        let beacon = beacon || self.after_voice.get() && auto_ok;
        if beacon && (auto_ok || self.beacon_now.get()) {
            // GPS-33: never a fix older than the beacon interval as current (the fixed position stands in as
            // itself, GPS-3). An automatic beacon waits for a fresh fix, then is skipped (the caller logs it); one
            // asked for now is refused at once.
            let Some(f) = fresh(fix, cfg) else {
                if !self.beacon_now.get() {
                    let since = *self.fix_wait.get().get_or_insert(now_s);
                    self.fix_wait.set(Some(since));
                    if now_s.wrapping_sub(since) < FIX_WAIT_S {
                        return Tick { what: "Beacon: waiting for a fix", ..t };
                    }
                    self.beacon_last.set(Some(LastBeacon { at_s: now_s, ..self.beacon_last.get().unwrap_or_default() }));
                }
                self.fix_wait.set(None);
                self.beacon_now.set(false);
                self.after_voice.set(false);
                return Tick { what: if fix.is_some() { "Beacon: old fix" } else { "Beacon: no position" }, ..t };
            };
            self.after_voice.set(false);
            self.fix_wait.set(None);
            // APRS-50.1: Beacon now always carries the text; an automatic beacon the first and then every 2nd, 4th
            // or 8th (0 5 6 5)
            let text = self.beacon_now.get() || self.beacons.get().is_multiple_of(1 << cfg.text_every.min(3));
            if !self.beacon_now.get() {
                self.beacons.set(self.beacons.get().wrapping_add(1));
            }
            self.beacon_now.set(false);
            self.beacon_last.set(Some(LastBeacon::new(now_s, Some(f))));
            self.last_auto.set(Some(now_s));
            if let Some(n) = position_text(&f, cfg, text, self.readings.get(), &mut info, &mut dest) {
                t.changed = true;
                return self.framed(my, &dest, &info[..n], "Beacon", cfg, out, t);
            }
        }
        t
    }

    /// Our frame with 0 5 9 1's path (APRS-26).
    fn framed(&self, me: Address, dest: &[u8; 6], info: &[u8], what: &'static str, cfg: &AprsCfg, out: &mut [u8; FRAME_MAX], t: Tick) -> Tick {
        let names = cfg.path();
        let mut path = [me; 2];
        for (a, n) in path.iter_mut().zip(names.iter()) {
            *a = Address::from_text(n).unwrap_or(me);
        }
        let dest = Address::from_text(dest).unwrap_or(me);
        let len = Frame::new(me, dest, &path[..names.len().min(2)], info).and_then(|f| f.write(out)).unwrap_or(0);
        self.echo.set(self.echo.get() || len > 0);
        if what == "Beacon" && len > 0 {
            self.sent_key.set(info.iter().fold(0xFFFF, |h, &b| crc16(h, b))); // APRS-53.1
        }
        Tick { len, what, ..t }
    }
}

/// APRS-54: what our position reports' values come from ([`AprsLive::readings`]).
type Readings = (u16, Option<i8>, Freq10);

/// APRS-54.4: the beacon power that really keys on the APRS frequency `f` (TX-9: 1.25 m keys Low whatever is
/// chosen, `band_power`), in tenths of a watt.
pub fn beacon_watts10(f: Freq10, power: u8) -> u8 {
    let level = ferrite_core::txcal::band_power(f, power).min(2) as usize;
    if f < ferrite_core::freq::mhz(200) { WATTS10_VHF[level] } else { WATTS10_NOMINAL[level] }
}

/// APRS-54.2: the values our position reports carry with 0 5 6 4 on, in order, space-separated (`7.8V 31C 9W`):
/// the battery to 0.1 V (none before its first reading), the MCU in whole degrees C (none before its first
/// reading, SBAR-20), the beacon power in watts. With where each value ends, so whole values can be dropped
/// from the end.
fn comment_values(r: Readings, power: u8) -> (Buf<24>, [usize; 3], usize) {
    let (mv, mcu_c, f) = r;
    let mut v = Buf::<24>::new();
    let mut ends = [0usize; 3];
    let mut k = 0;
    let mut next = |v: &mut Buf<24>| {
        ends[k] = v.len();
        k += 1;
    };
    if mv > 0 {
        let d = (mv as u32 + 50) / 100;
        let _ = uwrite!(v, "{}.{}V", d / 10, d % 10);
        next(&mut v);
    }
    if let Some(c) = mcu_c {
        let _ = uwrite!(v, "{}{}C", if v.is_empty() { "" } else { " " }, c);
        next(&mut v);
    }
    let w = beacon_watts10(f, power);
    let sp = if v.is_empty() { "" } else { " " };
    let _ = if w.is_multiple_of(10) { uwrite!(v, "{}{}W", sp, w / 10) } else { uwrite!(v, "{}{}.{}W", sp, w / 10, w % 10) };
    next(&mut v);
    (v, ends, k)
}

/// GPS-33: the fix a beacon (or a query's answer, APRS-33) may send: never one older than the beacon interval as
/// current; the fixed position stands in as itself (GPS-3).
fn fresh(fix: Option<Fix>, cfg: &AprsCfg) -> Option<Fix> {
    let max_s = cfg.every_min().map_or(u32::MAX, |m| m as u32 * 60);
    fix.filter(|f| f.age_s == crate::gpsdata::FIXED_AGE || f.age_s as u32 <= max_s)
}

/// Our position report: the beacon's, and a position query's answer (APRS-33), with 0 5 5 3's comment and
/// 0 5 5 6's ambiguity; with 0 5 5 7 on, a Mic-E report (APRS-52), its latitude in `dest` in place of the tocall.
/// With 0 5 6 4 on, the values (APRS-54) follow the comment, a space between: in the uncompressed form only as
/// many whole values as keep the report within [`POSITION_TAIL_MAX`] after its symbol (dropped from the end, the
/// comment itself never cut); Mic-E's comment has no limit but the frame's, so there all of them.
fn position_info(f: &Fix, cfg: &AprsCfg, rd: Readings, info: &mut [u8], dest: &mut [u8; 6]) -> Option<usize> {
    position_text(f, cfg, true, rd, info, dest)
}

/// [`position_info`], with 0 5 5 3's text only when `text` (APRS-50.1: a beacon between the ones that carry it);
/// the telemetry values go on every one.
fn position_text(f: &Fix, cfg: &AprsCfg, text: bool, rd: Readings, info: &mut [u8], dest: &mut [u8; 6]) -> Option<usize> {
    let (vals, ends, mut k) = if cfg.telemetry { comment_values(rd, cfg.power) } else { (Buf::new(), [0; 3], 0) };
    loop {
        let mut c = Buf::<{ crate::aprs::COMMENT_LEN + 25 }>::from(if text { cfg.comment_text() } else { b"" });
        if k > 0 {
            if !c.is_empty() {
                c.push(b" ");
            }
            c.push(&vals.as_bytes()[..ends[k - 1]]);
        }
        let n = position_with(f, cfg, c.as_bytes(), info, dest)?;
        if k == 0 || cfg.mic_e != 0 || n <= POSITION_HEAD + POSITION_TAIL_MAX {
            return Some(n);
        }
        k -= 1;
    }
}

/// [`position_info`] with the comment given.
fn position_with(f: &Fix, cfg: &AprsCfg, comment: &[u8], info: &mut [u8], dest: &mut [u8; 6]) -> Option<usize> {
    let (table, code) = cfg.symbol_chars(); // APRS-47
    let r = uaprs::build::Report {
        lat: f.pos.lat / 10,
        lon: f.pos.lon / 10,
        table,
        code,
        course_speed: (f.speed_kmh > 0).then(|| (f.heading.clamp(1, 360), (f.speed_kmh as u32 * 1000 / 1852) as u16)),
        altitude: f.three_d.then_some(f.alt_m * 10_000 / 3048),
        messaging: true,
        timestamp: None,
        comment, // APRS-50, APRS-54
        ambiguity: cfg.ambiguity,    // APRS-51: the uncompressed form carries it
    };
    if cfg.mic_e == 0 {
        return uaprs::build::position(&r, info);
    }
    let mut d = [0u8; 6];
    let n = uaprs::build::mic_e(&r, 8 - cfg.mic_e, &mut d, info)?;
    *dest = d;
    Some(n)
}

/// APRS-33: our status report, `>Ferrite v0.8.0` (no status text is set on the radio).
fn status_info(info: &mut [u8]) -> Option<usize> {
    let v = ferrite_core::VERSION.as_bytes();
    let n = 9 + v.len();
    let o = info.get_mut(..n)?;
    o[..9].copy_from_slice(b">Ferrite ");
    o[9..].copy_from_slice(v);
    Some(n)
}

/// APRS-33's trace answer (`?APRST`, `?PING?`): the query's path as heard, `FROM>TO,DIGI1,DIGI2*` (the last
/// digipeater that repeated it marked, as TNC2 text does).
fn trace(f: &Frame, b: &mut Buf<67>) {
    let put = |b: &mut Buf<67>, a: &Address| {
        a.emit(&mut |c| {
            b.push(&[c]);
        })
    };
    put(b, &f.src);
    b.push(b">");
    put(b, &f.dest);
    let last = f.digis().iter().rposition(|d| d.repeated);
    for (i, d) in f.digis().iter().enumerate() {
        b.push(b",");
        put(b, d);
        if Some(i) == last {
            b.push(b"*");
        }
    }
}

/// APRS-35: a general query's footprint, `lat,lon,radius` (decimal degrees, miles; e.g. `34.02,-117.15,0200`):
/// the centre in millionths of a degree and the radius in metres.
fn footprint(t: &[u8]) -> Option<((i32, i32), u32)> {
    let mut it = t.split(|&c| c == b',');
    let (lat, lon, r) = (degrees(trim(it.next()?))?, degrees(trim(it.next()?))?, trim(it.next()?));
    if it.next().is_some() || r.is_empty() || r.len() > 5 || !r.iter().all(u8::is_ascii_digit) || lat.abs() > 90_000_000 || lon.abs() > 180_000_000 {
        return None;
    }
    let miles = r.iter().fold(0u32, |v, &c| v * 10 + (c - b'0') as u32);
    Some(((lat, lon), miles * 1609))
}

/// Whether `p` is within `r_m` metres of `c` (millionths of a degree): flat earth around `c`, its latitude's
/// cosine by Bhaskara's approximation (within 0.2 %), ample for a footprint of miles.
fn within(c: (i32, i32), p: (i32, i32), r_m: u32) -> bool {
    let x = (c.0 / 1_000_000) as i64; // whole degrees
    let cos_q16 = (32_400 - 4 * x * x) * 65_536 / (32_400 + x * x);
    let mut dlon = (p.1 as i64 - c.1 as i64) % 360_000_000;
    if dlon > 180_000_000 {
        dlon -= 360_000_000;
    } else if dlon < -180_000_000 {
        dlon += 360_000_000;
    }
    // 0.111195 m per millionth of a degree of latitude.
    let ny = (p.0 as i64 - c.0 as i64) * 111_195 / 1_000_000;
    let nx = dlon * 111_195 / 1_000_000 * cos_q16 / 65_536;
    ny * ny + nx * nx <= (r_m as i64) * (r_m as i64)
}

/// Decimal degrees (`-117.15`) in millionths.
fn degrees(t: &[u8]) -> Option<i32> {
    let (neg, t) = match t.split_first() {
        Some((b'-', r)) => (true, r),
        _ => (false, t),
    };
    let (whole, frac) = match t.iter().position(|&c| c == b'.') {
        Some(i) => (&t[..i], &t[i + 1..]),
        None => (t, &b""[..]),
    };
    if whole.is_empty() || whole.len() > 3 || frac.len() > 6 || !whole.iter().chain(frac).all(u8::is_ascii_digit) {
        return None;
    }
    let mut v = whole.iter().fold(0i32, |v, &c| v * 10 + (c - b'0') as i32);
    for i in 0..6 {
        v = v * 10 + frac.get(i).map_or(0, |&c| (c - b'0') as i32);
    }
    Some(if neg { -v } else { v })
}

/// Spaces off both ends.
fn trim(mut t: &[u8]) -> &[u8] {
    while let Some((b' ', r)) = t.split_first() {
        t = r;
    }
    while let Some((b' ', r)) = t.split_last() {
        t = r;
    }
    t
}

fn push_state(t: &mut Tick, s: (Buf<10>, Buf<67>, MsgState)) {
    if let Some(x) = t.states.iter_mut().find(|x| x.is_none()) {
        *x = Some(s);
    }
}

/// A message number as its digits.
fn num(id: u8) -> Buf<5> {
    let mut b = Buf::<5>::new();
    if id >= 10 {
        b.push(&[b'0' + id / 10]);
    }
    b.push(&[b'0' + id % 10]);
    b
}

/// The heard list's symbol names (APRS-3) from the APRS symbol table and code, as APRS 1.2's symbol tables
/// name them (datasheets: APRS_1.2c, chapter 21). `table` is `/` (primary), `\` (alternate) or an overlay's
/// character, which takes the alternate symbol.
fn sym(table: u8, code: u8) -> Sym {
    let primary = table == b'/';
    match (primary, code) {
        (_, b'>') | (true, b'j') | (_, b'v') | (false, b'k') => Sym::Car, // \k: SUV, ATV, 4x4
        (_, b'-') => Sym::Home,                                           // \-: house (HF)
        (_, b'#') => Sym::Digi,
        (_, b'_') => Sym::Wx,
        (false, b'[') if table == b'\\' => Sym::Wx, // wall cloud; with an overlay a person
        (_, b'&') => Sym::IGate,
        (true, b'k') | (_, b'u') => Sym::Truck,
        (_, b's') | (true, b'Y') => Sym::Boat, // \Y: radios and devices
        _ => Sym::Ht,                          // \j: work zone
    }
}

/// CRC-16/CCITT, one byte.
fn crc16(mut crc: u16, b: u8) -> u16 {
    crc ^= (b as u16) << 8;
    for _ in 0..8 {
        crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
    }
    crc
}

/// APRS-56: the event log's words for a packet heard but not decoded (0 5 9 8 on): the sender's best guess as
/// the name (`?` for each character a bit error spoiled), and "Not decoded: 84 B, peak 35%, >APRS,WIDE1-1" (its
/// length with the FCS, the audio's peak then as a share of full scale, its destination and path). Only a miss
/// with an address field is logged ([`miss_logged`]).
pub fn miss_text(m: &ferrite_core::audio::uafsk::Miss, peak: u16, name: &mut Buf<20>, text: &mut Buf<64>) {
    let mut hdr = Buf::<80>::new();
    if m.addresses {
        m.header(|c| hdr.push(&[c]));
    }
    let h = hdr.as_bytes();
    let gt = h.iter().position(|&c| c == b'>').unwrap_or(h.len());
    name.push(if gt == 0 { b"?" } else { &h[..gt] });
    let _ = uwrite!(text, "Not decoded: {} B, peak {}%", m.len, peak as u32 * 100 / 32767);
    if m.fx25 {
        text.push(b", FX.25");
    }
    if gt < h.len() {
        text.push(b", ");
        text.push(&h[gt..]);
    }
}

/// APRS-56.2 (decision 366): a miss goes to the event log only with an AX.25 address field; one without (chance
/// flags in noise, far more often than a packet's lost start) is counted as noise alone.
pub fn miss_logged(m: &ferrite_core::audio::uafsk::Miss) -> bool {
    m.addresses
}

/// APRS-58: a third-party packet's own sender as a callsign for the message list and our acks: 1 to 9 capitals,
/// digits and `-` (an SSID), as APRS-IS stations and services use; anything else (`TCPIP*`, a lower-case name) none.
fn third_party_call(src: &[u8]) -> Option<Buf<10>> {
    let ok = !src.is_empty() && src.len() <= 9 && src.iter().all(|&c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'-') && src[0] != b'-';
    ok.then(|| Buf::from(src))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(src: &[u8], dest: &[u8], path: &[&[u8]], info: &[u8]) -> ([u8; FRAME_MAX], usize) {
        let p: std::vec::Vec<Address> = path.iter().map(|p| Address::from_text(p).unwrap()).collect();
        let mut b = [0u8; FRAME_MAX];
        let n = Frame::new(Address::from_text(src).unwrap(), Address::from_text(dest).unwrap(), &p, info).unwrap().write(&mut b).unwrap();
        (b, n)
    }

    /// APRS-3: the heard list's names follow APRS 1.2's symbol tables, the alternate table's own meanings too
    /// (\[ a wall cloud, \- a house, \k an SUV, \j a work zone, \Y radios); an overlay takes the alternate symbol.
    #[test]
    fn symbol_names_by_table() {
        for (t, c, want) in [
            (b'/', b'>', Sym::Car), (b'\\', b'>', Sym::Car), (b'/', b'j', Sym::Car), (b'\\', b'j', Sym::Ht),
            (b'/', b'k', Sym::Truck), (b'\\', b'k', Sym::Car), (b'/', b'-', Sym::Home), (b'\\', b'-', Sym::Home),
            (b'/', b'Y', Sym::Boat), (b'\\', b'Y', Sym::Ht), (b'/', b'[', Sym::Ht), (b'\\', b'[', Sym::Wx),
            (b'A', b'[', Sym::Ht), (b'/', b'_', Sym::Wx), (b'\\', b'_', Sym::Wx), (b'\\', b'#', Sym::Digi),
            (b'I', b'&', Sym::IGate), (b'/', b'u', Sym::Truck), (b'\\', b's', Sym::Boat),
        ] {
            assert_eq!(sym(t, c), want, "{}{}", t as char, c as char);
        }
    }

    fn fix() -> Fix {
        Fix { pos: Pos { lat: 454_252_000, lon: -756_901_000 }, three_d: false, alt_m: 0, speed_kmh: 0, heading: 0, hdop_x10: 10, used: 8, age_s: 0, utc_s: 0, phone: false }
    }

    /// APRS-4.5: the details (path, course, altitude, comment, FX.25) stay in RAM for the stations heard most
    /// recently; an older station's come from its stored row ([`AprsLive::row_of`] names the seq and fields), and
    /// one with no row yet keeps what RAM has (its list fields).
    #[test]
    fn details_in_ram_for_the_recent_only() {
        let a = AprsLive::new();
        for i in 0..DET as u32 + 2 {
            let call = std::format!("VE3A{}", (b'A' + i as u8) as char);
            let (f, n) = frame(call.as_bytes(), b"APRS", &[b"WIDE1-1*"], b"!4525.51N/07554.20W>Mobile");
            assert!(a.receive(&f[..n], i, b"VE3XYZ", &AprsCfg::DEFAULT).row, "first heard: a row due");
            a.row_stored(&Buf::from(call.as_bytes()), 100 + i);
        }
        let old = DET as u16 + 1;
        assert_eq!(a.row_of(0), None, "the newest: all in RAM");
        assert_eq!(a.row_of(old), Some((100, DET_ALL)), "the oldest: from its row");
        let s = a.station(old).unwrap();
        assert!(s.pos.is_some() && s.sym == Sym::Car && s.times == 1, "the list's fields stay");
        assert_eq!(s.comment.as_bytes(), b"", "the comment comes from the row");
        // Heard again with a status: its comment is known again, its position kept, the rest still from the row.
        let (f, n) = frame(b"VE3AA", b"APRS", &[], b">Parked");
        a.receive(&f[..n], 60, b"VE3XYZ", &AprsCfg::DEFAULT);
        assert_eq!(a.station(0).unwrap().comment.as_bytes(), b"Parked");
        assert_eq!(a.row_of(0), Some((100, DET_COURSE)), "course and altitude from its row");
    }

    /// Fuzzing (Oct 31): an altitude as large as `/A=` can carry (6 digits) converts to metres without overflow.
    #[test]
    fn huge_altitude_converts() {
        let a = AprsLive::new();
        for (alt, m) in [(&b"999999"[..], 304_799), (b"-99999", -30_479)] {
            let mut info = b"!4525.51N/07554.20W>/A=".to_vec();
            info.extend_from_slice(alt);
            let (f, n) = frame(b"VE3AA", b"APRS", &[], &info);
            a.receive(&f[..n], 1, b"VE3XYZ", &AprsCfg::DEFAULT);
            assert_eq!(a.station(0).unwrap().alt_m, Some(m));
        }
    }

    #[test]
    fn heard_list_newest_first_and_capped() {
        let a = AprsLive::new();
        let n = HEARD_MAX as u32 + 5;
        for i in 0..n {
            let call = std::format!("VE3{}{}", (b'A' + (i / 26) as u8) as char, (b'A' + (i % 26) as u8) as char);
            let (f, n) = frame(call.as_bytes(), b"APRS", &[b"WIDE1-1*"], b"!4525.51N/07554.20W>Mobile");
            assert!(a.receive(&f[..n], i, b"VE3XYZ", &AprsCfg::DEFAULT).changed);
        }
        assert_eq!(a.stations(), HEARD_MAX);
        let s = a.station(0).unwrap();
        let last = n - 1;
        assert_eq!(s.call.as_bytes(), std::format!("VE3{}{}", (b'A' + (last / 26) as u8) as char, (b'A' + (last % 26) as u8) as char).as_bytes());
        assert_eq!(s.sym, Sym::Car);
        assert!(s.pos.is_some_and(|p| (p.lat - 454_251_666).abs() < 200));
        assert_eq!(s.via.as_bytes(), b"WIDE1-1*");
        assert_eq!(s.comment.as_bytes(), b"Mobile");
        // Heard again: to the front, counted, not twice.
        let (f, n) = frame(b"VE3AM", b"APRS", &[], b">On the air");
        a.receive(&f[..n], 30, b"VE3XYZ", &AprsCfg::DEFAULT);
        assert_eq!(a.station(0).unwrap().call.as_bytes(), b"VE3AM");
        assert_eq!(a.station(0).unwrap().times, 2);
        assert!(a.station(0).unwrap().pos.is_some(), "a status keeps the last position");
        assert_eq!(a.stations(), HEARD_MAX);
    }

    /// APRS-5.2: a Mic-E frame's device codes leave the heard comment and name the radio; a later frame without a
    /// position keeps it.
    #[test]
    fn mic_e_device_heard() {
        let a = AprsLive::new();
        let (f, n) = frame(b"VE7LSE-9", b"S4QVYV", &[], b"`.4&l-/k/`146.520[1");
        a.receive(&f[..n], 0, b"VE3XYZ", &AprsCfg::DEFAULT);
        let s = a.station(0).unwrap();
        assert_eq!((s.comment.as_bytes(), s.device), (&b"146.520"[..], Some(25)));
        assert_eq!(uaprs::parse::MIC_E_DEVICES[25].1, "APRSdroid");
        let (f, n) = frame(b"VE7LSE-9", b"APRS", &[], b">QRT");
        a.receive(&f[..n], 30, b"VE3XYZ", &AprsCfg::DEFAULT);
        assert_eq!(a.station(0).unwrap().device, Some(25));
    }

    /// APRS-44: a frame that came as FX.25 marks its station with the bytes corrected (the station screen's Path
    /// row); the station's next plain frame clears it. APRS-45: our own frames take 0 5 9 5's check bytes, a
    /// digipeat none.
    #[test]
    fn fx25_heard_and_sent() {
        let a = AprsLive::new();
        let (f, n) = frame(b"VE3KPW-9", b"APRS", &[], b"!4525.51N/07554.20W>Mobile");
        a.receive(&f[..n], 1, b"VE3XYZ", &AprsCfg::DEFAULT);
        assert_eq!(a.station(0).unwrap().fx25, None);
        a.note_fx25(&f[..n], 3);
        assert_eq!(a.station(0).unwrap().fx25, Some(3));
        a.receive(&f[..n], 40, b"VE3XYZ", &AprsCfg::DEFAULT);
        assert_eq!(a.station(0).unwrap().fx25, None, "heard plain since");
        let c = AprsCfg { fx25: 32, ..AprsCfg::DEFAULT };
        assert_eq!((AprsCfg::DEFAULT.fx25, c.fx25_for("Beacon"), c.fx25_for("Message"), c.fx25_for("Digipeat")), (16, 32, 32, 0));
    }

    /// APRS-11: what we send is capped and identifies us as the protocol requires: the source address is our
    /// callsign and SSID, the destination an APRS tocall, the path the chosen one, and the information field
    /// within 256 bytes; a message's text is cut to APRS's 67 characters however long it was typed.
    #[test]
    fn sent_frames_carry_our_id_and_stay_capped() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let cfg = AprsCfg::DEFAULT;
        let long = [b'x'; 200];
        assert!(a.send(b"VE3KPW-9", &long));
        let t = a.tick(0, Sending::All, true, b"VE3XYZ-7", None, &cfg, &mut out);
        assert_eq!(t.what, "Message");
        let g = Frame::parse(&out[..t.len]).unwrap();
        assert_eq!((g.src.call(), g.src.ssid), (&b"VE3XYZ"[..], 7), "our callsign and SSID");
        assert!(g.dest.call().starts_with(b"AP"), "an APRS tocall");
        assert_eq!(g.digis().len(), 2, "the default path, WIDE1-1,WIDE2-1");
        assert!(g.info.len() <= INFO_MAX);
        // ":VE3KPW-9 :" + text + "{n": the text is APRS's 67 characters at most
        let text = &g.info[11..g.info.iter().rposition(|&c| c == b'{').unwrap()];
        assert!(!text.is_empty() && text.len() <= 67, "message text {} long", text.len());
        let a = AprsLive::new(); // a beacon alone (the message above would still be retrying)
        a.beacon_now();
        let t = a.tick(400, Sending::All, true, b"VE3XYZ-7", Some(fix()), &cfg, &mut out);
        assert_eq!(t.what, "Beacon");
        let g = Frame::parse(&out[..t.len]).unwrap();
        assert_eq!(g.src.call(), b"VE3XYZ");
        assert!(g.info.len() <= INFO_MAX);
    }

    /// APRS-36: our own frame heard back isn't a station; the first one after a transmission names the
    /// digipeater that repeated it, later ones say nothing until we send again.
    #[test]
    fn own_frames_heard_back_name_the_digipeater() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let me = b"VE3XYZ-7";
        let (f, n) = frame(me, b"APFRT1", &[b"VE3RSB-1*", b"WIDE1*", b"WIDE2-1"], b"!4525.00N/07541.00W[");
        assert!(a.receive(&f[..n], 10, me, &AprsCfg::DEFAULT).echo.is_none(), "nothing sent yet");
        a.beacon_now();
        assert_eq!(a.tick(20, Sending::All, true, me, Some(fix()), &AprsCfg::DEFAULT, &mut out).what, "Beacon");
        let rx = a.receive(&f[..n], 21, me, &AprsCfg::DEFAULT);
        assert_eq!(rx.echo.unwrap().as_bytes(), b"VE3RSB-1", "the last used entry that names a station");
        assert!(a.receive(&f[..n], 22, me, &AprsCfg::DEFAULT).echo.is_none(), "once per transmission");
        assert_eq!(a.stations(), 0, "not a station heard");
        assert_eq!(a.beacon(&AprsCfg::DEFAULT).digi.as_bytes(), b"VE3RSB-1");
    }

    /// APRS-53, APRS-53.1: a beacon that goes out is a Sent entry, newest first, its age from the APRS clock; every
    /// digipeater heard repeating it is added once, two at most, while APRS-36's toast is still the first copy's
    /// alone. Our message heard back isn't a beacon's copy; the ring keeps the newest 20.
    #[test]
    fn sent_beacons_and_who_repeated_them() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let me = b"VE3XYZ-7";
        assert_eq!(a.beacons_sent(), 0);
        a.beacon_now();
        let t = a.tick(20, Sending::All, true, me, Some(fix()), &AprsCfg::DEFAULT, &mut out);
        assert_eq!(t.what, "Beacon");
        assert_eq!(a.beacons_sent(), 0, "framed, not yet sent");
        a.beacon_sent(21);
        let info = Frame::parse(&out[..t.len]).unwrap().info.to_vec();
        let copy = |via: &[&[u8]]| frame(me, b"APZFER", via, &info);
        let (f, n) = copy(&[b"VE3RSB-1*", b"WIDE2-1"]);
        let rx = a.receive(&f[..n], 22, me, &AprsCfg::DEFAULT);
        assert!(rx.echo.is_some() && rx.changed);
        let (f, n) = copy(&[b"VE3RSB-1*", b"VE3OTW-10*"]);
        let rx = a.receive(&f[..n], 23, me, &AprsCfg::DEFAULT);
        assert!(rx.echo.is_none() && rx.changed, "a second digipeater: no toast, but kept");
        let (f, n) = copy(&[b"VE3RSB-1*", b"WIDE2*"]);
        assert!(!a.receive(&f[..n], 24, me, &AprsCfg::DEFAULT).changed, "VE3RSB-1 again: once");
        let (f, n) = copy(&[b"VA3ABC-2*"]);
        assert!(!a.receive(&f[..n], 25, me, &AprsCfg::DEFAULT).changed, "two kept at most");
        let (f, n) = frame(me, b"APZFER", &[b"VA3MSG*"], b":VE3RSB-1 :hello{1");
        assert!(!a.receive(&f[..n], 26, me, &AprsCfg::DEFAULT).changed, "a message heard back isn't the beacon's");
        let e = a.sent_beacon(0).unwrap();
        assert_eq!((e.ago_s, e.by[0].as_bytes(), e.by[1].as_bytes()), (5, &b"VE3RSB-1"[..], &b"VE3OTW-10"[..]));
        for s in 0..SENT_MAX as u32 + 3 {
            a.beacon_sent(100 + s);
        }
        assert_eq!(a.beacons_sent(), SENT_MAX as u16);
        assert_eq!(a.sent_beacon(0).unwrap().ago_s, 0, "newest first");
        assert_eq!(a.sent_beacon(SENT_MAX as u16 - 1).unwrap().ago_s, SENT_MAX as u32 - 1);
        assert!(a.sent_beacon(SENT_MAX as u16).is_none());
        assert!(a.sent_beacon(0).unwrap().by.iter().all(|b| b.is_empty()), "a new entry: not heard back");
    }

    /// APRS-25: a frame's information field is at most 256 bytes: 256 is taken whole, 257 is dropped.
    #[test]
    fn info_field_capped_at_256() {
        let a = AprsLive::new();
        for (len, kept) in [(INFO_MAX, true), (INFO_MAX + 1, false)] {
            let mut info = std::vec![b'x'; len];
            info[0] = b'>';
            let mut b = std::vec![0u8; 512];
            let src: &[u8] = if kept { b"VE3AA" } else { b"VE3BB" };
            let n = Frame::new(Address::from_text(src).unwrap(), Address::from_text(b"APRS").unwrap(), &[], &info).unwrap().write(&mut b).unwrap();
            assert_eq!(a.receive(&b[..n], 1, b"VE3XYZ", &AprsCfg::DEFAULT).changed, kept, "{len} bytes");
        }
    }

    /// APRS-29 (decision 181): beacons alone (APRS mode off) send the beacon and nothing else: a message to us isn't
    /// acked, our own messages wait, a digipeat isn't sent.
    #[test]
    fn beacons_alone_send_only_beacons() {
        let a = AprsLive::new();
        let cfg = AprsCfg { beacon: 4, ..AprsCfg::DEFAULT }; // every 5 min
        let (f, n) = frame(b"VE3KPW-9", b"APRS", &[], b":VE3XYZ   :On my way{12");
        a.receive(&f[..n], 100, b"VE3XYZ", &cfg);
        assert!(a.send(b"VE3KPW-9", b"QSL"));
        let mut out = [0u8; FRAME_MAX];
        let t = a.tick(100, Sending::Beacons, true, b"VE3XYZ", Some(fix()), &cfg, &mut out);
        assert_eq!(t.what, "Beacon", "the beacon, not the ack or the message");
        for s in 101..131 {
            assert_eq!(a.tick(s, Sending::Beacons, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).len, 0, "{s}: nothing else");
        }
        // APRS mode on later: the ack owed meanwhile was dropped; our message goes.
        assert_eq!(a.tick(131, Sending::All, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).what, "Message");
        // Nothing at all without APRS mode or beacons (APRS-22).
        let b = AprsLive::new();
        b.beacon_now();
        assert_eq!(b.tick(0, Sending::Nothing, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).len, 0);
    }

    /// APRS-10, APRS-29 (decision 181): a frame goes in the first clear moment (p-persistence on C's squelch) and
    /// never while C is busy; busy for the whole 5 s window, it isn't sent.
    #[test]
    fn frame_waits_for_a_clear_moment() {
        let mut w = ClearWait::default();
        assert_eq!(w.poll(0, false, 0), Clear::Wait, "nothing waiting");
        w.start(1_000);
        assert!(w.waiting());
        for t in (1_000..3_000).step_by(10) {
            assert_eq!(w.poll(t, true, 0), Clear::Wait, "busy at {t}");
        }
        // Clear: a try each 100 ms slot at 64/256; with these draws the third slot sends.
        assert_eq!(w.poll(3_000, false, 200), Clear::Wait);
        assert_eq!(w.poll(3_050, false, 0), Clear::Wait, "within the slot");
        assert_eq!(w.poll(3_100, false, 100), Clear::Wait);
        assert_eq!(w.poll(3_200, false, 10), Clear::Send);
        assert!(!w.waiting());
        w.start(10_000);
        for t in (10_000..15_000).step_by(10) {
            assert_eq!(w.poll(t, true, 0), Clear::Wait, "busy at {t}");
        }
        assert_eq!(w.poll(15_000, true, 0), Clear::GiveUp, "busy for 5 s: not sent");
    }

    /// APRS-57 on, everyone, 10 s, with a reply.
    fn reply_cfg(to: &[u8]) -> AprsCfg {
        let mut c = AprsCfg { reply: 1, ..AprsCfg::DEFAULT };
        c.reply_to = [0; crate::aprs::REPLY_TO_LEN];
        c.reply_to[..to.len()].copy_from_slice(to);
        c.reply_text[..14].copy_from_slice(b"Away, back 5pm");
        c
    }

    /// APRS-57, APRS-57.2: a message to us gets the auto reply after its delay (10 s), once; the same station again
    /// within 30 minutes gets none (two radios replying to each other stop there), after them it does again.
    #[test]
    fn auto_reply_after_its_delay_once_per_station() {
        let a = AprsLive::new();
        let cfg = reply_cfg(b"*");
        let (f, n) = frame(b"VE3KPW-9", b"APRS", &[], b":VE3XYZ-7 :On my way{12");
        assert!(a.receive(&f[..n], 100, b"VE3XYZ-7", &cfg).inbound.is_some());
        assert_eq!(a.reply_now(109), None, "not before its delay");
        assert_eq!(a.reply_now(110).unwrap().as_bytes(), b"VE3KPW-9");
        assert_eq!(a.reply_now(111), None, "once");
        let (g, m) = frame(b"VE3KPW-9", b"APRS", &[], b":VE3XYZ-7 :Thanks{13");
        assert!(a.receive(&g[..m], 200, b"VE3XYZ-7", &cfg).inbound.is_some());
        assert_eq!(a.reply_now(300), None, "the same station within 30 minutes: none");
        let (h, k) = frame(b"VE3KPW-9", b"APRS", &[], b":VE3XYZ-7 :Hello again{14");
        assert!(a.receive(&h[..k], 110 + REPLY_HOLD_S, b"VE3XYZ-7", &cfg).inbound.is_some());
        assert!(a.reply_now(120 + REPLY_HOLD_S).is_some(), "after 30 minutes, again");
    }

    /// APRS-57: no reply when it is off or has no text, to a station the pattern doesn't take, to ourselves on
    /// another SSID, to a service (APRS-39), or to an ack; the delay follows the setting (bits 4-5: 2 min).
    #[test]
    fn auto_reply_only_where_it_should() {
        let to_us = |a: &AprsLive, from: &[u8], at: u32, cfg: &AprsCfg| {
            let (f, n) = frame(from, b"APRS", &[], b":VE3XYZ-7 :Ping{77");
            a.receive(&f[..n], at, b"VE3XYZ-7", cfg);
        };
        let a = AprsLive::new();
        to_us(&a, b"VE3KPW-9", 100, &AprsCfg::DEFAULT);
        assert_eq!(a.reply_now(1000), None, "off");
        let mut empty = reply_cfg(b"*");
        empty.reply_text = [0; crate::aprs::REPLY_TEXT_LEN];
        to_us(&a, b"VE3KPW-9", 100, &empty);
        assert_eq!(a.reply_now(1000), None, "no text");
        let a = AprsLive::new();
        let ve7 = reply_cfg(b"VE7*");
        to_us(&a, b"VE3KPW-9", 100, &ve7);
        assert_eq!(a.reply_now(1000), None, "VE7* doesn't take VE3KPW-9");
        to_us(&a, b"VE7ABC-9", 2000, &ve7);
        assert!(a.reply_now(2010).is_some(), "VE7* takes VE7ABC-9");
        let a = AprsLive::new();
        to_us(&a, b"VE3XYZ-9", 100, &reply_cfg(b"*"));
        assert_eq!(a.reply_now(1000), None, "ourselves on another SSID");
        to_us(&a, b"WXBOT", 100, &reply_cfg(b"*"));
        assert_eq!(a.reply_now(1000), None, "a service");
        let a = AprsLive::new();
        let (f, n) = frame(b"VE3KPW-9", b"APRS", &[], b":VE3XYZ-7 :ack77");
        a.receive(&f[..n], 100, b"VE3XYZ-7", &reply_cfg(b"*"));
        assert_eq!(a.reply_now(1000), None, "an ack");
        let a = AprsLive::new();
        let slow = AprsCfg { reply: 0x31, ..reply_cfg(b"*") };
        to_us(&a, b"VE3KPW-9", 100, &slow);
        assert_eq!((a.reply_now(219), a.reply_now(220).map(|c| c.len())), (None, Some(8)), "2 min");
    }

    /// APRS-57.1: the pattern: `*` (or nothing) everyone, `X*` every callsign starting X, else one exactly (SSID 0
    /// written or not), whatever the case.
    #[test]
    fn auto_reply_patterns() {
        use crate::aprs::reply_pattern as m;
        assert!(m(b"*", b"VE3KPW-9") && m(b"", b"VE3KPW-9"));
        assert!(m(b"VE3*", b"VE3KPW-9") && !m(b"VE3*", b"VA3KPW"));
        assert!(m(b"VE3KPW*", b"VE3KPW") && m(b"VE3KPW*", b"VE3KPW-15"));
        assert!(m(b"VE3KPW-9", b"VE3KPW-9") && !m(b"VE3KPW-9", b"VE3KPW-7") && !m(b"VE3KPW", b"VE3KPW-9"));
        assert!(m(b"VE3KPW", b"VE3KPW-0") && m(b"VE3KPW-0", b"VE3KPW") && m(b"ve3kpw", b"VE3KPW"));
    }

    #[test]
    fn message_to_us_is_acked_once() {
        let a = AprsLive::new();
        let (f, n) = frame(b"VE3KPW-9", b"APRS", &[], b":VE3XYZ   :On my way{12");
        let rx = a.receive(&f[..n], 100, b"VE3XYZ", &AprsCfg::DEFAULT);
        assert_eq!(rx.inbound.unwrap().1.as_bytes(), b"On my way");
        let mut out = [0u8; FRAME_MAX];
        let t = a.tick(100, Sending::All, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out);
        assert_eq!(t.what, "Ack");
        let g = Frame::parse(&out[..t.len]).unwrap();
        assert_eq!(g.info, b":VE3KPW-9 :ack12");
        assert_eq!(g.digis().len(), 2);
        // The same message again (its sender didn't hear our ack): acked again, no second inbox entry.
        let rx = a.receive(&f[..n], 130, b"VE3XYZ", &AprsCfg::DEFAULT);
        assert!(rx.inbound.is_none());
        assert_eq!(a.tick(131, Sending::All, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out).what, "Ack");
        // Not to us: nothing.
        let (f, n) = frame(b"VE3KPW-9", b"APRS", &[], b":VA3ABC   :Hi{3");
        assert!(a.receive(&f[..n], 140, b"VE3XYZ", &AprsCfg::DEFAULT).inbound.is_none());
    }

    /// APRS-58: third-party traffic (an IGate relaying from the internet): a message inside to us is taken as from
    /// its own sender (inbox, our ack to it by name), and an ack inside acks ours, so a message to SMSGTE shows acked
    /// when delivered (a review of Nov 20, 2026: these were dropped). Not to us, or an odd sender: nothing.
    #[test]
    fn third_party_messages_and_acks_are_read() {
        let a = AprsLive::new();
        let cfg = AprsCfg::DEFAULT;
        let mut out = [0u8; FRAME_MAX];
        // a message from an internet station, gated by VE3IGT
        let (f, n) = frame(b"VE3IGT", b"APRS", &[b"WIDE2-1"], b"}VA7NET>APRS,TCPIP,VE3IGT*::VE3XYZ   :Hello from the net{7");
        let rx = a.receive(&f[..n], 100, b"VE3XYZ", &cfg);
        let (from, text) = rx.inbound.expect("in the inbox");
        assert_eq!((from.as_bytes(), text.as_bytes()), (&b"VA7NET"[..], &b"Hello from the net"[..]));
        let t = a.tick(100, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out);
        assert_eq!(t.what, "Ack");
        assert_eq!(Frame::parse(&out[..t.len]).unwrap().info, b":VA7NET   :ack7", "acked to its own sender");
        // ours to SMSGTE, acked from the internet through the IGate
        assert!(a.send(b"SMSGTE", b"@5551234567 on my way"));
        let t = a.tick(200, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out);
        assert_eq!(t.what, "Message");
        let (f, n) = frame(b"VE3IGT", b"APRS", &[], b"}SMSGTE>APRS,TCPIP,VE3IGT*::VE3XYZ   :ack1");
        let rx = a.receive(&f[..n], 210, b"VE3XYZ", &cfg);
        assert!(matches!(rx.state, Some((_, _, MsgState::Acked))), "acked, not tried on to failed");
        // not to us, or a sender that isn't a callsign: nothing
        let (f, n) = frame(b"VE3IGT", b"APRS", &[], b"}VA7NET>APRS,TCPIP,VE3IGT*::VA3ABC   :Hi{3");
        assert!(a.receive(&f[..n], 300, b"VE3XYZ", &cfg).inbound.is_none());
        let (f, n) = frame(b"VE3IGT", b"APRS", &[], b"}tcpip>APRS,TCPIP*::VE3XYZ   :Hi{4");
        assert!(a.receive(&f[..n], 310, b"VE3XYZ", &cfg).inbound.is_none());
    }

    /// APRS-58: the same message gated by two IGates: one inbox entry, acked for each copy.
    #[test]
    fn a_third_party_message_via_two_igates_is_kept_once() {
        let a = AprsLive::new();
        let cfg = AprsCfg::DEFAULT;
        let (f, n) = frame(b"VE3IGT", b"APRS", &[], b"}VA7NET>APRS,TCPIP,VE3IGT*::VE3XYZ   :Once{9");
        assert!(a.receive(&f[..n], 100, b"VE3XYZ", &cfg).inbound.is_some());
        let (f, n) = frame(b"VE3IG2", b"APRS", &[], b"}VA7NET>APRS,TCPIP,VE3IG2*::VE3XYZ   :Once{9");
        assert!(a.receive(&f[..n], 105, b"VE3XYZ", &cfg).inbound.is_none(), "stored once");
    }

    /// APRS-58: a sender with an SSID is acked by its full callsign.
    #[test]
    fn a_third_party_sender_keeps_its_ssid() {
        let a = AprsLive::new();
        let cfg = AprsCfg::DEFAULT;
        let mut out = [0u8; FRAME_MAX];
        let (f, n) = frame(b"VE3IGT", b"APRS", &[], b"}VA7NET-9>APRS,TCPIP,VE3IGT*::VE3XYZ   :Hi{5");
        let rx = a.receive(&f[..n], 100, b"VE3XYZ", &cfg);
        assert_eq!(rx.inbound.unwrap().0.as_bytes(), b"VA7NET-9");
        let t = a.tick(100, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out);
        assert_eq!(Frame::parse(&out[..t.len]).unwrap().info, b":VA7NET-9 :ack5");
    }

    /// APRS-58: a reply-ack inside a third-party message acks ours too (APRS 1.1).
    #[test]
    fn a_third_party_reply_ack_acks_ours() {
        let a = AprsLive::new();
        let cfg = AprsCfg::DEFAULT;
        let mut out = [0u8; FRAME_MAX];
        assert!(a.send(b"VA7NET", b"Ping"));
        assert_eq!(a.tick(0, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out).what, "Message");
        let (f, n) = frame(b"VE3IGT", b"APRS", &[], b"}VA7NET>APRS,TCPIP,VE3IGT*::VE3XYZ   :Pong{2}1");
        let rx = a.receive(&f[..n], 10, b"VE3XYZ", &cfg);
        assert!(matches!(rx.state, Some((_, _, MsgState::Acked))));
        assert!(rx.inbound.is_some(), "and the reply is in the inbox");
    }

    /// APRS-58: a third-party packet with a broken inner header is only heard (no panic, no message).
    #[test]
    fn a_broken_third_party_packet_is_only_heard() {
        let a = AprsLive::new();
        let cfg = AprsCfg::DEFAULT;
        for info in [&b"}"[..], b"}VA7NET", b"}>APRS::VE3XYZ   :x{1", b"}VA7NET>:", b"}VA7NET>APRS:"] {
            let (f, n) = frame(b"VE3IGT", b"APRS", &[], info);
            let rx = a.receive(&f[..n], 100, b"VE3XYZ", &cfg);
            assert!(rx.inbound.is_none());
            assert!(rx.changed, "the IGate is heard");
        }
    }

    #[test]
    fn ours_tried_until_acked() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let cfg = AprsCfg::DEFAULT;
        assert!(a.send(b"VE3KPW-9", b"QSL"));
        let t = a.tick(0, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out);
        assert_eq!(t.what, "Message");
        assert_eq!(Frame::parse(&out[..t.len]).unwrap().info, b":VE3KPW-9 :QSL{1");
        assert_eq!(a.tick(10, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out).len, 0, "next try 30 s on");
        let t = a.tick(30, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out);
        assert_eq!(t.what, "Message");
        assert!(matches!(t.states[0], Some((_, _, MsgState::Retrying(2)))));
        let (f, n) = frame(b"VE3KPW-9", b"APRS", &[], b":VE3XYZ   :ack1");
        let rx = a.receive(&f[..n], 40, b"VE3XYZ", &AprsCfg::DEFAULT);
        assert!(matches!(rx.state, Some((_, _, MsgState::Acked))));
        assert_eq!(a.tick(200, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out).len, 0, "acked: no more tries");
        // Never acked: five tries, then failed.
        a.send(b"VA3ABC", b"Anyone?");
        let mut tries = 0;
        let mut failed = false;
        for s in 300..1_200 {
            let t = a.tick(s, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out);
            tries += (t.what == "Message") as u32;
            failed |= t.states.iter().flatten().any(|s| s.2 == MsgState::Failed);
        }
        assert_eq!(tries, 5);
        assert!(failed);
    }

    #[test]
    fn beacons_wait_for_the_relay_and_need_a_position() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let cfg = AprsCfg::DEFAULT;
        a.beacon_now();
        let t = a.tick(0, Sending::All, false, b"VE3XYZ", Some(fix()), &cfg, &mut out);
        assert_eq!(t.len, 0);
        assert!(a.beacon(&cfg).waiting, "APRS-20: waits for the relay");
        let t = a.tick(1, Sending::All, true, b"VE3XYZ", Some(fix()), &cfg, &mut out);
        assert_eq!(t.what, "Beacon");
        let g = Frame::parse(&out[..t.len]).unwrap();
        assert_eq!(g.dest.call(), b"APZFER");
        match parse::parse(b"APZFER", g.info) {
            Ok(Packet::Position { pos, .. }) => assert!((pos.lat - 45_425_200).abs() < 200 && (pos.lon + 75_690_100).abs() < 200),
            other => panic!("{other:?}"),
        }
        a.beacon_now();
        assert_eq!(a.tick(2, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out).what, "Beacon: no position");
        a.beacon_now();
        assert_eq!(a.tick(3, Sending::All, true, b"", Some(fix()), &cfg, &mut out).len, 0, "APRS-8: no callsign, nothing sent");
    }

    /// GPS-33: a timed beacon with a fix older than its interval waits up to 2 minutes for a fresh one and goes on
    /// it; with none it is skipped (once, for the caller to log) and the next comes by the schedule. Beacon now
    /// with an old fix is refused at once.
    #[test]
    fn beacon_waits_for_a_fresh_fix() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let cfg = AprsCfg { beacon: 2, ..AprsCfg::DEFAULT }; // every minute
        let old = Some(Fix { age_s: 90, ..fix() });
        assert_eq!(a.tick(100, Sending::All, true, b"VE3XYZ", old, &cfg, &mut out).what, "Beacon: waiting for a fix");
        assert_eq!(a.tick(150, Sending::All, true, b"VE3XYZ", old, &cfg, &mut out).what, "Beacon: waiting for a fix");
        assert_eq!(a.tick(151, Sending::All, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).what, "Beacon", "a fresh fix: sent");
        // The next: the module never finds the sky again.
        let mut skipped = 0;
        let mut sent = 0;
        for s in 211..340 {
            let w = a.tick(s, Sending::All, true, b"VE3XYZ", old, &cfg, &mut out).what;
            skipped += (w == "Beacon: old fix") as u32;
            sent += (w == "Beacon") as u32;
        }
        assert_eq!((skipped, sent), (1, 0), "skipped once after 2 minutes, then the schedule's next");
        assert_eq!(a.tick(400, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out).what, "Beacon: waiting for a fix", "no position: waits too");
        // The fixed position stands in as itself.
        let fixed = Some(Fix { age_s: crate::gpsdata::FIXED_AGE, ..fix() });
        assert_eq!(a.tick(401, Sending::All, true, b"VE3XYZ", fixed, &cfg, &mut out).what, "Beacon");
        a.beacon_now();
        assert_eq!(a.tick(440, Sending::All, true, b"VE3XYZ", old, &cfg, &mut out).what, "Beacon: old fix", "beacon now: at once");
    }

    const DIGI: AprsCfg = AprsCfg { digi: crate::aprs::DIGI_ON, ..AprsCfg::DEFAULT };

    #[test]
    fn digipeater_fills_in_once() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        a.tick(9, Sending::All, true, b"VE3XYZ", None, &DIGI, &mut out); // APRS mode on (APRS-10.1)
        let (f, n) = frame(b"VE3OCB-7", b"APRS", &[b"WIDE1-1", b"WIDE2-1"], b">Walking");
        a.receive(&f[..n], 10, b"VE3XYZ", &DIGI);
        let t = a.tick(10, Sending::All, true, b"VE3XYZ", None, &DIGI, &mut out);
        assert_eq!(t.what, "Digipeat");
        let g = Frame::parse(&out[..t.len]).unwrap();
        assert!(g.digis()[0].repeated && g.digis()[0].call() == b"VE3XYZ");
        assert!(!g.digis()[1].repeated);
        // The same frame again within 30 s, or one already past WIDE1: not repeated.
        a.receive(&f[..n], 20, b"VE3XYZ", &DIGI);
        assert_eq!(a.tick(20, Sending::All, true, b"VE3XYZ", None, &DIGI, &mut out).len, 0);
        let (f, n) = frame(b"VE3OCB-7", b"APRS", &[b"WIDE1-1*", b"WIDE2-1"], b">Later");
        a.receive(&f[..n], 50, b"VE3XYZ", &DIGI);
        assert_eq!(a.tick(50, Sending::All, true, b"VE3XYZ", None, &DIGI, &mut out).len, 0);
        // Off by default (APRS-12).
        let (f, n) = frame(b"VE3OCB-7", b"APRS", &[b"WIDE1-1", b"WIDE2-1"], b">Again");
        a.receive(&f[..n], 90, b"VE3XYZ", &AprsCfg::DEFAULT);
        assert_eq!(a.tick(90, Sending::All, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out).len, 0);
    }

    /// APRS-10.1: with nothing allowed to send, everything waiting or asked for is dropped (a digipeat, Beacon
    /// now, the beacon after voice; acks as before), and a frame heard meanwhile isn't queued to digipeat, so
    /// turning APRS mode on later sends nothing stale. Our messages wait for it (LINK-45). A frame let go before
    /// its turn is checked again ([`Sending::may_send`]).
    #[test]
    fn nothing_allowed_drops_what_waits() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let me = b"VE3XYZ";
        a.tick(9, Sending::All, true, me, None, &DIGI, &mut out);
        let (f, n) = frame(b"VE3OCB-7", b"APRS", &[b"WIDE1-1", b"WIDE2-1"], b">Walking");
        let cfg = AprsCfg { digi: crate::aprs::DIGI_ON | 5 << 4, ..AprsCfg::DEFAULT }; // a 5 s wait
        a.receive(&f[..n], 10, me, &cfg);
        a.beacon_now();
        a.after_voice();
        assert!(a.send(b"VE3KPW", b"QSL"));
        assert_eq!(a.tick(11, Sending::Nothing, true, me, Some(fix()), &cfg, &mut out).len, 0);
        // Heard while nothing may go: not queued.
        let (g, m) = frame(b"VE3OCB-7", b"APRS", &[b"WIDE1-1", b"WIDE2-1"], b">Later");
        a.receive(&g[..m], 12, me, &cfg);
        // APRS mode on again: our message goes, nothing else.
        let mut sent = std::vec::Vec::new();
        for s in 20..45 {
            let t = a.tick(s, Sending::All, true, me, Some(fix()), &AprsCfg { beacon: 1, ..cfg }, &mut out);
            if t.len > 0 {
                sent.push(t.what);
            }
        }
        assert_eq!(sent, ["Message"], "no digipeat, Beacon now or beacon after voice");
        assert!(Sending::All.may_send("Digipeat") && Sending::Beacons.may_send("Beacon"));
        assert!(!Sending::Beacons.may_send("Message") && !Sending::Nothing.may_send("Beacon"));
    }

    /// APRS-10.1: the digipeater turned off (0 5 7 1) while a frame waits its turn (0 5 7 3): it doesn't go.
    #[test]
    fn digipeat_waiting_goes_when_turned_off() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let cfg = AprsCfg { digi: crate::aprs::DIGI_ON | 3 << 4, ..AprsCfg::DEFAULT };
        a.tick(9, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out);
        let (f, n) = frame(b"VE3OCB-7", b"APRS", &[b"WIDE1-1"], b">Walking");
        a.receive(&f[..n], 10, b"VE3XYZ", &cfg);
        assert_eq!(a.tick(11, Sending::All, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out).len, 0);
        assert_eq!(a.tick(14, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out).len, 0, "dropped, not delayed");
    }

    /// APRS-13.1, APRS-13.3, APRS-46: four of ours are tried at once (a fifth is refused, never left pending);
    /// one stopped or deleted is tried no more.
    #[test]
    fn messages_stop_and_room() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        for t in [&b"ONE"[..], b"TWO", b"THREE", b"FOUR"] {
            assert!(a.room());
            assert!(a.send(b"VE3KPW", t));
        }
        assert!(!a.room() && !a.send(b"VE3KPW", b"FIVE"), "a fifth refused");
        assert!(a.unqueue(b"VE3KPW", b"ONE") && !a.unqueue(b"VE3KPW", b"ONE"));
        assert!(a.room());
        let mut texts = std::vec::Vec::new();
        for s in 0..4 {
            let t = a.tick(s, Sending::All, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out);
            if t.what == "Message" {
                let info = sent(&out, &t);
                let body = &info[11..];
                texts.push(std::string::String::from_utf8_lossy(&body[..body.iter().position(|&c| c == b'{').unwrap()]).into_owned());
            }
        }
        assert_eq!(texts, ["TWO", "THREE", "FOUR"], "the stopped one isn't sent");
        a.clear_out();
        assert_eq!(a.tick(40, Sending::All, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out).len, 0, "deleted: no retries");
    }

    /// APRS-13.4: a message is sent as one callsign; changed mid-retry (its ack would come to the old one), it
    /// ends as failed rather than being retried as another station.
    #[test]
    fn callsign_change_ends_open_messages() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        assert!(a.send(b"VE3KPW", b"QSL"));
        assert_eq!(a.tick(0, Sending::All, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out).what, "Message");
        let t = a.tick(31, Sending::All, true, b"VE3XYZ-7", None, &AprsCfg::DEFAULT, &mut out);
        assert_eq!(t.len, 0, "not retried as VE3XYZ-7");
        assert!(t.states.iter().flatten().any(|s| s.2 == MsgState::Failed));
        assert!(a.room());
    }

    /// The reset (BOOT-14, APRS-10.1): nothing asked for survives it.
    #[test]
    fn reset_drops_everything_waiting() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        a.beacon_now();
        a.after_voice();
        assert!(a.send(b"VE3KPW", b"QSL"));
        a.clear();
        let cfg = AprsCfg { beacon: 1, ..AprsCfg::DEFAULT };
        assert_eq!(a.tick(100, Sending::All, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).len, 0);
    }

    /// APRS-26: our callsign as an alias when asked, and 0 5 7 3's wait before relaying.
    #[test]
    fn digipeater_mycall_alias_and_wait() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        a.tick(9, Sending::All, true, b"VE3XYZ", None, &DIGI, &mut out);
        let (f, n) = frame(b"VE3OCB-7", b"APRS", &[b"VE3XYZ", b"WIDE2-1"], b">To me");
        a.receive(&f[..n], 10, b"VE3XYZ", &DIGI);
        assert_eq!(a.tick(10, Sending::All, true, b"VE3XYZ", None, &DIGI, &mut out).len, 0, "my call isn't an alias by default");
        let cfg = AprsCfg { digi: crate::aprs::DIGI_ON | crate::aprs::DIGI_MYCALL | 3 << 4, ..AprsCfg::DEFAULT };
        let (f, n) = frame(b"VE3OCB-7", b"APRS", &[b"VE3XYZ", b"WIDE2-1"], b">To me again");
        a.receive(&f[..n], 40, b"VE3XYZ", &cfg);
        assert_eq!(a.tick(41, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out).len, 0, "waits 3 s");
        let t = a.tick(43, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out);
        assert_eq!(t.what, "Digipeat");
        assert!(Frame::parse(&out[..t.len]).unwrap().digis()[0].repeated);
    }

    /// 0 5 9 1: the path our frames ask for.
    #[test]
    fn path_setting() {
        let mut out = [0u8; FRAME_MAX];
        for (k, want) in [(0u8, 0usize), (1, 1), (2, 2)] {
            let a = AprsLive::new();
            let cfg = AprsCfg { path: k, ..AprsCfg::DEFAULT };
            a.beacon_now();
            let t = a.tick(1, Sending::All, true, b"VE3XYZ", Some(fix()), &cfg, &mut out);
            assert_eq!(t.what, "Beacon");
            let g = Frame::parse(&out[..t.len]).unwrap();
            assert_eq!(g.digis().len(), want);
            if want > 0 {
                assert_eq!(g.digis()[0].call(), b"WIDE1");
            }
        }
    }

    /// APRS-27: a beacon after voice, kept to APRS-9's 30 s spacing.
    #[test]
    fn beacon_after_voice() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let cfg = AprsCfg { beacon: 1, after_voice: true, ..AprsCfg::DEFAULT }; // manual beacons only
        a.after_voice();
        assert_eq!(a.tick(100, Sending::All, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).what, "Beacon");
        a.after_voice();
        assert_eq!(a.tick(110, Sending::All, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).len, 0, "30 s spacing");
        assert_eq!(a.tick(131, Sending::All, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).what, "Beacon");
        assert_eq!(a.tick(170, Sending::All, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).len, 0, "once per transmission");
    }

    /// The information field of the frame `tick` wrote.
    fn sent(out: &[u8; FRAME_MAX], t: &Tick) -> std::vec::Vec<u8> {
        Frame::parse(&out[..t.len]).unwrap().info.to_vec()
    }

    /// APRS-54, APRS-54.2: with 0 5 6 4 on, every position report (the beacon and a query's answer, plain or
    /// Mic-E) carries the battery, the MCU's temperature and the beacon power after the comment, a space between;
    /// a value not read yet is left out; off, the comment alone.
    #[test]
    fn values_follow_the_comment() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let mut cfg = AprsCfg { beacon: 1, power: 2, telemetry: true, ..AprsCfg::DEFAULT };
        cfg.comment[..7].copy_from_slice(b"Ferrite");
        let rd = (7_840, Some(31), 14_439_000);
        let mut info = [0u8; INFO_MAX];
        let n = position_info(&fix(), &cfg, rd, &mut info, &mut [0; 6]).unwrap();
        assert_eq!(&info[..n], b"=4525.51N/07541.41W[Ferrite 7.8V 31C 9W");
        // the beacon itself, Beacon now
        a.readings(7_840, Some(31), 14_439_000);
        a.beacon_now();
        let t = a.tick(0, Sending::All, true, b"VE3XYZ-7", Some(fix()), &cfg, &mut out);
        assert_eq!((t.what, sent(&out, &t)), ("Beacon", b"=4525.51N/07541.41W[Ferrite 7.8V 31C 9W".to_vec()));
        // no comment: the values are the comment; below zero; not read yet: left out
        let bare = AprsCfg { comment: [0; crate::aprs::COMMENT_LEN], power: 0, ..cfg };
        let n = position_info(&fix(), &bare, (6_960, Some(-5), 14_439_000), &mut info, &mut [0; 6]).unwrap();
        assert_eq!(&info[..n], b"=4525.51N/07541.41W[7.0V -5C 0.5W");
        let n = position_info(&fix(), &bare, (0, None, 14_439_000), &mut info, &mut [0; 6]).unwrap();
        assert_eq!(&info[..n], b"=4525.51N/07541.41W[0.5W");
        // off: the comment alone
        let n = position_info(&fix(), &AprsCfg { telemetry: false, ..cfg }, rd, &mut info, &mut [0; 6]).unwrap();
        assert_eq!(&info[..n], b"=4525.51N/07541.41W[Ferrite");
        // Mic-E: the same values after its comment, read back as the comment
        let mic = AprsCfg { mic_e: 2, power: 1, ..cfg };
        let mut d = [0u8; 6];
        let n = position_info(&fix(), &mic, rd, &mut info, &mut d).unwrap();
        let Ok(Packet::Position { comment, .. }) = parse::parse(&d, &info[..n]) else { panic!("{:?}", &info[..n]) };
        assert!(comment.bytes().eq(b"Ferrite 7.8V 31C 2.5W".iter().copied()));
        assert!(info[..n].ends_with(b"Ferrite 7.8V 31C 2.5W"));
    }

    /// APRS-54.3: the uncompressed report keeps to APRS 1.0.1's 43 characters after the symbol (course and speed,
    /// altitude and comment): only whole values that fit, dropped from the end, the comment never cut; Mic-E has
    /// no such limit and takes them all.
    #[test]
    fn values_within_the_limit() {
        let moving = Fix { three_d: true, alt_m: 120, speed_kmh: 40, heading: 90, ..fix() }; // CSE/SPD 7 + /A= 9
        let rd = (8_120, Some(24), 14_439_000);
        let mut info = [0u8; INFO_MAX];
        let tail = |cfg: &AprsCfg, f: &Fix, info: &mut [u8; INFO_MAX]| {
            let n = position_info(f, cfg, rd, info, &mut [0; 6]).unwrap();
            std::string::String::from_utf8(info[POSITION_HEAD..n].to_vec()).unwrap()
        };
        let mut cfg = AprsCfg { beacon: 1, power: 2, telemetry: true, ..AprsCfg::DEFAULT };
        // 20 characters: 16 + " " + 20 + " 8.1V" is 42; " 24C" would make 46
        cfg.comment[..20].copy_from_slice(&[b'x'; 20]);
        assert_eq!(tail(&cfg, &moving, &mut info), std::format!("090/021/A=000393 {} 8.1V", "x".repeat(20)));
        // standing still: all three fit (20 + 13 = 33)
        assert_eq!(tail(&cfg, &fix(), &mut info), std::format!("{} 8.1V 24C 9W", "x".repeat(20)));
        // 30 characters while moving: 47 already, none added, the comment as typed
        cfg.comment[..30].copy_from_slice(&[b'x'; 30]);
        assert_eq!(tail(&cfg, &moving, &mut info), std::format!("090/021/A=000393 {}", "x".repeat(30)));
        // 34 standing still: " 8.1V" makes 39, " 24C" 43 (just fits), " 9W" 46 doesn't
        cfg.comment[..34].copy_from_slice(&[b'x'; 34]);
        assert_eq!(tail(&cfg, &fix(), &mut info), std::format!("{} 8.1V 24C", "x".repeat(34)));
        assert!(tail(&cfg, &fix(), &mut info).len() <= POSITION_TAIL_MAX);
        // Mic-E: the longest comment, moving, all three
        cfg.comment = [b'x'; crate::aprs::COMMENT_LEN];
        let n = position_info(&moving, &AprsCfg { mic_e: 1, ..cfg }, rd, &mut info, &mut [0; 6]).unwrap();
        assert!(info[..n].ends_with(std::format!("{} 8.1V 24C 9W", "x".repeat(40)).as_bytes()));
    }

    /// APRS-54.4: the power is in watts as the radio really keys on the APRS frequency: 2 m as measured (0.5,
    /// 2.5, 9 W), 1.25 m Low whatever is chosen (TX-9), 70 cm 1, 5 and 10 W.
    #[test]
    fn watts_per_band() {
        let w = |f: Freq10| [0, 1, 2].map(|p| beacon_watts10(f, p));
        assert_eq!(w(14_439_000), [5, 25, 90], "2 m");
        assert_eq!(w(14_480_000), [5, 25, 90]);
        assert_eq!(w(22_350_000), [10, 10, 10], "1.25 m: Low only");
        assert_eq!(w(44_647_500), [10, 50, 100], "70 cm");
        let (v, _, k) = comment_values((7_400, Some(33), 22_350_000), 2);
        assert_eq!((v.as_bytes(), k), (&b"7.4V 33C 1W"[..], 3));
    }

    /// APRS-54.5: no telemetry frame is ever sent: with 0 5 6 4 on, timed beacons for an hour are beacons only,
    /// nothing `T#` and no definitions; with beacons alone (APRS-29) only beacons may go.
    #[test]
    fn no_telemetry_frames() {
        assert!(!Sending::Beacons.may_send("Telemetry") && Sending::Beacons.may_send("Beacon"));
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let cfg = AprsCfg { beacon: 5, power: 1, telemetry: true, ..AprsCfg::DEFAULT }; // every 10 min
        a.readings(7_400, Some(33), 14_439_000);
        let mut beacons = 0;
        for s in 100..3_700 {
            let t = a.tick(s, Sending::All, true, b"VE3XYZ-7", Some(fix()), &cfg, &mut out);
            if t.len > 0 {
                let info = sent(&out, &t);
                assert_eq!(t.what, "Beacon");
                assert!(!info.starts_with(b"T#") && !info.starts_with(b":"), "{info:?}");
                assert!(info.ends_with(b" 7.4V 33C 2.5W") || info.ends_with(b"[7.4V 33C 2.5W"));
                beacons += 1;
            }
        }
        assert_eq!(beacons, 6);
    }

    /// APRS-50.1: 0 5 6 5 sends the beacon text on the first timed beacon and then every 2nd, 4th or 8th; the
    /// others are the bare position (telemetry still on every one); Beacon now always carries it and doesn't count.
    #[test]
    fn beacon_text_every_nth() {
        for (every, want) in [(0u8, [true; 8]), (1, [true, false, true, false, true, false, true, false]), (2, [true, false, false, false, true, false, false, false]), (3, [true, false, false, false, false, false, false, false])] {
            let a = AprsLive::new();
            let mut out = [0u8; FRAME_MAX];
            let mut cfg = AprsCfg { beacon: 2, text_every: every, telemetry: true, ..AprsCfg::DEFAULT }; // every 2 min
            cfg.comment[..7].copy_from_slice(b"Hiking!");
            a.readings(7_400, Some(33), 14_439_000);
            let mut got = std::vec::Vec::new();
            let mut s = 100;
            while got.len() < 8 {
                let t = a.tick(s, Sending::All, true, b"VE3XYZ-7", Some(fix()), &cfg, &mut out);
                if t.len > 0 && t.what == "Beacon" {
                    let info = sent(&out, &t);
                    assert!(info.ends_with(b"7.4V 33C 0.5W"), "telemetry on every beacon: {info:?}");
                    got.push(info.windows(7).any(|w| w == b"Hiking!"));
                    if got.len() == 3 {
                        // Beacon now between: always the text, and the count goes on as before
                        a.beacon_now();
                        let t = a.tick(s + 1, Sending::All, true, b"VE3XYZ-7", Some(fix()), &cfg, &mut out);
                        assert!(t.len > 0 && sent(&out, &t).windows(7).any(|w| w == b"Hiking!"), "Beacon now carries it");
                        s += 1;
                    }
                }
                s += 1;
            }
            assert_eq!(got, want, "0 5 6 5 = {every}");
        }
    }

    /// APRS-32: a message to us heard again (the sender's retries, a second digipeater's copy) is acked every
    /// time but goes to the inbox once and rings once; the same number with other text, another sender's same
    /// number, and a message without a number after 30 s are new ones.
    #[test]
    fn repeats_acked_every_time_stored_once() {
        let a = AprsLive::new();
        let cfg = AprsCfg::DEFAULT;
        let mut out = [0u8; FRAME_MAX];
        let me = b"VE3XYZ-7";
        // A retry 30 s later, then the same frame through two digipeaters.
        let copies: [(&[&[u8]], u32); 4] = [(&[b"WIDE1-1", b"WIDE2-1"], 100), (&[b"WIDE1-1", b"WIDE2-1"], 130), (&[b"VE3RPT*", b"WIDE2-1"], 131), (&[b"VE3RPT", b"VE3DIG*"], 133)];
        let mut stored = 0;
        for (path, s) in copies {
            let (f, n) = frame(b"VE3KPW-9", b"APDR16", path, b":VE3XYZ-7 :Running late{AB12}");
            let rx = a.receive(&f[..n], s, me, &cfg);
            if let Some((from, text)) = rx.inbound {
                assert_eq!((from.as_bytes(), text.as_bytes(), rx.inbound_id.as_bytes()), (&b"VE3KPW-9"[..], &b"Running late"[..], &b"AB12"[..]));
                stored += 1;
            }
            let t = a.tick(s, Sending::All, true, me, None, &cfg, &mut out);
            assert_eq!((t.what, sent(&out, &t)), ("Ack", b":VE3KPW-9 :ackAB12".to_vec()), "{s}: acked again");
        }
        assert_eq!(stored, 1, "one inbox entry, one ring");
        // A reply-ack retry keys on its own number (MM of {MM}AA).
        for s in [200, 230] {
            let (f, n) = frame(b"VE3KPW-9", b"APDR16", &[], b":VE3XYZ-7 :Here now{7}3");
            assert_eq!(a.receive(&f[..n], s, me, &cfg).inbound.is_some(), s == 200);
        }
        // The same number with other text (a sender that started counting again), another sender's same number.
        let (f, n) = frame(b"VE3KPW-9", b"APDR16", &[], b":VE3XYZ-7 :Something else{AB12}");
        assert!(a.receive(&f[..n], 300, me, &cfg).inbound.is_some());
        let (f, n) = frame(b"VA3RTX", b"APK004", &[], b":VE3XYZ-7 :Running late{AB12}");
        assert!(a.receive(&f[..n], 301, me, &cfg).inbound.is_some());
        // Without a number: the same text within 30 s is the same message, later a new one.
        let (f, n) = frame(b"VA3RTX", b"APK004", &[b"WIDE1-1"], b":VE3XYZ-7 :QRV?");
        let (g, m) = frame(b"VA3RTX", b"APK004", &[b"VE3RPT*"], b":VE3XYZ-7 :QRV?");
        assert!(a.receive(&f[..n], 400, me, &cfg).inbound.is_some_and(|i| i.1.as_bytes() == b"QRV?"));
        assert!(a.receive(&g[..m], 410, me, &cfg).inbound.is_none(), "a digipeater's copy");
        assert!(a.receive(&f[..n], 431, me, &cfg).inbound.is_some(), "30 s on: new");
        assert!(a.receive(&f[..n], 431, me, &cfg).inbound_id.is_empty());
    }

    const QUERIES: AprsCfg = AprsCfg { beacon: 9, ..AprsCfg::DEFAULT }; // timed beacons, hourly

    /// A directed query from VE3KPW-9 at `s` (by `path`), and the frame that answers it in the next minute.
    fn ask(a: &AprsLive, text: &[u8], path: &[&[u8]], s: u32) -> Option<(&'static str, std::vec::Vec<u8>)> {
        let mut info = b":VE3XYZ-7 :".to_vec();
        info.extend_from_slice(text);
        let (f, n) = frame(b"VE3KPW-9", b"APDR16", path, &info);
        let rx = a.receive(&f[..n], s, b"VE3XYZ-7", &QUERIES);
        assert!(rx.inbound.is_none(), "a query isn't kept as a message");
        let mut out = [0u8; FRAME_MAX];
        (s..s + 60).find_map(|s| {
            let t = a.tick(s, Sending::All, true, b"VE3XYZ-7", Some(fix()), &QUERIES, &mut out);
            (t.len > 0).then(|| (t.what, sent(&out, &t)))
        })
    }

    /// APRS-50, APRS-51: the comment ends every position report, the beacon's and a query's answer, and ambiguity
    /// blanks the same digits of both coordinates; exact with none, as before.
    #[test]
    fn position_reports_carry_comment_and_ambiguity() {
        let mut cfg = AprsCfg { beacon: 1, ambiguity: 2, ..AprsCfg::DEFAULT };
        cfg.comment[..13].copy_from_slice(b"Ferrite, 73!!");
        let mut info = [0u8; INFO_MAX];
        let n = position_info(&fix(), &cfg, (0, None, 0), &mut info, &mut [0; 6]).unwrap();
        assert_eq!(&info[..n], b"=4525.  N/07541.  W[Ferrite, 73!!");
        let n = position_info(&fix(), &AprsCfg::DEFAULT, (0, None, 0), &mut info, &mut [0; 6]).unwrap();
        assert_eq!(&info[..n], b"=4525.51N/07541.41W[");
        // the beacon itself, and the longest comment with course, speed and altitude: within the frame
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        a.beacon_now();
        let t = a.tick(0, Sending::All, true, b"VE3XYZ-7", Some(fix()), &cfg, &mut out);
        assert_eq!(t.what, "Beacon");
        assert!(Frame::parse(&out[..t.len]).unwrap().info.ends_with(b"[Ferrite, 73!!"));
        cfg.comment = [b'x'; crate::aprs::COMMENT_LEN];
        let moving = Fix { three_d: true, alt_m: 120, speed_kmh: 40, heading: 90, ..fix() };
        let n = position_info(&moving, &cfg, (0, None, 0), &mut info, &mut [0; 6]).unwrap();
        assert!(info[..n].ends_with(&[b'x'; 40]) && n < INFO_MAX);
    }

    /// APRS-52, APRS-52.1, APRS-52.2: with 0 5 5 7 on, the beacon and a position query's answer go out as Mic-E with that status: the
    /// latitude in the destination in place of the tocall, our callsign, SSID and path as before, the comment and
    /// ambiguity kept; a message and a status answer keep the tocall.
    #[test]
    fn mic_e_beacons_and_answers() {
        let mut cfg = AprsCfg { beacon: 9, mic_e: 2, ambiguity: 1, ..AprsCfg::DEFAULT }; // En route
        cfg.comment[..7].copy_from_slice(b"Ferrite");
        let mut out = [0u8; FRAME_MAX];
        let check = |out: &[u8], len: usize| {
            let g = Frame::parse(&out[..len]).unwrap();
            assert_eq!((g.src.call(), g.src.ssid, g.dest.ssid, g.digis().len()), (&b"VE3XYZ"[..], 7, 0, 2));
            let Ok(uaprs::parse::Packet::Position { pos, mic_e: Some(m), comment, .. }) = uaprs::parse::parse(g.dest.call(), g.info) else { panic!("{:?}", g.info) };
            assert_eq!((m.bits, m.custom, pos.ambiguity), (0b110, false, 1));
            assert!((pos.lat - 45_425_200).abs() < 1000 && (pos.lon + 75_690_100).abs() < 1000, "{pos:?}");
            assert!(comment.bytes().eq(b"Ferrite".iter().copied()));
            std::vec::Vec::from(g.dest.call())
        };
        let a = AprsLive::new();
        a.beacon_now();
        let t = a.tick(0, Sending::All, true, b"VE3XYZ-7", Some(fix()), &cfg, &mut out);
        assert_eq!(t.what, "Beacon");
        assert_eq!(check(&out, t.len), b"TU2U5Z"); // 45 25.5_ N, En route 110, +0, W
        // a position query's answer: Mic-E too; a status query's: the tocall
        for (q, mic_e) in [(&b"?APRSP"[..], true), (b"?APRSS", false)] {
            let mut info = b":VE3XYZ-7 :".to_vec();
            info.extend_from_slice(q);
            let (f, n) = frame(b"VE3KPW-9", b"APDR16", &[b"WIDE1-1"], &info);
            a.receive(&f[..n], 1000, b"VE3XYZ-7", &cfg);
            let t = (1000..1100).map(|s| a.tick(s, Sending::All, true, b"VE3XYZ-7", Some(fix()), &cfg, &mut out)).find(|t| t.len > 0).unwrap();
            assert_eq!(t.what, "Query reply");
            if mic_e {
                check(&out, t.len);
            } else {
                assert_eq!(Frame::parse(&out[..t.len]).unwrap().dest.call(), TOCALL);
            }
        }
        assert!(a.send(b"VE3KPW-9", b"hi"));
        let t = a.tick(2000, Sending::All, true, b"VE3XYZ-7", Some(fix()), &cfg, &mut out);
        assert_eq!((t.what, Frame::parse(&out[..t.len]).unwrap().dest.call()), ("Message", &TOCALL[..]));
    }

    /// APRS-52.3: a Mic-E comment that would read as a device type or an altitude goes out behind a space or two
    /// and is read back as typed (`build::mic_e`'s own tests cover each case).
    #[test]
    fn mic_e_comment_protected() {
        let mut info = [0u8; INFO_MAX];
        for (c, pad) in [(&b"]Ferrite"[..], 1), (b"abc} here", 2), (b"Ferrite", 0)] {
            let mut cfg = AprsCfg { mic_e: 1, ..AprsCfg::DEFAULT };
            cfg.comment[..c.len()].copy_from_slice(c);
            let mut dest = TOCALL;
            let n = position_info(&fix(), &cfg, (0, None, 0), &mut info, &mut dest).unwrap();
            assert_eq!(&info[9..n], [&b"  "[..pad], c].concat().as_slice());
            let Ok(uaprs::parse::Packet::Position { pos, comment, .. }) = uaprs::parse::parse(&dest, &info[..n]) else { panic!() };
            assert!(comment.bytes().eq(c.iter().copied()) && pos.altitude.is_none());
        }
    }

    /// APRS-33: each directed query's answer, exactly: ?APRSP and ?APRS? our position, ?APRSS a status naming the
    /// firmware, ?APRSD the stations heard direct, ?APRSH a station's count, ?APRST and ?PING? the path as heard,
    /// ?VER the version; the others aren't answered (nor kept as messages). A numbered query is acked too.
    #[test]
    fn directed_queries_answered() {
        let a = AprsLive::new();
        a.beacon_last.set(Some(LastBeacon::new(0, Some(fix())))); // the hourly beacon not due
        let cfg = QUERIES;
        let (f, n) = frame(b"VE3OCB-7", b"APRS", &[], b"!4525.51N/07554.20W>Mobile");
        a.receive(&f[..n], 0, b"VE3XYZ-7", &cfg);
        let (f, n) = frame(b"VA3ABC", b"APRS", &[b"VE3RPT*"], b">Via a digi");
        a.receive(&f[..n], 10, b"VE3XYZ-7", &cfg);
        let reply = |w: &[u8]| Some(("Query reply", w.to_vec()));
        let pos = b"=4525.51N/07541.41W[";
        assert_eq!(ask(&a, b"?APRSP", &[], 100), reply(pos));
        assert_eq!(ask(&a, b"?APRS?", &[], 200), reply(pos));
        let ver = std::format!("Ferrite {}", ferrite_core::VERSION);
        assert_eq!(ask(&a, b"?APRSS", &[], 300), reply(std::format!(">{ver}").as_bytes()));
        assert_eq!(ask(&a, b"?APRSD", &[], 400), reply(b":VE3KPW-9 :Directs= VE3KPW-9 VE3OCB-7"));
        assert_eq!(ask(&a, b"?APRSH VE3OCB-7", &[], 500), reply(b":VE3KPW-9 :VE3OCB-7 heard 1x, last 8 min ago"));
        assert_eq!(ask(&a, b"?APRSH N0CALL", &[], 600), reply(b":VE3KPW-9 :N0CALL not heard"));
        assert_eq!(ask(&a, b"?APRST", &[b"VE3RPT*", b"WIDE2-1"], 700), reply(b":VE3KPW-9 :VE3KPW-9>APDR16,VE3RPT*,WIDE2-1"));
        assert_eq!(ask(&a, b"?PING?", &[b"WIDE1-1*"], 800), reply(b":VE3KPW-9 :VE3KPW-9>APDR16,WIDE1-1*"));
        assert_eq!(ask(&a, b"?VER", &[], 900), reply(std::format!(":VE3KPW-9 :{ver}").as_bytes()));
        assert_eq!(ask(&a, b"?APRSM", &[], 1000), None, "messages waiting: not answered");
        assert_eq!(ask(&a, b"?APRSO", &[], 1100), None);
        // Numbered: acked first, then answered.
        let (f, n) = frame(b"VE3KPW-9", b"APDR16", &[], b":VE3XYZ-7 :?APRSP{5");
        a.receive(&f[..n], 1200, b"VE3XYZ-7", &cfg);
        let mut out = [0u8; FRAME_MAX];
        let t = a.tick(1200, Sending::All, true, b"VE3XYZ-7", Some(fix()), &cfg, &mut out);
        assert_eq!(sent(&out, &t), b":VE3KPW-9 :ack5");
        let t = a.tick(1201, Sending::All, true, b"VE3XYZ-7", Some(fix()), &cfg, &mut out);
        assert_eq!((t.what, sent(&out, &t)), ("Query reply", pos.to_vec()));
        // Plain text starting with ? is a message.
        let (f, n) = frame(b"VE3KPW-9", b"APDR16", &[], b":VE3XYZ-7 :?? where are you");
        assert!(a.receive(&f[..n], 1300, b"VE3XYZ-7", &cfg).inbound.is_some());
    }

    /// APRS-33 with APRS-9: answers are automatic transmissions, 30 s apart; one waits at a time (a second query
    /// meanwhile, or a digipeater's copy, isn't answered); no position without a fresh fix; none at all without a
    /// callsign (APRS-8), without APRS mode, or without automatic beacons, and one waiting is dropped then.
    #[test]
    fn query_answers_follow_the_transmit_rules() {
        let mut out = [0u8; FRAME_MAX];
        let me = b"VE3XYZ-7";
        let q = |path: &[&[u8]], text: &[u8]| {
            let mut info = b":VE3XYZ-7 :".to_vec();
            info.extend_from_slice(text);
            frame(b"VE3KPW-9", b"APDR16", path, &info)
        };
        let a = AprsLive::new();
        a.beacon_last.set(Some(LastBeacon::new(0, Some(fix())))); // the hourly beacon not due
        let (f, n) = q(&[], b"?VER");
        a.receive(&f[..n], 100, me, &QUERIES);
        assert_eq!(a.tick(100, Sending::All, true, me, None, &QUERIES, &mut out).what, "Query reply");
        let (f, n) = q(&[], b"?APRSS");
        a.receive(&f[..n], 105, me, &QUERIES);
        let (g, m) = q(&[b"VE3RPT*"], b"?APRSS");
        a.receive(&g[..m], 107, me, &QUERIES); // the same query through a digipeater
        let (h, k) = q(&[], b"?APRSD");
        a.receive(&h[..k], 108, me, &QUERIES); // one waits already
        let sends: std::vec::Vec<u32> = (108..200).filter(|&s| a.tick(s, Sending::All, true, me, None, &QUERIES, &mut out).len > 0).collect();
        assert_eq!(sends, [130], "30 s after the last, once");
        // No fresh fix: a position query goes unanswered.
        let (f, n) = q(&[], b"?APRSP");
        a.receive(&f[..n], 300, me, &QUERIES);
        let old = Some(Fix { age_s: 4_000, ..fix() }); // older than the hourly beacon interval (GPS-33)
        assert_eq!(a.tick(300, Sending::All, true, me, old, &QUERIES, &mut out).len, 0);
        // No callsign: nothing heard as ours, nothing sent.
        let b = AprsLive::new();
        let (f, n) = q(&[], b"?VER");
        assert!(b.receive(&f[..n], 100, b"", &QUERIES).inbound.is_none());
        assert_eq!(b.tick(100, Sending::All, true, b"", None, &QUERIES, &mut out).len, 0);
        // Without APRS mode, or with beacons off or manual: not answered, and not later either.
        for (send, cfg) in [(Sending::Beacons, QUERIES), (Sending::Nothing, QUERIES), (Sending::All, AprsCfg::DEFAULT), (Sending::All, AprsCfg { beacon: 1, ..AprsCfg::DEFAULT })] {
            let b = AprsLive::new();
            b.receive(&f[..n], 100, me, &cfg);
            assert_eq!(b.tick(100, send, true, me, Some(fix()), &cfg, &mut out).what, if send == Sending::Beacons { "Beacon" } else { "" });
            for s in 101..200 {
                assert_ne!(b.tick(s, Sending::All, true, me, Some(fix()), &QUERIES, &mut out).what, "Query reply", "{send:?} {s}");
            }
        }
    }

    /// APRS-35: a general query (`?APRS?` heard on air) is answered with our position after a random 5-60 s, at
    /// most once in 5 minutes; with a footprint only when we are inside it.
    #[test]
    fn general_query_rate_limited() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let me = b"VE3XYZ-7";
        let cfg = QUERIES;
        let mut answers = std::vec::Vec::new();
        let (f, n) = frame(b"VE3OCB", b"APRS", &[b"WIDE1-1", b"WIDE2-1"], b"?APRS?");
        let (g, m) = frame(b"VA3ABC", b"APRS", &[b"WIDE2-2"], b"?APRS? 45.42,-75.69,0010");
        let (h, k) = frame(b"W6XYZ", b"APRS", &[], b"?APRS? 34.02,-117.15,0200");
        a.beacon_last.set(Some(LastBeacon::new(0, Some(fix()))));
        for s in 0..1500u32 {
            match s {
                100 | 200 | 380 => _ = a.receive(&f[..n], s, me, &cfg),
                700 => _ = a.receive(&h[..k], s, me, &cfg), // far away
                800 => _ = a.receive(&g[..m], s, me, &cfg),
                _ => {}
            }
            let t = a.tick(s, Sending::All, true, me, Some(fix()), &cfg, &mut out);
            if t.len > 0 {
                assert_eq!((t.what, sent(&out, &t)), ("Query reply", b"=4525.51N/07541.41W[".to_vec()));
                answers.push(s);
            }
        }
        assert_eq!(answers.len(), 2, "{answers:?}");
        assert!((105..=160).contains(&answers[0]), "{answers:?}: a random wait");
        assert!((805..=860).contains(&answers[1]), "{answers:?}: the 200 and 380 queries within 5 min, the 700 one outside its footprint");
        assert_eq!(footprint(b"34.02,-117.15,0200"), Some(((34_020_000, -117_150_000), 200 * 1609)));
        assert_eq!(footprint(b"34.02,-117.15"), None);
        // About 45.4 km east of Ottawa at 45.42 N (0.58 degrees of longitude): inside 30 miles, outside 25.
        assert!(within((45_420_000, -75_690_000), (45_420_000, -75_110_000), 30 * 1609));
        assert!(!within((45_420_000, -75_690_000), (45_420_000, -75_110_000), 25 * 1609));
    }

    /// APRS-34: a rejection of one of ours ("rejNN", or "rejNN}" from a reply-ack station) ends it at once as
    /// rejected: no more tries. Another station's rejection, or one for a number we don't have waiting, changes
    /// nothing.
    #[test]
    fn rejected_stops_retries() {
        let mut out = [0u8; FRAME_MAX];
        let cfg = AprsCfg::DEFAULT;
        for rej in [&b":VE3XYZ   :rej1"[..], b":VE3XYZ   :rej1}", b":VE3XYZ   :rej1}45"] {
            let a = AprsLive::new();
            assert!(a.send(b"VE3KPW-9", b"QSL"));
            assert_eq!(a.tick(0, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out).what, "Message");
            let (f, n) = frame(b"VA3RTX", b"APRS", &[], rej);
            assert!(a.receive(&f[..n], 5, b"VE3XYZ", &cfg).state.is_none(), "not from the recipient");
            let (f, n) = frame(b"VE3KPW-9", b"APRS", &[], b":VE3XYZ   :rej9");
            assert!(a.receive(&f[..n], 6, b"VE3XYZ", &cfg).state.is_none(), "not our number");
            let (f, n) = frame(b"VE3KPW-9", b"APRS", &[], rej);
            let rx = a.receive(&f[..n], 10, b"VE3XYZ", &cfg);
            assert!(matches!(rx.state, Some((p, t, MsgState::Rejected)) if p.as_bytes() == b"VE3KPW-9" && t.as_bytes() == b"QSL"), "{:?}", core::str::from_utf8(rej));
            assert!(rx.inbound.is_none());
            for s in 11..600 {
                assert_eq!(a.tick(s, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out).len, 0, "no more tries");
            }
        }
    }

    /// APRS-13: a message whose text APRS can't carry (`|`, `~`, `{`) fails at once instead of five silent tries.
    #[test]
    fn unsendable_text_fails_at_once() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        assert!(a.send(b"VE3KPW-9", b"a{b"));
        let t = a.tick(0, Sending::All, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out);
        assert_eq!(t.len, 0);
        assert!(matches!(t.states[0], Some((_, _, MsgState::Failed))));
    }

    /// APRS-4.1: when a heard station gets a stored row: the first time this power-on, a new position or comment
    /// after 10 minutes, and every 30 minutes while it's heard.
    #[test]
    fn heard_rows_are_due_by_the_rule() {
        assert!(row_due(None, false, 5));
        assert!(!row_due(Some(100), true, 100 + ROW_CHANGE_S - 1));
        assert!(row_due(Some(100), true, 100 + ROW_CHANGE_S));
        assert!(!row_due(Some(100), false, 100 + ROW_REFRESH_S - 1));
        assert!(row_due(Some(100), false, 100 + ROW_REFRESH_S));
        let a = AprsLive::new();
        let hear = |at: u32, info: &[u8]| {
            let (f, n) = frame(b"VE3KPW-9", b"APRS", &[b"WIDE1-1*"], info);
            a.receive(&f[..n], at, b"VE3XYZ", &AprsCfg::DEFAULT).row
        };
        let here = b"!4525.51N/07554.20W>Mobile";
        assert!(hear(1000, here), "new since power-on");
        assert!(!hear(1060, here), "the same, a minute on");
        assert!(!hear(1300, b"!4526.51N/07554.20W>Mobile"), "moved, but within 10 min of its row");
        assert!(hear(1000 + ROW_CHANGE_S, b"!4526.51N/07554.20W>Mobile"), "moved, 10 min on");
        let t = 1000 + ROW_CHANGE_S;
        assert!(!hear(t + 60, b"!4526.51N/07554.20W>Mobile"));
        assert!(hear(t + ROW_REFRESH_S, b"!4526.51N/07554.20W>Mobile"), "the 30-minute refresh");
    }

    /// APRS-4.2: stations restored at power-on go behind those heard since, keep their count and data, have no age
    /// until one is known (INBOX-6.1), and get their first row of this power-on when heard again.
    #[test]
    fn restored_stations() {
        let a = AprsLive::new();
        let (f, n) = frame(b"VA3RTX", b"APRS", &[], b">Net at 8");
        assert!(a.receive(&f[..n], 50, b"VE3XYZ", &AprsCfg::DEFAULT).row);
        let old = |call: &[u8], ago_s: u32| Station {
            call: Buf::from(call),
            sym: Sym::Home,
            pos: Some(Pos { lat: 454_000_000, lon: -757_000_000 }),
            ago_s,
            via: Buf::from(b"VE3RSB-1*".as_slice()),
            course: None,
            alt_m: Some(70),
            times: 7,
            comment: Buf::from(b"Home".as_slice()),
            msg: Buf::new(),
            msg_min: None,
            fx25: None,
            device: None,
        };
        a.restore(&old(b"VE3JLH", AGO_UNKNOWN), 17, 50);
        a.restore(&old(b"VA3RTX", AGO_UNKNOWN), 18, 50); // heard since: left as it is
        let s = a.station(1).unwrap();
        // APRS-4.5: RAM keeps the list's fields; the path, altitude and comment come from row 17 (the store reads it)
        assert_eq!((s.call.as_bytes(), s.ago_s, s.times, s.via.as_bytes(), s.alt_m), (&b"VE3JLH"[..], AGO_UNKNOWN, 7, &b""[..], None));
        assert_eq!(a.row_of(1), Some((17, DET_ALL)));
        assert_eq!(a.row_of(0), None, "heard this power-on: its details in RAM");
        assert_eq!((a.stations(), a.station(0).unwrap().times), (2, 1));
        a.restore(&old(b"VE3JLH", 4000), 17, 50); // GPS time: its age is known now
        assert_eq!(a.station(1).unwrap().ago_s, 4000);
        let (f, n) = frame(b"VE3JLH", b"APRS", &[], b">Back");
        assert!(a.receive(&f[..n], 70, b"VE3XYZ", &AprsCfg::DEFAULT).row, "its first row this power-on");
        assert_eq!((a.station(0).unwrap().times, a.station(0).unwrap().ago_s), (8, 0));
    }

    #[test]
    fn size() {
        let n = core::mem::size_of::<AprsLive>();
        std::println!("AprsLive: {n} B, heard entry {} B, out {} B", core::mem::size_of::<Option<Heard>>(), core::mem::size_of::<Option<Out>>());
        assert!(n <= 8_500, "{n}"); // 50 stations (decision 246)
    }

    /// APRS-35: a general query's footprint: decimal degrees and miles; anything malformed is none; the distance
    /// test wraps at the date line.
    #[test]
    fn footprints() {
        assert_eq!(footprint(b"34.02,-117.15,0200"), Some(((34_020_000, -117_150_000), 200 * 1609)));
        assert_eq!(footprint(b" 45 , -75.5 , 10 "), Some(((45_000_000, -75_500_000), 10 * 1609)));
        for bad in [&b"34.02,-117.15"[..], b"34.02,-117.15,", b"91,0,5", b"0,181,5", b"0,0,123456", b"0,0,5,6", b"x,0,5", b"1.1234567,0,5", b"1234,0,5", b".5,0,5"] {
            assert_eq!(footprint(bad), None, "{}", core::str::from_utf8(bad).unwrap());
        }
        // across the date line: 179.9 E and 179.9 W are about 22 km apart at the equator
        assert!(within((0, 179_900_000), (0, -179_900_000), 30_000));
        assert!(within((0, -179_900_000), (0, 179_900_000), 30_000));
        assert!(!within((0, 179_900_000), (0, -179_900_000), 10_000));
        assert_eq!(degrees(b"-0.5"), Some(-500_000));
    }

    /// A message number as its digits.
    #[test]
    fn message_numbers() {
        assert_eq!(num(7).as_bytes(), b"7");
        assert_eq!(num(42).as_bytes(), b"42");
        assert!(AprsLive::default().stations() == 0);
    }

    /// APRS-56.2: a missed packet's event-log words.
    #[test]
    fn miss_text_names_the_sender() {
        use ferrite_core::audio::uafsk::{Miss, MISS_HEAD};
        let mut head = [0u8; MISS_HEAD];
        let addr = |s: &[u8; 6], ssid: u8, last: bool| -> [u8; 7] {
            let mut a = [0u8; 7];
            for (o, c) in a.iter_mut().zip(s) {
                *o = c << 1;
            }
            a[6] = 0x60 | ssid << 1 | last as u8;
            a
        };
        head[..7].copy_from_slice(&addr(b"APRS  ", 0, false));
        head[7..14].copy_from_slice(&addr(b"VE7ABC", 9, false));
        head[14..21].copy_from_slice(&addr(b"WIDE1 ", 1, true));
        head[8] = b'#' << 1; // a bit error in the sender's second letter
        let m = Miss { len: 84, addresses: true, fx25: false, head };
        let (mut name, mut text) = (Buf::<20>::new(), Buf::<64>::new());
        miss_text(&m, 16384, &mut name, &mut text);
        assert_eq!(name.as_bytes(), b"V?7ABC-9");
        assert_eq!(text.as_bytes(), &b"Not decoded: 84 B, peak 50%, >APRS,WIDE1-1"[..]);
        assert!(miss_logged(&m));
        // no address field to read: noise, not logged
        assert!(!miss_logged(&Miss { len: 21, addresses: false, fx25: false, head }));
    }
}
