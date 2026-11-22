//! The two remote ports (roadmap 3d): the terminal and the link on Bluetooth, the cable's priority for the
//! terminal, a link session on each port at once (LINK-28), KISS beside them, and the PIN Bluetooth gives
//! (TERM-25, LINK-7, LINK-30).
#![cfg_attr(not(feature = "line-cmds"), allow(unused))] // helpers for the tests gated on the line commands

use crate::link;
use crate::chan::ChannelStore;
use ferrite_core::link::what;
use crate::ports::{Port, Ports, To, BT_IDLE_MS};
use crate::sample::Sample;
use crate::term::Sink;
use crate::{App, Screen};
use ferrite_core::link::{self as wire, msg, Status, MAX_WIRE};
use ferrite_core::msg::{Device, LiveAll};
use std::vec::Vec;

#[derive(Default)]
struct Out(Vec<u8>);

impl Sink for Out {
    fn room(&self) -> usize {
        8192
    }
    fn put(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
}

struct Rig {
    app: App,
    st: Sample,
    p: Ports,
    now: u32,
    cable: Out,
    bt: Out,
}

impl Rig {
    fn new() -> Self {
        let mut app = App::new(crate::tests::set(), crate::tests::alert());
        while app.pop_cmd().is_some() {}
        app.set.bt = true;
        Rig { app, st: Sample::new(), p: Ports::new(), now: 1000, cable: Out::default(), bt: Out::default() }
    }

    /// Bytes from a port, each routed and fed; then a few polls. What each byte was routed to.
    fn send(&mut self, port: Port, bytes: &[u8]) -> Vec<To> {
        let mut to = Vec::new();
        for &b in bytes {
            let t = self.p.route(&self.app, port, b, self.now);
            to.push(t);
            self.p.feed(&mut self.app, &self.st, &Default::default(), &Default::default(), t, b, self.now);
            while let Some(w) = self.app.pop_write() {
                self.st.apply(w);
            }
            self.poll();
        }
        for _ in 0..3 {
            self.poll();
        }
        to
    }

    fn poll(&mut self) {
        self.now += 10;
        self.p.poll(&mut self.app, &self.st, &Device::default(), &LiveAll::default(), self.now, &mut self.cable, &mut self.bt);
        while let Some(w) = self.app.pop_write() {
            self.st.apply(w);
        }
    }

    /// One link request from `port`: its answer's status.
    fn ask(&mut self, port: Port, ty: u8, payload: &[u8]) -> u8 {
        self.ask_full(port, ty, payload).0
    }

    /// One link request from `port`: its answer's status and the rest of the payload; nothing reaches the other port.
    fn ask_full(&mut self, port: Port, ty: u8, payload: &[u8]) -> (u8, Vec<u8>) {
        let before = if port == Port::Bt { self.cable.0.len() } else { self.bt.0.len() };
        let mut w = [0u8; MAX_WIRE];
        let n = wire::build(ty, 1, payload, &mut w).unwrap();
        let mut f = std::vec![0u8];
        f.extend_from_slice(&w[..n]);
        self.send(port, &f);
        let out = if port == Port::Bt { &mut self.bt } else { &mut self.cable };
        let mut a = core::mem::take(&mut out.0);
        assert_eq!(a.last(), Some(&0), "one delimited answer");
        let k = a.len() - 1;
        let p = wire::parse(&mut a[..k]).unwrap().payload.to_vec();
        let after = if port == Port::Bt { self.cable.0.len() } else { self.bt.0.len() };
        assert_eq!(before, after, "the answer went to the asking port only");
        (p[0], p[1..].to_vec())
    }
}

/// TERM-25: the terminal opens on Bluetooth with Enter and draws there, not on the cable; with no PIN lock it
/// reads and writes at once, with no prompt on the radio (LINK-7, decision 104).
#[cfg(feature = "line-cmds")]
#[test]
fn bluetooth_terminal_without_pin() {
    let mut r = Rig::new();
    r.send(Port::Bt, b"\r");
    assert_eq!(r.p.owner(), Port::Bt);
    assert!(r.p.term.on());
    assert!(!r.bt.0.is_empty() && r.cable.0.is_empty(), "drawn on Bluetooth only");
    r.send(Port::Bt, b":home 3\r");
    assert_eq!(r.app.set.home_ch, Some(2), "written, no approval");
    assert!(!matches!(r.app.screen(), Screen::Link(_)), "no prompt on the radio");
    assert!(crate::term::cmd::writes(b"home 3") && !crate::term::cmd::writes(b"get 0 9 2 1") && !crate::term::cmd::writes(b"ch 12"));
    assert!(crate::term::cmd::writes(b"bk 0 30 0") && crate::term::cmd::writes(b"ch 12 \"X\" 146.52"));
}

/// LINK-30, LINK-31: with the PIN lock on, a Bluetooth terminal asks for the radio's PIN before anything else
/// (shown as *, nothing else read); wrong ones count toward the radio's own wait (PIN-3); the cable never asks.
#[cfg(feature = "line-cmds")]
#[test]
fn bluetooth_terminal_asks_for_the_pin() {
    let mut r = Rig::new();
    (r.app.set.pin, r.app.set.pin_len, r.app.set.pin_mode) = (2468, 4, 1);
    r.send(Port::Bt, b"\r");
    assert!(r.p.term.on());
    let shown = std::string::String::from_utf8_lossy(&r.bt.0).into_owned();
    assert!(shown.contains("PIN"), "the prompt: {shown}");
    let home = r.app.set.home_ch;
    r.send(Port::Bt, b":home 3\r");
    assert_eq!(r.app.set.home_ch, home, "nothing runs before the PIN");
    assert_eq!(r.app.set.pin_misses, 1, "only its digit was read, as a wrong PIN");
    r.send(Port::Bt, b"1111\r");
    assert_eq!(r.app.set.pin_misses, 2, "a wrong PIN counts on the radio");
    r.bt.0.clear();
    r.send(Port::Bt, b"2468\r");
    assert_eq!(r.app.set.pin_misses, 0);
    r.send(Port::Bt, b":home 3\r");
    assert_eq!(r.app.set.home_ch, Some(2), "runs after the PIN");
    // the cable takes over: never asked
    let mut c = Rig::new();
    (c.app.set.pin, c.app.set.pin_len, c.app.set.pin_mode) = (2468, 4, 1);
    c.send(Port::Cable, b"\r");
    c.send(Port::Cable, b":home 3\r");
    assert_eq!(c.app.set.home_ch, Some(2), "the cable is trusted");
}

/// The cable always takes the terminal (physical access, LINK-7); while it has it, Bluetooth's terminal bytes are
/// dropped, and Bluetooth gets it back once the cable's terminal has closed (LINK-28).
#[test]
fn the_cable_wins() {
    let mut r = Rig::new();
    r.send(Port::Bt, b"\r");
    assert_eq!(r.p.owner(), Port::Bt);
    r.send(Port::Cable, b"\r");
    assert_eq!(r.p.owner(), Port::Cable, "the cable took it");
    assert!(r.p.term.on() && !r.cable.0.is_empty());
    let to = r.send(Port::Bt, b"x");
    assert_eq!(to, [To::Drop]);
    assert_eq!(r.p.owner(), Port::Cable);
    r.send(Port::Cable, &[0x03]); // Ctrl-C closes the cable's terminal
    r.poll();
    assert!(!r.p.term.on());
    r.send(Port::Bt, b"\r");
    assert_eq!(r.p.owner(), Port::Bt, "free again: Bluetooth takes it");
}

/// KISS-6: with 0 3 7 on Bluetooth, Bluetooth's bytes are KISS while the cable keeps its terminal; on Cable,
/// the cable's are.
#[test]
fn kiss_beside_the_terminal() {
    let mut r = Rig::new();
    r.app.set.bt = true;
    r.app.set.kiss_port = 2;
    r.app.open(crate::screens::nav::Dest::KissMode);
    r.send(Port::Cable, b"\r");
    assert!(r.p.term.on());
    assert_eq!(r.send(Port::Bt, &[0xC0, 0x00, 0x82]), [To::Kiss; 3]);
    assert!(r.p.term.on() && r.p.owner() == Port::Cable, "the cable's terminal stays");
    r.app.set.kiss_port = 1;
    assert_eq!(r.send(Port::Cable, &[0xC0]), [To::Kiss]);
}

/// LINK-29: a Bluetooth connection quiet for BT_IDLE_MS is over: its terminal closes, its PIN ends, the cable has
/// the terminal again, and turning Bluetooth off ends it at once; LINK-15.1: Bluetooth's `BAUD` takes only the
/// module's two rates, 9600 (LINK-25) and the unlocked 115200, and never moves the cable.
#[test]
fn bluetooth_goes_quiet() {
    let mut r = Rig::new();
    assert_eq!(crate::ports::BT_BAUD, 9_600, "the module's resting rate (LINK-25)");
    for rate in ferrite_core::link::RATES.into_iter().filter(|&x| x != 115_200) {
        assert_eq!(r.ask(Port::Bt, msg::BAUD, &rate.to_le_bytes()), Status::Refused as u8, "{rate}: not a module rate");
    }
    assert_eq!(r.ask(Port::Bt, msg::BAUD, &115_200u32.to_le_bytes()), 0, "the unlocked rate");
    assert_eq!((r.p.take_bt_baud(), r.p.take_baud()), (Some(115_200), None), "Bluetooth's alone");
    assert_eq!(r.ask(Port::Bt, msg::BAUD, &9_600u32.to_le_bytes()), 0, "back to the resting rate");
    assert_eq!(r.p.take_bt_baud(), Some(9_600));
    let h = r.p.bt_hellos();
    assert_eq!(r.ask(Port::Bt, msg::HELLO, &[2, 0, 0, 0, 0, 0, 0, 0, 0]), 0);
    assert_eq!(r.p.bt_hellos(), h.wrapping_add(1), "the proof a new rate holds");
    (r.app.set.pin, r.app.set.pin_len, r.app.set.pin_mode) = (2468, 4, 1);
    assert_eq!(r.ask(Port::Bt, msg::AUTH, b"2468"), 0);
    assert_eq!(r.ask(Port::Bt, msg::INFO, &[]), 0, "past the PIN");
    r.send(Port::Bt, b"\r");
    assert!(r.p.bt_connected(&r.app, r.now));
    r.now += BT_IDLE_MS;
    r.poll();
    assert_eq!(r.p.owner(), Port::Cable);
    assert!(!r.p.term.on() && !r.p.bt_connected(&r.app, r.now));
    assert_eq!(r.ask(Port::Bt, msg::INFO, &[]), Status::NotAuthorised as u8, "a new connection gives the PIN again");
    (r.app.set.pin, r.app.set.pin_len, r.app.set.pin_mode) = (0, 0, 0);
    // Bluetooth off ends it at once
    r.send(Port::Bt, b"\r");
    r.app.set.bt = false;
    r.poll();
    assert_eq!(r.p.owner(), Port::Cable);
}

/// LINK-14: Bluetooth is off by default.
#[test]
fn bluetooth_off_by_default() {
    let set = core::hint::black_box(crate::settings::Settings::DEFAULT);
    assert!(!set.bt, "Bluetooth off by default");
}

/// LINK-28 (decision 152): a link session on the cable and one on Bluetooth at the same time, each answered on its
/// own port; a Bluetooth link session neither takes nor is blocked by the cable's terminal, and the cable's link
/// frames leave a Bluetooth terminal open.
#[cfg(feature = "line-cmds")]
#[test]
fn a_session_on_each_port() {
    let mut r = Rig::new();
    let hello = [ferrite_core::link::PROTOCOL];
    assert_eq!(r.ask(Port::Cable, msg::HELLO, &hello), 0);
    assert_eq!(r.ask(Port::Bt, msg::HELLO, &hello), 0);
    assert!(r.p.link_on(Port::Cable, r.now) && r.p.link_on(Port::Bt, r.now), "both running");
    assert_eq!(r.ask(Port::Cable, msg::INFO, &[]), 0);
    assert_eq!(r.ask(Port::Bt, msg::INFO, &[]), 0);
    // the cable's terminal open: Bluetooth's link goes on
    let mut c = Rig::new();
    c.app.set.cable_port = 2; // Terminal: the cable's bytes are the terminal's
    c.send(Port::Cable, b"\r");
    assert!(c.p.term.on() && c.p.owner() == Port::Cable);
    assert_eq!(c.ask(Port::Bt, msg::HELLO, &hello), 0, "Bluetooth's session beside the cable's terminal");
    assert!(c.p.term.on() && c.p.owner() == Port::Cable, "the terminal stays the cable's");
    // a Bluetooth terminal stays open through the cable's link frames
    let mut b = Rig::new();
    b.send(Port::Bt, b"\r");
    assert!(b.p.term.on() && b.p.owner() == Port::Bt);
    assert_eq!(b.ask(Port::Cable, msg::HELLO, &hello), 0);
    assert!(b.p.term.on() && b.p.owner() == Port::Bt, "the cable's session took nothing");
    // a CMD on each, at once: each gets its own output
    let (s, a) = b.ask_full(Port::Cable, msg::CMD, b"get 0 9 2 1");
    assert_eq!(s, 0);
    assert!(std::string::String::from_utf8_lossy(&a).contains("Bluetooth"), "{:?}", a);
}

/// LINK-28: writes go one port at a time. While the cable's write session runs (BEGIN .. END), or a record is half
/// written, Bluetooth's `BEGIN`, `WRITE` and `DELETE` answer `busy`, and the reverse; reads go on.
#[test]
fn writes_one_port_at_a_time() {
    let mut r = Rig::new();
    let hello = [ferrite_core::link::PROTOCOL];
    assert_eq!(r.ask(Port::Cable, msg::HELLO, &hello), 0);
    assert_eq!(r.ask(Port::Bt, msg::HELLO, &hello), 0);
    assert_eq!(r.ask(Port::Cable, msg::BEGIN, &1u16.to_le_bytes()), 0);
    assert_eq!(r.ask(Port::Bt, msg::BEGIN, &1u16.to_le_bytes()), Status::Busy as u8);
    assert_eq!(r.ask(Port::Bt, msg::DELETE, &[ferrite_core::link::what::CHANNEL, 0, 0]), Status::Busy as u8);
    let read = [ferrite_core::link::what::SETTINGS, 0, 0, 0, 0, 100];
    assert_eq!(r.ask(Port::Bt, msg::READ, &read), 0, "reads go on");
    assert_eq!(r.ask(Port::Cable, msg::END, &[]), 0);
    assert_eq!(r.ask(Port::Bt, msg::BEGIN, &1u16.to_le_bytes()), 0, "free again");
    assert_eq!(r.ask(Port::Cable, msg::BEGIN, &1u16.to_le_bytes()), Status::Busy as u8, "and the other way");
    assert_eq!(r.ask(Port::Bt, msg::END, &[]), 0);
    // a record half written counts too
    let mut w = std::vec![ferrite_core::link::what::SETTINGS, 0, 0, 0, 0];
    w.extend_from_slice(&300u16.to_le_bytes());
    w.extend_from_slice(&[0u8; 100]);
    assert_eq!(r.ask(Port::Cable, msg::WRITE, &w), 0, "the first part of three");
    assert_eq!(r.ask(Port::Bt, msg::WRITE, &w), Status::Busy as u8);
}

/// LINK-20.5, LINK-20.6, LINK-28: one keyer at a time. The cable's session keys; Bluetooth's key-up and keepalives are
/// refused and don't hold it; Bluetooth's session starting over leaves the cable's transmission alone; `KEY` off from
/// either port unkeys.
#[test]
fn one_remote_keyer() {
    use ferrite_core::link::{key, Ptt};
    let mut r = Rig::new();
    r.app.radio.tx_built = true;
    r.app.set_region(Some(ferrite_core::txplan::Region::Us), false);
    while r.app.pop_cmd().is_some() {}
    let ptt = |p: Ptt| {
        let mut b = [0u8; 3];
        let n = p.encode(&mut b);
        b[..n].to_vec()
    };
    assert_eq!(r.ask(Port::Cable, msg::KEY, &ptt(Ptt::On(key::ACTIVE))), 0);
    assert!(r.app.remote_keyed());
    let (s, a) = r.ask_full(Port::Bt, msg::KEY, &ptt(Ptt::On(key::ACTIVE)));
    assert_eq!((s, a.as_slice()), (Status::Invalid as u8, crate::remote::OTHER_PORT.as_bytes()));
    let (s, a) = r.ask_full(Port::Bt, msg::KEY, &ptt(Ptt::Alive));
    assert_eq!((s, a.as_slice()), (Status::Invalid as u8, crate::remote::OTHER_PORT.as_bytes()));
    assert_eq!(r.ask(Port::Cable, msg::KEY, &ptt(Ptt::Alive)), 0, "the keyer's own keepalive");
    // Bluetooth's connection ends: the cable's transmission goes on
    r.app.set.bt = false;
    r.poll();
    r.app.set.bt = true;
    assert!(r.app.remote_keyed(), "not Bluetooth's to end");
    // keepalives are the keyer's: Bluetooth's alone don't hold it past 2 s
    for _ in 0..25 {
        r.now += 100;
        let _ = r.ask(Port::Bt, msg::KEY, &ptt(Ptt::Alive));
        r.app.tick(r.now, &LiveAll::default(), &r.st);
    }
    assert!(!r.app.remote_keyed(), "the cable's keepalives stopped: unkeyed");
    // KEY off from either port unkeys
    assert_eq!(r.ask(Port::Cable, msg::KEY, &ptt(Ptt::On(key::ACTIVE))), 0);
    assert_eq!(r.ask(Port::Bt, msg::KEY, &ptt(Ptt::Off)), 0);
    assert!(!r.app.remote_keyed(), "stopping is always allowed");
}

/// PWR-26.2: the link is live (the MCU at the build's speed) for 5 s after a terminal byte on the cable or from a
/// connected Bluetooth device, and for link::ACTIVE_MS after a link frame; never without either.
#[test]
fn live_for_the_clock() {
    let mut r = Rig::new();
    assert!(!r.p.live(&r.app, r.now), "nothing yet");
    r.send(Port::Cable, b"\x1b");
    assert!(r.p.live(&r.app, r.now), "a cable byte");
    r.now += 5_000;
    assert!(!r.p.live(&r.app, r.now), "5 s later");
    r.send(Port::Bt, b"\r");
    assert!(r.p.live(&r.app, r.now), "a Bluetooth byte");
    r.app.set.bt = false;
    assert!(!r.p.live(&r.app, r.now), "Bluetooth off");
    r.app.set.bt = true;
    r.now += 6_000;
    assert_eq!(r.ask(Port::Cable, msg::HELLO, &[]), Status::Ok as u8);
    r.now += 6_000;
    assert!(r.p.live(&r.app, r.now), "a link frame 6 s ago: the session still active");
    r.now += crate::link::ACTIVE_MS;
    assert!(!r.p.live(&r.app, r.now), "the session over");
}

/// A PC's side of LINK-51: its windows and counts, its requests coded, the radio's frames decoded.
struct Pc {
    up: ferrite_core::linkz::LzEnc<{ ferrite_core::linkz::UP }>,
    down: ferrite_core::linkz::LzDec<{ ferrite_core::linkz::DOWN }>,
    up_n: u8,
    down_n: u8,
}

impl Pc {
    fn new() -> Self {
        Pc { up: ferrite_core::linkz::LzEnc::new(), down: ferrite_core::linkz::LzDec::new(), up_n: 0, down_n: 0 }
    }

    /// A request's wire bytes (coded when shorter).
    fn request(&mut self, ty: u8, seq: u8, payload: &[u8]) -> Vec<u8> {
        let mut c = [0u8; 300];
        let k = self.up.encode(payload, &mut c[2..]);
        (c[0], c[1]) = (ty, self.up_n);
        self.up_n = self.up_n.wrapping_add(1);
        let mut w = [0u8; MAX_WIRE];
        let n = match k.filter(|&k| k + 2 < payload.len()) {
            Some(k) => wire::build(link::z::Z, seq, &c[..k + 2], &mut w).unwrap(),
            None => wire::build(ty, seq, payload, &mut w).unwrap(),
        };
        let mut f = std::vec![0u8];
        f.extend_from_slice(&w[..n]);
        f
    }

    /// The radio's frames in `bytes`: (type, payload) each, decoded; None for a count that doesn't match.
    fn frames(&mut self, bytes: &mut [u8]) -> Vec<Option<(u8, Vec<u8>, bool)>> {
        let mut out = Vec::new();
        for raw in bytes.split_mut(|&b| b == 0).filter(|f| !f.is_empty()) {
            let f = wire::parse(raw).unwrap();
            if f.ty & 0x7F == link::z::Z {
                let mut b = [0u8; 300];
                let ok = f.payload[1] == self.down_n;
                let k = self.down.decode(&f.payload[2..], &mut b).unwrap();
                self.down_n = self.down_n.wrapping_add(1);
                out.push(ok.then(|| (f.payload[0], b[..k].to_vec(), true)));
            } else {
                self.down.seen(f.payload);
                self.down_n = self.down_n.wrapping_add(1);
                out.push(Some((f.ty, f.payload.to_vec(), false)));
            }
        }
        out
    }
}

/// LINK-51: a `HELLO` asking for compression gets it while the one block is free (the caps byte says so), and from
/// its answer on that session's frames go coded both ways when shorter, each window and count in step (plain frames
/// too); the other port's `HELLO` doesn't get it while it is taken, and gets it once the first session ends; a
/// frame with the wrong count is dropped, and a new `HELLO` starts both ends over. LINK-41.2: a `WRITE_MANY` record
/// of length 0 deletes the channel.
#[test]
/// (The caps byte is the seventh from last: the two layout bytes, LINK-62, and the lists generation, LINK-71, end the
/// answer.)
fn compression_first_come_first_served() {
    use ferrite_core::link::caps;
    let mut r = Rig::new();
    let mut hello = std::vec![wire::PROTOCOL, 1, 0, 0, 0, 0, 0, 0, 0];
    hello.extend_from_slice(b"test");
    let (s, a) = r.ask_full(Port::Cable, msg::HELLO, &hello);
    assert_eq!(s, 0);
    assert!(a.iter().rev().nth(6).is_some_and(|c| c & caps::LZ != 0), "granted: {a:?}");
    let (_, b) = r.ask_full(Port::Bt, msg::HELLO, &hello);
    assert!(b.iter().rev().nth(6).is_some_and(|c| c & caps::LZ == 0), "taken: Bluetooth runs plain");
    // the cable's session, compressed
    let mut pc = Pc::new();
    let mut got = Vec::new();
    for k in 0..4u8 {
        let req = pc.request(msg::UI, 10 + k, &[0]);
        r.send(Port::Cable, &req);
        let mut out = core::mem::take(&mut r.cable.0);
        got.extend(pc.frames(&mut out));
    }
    assert!(got.iter().all(|g| g.is_some()), "every count in step");
    assert!(got.iter().flatten().any(|g| g.2), "some answers coded");
    let plain: Vec<_> = got.iter().flatten().map(|g| g.1.clone()).collect();
    assert!(plain.windows(2).all(|w| w[0] == w[1]), "the same readback each time, decoded");
    // a coded request: channels written and deleted in one WRITE_MANY each
    let base = r.st.free_num(700).unwrap();
    let mut body = std::vec![what::CHANNEL];
    for k in 0..4u16 {
        let rec = crate::link::tests::chan_rec(base + k, 43_500_000, b"ZIP");
        body.extend_from_slice(&(base + k).to_le_bytes());
        body.push(rec.len() as u8);
        body.extend_from_slice(&rec);
    }
    let req = pc.request(msg::WRITE_MANY, 20, &body);
    assert_eq!(wire::parse(&mut req[1..req.len() - 1].to_vec()).unwrap().ty, link::z::Z, "the write went coded");
    r.send(Port::Cable, &req);
    let mut out = core::mem::take(&mut r.cable.0);
    let a = pc.frames(&mut out);
    assert_eq!(a[0].as_ref().map(|g| (g.0, g.1.clone())), Some((msg::WRITE_MANY | wire::ANSWER, std::vec![0, 4])));
    assert!(r.st.get(base + 3).is_some());
    let mut del = std::vec![what::CHANNEL];
    for k in 0..4u16 {
        del.extend_from_slice(&(base + k).to_le_bytes());
        del.push(0);
    }
    r.send(Port::Cable, &pc.request(msg::WRITE_MANY, 21, &del));
    let mut out = core::mem::take(&mut r.cable.0);
    assert!(pc.frames(&mut out)[0].as_ref().is_some_and(|g| g.1 == [0, 4]));
    assert!((0..4).all(|k| r.st.get(base + k).is_none()), "LINK-41.2: deleted in one request");
    // a frame lost on the way: the next coded one's count doesn't match and it is dropped (no answer); plain frames
    // carry no count, so the next coded frame is where it shows
    pc.up_n = pc.up_n.wrapping_add(1);
    let req = pc.request(msg::WRITE_MANY, 22, &body);
    assert_eq!(wire::parse(&mut req[1..req.len() - 1].to_vec()).unwrap().ty, link::z::Z);
    r.send(Port::Cable, &req);
    assert!(r.cable.0.is_empty(), "dropped");
    // a new HELLO starts both over
    let (s, a) = r.ask_full(Port::Cable, msg::HELLO, &hello);
    assert!(s == 0 && a.iter().rev().nth(6).is_some_and(|c| c & caps::LZ != 0));
    let mut pc = Pc::new();
    r.send(Port::Cable, &pc.request(msg::INFO, 23, &[]));
    let mut out = core::mem::take(&mut r.cable.0);
    assert!(pc.frames(&mut out)[0].is_some(), "in step again");
    // the cable's session ends (a HELLO not asking): Bluetooth may have it now
    hello[1] = 0;
    let (_, a) = r.ask_full(Port::Cable, msg::HELLO, &hello);
    assert!(a.iter().rev().nth(6).is_some_and(|c| c & caps::LZ == 0));
    r.poll();
    hello[1] = 1;
    let (_, b) = r.ask_full(Port::Bt, msg::HELLO, &hello);
    assert!(b.iter().rev().nth(6).is_some_and(|c| c & caps::LZ != 0), "Bluetooth gets it now");
}

/// LINK-13.4, LINK-13.5: receive audio to the PC: the cable only, never on by itself; while the cable's session
/// listens, a block goes out with the slot heard while a squelch is open, else a keep-alive each second; a new
/// `HELLO`, or the session going quiet, stops it.
#[test]
fn receive_audio_cable_only_and_opt_in() {
    use ferrite_core::rxaudio::{self, Block, NO_SLOT, SQ_OPEN};
    let mut r = Rig::new();
    assert_eq!(r.ask(Port::Cable, msg::HELLO, &[wire::PROTOCOL, 0]), 0);
    assert_eq!(r.p.audio_wanted(r.now), 0, "off until asked");
    assert_eq!(r.ask(Port::Bt, msg::HELLO, &[wire::PROTOCOL, 0]), 0);
    assert_eq!(r.ask(Port::Bt, msg::AUDIO, &[1]), Status::Refused as u8, "Bluetooth can't carry it");
    let (s, rest) = r.ask_full(Port::Cable, msg::AUDIO, &[1]);
    assert_eq!((s, rest), (0, std::vec![0xE0, 0x2E, 192, 0]), "12000 Hz, 192 samples a block (decision 330)");
    assert_eq!(r.p.audio_wanted(r.now), 1);
    let blk: Block = core::array::from_fn(|i| i as u8);
    let frames = |o: &mut Out| -> Vec<Vec<u8>> {
        let mut v = Vec::new();
        for raw in core::mem::take(&mut o.0).split(|&b| b == 0).filter(|f| !f.is_empty()) {
            let mut raw = raw.to_vec();
            let f = wire::parse(&mut raw).unwrap();
            assert_eq!(f.ty, msg::AUDIO_DATA);
            v.push(f.payload.to_vec());
        }
        v
    };
    let mut live = LiveAll::default();
    r.p.audio(&blk, &live, r.now + 2000, &mut r.cable); // nothing heard: a keep-alive (the last went long ago)
    r.p.audio(&blk, &live, r.now + 2100, &mut r.cable); // and not again within the second
    let k = frames(&mut r.cable);
    assert_eq!(k.len(), 1);
    assert_eq!(rxaudio::get(&k[0]).map(|(_, slot, fl, codes)| (slot, fl, codes.len())), Some((NO_SLOT, 0, 0)));
    live.slots[1].squelch_open = true;
    r.p.audio(&blk, &live, r.now + 2132, &mut r.cable);
    let b = frames(&mut r.cable);
    let (seq, slot, fl, codes) = rxaudio::get(&b[0]).unwrap();
    assert_eq!((seq, slot, fl, codes), (1, 1, SQ_OPEN, &blk[..]), "the block, slot B heard");
    assert_eq!(r.ask(Port::Cable, msg::HELLO, &[wire::PROTOCOL, 0]), 0);
    assert_eq!(r.p.audio_wanted(r.now), 0, "a HELLO ends it");
    assert_eq!(r.ask(Port::Cable, msg::AUDIO, &[1]), 0);
    assert_eq!(r.p.audio_wanted(r.now + link::ACTIVE_MS + 10), 0, "a quiet session ends it");
}

/// LINK-13.9: the raw stream (`AUDIO` 2, 12 kHz; 3, 16 kHz) is a bench build's only: a user build refuses it and
/// keeps not listening; a bench build answers its rate and block and listens in that mode, which the µ-law path
/// ([`Ports::audio`]) leaves to the DSP task.
#[test]
fn raw_audio_bench_only() {
    use ferrite_core::rawaudio::{BLOCK, MODE_12K, MODE_16K};
    let mut r = Rig::new();
    assert_eq!(r.ask(Port::Cable, msg::HELLO, &[wire::PROTOCOL, 0]), 0);
    for (m, hz) in [(MODE_12K, 12_000u16), (MODE_16K, 16_000)] {
        let (s, rest) = r.ask_full(Port::Cable, msg::AUDIO, &[m]);
        if cfg!(feature = "bench") {
            let mut want = hz.to_le_bytes().to_vec();
            want.extend_from_slice(&(BLOCK as u16).to_le_bytes());
            assert_eq!((s, rest), (0, want));
            assert_eq!(r.p.audio_wanted(r.now), m);
            r.p.audio(&[0; ferrite_core::rxaudio::BLOCK], &LiveAll::default(), r.now + 2000, &mut r.cable);
            assert!(r.cable.0.is_empty(), "no µ-law keep-alive in a raw mode");
        } else {
            assert_eq!(s, Status::Refused as u8);
            assert_eq!(r.p.audio_wanted(r.now), 0);
        }
    }
    assert_eq!(r.ask(Port::Cable, msg::AUDIO, &[4]), Status::BadArg as u8);
}

impl Rig {
    /// One link request from `port`: the answer's status and the rest, from among whatever else the port sent
    /// meanwhile (events, a terminal's redraw).
    fn ask_any(&mut self, port: Port, ty: u8, payload: &[u8]) -> (u8, Vec<u8>) {
        let mut w = [0u8; MAX_WIRE];
        let n = wire::build(ty, 9, payload, &mut w).unwrap();
        let mut f = std::vec![0u8];
        f.extend_from_slice(&w[..n]);
        self.send(port, &f);
        let out = if port == Port::Bt { &mut self.bt } else { &mut self.cable };
        let all = core::mem::take(&mut out.0);
        for raw in all.split(|&b| b == 0).filter(|f| !f.is_empty()) {
            let mut raw = raw.to_vec();
            if let Ok(f) = wire::parse(&mut raw) {
                if f.ty == ty | wire::ANSWER {
                    return (f.payload[0], f.payload[1..].to_vec());
                }
            }
        }
        panic!("no answer to {ty} on {port:?}: {:?}", &all[all.len().saturating_sub(60)..]);
    }
}

/// LINK-13.4: a block with no room in the cable's ring is dropped and counted; `listening` says the cable's
/// session asked.
#[test]
fn receive_audio_without_room_is_counted() {
    struct Full;
    impl Sink for Full {
        fn room(&self) -> usize {
            0
        }
        fn put(&mut self, _: &[u8]) {
            panic!("no room: nothing put");
        }
    }
    let mut r = Rig::new();
    assert!(!r.p.listening());
    assert_eq!(r.ask(Port::Cable, msg::AUDIO, &[1]), 0);
    assert!(r.p.listening());
    let mut live = LiveAll::default();
    live.slots[0].squelch_open = true;
    let blk = [0u8; ferrite_core::rxaudio::BLOCK];
    assert_eq!(r.p.audio_noroom(), 0);
    r.p.audio(&blk, &live, r.now, &mut Full);
    r.p.audio(&blk, &live, r.now + 32, &mut Full);
    assert_eq!(r.p.audio_noroom(), 2);
}

/// TERM-21, LINK-34: one debug request in flight at a time, from the terminal or either session's `CMD`; its
/// answer goes back to whoever asked.
#[cfg(feature = "line-cmds")]
#[test]
fn debug_requests_one_at_a_time() {
    use ferrite_core::msg::{DebugAns, DebugReq, GGA_MAX};
    let gga = || {
        let mut data = [0u8; GGA_MAX];
        let g = b"$GPGGA,1,4525.2950,N,07541.8600,W,1,09,0.9,71.2,M,-34.0,M,,*00";
        data[..g.len()].copy_from_slice(g);
        DebugAns::Gga { data, n: g.len() as u8, age_s: 4 }
    };
    let mut r = Rig::new();
    r.send(Port::Cable, b"\r\r:gps\r");
    assert_eq!(r.p.debug_req(), Some(DebugReq::Gga), "the terminal's");
    assert_eq!(r.p.debug_req(), None, "one in flight");
    r.cable.0.clear();
    r.p.debug_answer(gga());
    for _ in 0..4 {
        r.poll();
    }
    assert!(std::string::String::from_utf8_lossy(&r.cable.0).contains("GGA, 4 s ago"), "to the terminal");
    r.p.debug_answer(gga()); // nobody asking: dropped
    r.send(Port::Cable, b"\r\x03"); // the terminal closes
    assert_eq!(r.ask_any(Port::Bt, msg::CMD, b"gps").0, 0);
    assert_eq!(r.p.debug_req(), Some(DebugReq::Gga), "Bluetooth's session's");
    r.p.debug_answer(gga());
    let (_, a) = r.ask_any(Port::Bt, msg::MORE, &[]);
    assert!(std::string::String::from_utf8_lossy(&a).contains("GGA, 4 s ago"));
    r.now += 3000;
    r.cable.0.clear(); // the closed terminal's last words
    assert_eq!(r.ask_any(Port::Cable, msg::CMD, b"gps").0, 0);
    assert_eq!(r.p.debug_req(), Some(DebugReq::Gga), "the cable's session's");
}

/// LINK-24, BACKUP-7, LINK-9, LINK-15.1: what the ports hand the caller: a reboot asked on either port (once), the
/// cable's HELLOs, a write session's progress from either.
#[test]
fn reboot_hellos_and_progress() {
    let mut r = Rig::new();
    assert!(!r.p.take_reboot());
    assert_eq!(r.ask(Port::Bt, msg::REBOOT, &[]), 0);
    assert!(r.p.take_reboot());
    assert!(!r.p.take_reboot(), "once");
    assert_eq!(r.p.take_restore(), None);
    let h = r.p.hellos();
    assert_eq!(r.ask(Port::Cable, msg::HELLO, &[wire::PROTOCOL, 0]), 0);
    assert_eq!(r.p.hellos(), h + 1);
    assert_eq!(r.p.progress(), None);
    assert_eq!(r.ask(Port::Bt, msg::BEGIN, &4u16.to_le_bytes()), 0);
    assert_eq!(r.p.progress(), Some(0));
}

/// LINK-11: the cable's `BAUD` stays within the rate its UART reaches.
#[test]
fn cable_baud_within_the_uart() {
    let mut r = Rig::new();
    r.p.set_cable_max_baud(230_400);
    r.p.set_bt_fast(true);
    assert_eq!(r.ask(Port::Cable, msg::BAUD, &460_800u32.to_le_bytes()), Status::Refused as u8);
    assert_eq!(r.ask(Port::Cable, msg::BAUD, &230_400u32.to_le_bytes()), 0);
    assert_eq!(r.p.take_baud(), Some(230_400));
}

/// TERM-4, TERM-13: a whole frame waiting for its answer holds its port (and only its port) until a poll answers.
#[test]
fn a_waiting_frame_holds_its_port() {
    let mut r = Rig::new();
    assert!(r.p.ready() && !r.p.busy());
    let mut w = [0u8; MAX_WIRE];
    let n = wire::build(msg::INFO, 1, &[], &mut w).unwrap();
    for &b in [&[0u8][..], &w[..n]].concat().iter() {
        let t = r.p.route(&r.app, Port::Bt, b, r.now);
        r.p.feed(&mut r.app, &r.st, &Default::default(), &Default::default(), t, b, r.now);
    }
    assert!(!r.p.ready_for(Port::Bt));
    assert!(r.p.ready_for(Port::Cable));
    assert!(!r.p.ready() && r.p.busy());
    r.poll();
    assert!(r.p.ready_for(Port::Bt) && r.p.ready());
}

/// TERM-2, KISS-6: KISS on the cable (0 3 7 on Cable; 0 9 2 3 has no KISS since decision 386): every cable byte is
/// KISS's, none the terminal's.
#[test]
fn cable_port_kiss_drops_to_kiss() {
    let mut r = Rig::new();
    r.app.set.kiss_port = 1;
    r.app.open(crate::screens::nav::Dest::KissMode);
    assert_eq!(r.send(Port::Cable, b"\r:"), [To::Kiss; 2]);
    assert!(!r.p.term.on());
}

/// LINK-47: one Live View at a time: the newest subscription's port has it and the other's ends.
#[test]
fn live_view_moves_to_the_newest_subscriber() {
    use ferrite_core::link::event as ev;
    let mut r = Rig::new();
    assert_eq!(r.ask_any(Port::Cable, msg::SUBSCRIBE, &[ev::SUB_UI, 5]).0, 0);
    for _ in 0..5 {
        r.poll();
    }
    assert!(r.p.links[0].ui_wanted());
    assert_eq!(r.ask_any(Port::Bt, msg::SUBSCRIBE, &[ev::SUB_UI, 5]).0, 0);
    for _ in 0..5 {
        r.poll();
    }
    assert!(r.p.links[1].ui_wanted());
    assert!(!r.p.links[0].ui_wanted(), "the cable's ended");
    assert_eq!(r.ask_any(Port::Bt, msg::SUBSCRIBE, &[0]).0, 0);
    for _ in 0..3 {
        r.poll();
    }
    assert!(!r.p.links[1].ui_wanted());
}
