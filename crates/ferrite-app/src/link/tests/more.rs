//! Zones, presets and deletes over the link, and the refusals at their edges (LINK-5, LINK-21, LINK-41).
#![cfg_attr(not(feature = "line-cmds"), allow(unused))] // helpers for the tests gated on the line commands

use super::*;

fn reason(a: &[u8]) -> std::string::String {
    std::string::String::from_utf8_lossy(a).into_owned()
}

/// LINK-41: a zone written: the same one again changes nothing, a new name is saved, the next number adds one;
/// a bad name, resume, a priority channel past 3999 (LINK-41.3), a short record or a number past the next are
/// refused.
#[test]
fn zones_written() {
    let mut pc = Pc::new(true);
    let z = pc.read(what::ZONE, 0).unwrap();
    assert_eq!(pc.write(what::ZONE, 0, &z, 0).0, 0, "the same again");
    let mut r = z.clone();
    r[..16].fill(0);
    r[..5].copy_from_slice(b"Hills");
    assert_eq!(pc.write(what::ZONE, 0, &r, 0).0, 0);
    assert_eq!(pc.st.zone_name(0).unwrap().as_bytes(), b"Hills");
    let n = pc.st.zones();
    assert_eq!(pc.write(what::ZONE, n, &r, 0).0, 0, "the next number adds a zone");
    assert_eq!(pc.st.zones(), n + 1);
    let mut bad = z.clone();
    bad[..16].fill(0);
    let (s, a) = pc.write(what::ZONE, 0, &bad, 0);
    assert_eq!(s, Status::Invalid as u8);
    assert!(reason(&a).starts_with("name: 1 t"), "{}", reason(&a));
    let mut bad = z.clone();
    bad[19] = 13;
    let (s, a) = pc.write(what::ZONE, 0, &bad, 0);
    assert_eq!((s, reason(&a).as_str()), (Status::Invalid as u8, "resume out of range"));
    // LINK-41.3 (ferrite#148): a priority channel past the last channel number is refused; the last one is taken
    let mut bad = z.clone();
    bad[22..24].copy_from_slice(&5000u16.to_le_bytes());
    let (s, a) = pc.write(what::ZONE, 0, &bad, 0);
    assert_eq!((s, reason(&a).as_str()), (Status::Invalid as u8, "priority channel out of range"));
    let mut ok = z.clone();
    ok[22..24].copy_from_slice(&3999u16.to_le_bytes());
    assert_eq!(pc.write(what::ZONE, 0, &ok, 0).0, 0, "channel 4000 (3999) as priority");
    assert_eq!(pc.write(what::ZONE, 0, &z[..10], 0).0, Status::BadArg as u8, "short");
    assert_eq!(pc.write(what::ZONE, n + 5, &z, 0).0, Status::NotFound as u8, "past the next");
}

/// LINK-41, LINK-5: a range preset written: the same again is Ok (its skips kept); a new number adds one; a bad
/// name, resume, range, step or mode, a short record or a number past the next are refused.
#[test]
fn presets_written() {
    let mut pc = Pc::new(true);
    let p = pc.read(what::PRESET, 0).unwrap();
    let v = pc.st.data().version();
    assert_eq!(pc.write(what::PRESET, 0, &p, 0).0, 0);
    assert_eq!(pc.st.data().version(), v, "nothing written for the same preset");
    let mut r = p.clone();
    r[..12].fill(0);
    r[..4].copy_from_slice(b"Wide");
    assert_eq!(pc.write(what::PRESET, 0, &r, 0).0, 0);
    assert_eq!(pc.st.data().preset(0).unwrap().name.as_bytes(), b"Wide");
    let n = pc.st.data().presets() as u16;
    assert_eq!(pc.write(what::PRESET, n, &r, 0).0, 0, "the next number adds one");
    assert_eq!(pc.st.data().presets() as u16, n + 1);
    let edit = |f: &dyn Fn(&mut Vec<u8>)| {
        let mut x = p.clone();
        f(&mut x);
        x
    };
    for (rec, why) in [
        (edit(&|x| x[0] = 1), "name: printable ASCII only"),
        (edit(&|x| x[23] = 13), "resume out of range"),
        (edit(&|x| x.copy_within(12..16, 16)), "start and end: 18 to 1300 MHz, end above start"),
        (edit(&|x| x[12..16].copy_from_slice(&100u32.to_le_bytes())), "start and end: 18 to 1300 MHz, end above start"),
        (edit(&|x| x[20] = 99), "step or mode out of range"),
        (edit(&|x| x[21] = 99), "step or mode out of range"),
    ] {
        let (s, a) = pc.write(what::PRESET, 0, &rec, 0);
        assert_eq!((s, reason(&a).as_str()), (Status::Invalid as u8, why));
    }
    assert_eq!(pc.write(what::PRESET, 0, &p[..5], 0).0, Status::BadArg as u8, "short");
    assert_eq!(pc.write(what::PRESET, n + 5, &p, 0).0, Status::NotFound as u8, "past the next");
}

/// LINK-21: `DELETE`: a preset by number; a zone or preset past the last is not found; a channel that isn't
/// there is not found; another record kind or a short request is a bad argument; the channel open in the radio's
/// editor is busy (TERM-17).
#[test]
fn deletes() {
    let mut pc = Pc::new(true);
    let n = pc.st.data().presets();
    assert_eq!(pc.ask(msg::DELETE, &[what::PRESET, 0, 0]).0, 0);
    assert_eq!(pc.st.data().presets(), n - 1);
    assert_eq!(pc.ask(msg::DELETE, &[what::PRESET, 40, 0]).0, Status::NotFound as u8);
    assert_eq!(pc.ask(msg::DELETE, &[what::ZONE, 200, 0]).0, Status::NotFound as u8);
    assert_eq!(pc.ask(msg::DELETE, &[what::CHANNEL, 0xE7, 0x03]).0, Status::NotFound as u8, "channel 999: none");
    assert_eq!(pc.ask(msg::DELETE, &[what::CONTACT, 0, 0]).0, Status::BadArg as u8);
    assert_eq!(pc.ask(msg::DELETE, &[what::CHANNEL]).0, Status::BadArg as u8);
    let c = pc.st.get(9).unwrap();
    pc.app.set_channel_rec(0, c);
    pc.app.open(crate::screens::nav::Dest::ChannelEdit);
    assert_eq!(pc.ask(msg::DELETE, &[what::CHANNEL, 9, 0]).0, Status::Busy as u8);
    assert!(pc.st.get(9).is_some());
    // TERM-17 for a write, and for a write of no bytes (a delete)
    let rec = chan_rec(9, 14_652_000, b"Other");
    assert_eq!(pc.write(what::CHANNEL, 9, &rec, 0).0, Status::Busy as u8);
    assert_eq!(pc.write(what::CHANNEL, 9, &[], 0).0, Status::Busy as u8);
    assert_eq!(pc.st.get(9), Some(c));
}

/// LINK-21: `WRITE` in parts: too short, larger than a record, a part past its total, or parts out of order are a
/// bad argument; settings by `WRITE` while cancelled (LINK-9) are cancelled.
#[test]
fn write_parts_checked() {
    let mut pc = Pc::new(true);
    assert_eq!(pc.ask(msg::WRITE, &[what::SETTINGS, 0, 0]).0, Status::BadArg as u8, "short");
    let head = |off: u16, total: u16| {
        let mut p = std::vec![what::SETTINGS, 0, 0];
        p.extend_from_slice(&off.to_le_bytes());
        p.extend_from_slice(&total.to_le_bytes());
        p
    };
    let mut p = head(0, 4);
    p.extend_from_slice(&[1, 2, 3, 4, 5]);
    assert_eq!(pc.ask(msg::WRITE, &p).0, Status::BadArg as u8, "past its total");
    let p = head(0, (RECORD_MAX + 1) as u16);
    assert_eq!(pc.ask(msg::WRITE, &p).0, Status::BadArg as u8, "larger than a record");
    let mut p = head(0, 8);
    p.extend_from_slice(&[0; 4]);
    assert_eq!(pc.ask(msg::WRITE, &p).0, 0, "the first half");
    let mut p = head(2, 8);
    p.extend_from_slice(&[0; 4]);
    assert_eq!(pc.ask(msg::WRITE, &p).0, Status::BadArg as u8, "not where the first left off");
}

/// LINK-9: a write session ends by itself when the PC goes quiet; the progress screen goes.
#[test]
fn quiet_write_session_ends() {
    let mut pc = Pc::new(true);
    assert_eq!(pc.ask(msg::BEGIN, &2u16.to_le_bytes()).0, 0);
    assert_eq!(pc.link.progress(), Some(0));
    pc.now += SESSION_MS + 10;
    pc.link.poll(&mut pc.app, &pc.st, &pc.dev, &LiveAll::default(), pc.now, &mut Wire(Vec::new()));
    assert_eq!(pc.link.progress(), None);
    assert_eq!(pc.ask(msg::BEGIN, &[1]).0, Status::BadArg as u8, "BEGIN needs its count");
}

/// LINK-9: a `WRITE_MANY` after BACK on the progress screen is cancelled.
#[test]
fn write_many_cancelled() {
    let mut pc = Pc::new(true);
    assert_eq!(pc.ask(msg::BEGIN, &1u16.to_le_bytes()).0, 0);
    pc.key(Key::Exit);
    let rec = chan_rec(500, 14_652_000, b"X");
    assert_eq!(pc.write_many(what::CHANNEL, &[(500, rec)]).0, Status::Cancelled as u8);
    assert!(pc.st.get(500).is_none());
}

/// LINK-13.4, LINK-13.9: `AUDIO` on the cable: on, then the session listens; the raw modes (2, 3) are a bench
/// build's only and refused elsewhere; any other value is a bad argument.
#[test]
fn audio_on_and_bad() {
    let mut pc = Pc::new(true);
    assert!(!pc.link.listening());
    assert_eq!(pc.ask(msg::AUDIO, &[1]).0, 0);
    assert!(pc.link.listening());
    #[cfg(not(feature = "bench"))]
    assert_eq!(pc.ask(msg::AUDIO, &[2]).0, Status::Refused as u8);
    assert_eq!(pc.ask(msg::AUDIO, &[9]).0, Status::BadArg as u8);
    assert_eq!(pc.ask(msg::AUDIO, &[0]).0, 0);
    assert!(!pc.link.listening());
    assert_eq!(pc.ask(msg::BAUD, &[1, 2]).0, Status::BadArg as u8, "BAUD needs four bytes");
}

/// LINK-34, TERM-30: `gps` over the link waits for the firmware's GGA (MORE answers "still working"), then its
/// lines; a blank command line answers nothing to follow.
#[cfg(feature = "line-cmds")]
#[test]
fn gps_over_the_link_waits_for_the_answer() {
    let mut pc = Pc::new(true);
    let (s, a) = pc.ask(msg::CMD, b"gps");
    assert_eq!((s, a.as_slice()), (0, &[1u8][..]), "waiting for the firmware");
    assert_eq!(pc.link.debug_req(), Some(DebugReq::Gga));
    assert_eq!(pc.ask(msg::MORE, &[]).1, std::vec![1], "still waiting");
    let mut data = [0u8; ferrite_core::msg::GGA_MAX];
    let g = b"$GPGGA,1,4525.2950,N,07541.8600,W,1,09,0.9,71.2,M,-34.0,M,,*00";
    data[..g.len()].copy_from_slice(g);
    pc.link.debug_answer(DebugAns::Gga { data, n: g.len() as u8, age_s: 3 });
    let mut all = std::string::String::new();
    loop {
        let (s, a) = pc.ask(msg::MORE, &[]);
        assert_eq!(s, 0);
        all.push_str(text(&a[1..]));
        if a[0] == 0 {
            break;
        }
    }
    assert!(all.contains("GGA, 3 s ago"), "{all}");
    pc.link.debug_answer(DebugAns::Gga { data, n: 0, age_s: 0 }); // after the end: ignored
    let (s, a) = pc.ask(msg::CMD, b"   ");
    assert_eq!((s, a.as_slice()), (0, &[0u8][..]), "nothing to run");
}

/// A frame longer than the receive buffer is dropped whole, and the next frame is read.
#[test]
fn overlong_frame_dropped() {
    let mut pc = Pc::new(true);
    for _ in 0..(MAX_WIRE + 20) {
        pc.link.feed(0x55, pc.now);
    }
    pc.link.feed(0, pc.now);
    assert!(pc.link.ready(), "nothing waits for an answer");
    assert_eq!(pc.ask(msg::INFO, &[]).0, 0);
}

/// A `STREAM` of `p` (the inner type first): each frame's type and payload.
fn stream(pc: &mut Pc, seq: u8, p: &[u8]) -> Vec<(u8, Vec<u8>)> {
    let mut w = [0u8; MAX_WIRE];
    let n = wire::build(msg::STREAM, seq, p, &mut w).unwrap();
    pc.raw(&w[..n]);
    let mut out = Wire(Vec::new());
    for _ in 0..16 {
        pc.now += 10;
        pc.link.poll(&mut pc.app, &pc.st, &pc.dev, &LiveAll::default(), pc.now, &mut out);
    }
    out.0.split_mut(|&b| b == 0).filter(|f| !f.is_empty()).map(|f| {
        let f = wire::parse(f).unwrap();
        (f.ty, f.payload.to_vec())
    }).collect()
}

/// LINK-50: a stream of the heard list, the messages or the log starts with what asking once gives and goes on
/// (`STREAM_MORE`) to the last frame (`STREAM`); the log from the newest when no start is given, every event once.
#[test]
fn streams_of_heard_messages_and_log() {
    let mut pc = Pc::new(true);
    for (inner, p) in [(msg::HEARD, std::vec![0u8]), (msg::MSGS, std::vec![0u8, 0])] {
        let (s, once) = pc.ask(inner, &p);
        assert_eq!(s, 0);
        let mut sp = std::vec![inner];
        sp.extend_from_slice(&p);
        let got = stream(&mut pc, 90, &sp);
        let (last, more) = got.split_last().unwrap();
        assert_eq!(last.0, msg::STREAM | wire::ANSWER, "{inner}");
        assert!(more.iter().all(|g| g.0 == msg::STREAM_MORE | wire::ANSWER), "{inner}");
        assert!(got.iter().all(|g| g.1[0] == 0), "{inner}");
        assert_eq!(&got[0].1[1..], &once[..], "{inner}: the first frame is what asking once gives");
    }
    let (s, once) = pc.ask(msg::LOG_READ, &u32::MAX.to_le_bytes());
    assert_eq!(s, 0);
    let got = stream(&mut pc, 91, &[msg::LOG_READ]);
    assert_eq!(got.last().unwrap().0, msg::STREAM | wire::ANSWER);
    assert_eq!(&got[0].1[1..], &once[..], "from the newest");
    let events: u32 = got.iter().map(|g| g.1[7] as u32).sum();
    assert_eq!(events, pc.st.data().events() as u32, "every event once, across the frames");
    assert_eq!(pc.ask(msg::LOG_READ, &[1, 2]).0, Status::BadArg as u8);
}

/// APRS-42: a contact deleted that isn't saved is not found; the same contact again changes nothing; a record
/// of the wrong length is a bad argument.
#[test]
fn contacts_edges() {
    let mut pc = Pc::new(true);
    let rec = |call: &[u8], name: &[u8], flags: u8| {
        let mut r = std::vec![0u8; 23];
        r[..call.len()].copy_from_slice(call);
        r[10..10 + name.len()].copy_from_slice(name);
        r[22] = flags;
        r
    };
    assert_eq!(pc.write_many(what::CONTACT, &[(0, rec(b"VE9ZZZ", b"", 1))]).0, Status::NotFound as u8);
    let v = pc.st.data().version();
    assert_eq!(pc.write_many(what::CONTACT, &[(0, rec(b"VA3RTX", b"Ron", 0))]).0, 0);
    assert_eq!(pc.st.data().version(), v, "the same contact: nothing written");
    assert_eq!(pc.write_many(what::CONTACT, &[(0, std::vec![b'V'; 5])]).0, Status::BadArg as u8);
}

/// The App's clock brought to the link's and the LEDs then.
fn leds_now(pc: &mut Pc, live: &LiveAll) -> crate::lights::Leds {
    pc.app.tick(pc.now, live, &pc.st);
    pc.app.leds(live, &pc.dev)
}

/// How many times the green LED comes on over `ms`, sampled every 10 ms; and whether red ever lit.
fn flashes(pc: &mut Pc, live: &LiveAll, ms: u32) -> (u32, bool) {
    let (mut n, mut on, mut red) = (0, false, false);
    for _ in 0..ms / 10 {
        let l = leds_now(pc, live);
        n += (l.green && !on) as u32;
        (on, red) = (l.green, red || l.red);
        pc.now += 10;
    }
    (n, red)
}

/// DISP-30 (decision 332): records read or written over the link (here `READ_MANY`, `WRITE_MANY`, a `STREAM` of
/// them) flash the green LED, lit screen or dark, and for a second after the last frame; the screen readback (`UI`,
/// a `STREAM` of it), keys and commands don't. Receive's steady green and transmit's red win; 0 7 6 Off shows
/// nothing. On Bluetooth with the PIN lock on, a refused request flashes nothing, and a `STREAM` is refused before
/// AUTH as everything else is (LINK-30).
#[cfg(feature = "line-cmds")]
#[test]
fn big_transfers_flash_the_led() {
    let quiet = LiveAll::default();
    let mut pc = Pc::new(true);
    assert_eq!(pc.ask(msg::HELLO, &[PROTOCOL]).0, 0);
    assert_eq!(pc.ask(msg::UI, &[0]).0, 0);
    assert_eq!(pc.ask(msg::CMD, b"slot").0, 0);
    assert!(!pc.app.transferring(), "hello, the readback and a command aren't transfers");
    assert_eq!(leds_now(&mut pc, &quiet), crate::lights::Leds::default());
    let rm = |first: u16| {
        let mut p = std::vec![what::CHANNEL];
        p.extend_from_slice(&first.to_le_bytes());
        p.extend_from_slice(&1000u16.to_le_bytes());
        p
    };
    assert_eq!(pc.ask(msg::READ_MANY, &rm(0)).0, 0);
    assert!(pc.app.transferring());
    let l = leds_now(&mut pc, &quiet);
    assert!(l.next_ms > 0 && !l.red, "a flash with its next change");
    let (n, red) = flashes(&mut pc, &quiet, 1000);
    assert!(n >= 3 && !red, "green flashes while it lingers: {n}");
    let l = leds_now(&mut pc, &quiet);
    assert!(!pc.app.transferring() && l == crate::lights::Leds::default(), "over a second after the last frame");
    // a stream of records: on; a stream of the readback: off
    stream(&mut pc, 40, &[&[msg::READ_MANY][..], &rm(0)].concat());
    assert!(pc.app.transferring(), "a stream of records");
    pc.now += 1100;
    assert!(!leds_now(&mut pc, &quiet).green);
    stream(&mut pc, 60, &[msg::UI, 0]);
    assert!(!pc.app.transferring(), "the readback streamed");
    // a write
    let c = pc.st.by_number(0).unwrap();
    let rec = pc.read(what::CHANNEL, c.num).unwrap();
    pc.now += 1100;
    assert!(!leds_now(&mut pc, &quiet).green);
    assert_eq!(pc.write(what::CHANNEL, c.num, &rec, 240).0, 0);
    assert!(pc.app.transferring(), "a write");
    // receive and transmit win
    let mut rx = LiveAll::default();
    (rx.slots[0].squelch_open, rx.slots[0].carrier) = (true, true); // SQL-13.1: a carrier lights it
    assert!((0..30).all(|_| {
        pc.now += 10;
        leds_now(&mut pc, &rx).green
    }), "receive: steady green");
    let mut tx = LiveAll::default();
    tx.tx.keyed = true;
    let l = leds_now(&mut pc, &tx);
    assert!(l.red && !l.green, "transmit: red");
    // 0 7 6 Off: nothing
    pc.app.set.leds = 0;
    assert_eq!(pc.ask(msg::READ_MANY, &rm(0)).0, 0);
    assert_eq!(flashes(&mut pc, &quiet, 500), (0, false), "LEDs off");
    // the dark screen flashes too, over the heartbeat
    pc.app.set.leds = 2;
    pc.app.backlight_out();
    pc.now += 1100;
    leds_now(&mut pc, &quiet);
    assert!(pc.app.dark());
    assert_eq!(pc.ask(msg::READ_MANY, &rm(0)).0, 0);
    assert!(flashes(&mut pc, &quiet, 1000).0 >= 3, "dark: the transfer flash");
    // Bluetooth with the PIN lock, before AUTH
    let mut bt = Pc::new(false);
    (bt.app.set.pin, bt.app.set.pin_len, bt.app.set.pin_mode) = (1234, 4, 1);
    assert_eq!(bt.ask(msg::READ_MANY, &rm(0)).0, Status::NotAuthorised as u8);
    assert!(!bt.app.transferring(), "a refused read flashes nothing");
    let got = stream(&mut bt, 70, &[&[msg::READ_MANY][..], &rm(0)].concat());
    assert_eq!(got.len(), 1, "one answer");
    assert_eq!(got[0].1, [Status::NotAuthorised as u8], "LINK-30: no stream before AUTH");
    assert!(!bt.app.transferring());
}

/// LINK-70: `HEARD` with a maximum age: the count and the stations are those heard within it, newest first, the
/// index counting among them; 0 is no limit; a `STREAM` of it gives what asking once does; a station whose age isn't
/// known is never within one.
#[test]
fn heard_by_age() {
    use crate::data::AGO_UNKNOWN;
    let mut pc = Pc::new(true);
    let d = pc.st.data();
    let all: Vec<(Buf<10>, u32)> = (0..d.stations()).filter_map(|i| d.station(i)).map(|s| (s.call, s.ago_s)).collect();
    let within: Vec<&Buf<10>> = all.iter().filter(|(_, a)| *a != AGO_UNKNOWN && *a <= 3600).map(|(c, _)| c).collect();
    assert!(!within.is_empty() && within.len() < all.len(), "the sample has stations both sides of an hour");
    let calls = |a: &[u8]| {
        // each station: flags, call (length, text), symbol, age, times, [position], [course], [altitude], path, comment
        let mut v = Vec::new();
        let mut i = 1;
        while i < a.len() {
            let f = a[i];
            let n = a[i + 1] as usize;
            v.push(a[i + 2..i + 2 + n].to_vec());
            i += 2 + n + 1 + 4 + 2 + if f & 1 != 0 { 8 } else { 0 } + if f & 2 != 0 { 4 } else { 0 } + if f & 4 != 0 { 4 } else { 0 };
            i += 1 + a[i] as usize;
            i += 1 + a[i] as usize;
        }
        v
    };
    let (s, a) = pc.ask(msg::HEARD, &[0, 60, 0]);
    assert_eq!(s, 0);
    assert_eq!(a[0] as usize, within.len(), "the count is those within the hour");
    let got = calls(&a);
    assert_eq!(got, within.iter().take(got.len()).map(|c| c.as_bytes().to_vec()).collect::<Vec<_>>());
    let (_, b) = pc.ask(msg::HEARD, &[1, 60, 0]);
    assert_eq!(calls(&b)[0], within[1].as_bytes(), "the index counts among those within");
    let (_, z) = pc.ask(msg::HEARD, &[0, 0, 0]);
    assert_eq!(z[0] as usize, all.len(), "0: no limit");
    let (_, plain) = pc.ask(msg::HEARD, &[0]);
    assert_eq!(z, plain, "as without an age");
    let got = stream(&mut pc, 92, &[msg::HEARD, 0, 60, 0]);
    assert_eq!(&got[0].1[1..], &a[..], "a stream's first frame is what asking once gives");
}
