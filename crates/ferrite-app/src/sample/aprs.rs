//! The simulator's APRS task: what the radio's APRS task will do with slot C in APRS mode, with made-up
//! traffic. Stations are heard one by one, a message for us comes now and then (and is acked), our messages are
//! tried on APRS-13's schedule until an ack comes, and beacons go out by the beacon mode (0 5 6 1) or Beacon now,
//! waiting while the band relay is in the other state (APRS-20). A station we have heard acks our message on its
//! second try; anyone else never does, so the message ends failed. Nothing here is in the firmware.

use crate::aprs::{beacon_due, retry_wait, AprsCfg, LastBeacon, TRIES};
use crate::aprs_live::SentLog;
use crate::data::{Beacon, Fix, Message, MsgState, Pos, SentBeacon, Station, Sym};
use core::cell::Cell;
use ferrite_ui::text::Buf;

/// Messages sent or received since start, newest first; older ones drop off.
const LIVE: usize = 6;
/// New stations heard after APRS mode goes on: the first after this long, then one every so often.
const HEARD_FIRST_S: u32 = 8;
const HEARD_EVERY_S: u32 = 20;
/// Messages for us: the first after this long, then one every so often.
const IN_FIRST_S: u32 = 40;
const IN_EVERY_S: u32 = 180;
/// A heard station's ack comes this long after our second try.
const ACK_AFTER_S: u32 = 3;
/// APRS-9: automatic transmissions at least this far apart (the first try of a message is PTT's, exempt).
const AUTO_GAP_S: u32 = 30;

// call, symbol, position, comment
const NEW: [(&str, Sym, Option<Pos>, &str); 4] = [
    ("VE3OCB-7", Sym::Ht, Some(Pos { lat: 454_252_000, lon: -756_901_000 }), "Walking the canal"),
    ("VA3MBK-9", Sym::Car, Some(Pos { lat: 453_901_000, lon: -757_552_000 }), "Mobile, 146.520"),
    ("VE3BTV", Sym::Home, None, "Barrhaven"),
    ("VE3OTW-10", Sym::IGate, Some(Pos { lat: 454_401_000, lon: -756_310_000 }), "Ottawa iGate"),
];

const INBOUND: [(&str, &str); 3] = [("VE3KPW-9", "Are you on the net tonight?"), ("VE3OCB-7", "QSL, see you at the shack"), ("VA3RTX", "Repeater is back up, 73")];

#[derive(Clone, Copy)]
struct Live {
    msg: Message,
    at_s: u32,
    tries: u8,
    /// When the next try is due, or (after the last) when it is marked failed.
    next_s: u32,
    /// When the peer's ack arrives.
    ack_s: Option<u32>,
}

/// What a tick did: a message for us arrived, something was transmitted (what, for the toast).
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct AprsNews {
    pub inbound: Option<(Buf<10>, Buf<67>)>,
    pub tx: Option<&'static str>,
    /// Anything the lists show changed.
    pub changed: bool,
    /// Our messages that moved on (tried again, acked, failed), for the store that keeps them.
    pub states: [Option<(Buf<10>, Buf<67>, MsgState)>; 3],
}

impl AprsNews {
    fn state(&mut self, m: &Message) {
        if let Some(s) = self.states.iter_mut().find(|s| s.is_none()) {
            *s = Some((m.peer, m.text, m.state));
        }
    }
}

pub(super) struct AprsSim {
    pub cfg: Cell<AprsCfg>,
    now_s: Cell<u32>,
    on_since: Cell<Option<u32>>,
    live: [Cell<Option<Live>>; LIVE],
    heard: Cell<u8>,
    heard_at: [Cell<u32>; NEW.len()],
    /// New stations hidden by "clear the heard list".
    heard_gone: Cell<u8>,
    inbound: Cell<u8>,
    acks_due: Cell<u8>,
    beacon_last: Cell<Option<LastBeacon>>,
    beacon_now: Cell<bool>,
    waiting: Cell<bool>,
    last_auto: Cell<Option<u32>>,
    /// APRS-53: our beacons (three made up from before start), and when the last one is heard back.
    sent: SentLog,
    echo_s: Cell<Option<u32>>,
}

/// A sent beacon heard back through the iGate this long after it went out.
const ECHO_AFTER_S: u32 = 2;

impl AprsSim {
    pub fn new() -> Self {
        AprsSim {
            cfg: Cell::new(AprsCfg::DEFAULT),
            now_s: Cell::new(0),
            on_since: Cell::new(None),
            live: core::array::from_fn(|_| Cell::new(None)),
            heard: Cell::new(0),
            heard_at: core::array::from_fn(|_| Cell::new(0)),
            heard_gone: Cell::new(0),
            inbound: Cell::new(0),
            acks_due: Cell::new(0),
            beacon_last: Cell::new(None),
            beacon_now: Cell::new(false),
            waiting: Cell::new(false),
            last_auto: Cell::new(None),
            sent: sample_sent(),
            echo_s: Cell::new(None),
        }
    }

    /// APRS-53: the Sent page's entries.
    pub fn beacons_sent(&self) -> u16 {
        self.sent.len()
    }

    pub fn sent_beacon(&self, i: u16) -> Option<SentBeacon> {
        self.sent.get(i, self.now_s())
    }

    pub fn now_s(&self) -> u32 {
        self.now_s.get()
    }

    fn push(&self, l: Live) {
        for i in (1..LIVE).rev() {
            self.live[i].set(self.live[i - 1].get());
        }
        self.live[0].set(Some(l));
    }

    pub fn live_n(&self) -> u16 {
        self.live.iter().filter(|c| c.get().is_some()).count() as u16
    }

    pub fn message(&self, i: u16) -> Option<Message> {
        let l = self.live.get(i as usize)?.get()?;
        Some(Message { ago_s: self.now_s().saturating_sub(l.at_s), ..l.msg })
    }

    /// Op::ClearMessages: every message goes.
    pub fn clear_messages(&self) {
        self.live.iter().for_each(|c| c.set(None));
    }

    pub fn read(&self, i: u16) {
        if let Some(c) = self.live.get(i as usize) {
            if let Some(mut l) = c.get().filter(|l| l.msg.state == MsgState::Unread) {
                l.msg.state = MsgState::Read;
                c.set(Some(l));
            }
        }
    }

    /// Our message (Op::Send): pending, its first try at the next tick.
    pub fn send(&self, to: &[u8], text: Buf<67>) {
        let msg = Message { inbound: false, peer: Buf::from(to), text, ago_s: 0, state: MsgState::Pending, to: Buf::new() };
        self.push(Live { msg, at_s: self.now_s(), tries: 0, next_s: self.now_s(), ack_s: None });
    }

    /// APRS-46.1: message i goes (one of ours being tried stops with it); the ones after it move up.
    pub fn delete(&self, i: u16) -> bool {
        let i = i as usize;
        if self.live.get(i).and_then(Cell::get).is_none() {
            return false;
        }
        for k in i..LIVE - 1 {
            self.live[k].set(self.live[k + 1].get());
        }
        self.live[LIVE - 1].set(None);
        true
    }

    /// APRS-13.1: ours being tried stops, cancelled.
    pub fn stop(&self, i: u16) -> bool {
        let Some(c) = self.live.get(i as usize) else { return false };
        let Some(mut l) = c.get().filter(|l| !l.msg.inbound && matches!(l.msg.state, MsgState::Pending | MsgState::Retrying(_))) else { return false };
        l.msg.state = MsgState::Cancelled;
        c.set(Some(l));
        true
    }

    /// APRS-13.2: ours that failed or was cancelled goes again, as a new message.
    pub fn resend(&self, i: u16) -> bool {
        let Some(l) = self.live.get(i as usize).and_then(Cell::get).filter(|l| !l.msg.inbound && matches!(l.msg.state, MsgState::Failed | MsgState::Cancelled)) else { return false };
        self.send(l.msg.peer.as_bytes(), l.msg.text);
        true
    }

    pub fn beacon_now(&self) {
        self.beacon_now.set(true);
    }

    /// A beacon asked for and not yet sent (tests).
    #[cfg(test)]
    pub fn beacon_asked(&self) -> bool {
        self.beacon_now.get()
    }

    /// New stations heard (not cleared), newest first.
    pub fn stations(&self) -> u16 {
        (self.heard.get() - self.heard_gone.get()) as u16
    }

    pub fn station(&self, i: u16) -> Option<Station> {
        let k = (self.heard.get() as usize).checked_sub(1 + i as usize).filter(|_| i < self.stations())?;
        let (call, sym, pos, comment) = NEW[k];
        let ago_s = self.now_s().saturating_sub(self.heard_at[k].get());
        Some(Station {
            call: Buf::from(call.as_bytes()),
            sym,
            pos,
            ago_s,
            via: Buf::from(b"WIDE1-1*".as_slice()),
            course: None,
            alt_m: None,
            times: 1,
            comment: Buf::from(comment.as_bytes()),
            msg: Buf::new(),
            msg_min: None,
            fx25: (k == 1).then_some(3),
            device: (k == 2).then_some(6), // a Yaesu FT5D (APRS-5.2)
        })
    }

    /// The reset: messages, the heard list and the APRS settings go.
    pub fn clear(&self) {
        self.live.iter().for_each(|c| c.set(None));
        self.clear_heard();
        self.cfg.set(AprsCfg::DEFAULT);
    }

    pub fn clear_heard(&self) {
        self.heard_gone.set(self.heard.get());
    }

    pub fn heard(&self, call: &[u8]) -> bool {
        NEW[..self.heard.get() as usize].iter().any(|n| n.0.as_bytes() == call)
    }

    pub fn beacon(&self) -> Beacon {
        let c = self.cfg.get();
        let next_s = c.every_min().map(|m| match self.beacon_last.get() {
            Some(l) => (l.at_s + m as u32 * 60).saturating_sub(self.now_s()),
            None => 0,
        });
        Beacon { waiting: self.waiting.get(), next_s: next_s.filter(|_| self.on_since.get().is_some()), ..Beacon::default() }
    }

    /// A second of the APRS task: `on` is APRS mode, `relay_ok` whether slot C can transmit with the band
    /// relay where the active slot holds it, `known` whether a station acks (it has been heard), `fix` the GPS's
    /// for distance and smart beacons (APRS-24).
    pub fn tick(&self, now_s: u32, on: bool, relay_ok: bool, known: &dyn Fn(&[u8]) -> bool, fix: Option<Fix>) -> AprsNews {
        self.now_s.set(now_s);
        let mut news = AprsNews::default();
        // APRS-53.1: the last beacon heard back through the iGate
        if self.echo_s.get().is_some_and(|t| now_s >= t) {
            self.echo_s.set(None);
            news.changed |= self.sent.heard_by(b"VE3OTW-10");
        }
        if !on {
            self.on_since.set(None);
            news.changed = self.waiting.replace(false);
            return news;
        }
        let since = *self.on_since.get().get_or_insert(now_s);
        self.on_since.set(Some(since));
        let up = now_s - since;
        // heard
        let h = self.heard.get();
        if (h as usize) < NEW.len() && up >= HEARD_FIRST_S + h as u32 * HEARD_EVERY_S {
            self.heard_at[h as usize].set(now_s);
            self.heard.set(h + 1);
            news.changed = true;
        }
        // a message for us, acked (APRS-6)
        let k = self.inbound.get();
        if up >= IN_FIRST_S + k as u32 * IN_EVERY_S {
            let (from, text) = INBOUND[k as usize % INBOUND.len()];
            let msg = Message { inbound: true, peer: Buf::from(from.as_bytes()), text: Buf::from(text.as_bytes()), ago_s: 0, state: MsgState::Unread, to: Buf::new() };
            self.push(Live { msg, at_s: now_s, tries: 0, next_s: 0, ack_s: None });
            self.inbound.set(k + 1);
            self.acks_due.set(self.acks_due.get() + 1);
            news.inbound = Some((msg.peer, msg.text));
            news.changed = true;
        }
        // acks for ours, and the ones that ran out of tries
        for c in &self.live {
            let Some(mut l) = c.get().filter(|l| !l.msg.inbound && matches!(l.msg.state, MsgState::Pending | MsgState::Retrying(_))) else { continue };
            if l.ack_s.is_some_and(|t| now_s >= t) {
                l.msg.state = MsgState::Acked;
            } else if l.tries >= TRIES && now_s >= l.next_s {
                l.msg.state = MsgState::Failed;
            } else {
                continue;
            }
            c.set(Some(l));
            news.state(&l.msg);
            news.changed = true;
        }
        // one transmission a tick, on C's frequency: the relay must already be where C needs it (APRS-20)
        let auto_ok = self.last_auto.get().is_none_or(|t| now_s - t >= AUTO_GAP_S);
        let beacon = self.beacon_now.get() || beacon_due(&self.cfg.get(), self.beacon_last.get(), now_s, fix);
        let waiting = beacon && !relay_ok;
        news.changed |= self.waiting.replace(waiting) != waiting;
        if !relay_ok {
            return news;
        }
        if self.acks_due.get() > 0 {
            self.acks_due.set(self.acks_due.get() - 1);
            news.tx = Some("Ack");
            return news;
        }
        for c in &self.live {
            let Some(mut l) = c.get().filter(|l| !l.msg.inbound && l.tries < TRIES && now_s >= l.next_s) else { continue };
            if !matches!(l.msg.state, MsgState::Pending | MsgState::Retrying(_)) || l.tries > 0 && !auto_ok {
                continue;
            }
            l.tries += 1;
            l.msg.state = if l.tries == 1 { MsgState::Pending } else { MsgState::Retrying(l.tries) };
            l.next_s = now_s + retry_wait(l.tries) as u32;
            if l.tries == 2 && known(l.msg.peer.as_bytes()) {
                l.ack_s = Some(now_s + ACK_AFTER_S);
            }
            c.set(Some(l));
            news.state(&l.msg);
            if l.tries > 1 {
                self.last_auto.set(Some(now_s));
            }
            news.tx = Some("Message");
            news.changed = true;
            return news;
        }
        if beacon && (auto_ok || self.beacon_now.get()) {
            self.beacon_now.set(false);
            self.beacon_last.set(Some(LastBeacon::new(now_s, fix)));
            self.last_auto.set(Some(now_s));
            news.tx = Some("Beacon");
            news.changed = true;
            self.sent.push(now_s);
            self.echo_s.set(Some(now_s + ECHO_AFTER_S));
        }
        news
    }
}

/// APRS-53: three beacons from before start (31, 21 and 11 minutes before it): one heard back through two
/// digipeaters, one not heard back, one through one.
fn sample_sent() -> SentLog {
    let l = SentLog::new();
    for (ago, by) in [(31 * 60, &[&b"VE3RSB-1"[..], b"VE3OTW-10"][..]), (21 * 60, &[]), (11 * 60, &[&b"VE3RSB-1"[..]])] {
        l.push(0u32.wrapping_sub(ago));
        for b in by {
            l.heard_by(b);
        }
    }
    l
}
