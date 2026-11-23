//! APRS over the link and its events, against the sample store: what Ferrite Console's APRS section and Live View
//! see.

use super::*;
use crate::data::Data;
use crate::sample::Sample;
use ferrite_core::link::what;
use ferrite_core::keys::Key;
use ferrite_core::liveview::{self, Lines};
use std::vec::Vec;

struct Wire(Vec<u8>);

impl Sink for Wire {
    fn room(&self) -> usize {
        4096
    }
    fn put(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
}

/// A PC on a session, with the Live View's tracker the ports would give it.
struct Pc {
    app: App,
    st: Sample,
    link: Session,
    ui: Track,
    now: u32,
    seq: u8,
    dev: Device,
    live: LiveAll,
}

impl Pc {
    fn new(trusted: bool) -> Self {
        let mut app = App::new(crate::tests::set(), crate::tests::alert());
        while app.pop_cmd().is_some() {}
        let dev = Device { tx_built: true, ..Device::default() };
        Pc { app, st: Sample::new(), link: Session::new(trusted), ui: Track::new(), now: 1000, seq: 0, dev, live: LiveAll::default() }
    }

    fn ask(&mut self, ty: u8, payload: &[u8]) -> (u8, Vec<u8>) {
        self.seq = self.seq.wrapping_add(1);
        let mut w = [0u8; MAX_WIRE];
        let n = wire::build(ty, self.seq, payload, &mut w).unwrap();
        for &c in &w[..n] {
            self.link.feed(c, self.now);
        }
        let mut out = Wire(Vec::new());
        for _ in 0..4 {
            self.now += 10;
            self.link.poll(&mut self.app, &self.st, &self.dev, &self.live, self.now, &mut out);
            while let Some(wr) = self.app.pop_write() {
                self.st.apply(wr);
            }
            if !out.0.is_empty() {
                break;
            }
        }
        let k = out.0.len() - 1;
        let f = wire::parse(&mut out.0[..k]).unwrap();
        assert_eq!((f.ty, f.seq), (ty | wire::ANSWER, self.seq));
        (f.payload[0], f.payload[1..].to_vec())
    }

    /// Time passes (`ms`), the session pushing events: each event's payload (kind first).
    fn events(&mut self, ms: u32) -> Vec<Vec<u8>> {
        let mut out = Wire(Vec::new());
        let end = self.now + ms;
        while self.now < end {
            self.now += 50;
            self.link.events(&mut self.app, &self.st, &self.dev, &self.live, self.now, Some(&mut self.ui), &mut out);
        }
        out.0
            .split(|&b| b == 0)
            .filter(|f| !f.is_empty())
            .map(|f| {
                let mut f = f.to_vec();
                let p = wire::parse(&mut f).unwrap();
                assert_eq!(p.ty, msg::EVENT);
                p.payload.to_vec()
            })
            .collect()
    }

    /// A second of the sample's APRS traffic (APRS mode on).
    fn aprs_second(&mut self) {
        self.now += 1000;
        self.st.aprs_tick(self.now / 1000, true, true);
    }

    fn aprs_rec(&mut self) -> Vec<u8> {
        let (s, a) = self.ask(msg::READ, &[what::APRS, 0, 0, 0, 0, 200]);
        assert_eq!(s, 0);
        assert_eq!(u16::from_le_bytes([a[0], a[1]]) as usize, APRS_REC);
        a[2..].to_vec()
    }

    fn aprs_write(&mut self, rec: &[u8]) -> (u8, Vec<u8>) {
        let mut p = std::vec![what::APRS, 0, 0, 0, 0];
        p.extend_from_slice(&(rec.len() as u16).to_le_bytes());
        p.extend_from_slice(rec);
        self.ask(msg::WRITE, &p)
    }
}

/// LINK-43: the APRS settings read and written as one record, each value checked as MENU 0 5's editors check it;
/// a refused record changes nothing; the region's frequency is kept as "the region's".
#[test]
fn aprs_settings_record() {
    let mut pc = Pc::new(true);
    let rec = pc.aprs_rec();
    let c = pc.app.aprs_cfg();
    assert_eq!(&rec[..8], &[0, 0, c.beacon, c.power, c.path, c.digi, 0, c.ssid]);
    let mut new = rec.clone();
    new[8..15].copy_from_slice(b"WIDE2-2");
    new[4] = crate::aprs::PATH_CUSTOM;
    (new[7], new[3], new[5]) = (9, 2, 0x31); // SSID 9, power high, digipeater on, 3 s
    assert_eq!(pc.aprs_write(&new).0, 0);
    let c = pc.app.aprs_cfg();
    assert_eq!((c.ssid, c.power, c.digi_on(), c.digi_wait_s(), c.custom_text(), c.path), (9, 2, true, 3, &b"WIDE2-2"[..], crate::aprs::PATH_CUSTOM));
    assert_eq!(pc.aprs_rec(), new);
    let fx = super::APRS_V1 - 1;
    // APRS-45: FX.25 on transmit, after the custom path (16 by default); a record without it keeps the radio's.
    assert_eq!(rec[fx], crate::aprs::FX25_DEFAULT);
    new[fx] = 64;
    assert_eq!(pc.aprs_write(&new).0, 0);
    assert_eq!(pc.app.aprs_cfg().fx25, 64);
    new[fx] = 32;
    assert_eq!(pc.aprs_write(&new[..fx]).0, 0);
    assert_eq!(pc.app.aprs_cfg().fx25, 64, "a short record keeps it");
    new[fx] = 64;
    // Protocol 4 (decision 347): every MENU 0 5 value is in the record, read and written.
    let v2 = super::APRS_V1;
    (new[v2], new[v2 + 1], new[v2 + 2], new[v2 + 3]) = (2, 3, 50, 1); // sound, symbol, TX delay, TX level
    new[v2 + 4..v2 + 4 + 9].copy_from_slice(b"Ferrite!!");
    let end = v2 + 4 + crate::aprs::COMMENT_LEN;
    (new[end], new[end + 1], new[end + 2]) = (2, 3, 1); // ambiguity, Mic-E, telemetry
    assert_eq!(pc.aprs_write(&new).0, 0);
    let c = pc.app.aprs_cfg();
    assert_eq!((c.sound, c.symbol, c.txdelay, c.tx_level, &c.comment[..9], c.ambiguity, c.mic_e, c.telemetry), (2, 3, 50, 1, &b"Ferrite!!"[..], 2, 3, true));
    assert_eq!(pc.aprs_rec(), new);
    let bad2: [(usize, u8); 7] = [(v2, 6), (v2 + 1, 10), (v2 + 2, 5), (v2 + 3, 3), (v2 + 4, 7), (end, 5), (end + 2, 2)];
    for (at, v) in bad2 {
        let mut b = new.clone();
        b[at] = v;
        assert_eq!(pc.aprs_write(&b).0, Status::Invalid as u8, "byte {at} = {v}");
        assert_eq!(pc.aprs_rec(), new, "unchanged after byte {at}");
    }
    let bad: [(usize, u8, &str); 7] = [(7, 16, "SSID"), (3, 3, "power"), (2, 40, "beacon"), (5, 0x61, "digipeater"), (8, b'w', "custom"), (4, 9, "path"), (fx, 20, "FX.25")];
    for (at, v, why) in bad {
        let mut b = new.clone();
        b[at] = v;
        let (s, text) = pc.aprs_write(&b);
        assert_eq!(s, Status::Invalid as u8, "{why}");
        assert!(core::str::from_utf8(&text).unwrap().contains(why) || why == "beacon" || why == "path", "{why}: {text:?}");
        assert_eq!(pc.aprs_rec(), new, "unchanged after {why}");
    }
    // 144.390 (the US region's) is kept as the region's; 300.005 MHz is past the range
    let mut b = new.clone();
    b[..2].copy_from_slice(&(14_439_000u32 / 500).to_le_bytes()[..2]);
    pc.app.set.region = 0; // US
    assert_eq!(pc.aprs_write(&b).0, 0);
    assert_eq!(pc.app.aprs_cfg().f5k, 0);
    b[..2].copy_from_slice(&(30_000_500u32 / 500).to_le_bytes()[..2]);
    assert_eq!(pc.aprs_write(&b).0, Status::Invalid as u8);
    assert_eq!(pc.aprs_write(&b[..20]).0, Status::BadArg as u8, "the whole record");
    // APRS-50.1 (0.8.5): the beacon text rate after auto reply; a record without it keeps the radio's
    let t = crate::link::APRS_TEXT_AT;
    let mut b = pc.aprs_rec();
    assert_eq!(b.len(), crate::link::APRS_REC);
    b[t] = 2; // every 4th
    assert_eq!(pc.aprs_write(&b).0, 0);
    assert_eq!(pc.app.aprs_cfg().text_every, 2);
    assert_eq!(pc.aprs_rec()[t], 2);
    assert_eq!(pc.aprs_write(&b[..t]).0, 0);
    assert_eq!(pc.app.aprs_cfg().text_every, 2, "a record without it keeps it");
    b[t] = 4;
    assert_eq!(pc.aprs_write(&b).0, Status::Invalid as u8, "text rate past every 8th");
    b[t] = 2;
    // APRS-13.6 (0.9.6): message retries, the record's last byte; a record without it (0.9.5's Console) keeps them
    let r = crate::link::APRS_RETRIES_AT;
    assert_eq!((r, b.len()), (t + 1, r + 1));
    assert_eq!(b[r], crate::aprs::RETRIES_DEFAULT, "four by default");
    b[r] = 0; // off: sent once
    assert_eq!(pc.aprs_write(&b).0, 0);
    assert_eq!((pc.app.aprs_cfg().retries, pc.app.aprs_cfg().tries()), (0, 1));
    assert_eq!(pc.aprs_rec()[r], 0);
    b[t] = 1;
    assert_eq!(pc.aprs_write(&b[..r]).0, 0, "0.9.5's record");
    assert_eq!((pc.app.aprs_cfg().text_every, pc.app.aprs_cfg().retries), (1, 0), "its text rate taken, the retries kept");
    b[r] = 5;
    assert_eq!(pc.aprs_write(&b).0, Status::Invalid as u8, "retries past 4");
}

/// LINK-44: the heard list and the messages read in answers that hold whole records, from any index.
#[test]
fn heard_and_messages() {
    let mut pc = Pc::new(false);
    let (s, a) = pc.ask(msg::HEARD, &[0]);
    assert_eq!(s, 0);
    let n = a[0] as u16;
    assert_eq!(n, pc.st.stations());
    let first = pc.st.station(0).unwrap();
    assert_eq!(a[1] & 1, first.pos.is_some() as u8);
    assert_eq!(&a[3..3 + a[2] as usize], first.call.as_bytes());
    let (_, b) = pc.ask(msg::HEARD, &[1]);
    let second = pc.st.station(1).unwrap();
    assert_eq!(&b[3..3 + b[2] as usize], second.call.as_bytes());
    let (s, m) = pc.ask(msg::MSGS, &[0, 0]);
    assert_eq!(s, 0);
    assert_eq!(u16::from_le_bytes([m[0], m[1]]), pc.st.messages());
    let newest = pc.st.message(0).unwrap();
    assert_eq!(m[2], newest.inbound as u8);
    let peer = &m[10..10 + m[9] as usize];
    assert_eq!(peer, newest.peer.as_bytes());
    // past the end: the count, nothing more
    assert_eq!(pc.ask(msg::MSGS, &[200, 0]).1.len(), 2);
}

fn send_payload(to: &[u8], text: &[u8]) -> Vec<u8> {
    let mut p = std::vec![to.len() as u8];
    p.extend_from_slice(to);
    p.push(text.len() as u8);
    p.extend_from_slice(text);
    p
}

/// LINK-45: a message sent from the PC is queued as compose queues it (the radio's own draft kept), says whether
/// APRS mode will send it now; refused on a receive-only build, without a callsign, or with a bad address or
/// text.
#[test]
fn message_send() {
    let mut pc = Pc::new(false);
    let draft = Draft { to: Buf::from(b"VE7ABC"), text: Buf::from(b"half written") };
    pc.st.request(Op::Draft(draft));
    let n = pc.st.messages();
    let (s, a) = pc.ask(msg::MSG_SEND, &send_payload(b"ve3kpw-9", b"On my way"));
    assert_eq!(s, 0, "{a:?}");
    assert_eq!(a, [pc.app.aprs_on() as u8]);
    assert_eq!(pc.st.messages(), n + 1);
    let m = pc.st.message(0).unwrap();
    assert_eq!((m.inbound, m.peer.as_bytes(), m.text.as_bytes()), (false, &b"VE3KPW-9"[..], &b"On my way"[..]));
    assert!(pc.st.draft() == draft, "the radio's draft kept");
    for (to, text) in [(&b""[..], &b"hi"[..]), (b"VE3KPW-10X", b"hi"), (b"VE3 KPW", b"hi"), (b"VE3KPW", b""), (b"VE3KPW", b"a {1"), (b"VE3KPW", &[b'x'; 68][..])] {
        assert_eq!(pc.ask(msg::MSG_SEND, &send_payload(to, text)).0, Status::Invalid as u8, "{to:?} {text:?}");
    }
    assert_eq!(pc.ask(msg::MSG_SEND, &[3, b'A']).0, Status::BadArg as u8);
    pc.st.call.set(None);
    let (s, why) = pc.ask(msg::MSG_SEND, &send_payload(b"VE3KPW", b"hi"));
    assert_eq!((s, &why[..12]), (Status::Invalid as u8, &b"No callsign:"[..]));
    pc.dev.tx_built = false;
    let (s, why) = pc.ask(msg::MSG_SEND, &send_payload(b"VE3KPW", b"hi"));
    assert_eq!((s, &why[..12]), (Status::Invalid as u8, &b"receive-only"[..]));
    assert_eq!(pc.st.messages(), n + 1, "nothing more queued");
}

/// LINK-46: subscribed, the session is told of stations heard, messages in and their states as they happen,
/// nothing before them; unsubscribed or quiet for 10 s, nothing.
#[test]
fn aprs_events() {
    let mut pc = Pc::new(false);
    let (s, a) = pc.ask(msg::SUBSCRIBE, &[ev::SUB_HEARD | ev::SUB_MSGS]);
    assert_eq!((s, a), (0, std::vec![ev::SUB_HEARD | ev::SUB_MSGS, DICT_VERSION]));
    assert!(pc.events(500).is_empty(), "nothing has happened yet");
    let (mut heard, mut ins, mut states) = (0, 0, 0);
    for _ in 0..45 {
        pc.aprs_second(); // the sample hears a station at 8 s, a message comes at 40 s
        // the PC keeps the session alive (any request does)
        pc.ask(msg::SUBSCRIBE, &[ev::SUB_HEARD | ev::SUB_MSGS, 0, ev::RENEW]);
        for e in pc.events(300) {
            match e[0] {
                ev::HEARD => {
                    heard += 1;
                    let s = pc.st.station(0).unwrap();
                    assert_eq!(e[1] as u16, pc.st.stations());
                    assert_eq!(&e[4..4 + e[3] as usize], s.call.as_bytes());
                }
                ev::MSG => {
                    let m = pc.st.message(0).unwrap();
                    assert_eq!((e[3], &e[11..11 + e[10] as usize]), (m.inbound as u8, m.peer.as_bytes()));
                    ins += 1;
                }
                ev::MSG_STATE => states += 1,
                k => panic!("kind {k}"),
            }
        }
    }
    assert!(heard >= 1 && ins >= 1, "heard {heard}, messages {ins}, states {states}");
    // our own message, sent from the PC: a MSG event for it
    pc.ask(msg::MSG_SEND, &send_payload(b"VE3KPW", b"Test"));
    let e = pc.events(200);
    assert!(e.iter().any(|e| e[0] == ev::MSG && e[3] == 0), "{e:?}");
    // the PC goes quiet: events stop
    pc.now += ev::QUIET_MS;
    pc.aprs_second();
    pc.aprs_second();
    assert!(pc.events(2000).is_empty());
    assert_eq!(pc.ask(msg::SUBSCRIBE, &[0x40]).0, Status::BadArg as u8);
}

#[derive(Default)]
struct Screen(Vec<Vec<u8>>, Option<u8>);

impl Lines for Screen {
    fn count(&self) -> usize {
        self.0.len()
    }
    fn set_count(&mut self, n: usize) {
        self.0.resize(n, Vec::new());
    }
    fn line(&self, i: usize) -> &[u8] {
        &self.0[i]
    }
    fn set_line(&mut self, i: usize, t: &[u8]) {
        self.0[i] = t.to_vec();
    }
}

impl Screen {
    fn take(&mut self, e: &[u8]) {
        assert_eq!(e[0], ev::UI);
        self.1 = Some(liveview::apply(self, &e[1..], self.1).unwrap());
    }
}

/// The readback now, as the compact form has it.
fn compacted(pc: &Pc) -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    let mut l = Line::new();
    while crate::uistate::line(&pc.app, &pc.st, &pc.dev, &pc.live, v.len() as u16, &mut l) {
        let mut b = [0u8; 256];
        let n = liveview::compact(l.as_bytes(), &mut b);
        v.push(b[..n].to_vec());
    }
    v
}

/// LINK-47, LINK-48: subscribed to the UI, the session gets the whole compact state, then a delta when a key
/// changes the screen (at once, not waiting for the gap), each applying exactly; the cable's rate cap holds.
#[test]
fn live_view_events() {
    let mut pc = Pc::new(true);
    pc.ask(msg::SUBSCRIBE, &[ev::SUB_UI, 5]);
    let mut scr = Screen::default();
    for e in pc.events(200) {
        scr.take(&e);
    }
    assert_eq!(scr.0, compacted(&pc));
    // nothing changes: nothing sent
    assert!(pc.events(1000).is_empty());
    // a key: the delta within 100 ms
    pc.app.key_down(Key::Down, pc.now, &pc.st);
    pc.app.key_up(Key::Down, pc.now + 50, &pc.st);
    pc.ask(msg::SUBSCRIBE, &[ev::SUB_UI, 5, ev::RENEW]);
    let e = pc.events(150);
    assert!(!e.is_empty(), "a delta after the key");
    for e in e {
        scr.take(&e);
    }
    assert_eq!(scr.0, compacted(&pc));
    // MENU: a whole new screen, in more than one event within the cap
    pc.app.key_down(Key::Menu, pc.now, &pc.st);
    pc.app.key_up(Key::Menu, pc.now + 50, &pc.st);
    for e in pc.events(1000) {
        scr.take(&e);
    }
    assert_eq!(scr.0, compacted(&pc));
}

/// APRS-34, LINK-44: a rejected message reads as state 6, a value of its own (0-5 keep their meaning, so an
/// older reader shows it as an unknown state rather than another one).
#[test]
fn rejected_state_byte() {
    assert_eq!(state_byte(MsgState::Rejected), (6, 0));
    assert_eq!(state_byte(MsgState::Failed), (5, 0));
}

/// LINK-49, APRS-13.1: `MSG_STOP` stops one of ours still being tried, as the radio's Stop sending does: kept as
/// cancelled; `invalid` when none to that station with that text is being tried; `bad argument` when malformed.
#[test]
fn message_stop() {
    let mut pc = Pc::new(false);
    assert_eq!(pc.ask(msg::MSG_SEND, &send_payload(b"VE3KPW-9", b"On my way")).0, 0);
    let (s, a) = pc.ask(msg::MSG_STOP, &send_payload(b"ve3kpw-9", b"On my way"));
    assert_eq!(s, 0, "{a:?}");
    assert_eq!(pc.st.message(0).map(|m| m.state), Some(MsgState::Cancelled));
    let (s, why) = pc.ask(msg::MSG_STOP, &send_payload(b"VE3KPW-9", b"On my way"));
    assert_eq!((s, &why[..]), (Status::Invalid as u8, &b"not being sent"[..]));
    assert_eq!(pc.ask(msg::MSG_STOP, &[3, b'A']).0, Status::BadArg as u8);
}

/// LINK-52, APRS-46.1: `MSG_DELETE` deletes the newest message that way with that station and text, as the radio's
/// Delete message does; `invalid` when there is none; `bad argument` when malformed.
#[test]
fn message_delete() {
    let mut pc = Pc::new(false);
    let n = pc.st.messages();
    let del = |inbound: bool, call: &[u8], text: &[u8]| [&[inbound as u8][..], &send_payload(call, text)].concat();
    let (s, a) = pc.ask(msg::MSG_DELETE, &del(false, b"VE3KPW-9", b"On my way"));
    assert_eq!((s, &a[..]), (Status::Invalid as u8, &b"no such message"[..]), "that one is to us, not ours");
    let (s, a) = pc.ask(msg::MSG_DELETE, &del(true, b"ve3kpw-9", b"On my way"));
    assert_eq!(s, 0, "{a:?}");
    assert_eq!(pc.st.messages(), n - 1);
    assert!((0..pc.st.messages()).all(|i| pc.st.message(i).is_some_and(|m| m.text.as_bytes() != b"On my way")));
    assert_eq!(pc.ask(msg::MSG_DELETE, &del(true, b"VE3KPW-9", b"On my way")).0, Status::Invalid as u8, "gone");
    assert_eq!(pc.ask(msg::MSG_DELETE, &[1, 3, b'A']).0, Status::BadArg as u8);
}

/// LINK-57, INBOX-11: subscribed to the event log, the session is sent each event logged after the subscription, oldest
/// first, as `LOG_READ` gives it (its sequence number first, the next after the one before); none of the ones
/// before it; nothing without the bit.
#[test]
fn event_log_events() {
    let mut pc = Pc::new(true);
    let (s, a) = pc.ask(msg::SUBSCRIBE, &[ev::SUB_LOG]);
    assert_eq!((s, a[0]), (0, ev::SUB_LOG));
    assert!(pc.events(500).is_empty(), "the events logged before aren't sent");
    let top = pc.st.event_seq(0).unwrap() + 1;
    pc.st.log_dtmf();
    pc.st.log_dtmf();
    pc.st.log_dtmf();
    let mut got = Vec::new();
    for _ in 0..4 {
        for e in pc.events(300) {
            assert_eq!(e[0], ev::LOG);
            let seq = u32::from_le_bytes(e[1..5].try_into().unwrap());
            assert_eq!(e[9], crate::data::Src::Dtmf as u8, "the source");
            let (nl, at) = (e[1 + ferrite_core::link::LOG_FIXED] as usize, 2 + ferrite_core::link::LOG_FIXED);
            let tl = e[at + nl] as usize;
            got.push((seq, std::string::String::from_utf8(e[at + nl + 1..at + nl + 1 + tl].to_vec()).unwrap()));
        }
        pc.ask(msg::SUBSCRIBE, &[ev::SUB_LOG, 0, ev::RENEW]);
    }
    let want: Vec<(u32, std::string::String)> = (0..3).map(|k| (top + k, std::format!("*7{k}#"))).collect();
    assert_eq!(got, want, "oldest first, each once");
    // without the bit: nothing
    pc.ask(msg::SUBSCRIBE, &[ev::SUB_HEARD]);
    pc.st.log_dtmf();
    assert!(pc.events(500).is_empty());
}

/// LINK-13.6 (decision 331): while the cable's session listens, its events (here a Live View drawn as the screen
/// changes every 100 ms) stay within `RATE_CABLE_LISTEN`, so the audio keeps its share of the cable.
#[test]
fn listening_slows_the_events() {
    let bytes = |listen: bool| {
        let mut pc = Pc::new(true);
        if listen {
            assert_eq!(pc.ask(msg::AUDIO, &[1]).0, 0);
        }
        pc.ask(msg::SUBSCRIBE, &[ev::SUB_UI, 1, ev::TEXTS]);
        let mut out = Wire(Vec::new());
        let start = pc.now;
        let keys = [Key::Menu, Key::Down, Key::Down, Key::Exit, Key::Up, Key::Exit];
        for i in 0..60u32 {
            if i % 20 == 19 {
                pc.ask(msg::SUBSCRIBE, &[ev::SUB_UI, 1, ev::TEXTS | ev::RENEW]); // the PC keeps it going
            }
            let k = keys[i as usize % keys.len()];
            pc.app.key_down(k, pc.now, &pc.st);
            pc.app.key_up(k, pc.now + 20, &pc.st);
            for _ in 0..10 {
                pc.now += 10;
                pc.link.events(&mut pc.app, &pc.st, &pc.dev, &pc.live, pc.now, Some(&mut pc.ui), &mut out);
            }
        }
        (out.0.len() as u32, pc.now - start)
    };
    let (quiet, ms) = bytes(true);
    assert!(quiet <= ev::RATE_CABLE_LISTEN * ms / 1000 + ev::BURST + MAX_WIRE as u32, "{quiet} B in {ms} ms while listening");
    let (busy, _) = bytes(false);
    assert!(busy > quiet, "without audio the events take more: {busy} against {quiet}");
}

/// LINK-43, APRS-57: auto reply at the record's end (0.8.2): read and written; a bad pattern, reply or delay is
/// refused and changes nothing; an empty pattern reads as everyone; the record before it keeps the radio's.
#[test]
fn aprs_record_carries_auto_reply() {
    let mut pc = Pc::new(true);
    let at = super::APRS_REPLY_AT;
    let mut rec = pc.aprs_rec();
    assert_eq!((rec[at], &rec[at + 1..at + 2]), (0, &b"*"[..]), "off, everyone");
    rec[at] = 0x21; // on, 1 min
    rec[at + 1..at + 1 + crate::aprs::REPLY_TO_LEN].copy_from_slice(b"VE7*\0\0\0\0\0");
    let text = at + 1 + crate::aprs::REPLY_TO_LEN;
    rec[text..text + 4].copy_from_slice(b"Away");
    assert_eq!(pc.aprs_write(&rec).0, 0);
    let c = pc.app.aprs_cfg();
    assert!(c.reply_on() && c.reply_delay_s() == 60 && c.reply_to_text() == b"VE7*" && c.reply_text() == b"Away");
    assert_eq!(pc.aprs_rec(), rec, "read back as written");
    for (i, v, why) in [(at, 0x41u8, "delay"), (at + 1, b'v', "lower case"), (at + 1, b'/', "a slash"), (text, 0x07, "a control character")] {
        let mut bad = rec.clone();
        bad[i] = v;
        let (s, a) = pc.aprs_write(&bad);
        assert_ne!(s, 0, "{why} refused");
        assert!(core::str::from_utf8(&a).unwrap().contains("auto reply"), "{why}: says what");
        assert!(pc.app.aprs_cfg().reply_text() == b"Away", "{why}: nothing changed");
    }
    let mut empty = rec.clone();
    empty[at + 1..at + 1 + crate::aprs::REPLY_TO_LEN].fill(0);
    assert_eq!(pc.aprs_write(&empty).0, 0);
    assert_eq!(pc.app.aprs_cfg().reply_to_text(), b"*", "an empty pattern is everyone");
    // the record before auto reply (Console 0.8.1) keeps the radio's
    assert_eq!(pc.aprs_write(&rec[..at]).0, 0);
    assert!(pc.app.aprs_cfg().reply_on() && pc.app.aprs_cfg().reply_text() == b"Away");
}

/// LINK-67.1: subscribed to the radio state, the session gets the whole record once, then only the parts that
/// changed (a slot's squelch opening: that slot's part alone), at most every gap; nothing while nothing changes.
#[test]
fn radio_state_events() {
    use ferrite_core::link::state as m;
    let mut pc = Pc::new(true);
    assert_eq!(pc.ask(msg::SUBSCRIBE, &[ev::SUB_STATE, 2]).0, 0);
    let e = pc.events(300);
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!((e[0][0], e[0][1], e[0].len()), (ev::STATE, m::PART_ALL, 2 + m::LEN));
    let (_, rec) = pc.ask(msg::STATE, &[]);
    assert_eq!(&e[0][2..], &rec[..]);
    pc.ask(msg::SUBSCRIBE, &[ev::SUB_STATE, 2, ev::RENEW]);
    assert!(pc.events(1000).is_empty(), "nothing changed");
    // slot B hears a signal: B's part alone, with the signal
    (pc.live.slots[1].squelch_open, pc.live.slots[1].rssi) = (true, -93);
    pc.ask(msg::SUBSCRIBE, &[ev::SUB_STATE, 2, ev::RENEW]);
    let e = pc.events(300);
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!((e[0][1], e[0].len()), (m::PART_SLOT << 1, 2 + m::SLOT_LEN));
    assert_eq!(e[0][2] & m::OPEN, m::OPEN);
    assert_eq!(i16::from_le_bytes([e[0][13], e[0][14]]), -93);
}

/// LINK-71: `HELLO` ends with the lists generation; subscribed to the lists, a burst of channel writes is
/// one event once writing has been quiet for 500 ms (what 2, the generation `HELLO` then gives), a zone renamed
/// another (what 3); a write that changes no list (a message) says nothing.
#[test]
fn lists_generation_and_events() {
    use crate::chan::{channel, ChanWrite, NO_ZONE};
    use ferrite_core::link::PROTOCOL;
    use crate::data::Op;
    let mut pc = Pc::new(false);
    let gen_of = |a: &[u8]| {
        let n = a.len();
        u32::from_le_bytes([a[n - 4], a[n - 3], a[n - 2], a[n - 1]])
    };
    let (_, h) = pc.ask(msg::HELLO, &[PROTOCOL]);
    let g0 = gen_of(&h);
    assert_eq!(pc.ask(msg::SUBSCRIBE, &[ev::SUB_LISTS]).0, 0);
    assert!(pc.events(1000).is_empty(), "nothing stored: nothing to say");
    for n in 0..20u16 {
        pc.st.apply(ChanWrite::Put { ch: channel(500 + n, NO_ZONE, 14_600_000 + n as u32 * 1250, "Ch", crate::slot::DEFAULT_CFG), from: None });
    }
    let e = pc.events(200);
    assert!(e.is_empty(), "not before the writing has been quiet 500 ms: {e:?}");
    let e = pc.events(600);
    assert_eq!(e.len(), 1, "one event for the burst: {e:?}");
    assert_eq!(e[0][..2], [ev::LISTS, what::CHANNEL]);
    let g1 = u32::from_le_bytes([e[0][2], e[0][3], e[0][4], e[0][5]]);
    assert_ne!(g1, g0);
    assert!(pc.st.request(Op::RenameZone(0, crate::chan::Name::from(b"Renamed".as_slice()))));
    let e = pc.events(1000);
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!(e[0][1], what::ZONE);
    let g2 = u32::from_le_bytes([e[0][2], e[0][3], e[0][4], e[0][5]]);
    assert_ne!(g2, g1);
    pc.ask(msg::MSG_SEND, &send_payload(b"VE3KPW", b"Test"));
    assert!(!pc.events(1000).iter().any(|e| e[0] == ev::LISTS), "a message isn't a list");
    let (_, h) = pc.ask(msg::HELLO, &[PROTOCOL]);
    assert_eq!(gen_of(&h), g2, "HELLO gives the generation the last event did");
}

/// LINK-72: `PHONE_POS` takes 14 bytes (latitude, longitude, altitude, accuracy, age); a short one is a bad argument,
/// a latitude past 90 degrees invalid. (What the radio does with it: gpsdata's GPS-3.1 test.)
#[test]
fn phone_position_message() {
    let mut pc = Pc::new(false);
    let mut p = std::vec::Vec::new();
    p.extend_from_slice(&491_000_000i32.to_le_bytes());
    p.extend_from_slice(&(-1_231_000_000i32).to_le_bytes());
    p.extend_from_slice(&0x7FFFi16.to_le_bytes());
    p.extend_from_slice(&8u16.to_le_bytes());
    p.extend_from_slice(&5u16.to_le_bytes());
    assert_eq!(pc.ask(msg::PHONE_POS, &p).0, 0);
    assert_eq!(pc.ask(msg::PHONE_POS, &p[..13]).0, Status::BadArg as u8);
    p[..4].copy_from_slice(&910_000_000i32.to_le_bytes());
    assert_eq!(pc.ask(msg::PHONE_POS, &p).0, Status::Invalid as u8);
}
