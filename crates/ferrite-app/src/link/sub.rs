//! APRS over the link and pushed events (manual chapter 29, LINK-43 – LINK-48; Appendix G): what Ferrite Console's
//! APRS section and its Bluetooth Live View use.
//!
//! - The APRS settings (MENU 0 5, [`AprsCfg`]) as a `READ`/`WRITE` record ([`what::APRS`]), checked as the radio's
//!   editors check them (LINK-43).
//! - `HEARD` and `MSGS` read the heard list and the messages (LINK-44); `MSG_SEND` queues a message exactly as the
//!   radio's compose does (LINK-45).
//! - `SUBSCRIBE` (LINK-46): the session is sent `EVENT`s unasked: a station heard, a message in or out, a message's
//!   state, and the Live View's UI updates ([`ferrite_core::liveview`], LINK-47, LINK-48), within a byte rate that
//!   leaves Bluetooth room for the answers (LINK-36). One subscription per session; it ends with the session (a new
//!   `HELLO`, a new connection) or when the session has sent nothing for [`ev::QUIET_MS`].

use super::{Out, Session, Window};
use crate::aprs::{path_ok, region_freq, AprsCfg, BEACON_MODES, COMMENT_LEN, CUSTOM_LEN, F_MAX, F_MIN};
use crate::chan::ChannelStore;
use crate::data::{Draft, Message, MsgState, Op, Station};
use crate::persist::Sink as PSink;
use crate::term::cmd::Line;
use crate::term::Sink;
use crate::App;
use ferrite_core::link::{self as wire, event as ev, msg, Status, MAX_PAYLOAD, MAX_WIRE};
use ferrite_core::liveview::{fnv, Track, DICT_VERSION, FNV0, MORE};
use ferrite_core::msg::{Device, LiveAll};
use ferrite_ui::text::Buf;

/// The APRS record's length (LINK-43): eight bytes, the custom path, FX.25 on transmit (APRS-45), then (protocol
/// 4, decision 347) the message sound (APRS-6.1), symbol (APRS-47), TX delay (APRS-48), TX level (APRS-49), the
/// comment (APRS-50), ambiguity (APRS-51), Mic-E (APRS-52) and telemetry (APRS-54): every MENU 0 5 value, so
/// Console needs no terminal for them; then (0.8.2) auto reply (APRS-57): on and the delay, the pattern, the
/// reply, at [`APRS_REPLY_AT`]; then (0.8.5) the beacon text rate (APRS-50.1, 0 5 6 5: 0 all, 1 every 2nd, 2 every 4th,
/// 3 every 8th) at [`APRS_TEXT_AT`]; then (0.9.6) message retries (APRS-13.6) at [`APRS_RETRIES_AT`]. A shorter
/// record (Console before them) keeps the radio's values past its end.
pub const APRS_REC: usize = APRS_RETRIES_AT + 1;
/// APRS-13.6: message retries (0 off - 4), after the text rate. A record without it keeps the radio's.
pub const APRS_RETRIES_AT: usize = APRS_TEXT_AT + 1;
/// APRS-50.1: where the beacon text rate is in the record (the record before it ends here).
pub const APRS_TEXT_AT: usize = APRS_REPLY_AT + 1 + crate::aprs::REPLY_TO_LEN + crate::aprs::REPLY_TEXT_LEN;
/// APRS-57: where auto reply starts in the record: its byte (bit 0 on, bits 4-5 the delay), the pattern (9, zero-
/// padded), the reply (45, zero-padded). The record before it (protocol 4, decision 347) ends here.
pub const APRS_REPLY_AT: usize = APRS_V1 + 4 + COMMENT_LEN + 3;
/// The record before protocol 4: up to FX.25.
const APRS_V1: usize = 8 + CUSTOM_LEN + 1;
/// The newest messages watched for events.
const WATCHED: usize = 4;

/// A session's subscription (LINK-46).
#[derive(Clone, Copy, Debug)]
pub struct Sub {
    mask: u8,
    gap_ms: u32,
    /// The rate cap: event bytes it may send, in thousandths (one event may take it below 0).
    budget: i32,
    at: u32,
    /// The `EVENT` frames' own counter (their seq).
    seq: u8,
    /// The records' version when last looked at, or more events waiting.
    ver: u32,
    dirty: bool,
    /// The newest station heard (its call, times heard, the count), and the newest messages (an ID of who and
    /// what, and the state) with their count.
    heard: u32,
    msgs: [(u16, u8); WATCHED],
    msgs_n: u16,
    /// LINK-57: the event log's next sequence number to send.
    log: u32,
    /// Live View: when the last UI update was looked for, the redraws and input then; more of one to send; start
    /// over with the whole state.
    ui_at: u32,
    ui_frames: u32,
    ui_input: u32,
    ui_more: bool,
    ui_start: bool,
    /// The Live View carries the screen as drawn (LINK-19.5).
    texts: bool,
    /// LINK-67: each state part's FNV-1a as last sent (the radio, slots A-C), when it was last looked at, and
    /// whether the next event sends them all.
    state: [u32; 4],
    state_at: u32,
    state_all: bool,
    /// LINK-71: the lists' write counts as last told, as last seen, and when they last moved.
    lists: [u32; 3],
    lists_seen: [u32; 3],
    lists_at: u32,
}

impl Sub {
    pub const fn new() -> Self {
        Sub { mask: 0, gap_ms: 0, budget: 0, at: 0, seq: 0, ver: 0, dirty: false, heard: 0, msgs: [(0, 0); WATCHED], msgs_n: 0, log: 0, ui_at: 0, ui_frames: 0, ui_input: 0, ui_more: false, ui_start: false, texts: false, state: [0; 4], state_at: 0, state_all: false, lists: [0; 3], lists_seen: [0; 3], lists_at: 0 }
    }
}

impl Default for Sub {
    fn default() -> Self {
        Self::new()
    }
}

fn state_byte(s: MsgState) -> (u8, u8) {
    match s {
        MsgState::Unread => (0, 0),
        MsgState::Read => (1, 0),
        MsgState::Pending => (2, 0),
        MsgState::Acked => (3, 0),
        MsgState::Retrying(n) => (4, n),
        MsgState::Failed => (5, 0),
        MsgState::Rejected => (6, 0), // APRS-34: a new value, so older readers see an unknown state
        MsgState::Cancelled => (7, 0), // APRS-13.1, likewise
    }
}

/// Who and what a message is (not its state): what tells one message from another in the watched ones.
fn msg_id(m: &Message) -> u16 {
    let h = fnv(fnv(fnv(FNV0, &[m.inbound as u8]), m.peer.as_bytes()), m.text.as_bytes());
    (h ^ h >> 16) as u16
}

/// The event log's next sequence number (the newest's + 1; 0 when empty), as `LOG_READ` gives it (LINK-42).
fn log_top(d: &dyn crate::data::Data) -> u32 {
    d.event_seq(0).map_or(0, |s| s.wrapping_add(1))
}

fn heard_sig(d: &dyn crate::data::Data) -> u32 {
    match d.station(0) {
        Some(s) => fnv(fnv(FNV0, s.call.as_bytes()), &[s.times as u8, (s.times >> 8) as u8, d.stations() as u8]),
        None => FNV0,
    }
}

/// The watched messages now: (ID, state) of the newest [`WATCHED`], and the count.
fn watched(d: &dyn crate::data::Data) -> ([(u16, u8); WATCHED], u16) {
    let mut w = [(0, 0); WATCHED];
    for (i, e) in w.iter_mut().enumerate() {
        if let Some(m) = d.message(i as u16) {
            *e = (msg_id(&m), state_byte(m.state).0 | state_byte(m.state).1 << 4);
        }
    }
    (w, d.messages())
}

impl Session {
    /// `SUBSCRIBE` (LINK-46): the mask, then optionally the UI's least gap (100 ms units) and flags.
    pub(super) fn subscribe<S: ChannelStore>(&mut self, st: &S, now: u32, p: &[u8], o: &mut Out) -> Status {
        let Some(&mask) = p.first().filter(|&&m| m & !(ev::SUB_HEARD | ev::SUB_MSGS | ev::SUB_UI | ev::SUB_LOG | ev::SUB_STATE | ev::SUB_LISTS) == 0) else { return Status::BadArg };
        let texts = p.get(2).is_some_and(|f| f & ev::TEXTS != 0);
        let renew = p.get(2).is_some_and(|f| f & ev::RENEW != 0) && mask == self.sub.mask && texts == self.sub.texts;
        let gap = match p.get(1) {
            Some(&g) if g > 0 => g as u32 * 100,
            _ if self.trusted => ev::UI_GAP_CABLE_MS,
            _ => ev::UI_GAP_BT_MS,
        };
        if !renew {
            let d = st.data();
            let (msgs, msgs_n) = watched(d);
            let ui_start = mask & ev::SUB_UI != 0;
            self.sub = Sub { mask, gap_ms: gap, at: now, seq: self.sub.seq, ver: d.version(), heard: heard_sig(d), msgs, msgs_n, log: log_top(d), ui_at: now, ui_start, texts, state_all: mask & ev::SUB_STATE != 0, lists: st.lists_versions(), lists_seen: st.lists_versions(), ..Sub::new() };
        }
        self.sub.gap_ms = gap;
        o.put(&[mask, DICT_VERSION]);
        Status::Ok
    }

    /// The session wants the Live View and is starting it over: [`crate::ports::Ports`] gives it the one tracker
    /// (one Live View at a time: the newest subscription's).
    pub fn ui_starting(&self) -> bool {
        self.sub.ui_start
    }

    /// Whether this session still wants the Live View.
    pub fn ui_wanted(&self) -> bool {
        self.sub.mask & ev::SUB_UI != 0
    }

    /// The other port's session took the Live View: this one's ends.
    pub fn ui_drop(&mut self) {
        self.sub.mask &= !ev::SUB_UI;
    }

    /// Events wait to go (the rest of a cut UI update, a message event, the whole state): poll again soon.
    pub fn events_waiting(&self) -> bool {
        self.sub.mask != 0 && (self.sub.ui_more || self.sub.dirty || self.sub.ui_start || self.sub.state_all)
    }

    /// The subscription ends (a new `HELLO`, a new connection).
    pub(super) fn unsubscribe(&mut self) {
        self.sub = Sub { seq: self.sub.seq, ..Sub::new() };
    }

    /// Events due, pushed into `out` (LINK-46): at most one of each kind a call, within the rate cap, only while
    /// no request waits for its answer and `out` keeps room for one. `ui` is the Live View's tracker while this
    /// session has it.
    #[allow(clippy::too_many_arguments)]
    #[inline(never)] // its buffers in its own frame, not stacked on the session's answer (SBAR-23)
    pub fn events<S: ChannelStore>(&mut self, app: &mut App, st: &S, dev: &Device, live: &LiveAll, now: u32, ui: Option<&mut Track>, out: &mut dyn Sink) {
        if self.sub.mask == 0 {
            return;
        }
        if !self.seen || now.wrapping_sub(self.last) >= ev::QUIET_MS || !self.authorised(app) {
            self.unsubscribe(); // the PC went quiet: events stop (LINK-46)
            return;
        }
        let rate = ev::rate(self.by == crate::remote::BY_CABLE, self.fast, self.audio != 0) as i32;
        let dt = now.wrapping_sub(self.sub.at).min(10_000) as i32;
        self.sub.budget = (self.sub.budget + dt * rate).min(ev::BURST as i32 * 1000);
        self.sub.at = now;
        if self.have || self.sub.budget < 0 {
            return;
        }
        let d = st.data();
        let ver = d.version();
        if ver != self.sub.ver || self.sub.dirty {
            self.sub.ver = ver;
            self.sub.dirty = false;
            let mut p = [0u8; MAX_PAYLOAD];
            if self.sub.mask & ev::SUB_HEARD != 0 {
                let sig = heard_sig(d);
                if sig != self.sub.heard {
                    if let Some(s) = d.station(0) {
                        let mut o = Out { a: &mut p, n: 0 };
                        o.put(&[ev::HEARD, d.stations().min(255) as u8]);
                        station(&mut o, &s);
                        let n = o.n;
                        if self.emit(&p[..n], out) {
                            self.sub.heard = sig;
                        } else {
                            self.sub.dirty = true;
                        }
                    } else {
                        self.sub.heard = sig; // emptied (cleared): nothing to say
                    }
                }
            }
            if self.sub.mask & ev::SUB_MSGS != 0 {
                self.message_event(d, out);
            }
            if self.sub.mask & ev::SUB_LOG != 0 {
                self.log_event(d, out);
            }
        }
        if self.sub.mask & ev::SUB_STATE != 0 {
            self.state_event(app, st, dev, live, now, out);
        }
        if self.sub.mask & ev::SUB_LISTS != 0 {
            self.lists_event(st, now, out);
        }
        if let Some(t) = ui.filter(|_| self.sub.mask & ev::SUB_UI != 0) {
            let since = now.wrapping_sub(self.sub.ui_at);
            let s = self.sub;
            // urgent (100 ms) for a key or the knob, and for what changes within a second of one (a short press acts
            // when let go); else the gap, when something was redrawn (or every 5 s)
            let drawn = app.frames != s.ui_frames;
            let urgent = app.last_input != s.ui_input || drawn && now.wrapping_sub(app.last_input) < 1000;
            let due = s.ui_start || s.ui_more || urgent && since >= 100 || since >= s.gap_ms && (drawn || since >= 5000);
            if !due || s.budget < 0 || out.room() < 2 * MAX_WIRE {
                return; // a start waits too: the whole state goes at the next chance
            }
            if s.ui_start {
                t.restart();
            }
            (self.sub.ui_start, self.sub.ui_more) = (false, false);
            (self.sub.ui_at, self.sub.ui_frames, self.sub.ui_input) = (now, app.frames, app.last_input);
            let mut p = [0u8; MAX_PAYLOAD];
            p[0] = ev::UI;
            let texts = self.sub.texts;
            let mut l = Line::new();
            let n = t.update(
                |i, b| {
                    crate::uistate::line_all(app, st, dev, live, texts, i, &mut l).then(|| {
                        b[..l.len()].copy_from_slice(l.as_bytes());
                        l.len()
                    })
                },
                &mut p[1..],
            );
            if let Some(n) = n {
                self.sub.ui_more = p[2] & MORE != 0;
                self.emit(&p[..1 + n], out); // room was checked: it goes
            }
        }
    }

    /// One message event, if one is due: new messages first (the oldest new one), then a state change.
    fn message_event(&mut self, d: &dyn crate::data::Data, out: &mut dyn Sink) {
        let (now, n) = watched(d);
        let prev = self.sub.msgs;
        if n < self.sub.msgs_n {
            (self.sub.msgs, self.sub.msgs_n) = (now, n); // deleted or cleared on the radio: nothing to say
            return;
        }
        // How many are new at the front: the count's growth if the rest lines up, else the least shift that does.
        let aligned = |k: usize| (k..WATCHED).all(|j| now[j].0 == prev[j - k].0 || j as u16 >= n);
        let grew = ((n - self.sub.msgs_n) as usize).min(WATCHED);
        let k = if aligned(grew) { grew } else { (0..=WATCHED).find(|&k| aligned(k)).unwrap_or(WATCHED) };
        let (kind, i) = if k > 0 {
            (ev::MSG, k - 1)
        } else if let Some(j) = (0..WATCHED.min(n as usize)).find(|&j| now[j].1 != prev[j].1) {
            (ev::MSG_STATE, j)
        } else {
            self.sub.msgs_n = n;
            return;
        };
        let Some(m) = d.message(i as u16) else { return };
        let mut p = [0u8; MAX_PAYLOAD];
        let mut o = Out { a: &mut p, n: 0 };
        o.put(&[kind]);
        o.put(&n.to_le_bytes());
        message(&mut o, &m);
        let len = o.n;
        if !self.emit(&p[..len], out) {
            self.sub.dirty = true;
            return;
        }
        if kind == ev::MSG {
            // that one is known now; any newer are found next time
            self.sub.msgs.copy_within(0..WATCHED - 1, 1);
            self.sub.msgs[0] = now[i];
            self.sub.msgs_n += 1;
        } else {
            self.sub.msgs[i] = now[i];
        }
        self.sub.dirty = true; // look again: more may be waiting
    }

    /// LINK-57: the oldest event logged since the last one sent (or since the subscription), as `LOG_READ` gives it;
    /// one a call, the rest next time. Events the log has already dropped are skipped; a log cleared on the radio
    /// says nothing.
    fn log_event(&mut self, d: &dyn crate::data::Data, out: &mut dyn Sink) {
        let (top, kept) = (log_top(d), d.events() as u32);
        let want = self.sub.log.max(top.saturating_sub(kept));
        if want >= top {
            self.sub.log = self.sub.log.min(top);
            return;
        }
        let i = (top - 1 - want) as u16;
        let (Some(e), Some(seq)) = (d.event(i), d.event_seq(i)) else { return };
        let mut p = [0u8; MAX_PAYLOAD];
        let mut o = Out { a: &mut p, n: 0 };
        o.put(&[ev::LOG]);
        super::log_record(&mut o, seq, &e);
        let len = o.n;
        if self.emit(&p[..len], out) {
            self.sub.log = seq.wrapping_add(1);
        }
        self.sub.dirty = true; // look again: more may be waiting
    }

    /// LINK-71: a list stored to since it was last told, once the writing has been quiet for
    /// [`ev::LISTS_QUIET_MS`] (a `WRITE_MANY` of many records: one event): what, then the lists generation; one list
    /// a call (channels, zones, then presets), the rest next time.
    fn lists_event<S: ChannelStore>(&mut self, st: &S, now: u32, out: &mut dyn Sink) {
        use ferrite_core::link::what;
        let v = st.lists_versions();
        if v != self.sub.lists_seen {
            (self.sub.lists_seen, self.sub.lists_at) = (v, now);
            return;
        }
        if v == self.sub.lists || now.wrapping_sub(self.sub.lists_at) < ev::LISTS_QUIET_MS {
            return;
        }
        let Some(i) = (0..3).find(|&i| v[i] != self.sub.lists[i]) else { return };
        let mut p = [0u8; 6];
        p[0] = ev::LISTS;
        p[1] = [what::CHANNEL, what::ZONE, what::PRESET][i];
        p[2..6].copy_from_slice(&super::lists_gen(st).to_le_bytes());
        if self.emit(&p, out) {
            self.sub.lists[i] = v[i];
        }
    }

    /// One `EVENT` frame out: false (nothing sent) if `out` lacks room for it and an answer after it.
    /// LINK-67: the radio state's parts that changed since they were last sent, at most every UI gap (the whole
    /// record after a subscription).
    #[inline(never)]
    fn state_event<S: ChannelStore>(&mut self, app: &App, st: &S, dev: &Device, live: &LiveAll, now: u32, out: &mut dyn Sink) {
        use ferrite_core::link::state as m;
        if !self.sub.state_all && now.wrapping_sub(self.sub.state_at) < self.sub.gap_ms || self.sub.budget < 0 {
            return;
        }
        self.sub.state_at = now;
        let r = super::state::record(app, st, dev, live);
        let mut parts = 0u8;
        let mut sig = [0u32; 4];
        for (i, h) in sig.iter_mut().enumerate() {
            *h = fnv(FNV0, &r[super::state::part(i)]);
            if *h != self.sub.state[i] || self.sub.state_all {
                parts |= 1 << i;
            }
        }
        if parts == 0 {
            return;
        }
        let mut p = [0u8; 2 + m::LEN];
        p[..2].copy_from_slice(&[ev::STATE, parts]);
        let mut n = 2;
        for i in (0..4).filter(|i| parts & 1 << i != 0) {
            let part = &r[super::state::part(i)];
            p[n..n + part.len()].copy_from_slice(part);
            n += part.len();
        }
        if self.emit(&p[..n], out) {
            (self.sub.state, self.sub.state_all) = (sig, false);
        }
    }

    fn emit(&mut self, payload: &[u8], out: &mut dyn Sink) -> bool {
        let mut w = [0u8; MAX_WIRE];
        let Some(k) = wire::build(msg::EVENT, self.sub.seq, payload, &mut w) else { return false };
        if out.room() < k + MAX_WIRE {
            return false;
        }
        out.put(&w[..k]);
        self.sub.seq = self.sub.seq.wrapping_add(1);
        self.sub.budget -= k as i32 * 1000;
        true
    }
}

// ---- Records ----

/// A station (LINK-44): flags (1 a position, 2 course and speed, 4 altitude), call (length, text), symbol, age
/// (u32 s), times heard (u16), [lat, lon (i32, degrees x 1e7)], [course (u16, degrees), speed (u16, km/h)],
/// [altitude (i32, m)], path (length, text: the digipeaters, the used one marked *), comment (length, text).
pub(super) fn station(o: &mut Out, s: &Station) {
    let flags = s.pos.is_some() as u8 | (s.course.is_some() as u8) << 1 | (s.alt_m.is_some() as u8) << 2;
    o.put(&[flags]);
    o.text(s.call.as_bytes());
    o.put(&[s.sym as u8]);
    o.put(&s.ago_s.to_le_bytes());
    o.put(&s.times.to_le_bytes());
    if let Some(p) = s.pos {
        o.put(&p.lat.to_le_bytes());
        o.put(&p.lon.to_le_bytes());
    }
    if let Some((c, v)) = s.course {
        o.put(&c.to_le_bytes());
        o.put(&v.to_le_bytes());
    }
    if let Some(a) = s.alt_m {
        o.put(&a.to_le_bytes());
    }
    o.text(s.via.as_bytes());
    o.text(s.comment.as_bytes());
}

fn station_len(s: &Station) -> usize {
    1 + 1 + s.call.len() + 1 + 4 + 2 + if s.pos.is_some() { 8 } else { 0 } + if s.course.is_some() { 4 } else { 0 } + if s.alt_m.is_some() { 4 } else { 0 } + 1 + s.via.len() + 1 + s.comment.len()
}

/// A message (LINK-44): flags (1 in, to us), state (0 unread, 1 read, 2 pending, 3 acked, 4 retrying, 5 failed,
/// 6 rejected: APRS-34, 7 cancelled: APRS-13.1),
/// tries so far (retrying), age (u32 s), peer (length, text: from, or to), text (length, text).
pub(super) fn message(o: &mut Out, m: &Message) {
    let (s, tries) = state_byte(m.state);
    o.put(&[m.inbound as u8, s, tries]);
    o.put(&m.ago_s.to_le_bytes());
    o.text(m.peer.as_bytes());
    o.text(m.text.as_bytes());
}

fn message_len(m: &Message) -> usize {
    3 + 4 + 1 + m.peer.len() + 1 + m.text.len()
}

/// `HEARD` (LINK-44): from index p[0]; the count, then as many stations as fit. LINK-70: p[1..3], when there, a
/// maximum age (u16, minutes; 0 none): only the stations heard within it, the count and the index among them.
pub(super) fn heard<S: ChannelStore>(st: &S, p: &[u8], o: &mut Out) -> Status {
    heard_at(st, p, o).0
}

/// [`heard`], and for a stream (LINK-50) the index (among the stations within the age) to go on from when stations
/// are left.
pub(super) fn heard_at<S: ChannelStore>(st: &S, p: &[u8], o: &mut Out) -> (Status, Option<u16>) {
    let d = st.data();
    let all = d.stations();
    let max_s = match p {
        [_, a, b, ..] => u16::from_le_bytes([*a, *b]) as u32 * 60,
        _ => 0,
    };
    // LINK-70: the age from RAM (no stored row read for a station left out); a station whose age isn't known (from
    // before a restart, before GPS time dated it) is never within one
    let within = |i: u16| max_s == 0 || d.station_ram(i).is_some_and(|s| s.ago_s != crate::data::AGO_UNKNOWN && s.ago_s <= max_s);
    let n = if max_s == 0 { all } else { (0..all).filter(|&i| within(i)).count() as u16 };
    o.put(&[n.min(255) as u8]);
    let from = p.first().copied().unwrap_or(0) as u16;
    let (mut k, mut i) = (0u16, 0u16); // k: the index among those within; i: in the whole list
    while i < all && k < from {
        k += within(i) as u16;
        i += 1;
    }
    while i < all {
        if !within(i) {
            i += 1;
            continue;
        }
        let Some(s) = d.station(i) else { break };
        if o.n + station_len(&s) > MAX_PAYLOAD {
            break;
        }
        station(o, &s);
        (i, k) = (i + 1, k + 1);
    }
    (Status::Ok, Some(k).filter(|&k| k < n.min(255) && k > from))
}

/// `MSGS` (LINK-44): from index (u16); the count (u16), then as many messages as fit, newest first.
pub(super) fn msgs<S: ChannelStore>(st: &S, p: &[u8], o: &mut Out) -> Status {
    msgs_at(st, p, o).0
}

/// [`msgs`], and for a stream (LINK-50) the index to go on from when messages are left.
pub(super) fn msgs_at<S: ChannelStore>(st: &S, p: &[u8], o: &mut Out) -> (Status, Option<u16>) {
    let d = st.data();
    let n = d.messages();
    o.put(&n.to_le_bytes());
    let from = match p {
        [a, b, ..] => u16::from_le_bytes([*a, *b]),
        _ => 0,
    };
    let mut i = from;
    while i < n {
        let Some(m) = d.message(i) else { break };
        if o.n + message_len(&m) > MAX_PAYLOAD {
            break;
        }
        message(o, &m);
        i += 1;
    }
    (Status::Ok, Some(i).filter(|&i| i < n && i > from))
}

/// `MSG_SEND` (LINK-45): to and the text, each a length and text; queued as the radio's compose queues it (the
/// store's `Send`, APRS-13's tries), the radio's own draft kept. Every transmission it makes follows the radio's
/// rules (APRS-7, chapter 15; Dry TX as set). The answer: 1 if APRS mode is on (it goes at the APRS task's next
/// turn), 0 if it waits for APRS mode (APRS-22).
pub(super) fn msg_send<S: ChannelStore>(app: &mut App, st: &S, dev: &Device, p: &[u8], o: &mut Out) -> Status {
    let mut r = p;
    let mut field = || -> Option<&[u8]> {
        let (&n, rest) = r.split_first()?;
        let t = rest.get(..n as usize)?;
        r = &rest[n as usize..];
        Some(t)
    };
    let (Some(to), Some(text)) = (field(), field()) else { return Status::BadArg };
    if !r.is_empty() {
        return Status::BadArg;
    }
    let invalid = |o: &mut Out, why: &str| {
        o.put(why.as_bytes());
        Status::Invalid
    };
    if !dev.tx_built {
        return invalid(o, "receive-only build: no transmit");
    }
    let d = st.data();
    if d.callsign().is_none() {
        return invalid(o, "No callsign: MENU 0 8 6 1"); // APRS-8
    }
    if !d.callsign().is_some_and(|c| crate::data::ax25_call_ok(c.as_bytes())) {
        return invalid(o, "Callsign over 6: APRS can't carry it"); // SETUP-13.1
    }
    let mut call = Buf::<9>::new();
    for &c in to {
        call.push(&[c.to_ascii_uppercase()]);
    }
    if to.is_empty() || to.len() > 9 || !call.as_bytes().iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'-') {
        return invalid(o, "to: 1-9 capitals, digits and -");
    }
    if text.is_empty() || text.len() > 67 || !text.iter().all(|&c| (0x20..0x7F).contains(&c) && !b"|~{".contains(&c)) {
        return invalid(o, "message: 1-67 printable characters, no | ~ {");
    }
    let keep = d.draft();
    let sent = d.request(Op::Draft(Draft { to: call, text: Buf::from(text) })) && d.request(Op::Send);
    d.request(Op::Draft(keep)); // the radio's own draft, as it was
    if !sent {
        return Status::Busy; // the queue is full: try again shortly
    }
    app.show_toast(b"Message queued", call.as_bytes(), false);
    o.put(&[app.aprs_on() as u8]);
    Status::Ok
}

/// `MSG_STOP` (LINK-49): to and the text, as `MSG_SEND`'s; the newest of our messages to that station with that text
/// still being tried stops, as the radio's own Stop sending does (APRS-13.1). `invalid` when none is being tried.
pub(super) fn msg_stop<S: ChannelStore>(st: &S, p: &[u8], o: &mut Out) -> Status {
    let Some((&n, rest)) = p.split_first() else { return Status::BadArg };
    let Some(to) = rest.get(..n as usize) else { return Status::BadArg };
    let Some((&m, rest)) = rest[n as usize..].split_first() else { return Status::BadArg };
    let Some(text) = rest.get(..m as usize).filter(|_| rest.len() == m as usize) else { return Status::BadArg };
    let d = st.data();
    let open = (0..d.messages()).find(|&i| {
        d.message(i).is_some_and(|x| {
            !x.inbound && matches!(x.state, MsgState::Pending | MsgState::Retrying(_)) && x.peer.as_bytes().eq_ignore_ascii_case(to) && x.text.as_bytes() == text
        })
    });
    match open {
        Some(i) if d.request(Op::StopMessage(i)) => Status::Ok,
        Some(_) => Status::Busy,
        None => {
            o.put(b"not being sent");
            Status::Invalid
        }
    }
}

/// `MSG_DELETE` (LINK-52): flags (1: one to us), the station and the text as `MSG_STOP`'s; the newest message that
/// way with that station and text goes, as the radio's own Delete message does (APRS-46.1). `invalid` when none.
pub(super) fn msg_delete<S: ChannelStore>(st: &S, p: &[u8], o: &mut Out) -> Status {
    let Some((&flags, p)) = p.split_first() else { return Status::BadArg };
    let Some((&n, rest)) = p.split_first() else { return Status::BadArg };
    let Some(peer) = rest.get(..n as usize) else { return Status::BadArg };
    let Some((&m, rest)) = rest[n as usize..].split_first() else { return Status::BadArg };
    let Some(text) = rest.get(..m as usize).filter(|_| rest.len() == m as usize) else { return Status::BadArg };
    let d = st.data();
    let found = (0..d.messages()).find(|&i| {
        d.message(i).is_some_and(|x| x.inbound == (flags & 1 != 0) && x.peer.as_bytes().eq_ignore_ascii_case(peer) && x.text.as_bytes() == text)
    });
    match found {
        Some(i) if d.request(Op::DeleteMessage(i)) => Status::Ok,
        Some(_) => Status::Busy,
        None => {
            o.put(b"no such message");
            Status::Invalid
        }
    }
}

/// The APRS record as the link reads it (LINK-43): the frequency (u16, 5 kHz units, 0 the region's), beacon mode,
/// power, path, digipeater, beacon after voice, SSID, then the custom path (16 bytes, 0-padded): the APRS record's
/// settings in their stored order, without the quick messages.
pub(super) fn aprs_record(app: &App, w: &mut Window) {
    let c = app.aprs_cfg();
    w.put(&c.f5k.to_le_bytes());
    w.put(&[c.beacon, c.power, c.path, c.digi, c.after_voice as u8, c.ssid]);
    w.put(&c.custom);
    w.put(&[c.fx25, c.sound, c.symbol, c.txdelay, c.tx_level]);
    w.put(&c.comment);
    w.put(&[c.ambiguity, c.mic_e, c.telemetry as u8]);
    w.put(&[c.reply]); // APRS-57
    w.put(&c.reply_to);
    w.put(&c.reply_text);
    w.put(&[c.text_every]); // APRS-50.1
    w.put(&[c.retries]); // APRS-13.6
}

/// A whole APRS record written (LINK-43): checked as MENU 0 5's editors check each value, then kept as they keep
/// it (written back at the next tick, slot C retuned when it is the APRS slot). Nothing changes if any value is
/// refused.
pub(super) fn aprs_apply(app: &mut App, rec: &[u8], o: &mut Out) -> Status {
    if rec.len() != APRS_REC && rec.len() != APRS_RETRIES_AT && rec.len() != APRS_TEXT_AT && rec.len() != APRS_REPLY_AT && rec.len() != APRS_V1 && rec.len() != APRS_V1 - 1 {
        return Status::BadArg;
    }
    let invalid = |o: &mut Out, why: &str| {
        o.put(why.as_bytes());
        Status::Invalid
    };
    let mut custom = [0u8; CUSTOM_LEN];
    custom.copy_from_slice(&rec[8..8 + CUSTOM_LEN]);
    let n = custom.iter().position(|&b| b == 0).unwrap_or(CUSTOM_LEN);
    if custom[n..].iter().any(|&b| b != 0) || n > 0 && !path_ok(&custom[..n]) {
        return invalid(o, "custom path: one or two entries like WIDE2-2");
    }
    let mut c = AprsCfg { f5k: u16::from_le_bytes([rec[0], rec[1]]), beacon: rec[2], power: rec[3], path: rec[4], digi: rec[5], after_voice: rec[6] == 1, custom, ssid: rec[7], fx25: rec.get(8 + CUSTOM_LEN).copied().unwrap_or(app.aprs_cfg().fx25), ..app.aprs_cfg() };
    if rec.len() >= APRS_REPLY_AT {
        let t = &rec[APRS_V1..APRS_REPLY_AT];
        let (head, tail) = t.split_at(4);
        let (comment, last) = tail.split_at(COMMENT_LEN);
        (c.sound, c.symbol, c.txdelay, c.tx_level) = (head[0], head[1], head[2], head[3]);
        c.comment.copy_from_slice(comment);
        (c.ambiguity, c.mic_e, c.telemetry) = (last[0], last[1], last[2] == 1);
        let n = c.comment.iter().position(|&b| b == 0).unwrap_or(COMMENT_LEN);
        // As the menu and the terminal check them (APRS-6.1, APRS-47 - APRS-52, APRS-54).
        if c.sound as usize >= ferrite_core::tonegen::CHIMES.len() || c.symbol as usize >= crate::aprs::SYMBOLS.len() {
            return invalid(o, "message sound or symbol out of range");
        }
        if !(crate::aprs::TXDELAY_MIN..=crate::aprs::TXDELAY_MAX).contains(&c.txdelay) || c.tx_level > 2 {
            return invalid(o, "TX delay: 10-100 (in 10 ms); TX level: 0-2");
        }
        if c.comment[n..].iter().any(|&b| b != 0) || !c.comment[..n].iter().all(|&b| (b' '..=b'~').contains(&b)) {
            return invalid(o, "comment: up to 40 characters");
        }
        if c.ambiguity > 4 || c.mic_e as usize >= crate::aprs::MIC_E_NAMES.len() || last[2] > 1 {
            return invalid(o, "ambiguity, Mic-E or telemetry out of range");
        }
    }
    // APRS-50.1: the beacon text rate, as 0 5 6 5 steps it
    if rec.len() == APRS_REC {
        if rec[APRS_RETRIES_AT] > crate::aprs::RETRIES_DEFAULT {
            return invalid(o, "message retries: 0-4");
        }
        c.retries = rec[APRS_RETRIES_AT];
    }
    if rec.len() >= APRS_RETRIES_AT {
        if rec[APRS_TEXT_AT] > 3 {
            return invalid(o, "text rate: 0-3 (all, every 2nd, 4th or 8th)");
        }
        c.text_every = rec[APRS_TEXT_AT];
    }
    // APRS-57: auto reply, as its page checks it: the pattern capitals, digits, - and *; the reply printable.
    if rec.len() >= APRS_TEXT_AT {
        let r = &rec[APRS_REPLY_AT..APRS_TEXT_AT];
        let (to, text) = r[1..].split_at(crate::aprs::REPLY_TO_LEN);
        let field = |t: &[u8], ok: &dyn Fn(u8) -> bool| {
            let n = t.iter().position(|&b| b == 0).unwrap_or(t.len());
            t[n..].iter().all(|&b| b == 0) && t[..n].iter().all(|&b| ok(b))
        };
        if r[0] & !0x31 != 0 {
            return invalid(o, "auto reply: on/off and a delay of 0-3");
        }
        if !field(to, &|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-' || b == b'*') {
            return invalid(o, "auto reply to: capitals, digits, - and *, up to 9");
        }
        if !field(text, &|b| (b' '..=b'~').contains(&b)) {
            return invalid(o, "auto reply: up to 45 printable characters");
        }
        c.reply = r[0];
        c.reply_to.copy_from_slice(to);
        if c.reply_to_text().is_empty() {
            c.reply_to = crate::aprs::REPLY_TO_DEFAULT; // everyone, as *
        }
        c.reply_text.copy_from_slice(text);
    }
    if c.f5k != 0 {
        let f = c.f5k as u32 * 500;
        if !(F_MIN..=F_MAX).contains(&f) {
            return invalid(o, "frequency: APRS takes 64-300 MHz");
        }
        if f == region_freq(app.set.region()) {
            c.f5k = 0; // the region's, kept as such (0 5 4)
        }
    }
    if c.beacon as usize & 0x7F >= BEACON_MODES {
        return invalid(o, "beacon mode out of range");
    }
    if c.power > 2 || rec[6] > 1 {
        return invalid(o, "power or beacon after voice out of range");
    }
    if !c.path_ok(c.path) {
        return invalid(o, "path: none, WIDE1-1, WIDE1-1,WIDE2-1, APRSAT, ARISS, or custom once set");
    }
    if c.digi & !0x73 != 0 || c.digi >> 4 & 7 > crate::aprs::DIGI_WAIT_MAX {
        return invalid(o, "digipeater out of range");
    }
    if c.ssid > 15 {
        return invalid(o, "SSID: 0-15");
    }
    if !crate::aprs::FX25_CHECKS.contains(&c.fx25) {
        return invalid(o, "FX.25: 0 (off), 16, 32 or 64 check bytes");
    }
    if c != app.aprs_cfg() {
        app.set_aprs(c);
    }
    Status::Ok
}

#[cfg(test)]
mod tests;
