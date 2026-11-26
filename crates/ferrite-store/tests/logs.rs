//! The event log, APRS messages, waypoints and the APRS record in the simulated flash: round trips across a
//! power cycle, the ring wrapping, waypoints moving down on a delete, the APRS settings saved like the settings,
//! a chip with old data in these areas, and power cuts at random points of every kind of write.

use ferrite_app::aprs::AprsCfg;
use ferrite_app::data::{Contact, Data, Draft, Event, Fix, Message, MsgState, Op, Pos, Src, Station, Sym, Waypoint, WpSrc, AGO_UNKNOWN};
use ferrite_app::screens::alert::AlertInfo;
use ferrite_app::settings::Settings;
use ferrite_app::App;
use ferrite_core::layout::{EVENT_LOG, WAYPOINTS};
use ferrite_store::sim::SimFlash;
use ferrite_store::FlashStore;
use ferrite_ui::text::Buf;

/// The records other tasks keep: a callsign, a fix and one heard station.
struct Tasks;

impl Data for Tasks {
    fn callsign(&self) -> Option<Buf<10>> {
        Some(Buf::from(b"VA7FER-9"))
    }
    fn fix(&self) -> Option<Fix> {
        Some(Fix { pos: Pos { lat: 454_000_000, lon: -757_000_000 }, three_d: true, alt_m: 70, speed_kmh: 0, heading: 0, hdop_x10: 9, used: 8, age_s: 1, utc_s: 0, phone: false })
    }
    fn stations(&self) -> u16 {
        1
    }
    fn station(&self, i: u16) -> Option<Station> {
        (i == 0).then(|| Station {
            call: Buf::from(b"VE3OCB-7"),
            sym: Sym::Ht,
            pos: Some(Pos { lat: 454_252_000, lon: -756_901_000 }),
            ago_s: 5,
            via: Buf::new(),
            course: None,
            alt_m: None,
            times: 1,
            comment: Buf::new(),
            msg: Buf::new(),
            msg_min: None,
            msg_to: Default::default(),
            fx25: None,
            device: None,
        })
    }
}

type Store = FlashStore<SimFlash, Tasks>;

fn mount(f: SimFlash) -> Store {
    FlashStore::mount(f, Tasks)
}

fn cycle(s: Store) -> Store {
    mount(s.into_parts().0)
}

fn ev(src: Src, n: u32) -> Event {
    let mut text = Buf::<64>::new();
    text.set(b"event ");
    for d in n.to_string().bytes() {
        text.push(&[d]);
    }
    Event { src, ago_s: 0, freq: 14_439_000 + n, slot: (n % 3) as u8, unread: true, id: n, x: 1, y: 2, pos: n.is_multiple_of(2).then_some(Pos { lat: n as i32, lon: -(n as i32) }), name: Buf::from(b"VE3KPW-9"), text }
}

fn wp(n: u32) -> Waypoint {
    let mut name = Buf::<16>::from(b"WP ");
    for d in n.to_string().bytes() {
        name.push(&[d]);
    }
    Waypoint { name, pos: Pos { lat: n as i32 * 1000, lon: -(n as i32) }, src: WpSrc::Link }
}

fn app() -> App {
    let set: &'static mut Settings = Box::leak(Box::new(Settings::DEFAULT));
    let alert: &'static mut AlertInfo = Box::leak(Box::new(AlertInfo::NONE));
    App::new(set, alert)
}

/// Each event keeps its own read mark across power cycles, newest first (INBOX-7).
#[test]
fn events_round_trip_newest_first_and_are_marked_read() {
    let s = mount(SimFlash::new());
    for n in 0..5 {
        assert!(s.log(&ev(Src::Pocsag, n)));
    }
    let s = cycle(s);
    assert_eq!(s.events(), 5);
    for i in 0..5u16 {
        let e = s.event(i).unwrap();
        let want = ev(Src::Pocsag, 4 - i as u32);
        assert!(e.text == want.text && e.pos == want.pos && e.freq == want.freq && e.id == want.id && e.slot == want.slot, "event {i}");
        assert!(e.unread);
    }
    assert!(s.request(Op::Read(1)));
    assert!(!s.event(1).unwrap().unread && s.event(0).unwrap().unread);
    assert!(s.request(Op::ReadAll));
    let s = cycle(s);
    assert!((0..5).all(|i| !s.event(i).unwrap().unread), "read marks are kept");
    assert!(s.request(Op::ClearLog));
    assert_eq!(cycle(s).events(), 0);
}

/// When the ring is full, the oldest go a sector (31 records) at a time, newest first with no gaps (INBOX-8).
#[test]
fn the_log_wraps_a_sector_at_a_time() {
    let s = mount(SimFlash::new());
    let cap = (EVENT_LOG.len() as u32 / 4096) * 31;
    for n in 0..cap + 40 {
        s.log(&ev(Src::Aprs, n));
    }
    // Entering a used sector drops its 31 oldest at once.
    let s = cycle(s);
    let n = s.events() as u32;
    assert!(n > cap - 31 && n <= cap, "{n} of {cap}");
    assert_eq!(s.event(0).unwrap().id, cap + 39, "newest first");
    assert_eq!(s.event(n as u16 - 1).unwrap().id, cap + 40 - n, "oldest last, no gaps");
    assert!(s.event(n as u16).is_none());
    // LINK-42: each keeps its sequence number (the link's LOG_READ goes on from one), newest first.
    assert_eq!(s.event_seq(0), Some(cap + 39));
    assert_eq!(s.event_seq(n as u16 - 1), Some(cap + 40 - n));
    assert_eq!(s.event_seq(n as u16), None);
}

#[test]
fn messages_keep_their_state() {
    let s = mount(SimFlash::new());
    assert!(s.message_in(true, b"VE3KPW-9", b"On my way", 0));
    s.request(Op::Draft(Draft { to: Buf::from(b"VA3RTX"), text: Buf::from(b"Net at 8?") }));
    assert!(s.draft().text.as_bytes() == b"Net at 8?", "the draft is held");
    assert!(s.request(Op::Send));
    assert!(s.draft() == Draft::default(), "sending empties the draft");
    let state = |s: &Store, i: u16| s.message(i).unwrap().state;
    assert_eq!(state(&s, 0), MsgState::Pending);
    s.message_state(b"VA3RTX", b"Net at 8?", MsgState::Pending);
    s.message_state(b"VA3RTX", b"Net at 8?", MsgState::Retrying(2));
    assert_eq!(state(&s, 0), MsgState::Retrying(2));
    s.message_state(b"VA3RTX", b"Net at 8?", MsgState::Acked);
    let s = cycle(s);
    assert_eq!(s.messages(), 2);
    let m: Message = s.message(1).unwrap();
    assert!(m.inbound && m.peer.as_bytes() == b"VE3KPW-9" && m.state == MsgState::Unread);
    assert!(s.request(Op::MessageRead(1)));
    let s = cycle(s);
    assert_eq!((state(&s, 0), state(&s, 1)), (MsgState::Acked, MsgState::Read));
    // An ended message isn't moved again.
    s.message_state(b"VA3RTX", b"Net at 8?", MsgState::Failed);
    assert_eq!(state(&s, 0), MsgState::Acked);
    // APRS-13.5: one still being tried at power-off ends as failed at power-on (its tries were in RAM).
    assert!(s.request(Op::Draft(Draft { to: Buf::from(b"VA3RTX"), text: Buf::from(b"Still there?") })) && s.request(Op::Send));
    s.message_state(b"VA3RTX", b"Still there?", MsgState::Retrying(3));
    let s = cycle(s);
    assert_eq!(state(&s, 0), MsgState::Failed);
    // APRS-8: nothing to send, nothing saved.
    assert!(!s.request(Op::Send));
    assert_eq!(s.messages(), 3);
}

/// APRS-34: a rejected message stays rejected across a power cycle, told apart from failed, and nothing moves it
/// on after.
#[test]
fn rejected_message_kept() {
    let s = mount(SimFlash::new());
    s.request(Op::Draft(Draft { to: Buf::from(b"VA3RTX"), text: Buf::from(b"Net at 8?") }));
    assert!(s.request(Op::Send));
    s.message_state(b"VA3RTX", b"Net at 8?", MsgState::Rejected);
    s.message_state(b"VA3RTX", b"Net at 8?", MsgState::Failed);
    let s = cycle(s);
    assert_eq!(s.message(0).unwrap().state, MsgState::Rejected);
}

/// APRS-6.5: a message to us keeps the address it was sent to across a power cycle (outside the record's CRC); one
/// stored without it (as before APRS-6.5) reads it as unknown, and ours never has one.
#[test]
fn message_keeps_the_address_it_was_sent_to() {
    let s = mount(SimFlash::new());
    s.message_to_us(b"VE7GPL-7", b"old style", b"1", b"");
    s.message_to_us(b"VE7GPL-7", b"plain call", b"2", b"VA7IF");
    s.request(Op::Draft(Draft { to: Buf::from(b"VE7GPL"), text: Buf::from(b"ours") }));
    assert!(s.request(Op::Send));
    let s = cycle(s);
    let to = |i| s.message(i).unwrap().to;
    assert_eq!((s.message(0).unwrap().text.as_bytes(), to(0).as_bytes()), (&b"ours"[..], &b""[..]));
    assert_eq!((s.message(1).unwrap().text.as_bytes(), to(1).as_bytes()), (&b"plain call"[..], &b"VA7IF"[..]));
    assert_eq!((s.message(2).unwrap().text.as_bytes(), to(2).as_bytes()), (&b"old style"[..], &b""[..]));
}

/// APRS-32: after a power cycle the APRS task's RAM table is empty, so a sender's retry of a message already
/// stored is found in the store (by sender, number and text) and not stored or rung again; the APRS task still
/// acks it. Another number, other text, and a message without a number are new.
#[test]
fn retry_after_power_cycle_stored_once() {
    use ferrite_app::aprs_live::uaprs::ax25::{Address, Frame};
    use ferrite_app::aprs_live::{AprsLive, Sending, FRAME_MAX};
    let frame = |info: &[u8]| {
        let mut b = [0u8; FRAME_MAX];
        let path = [Address::from_text(b"WIDE1-1").unwrap()];
        let n = Frame::new(Address::from_text(b"VE3KPW-9").unwrap(), Address::from_text(b"APDR16").unwrap(), &path, info).unwrap().write(&mut b).unwrap();
        b[..n].to_vec()
    };
    let me = b"VA7FER-9";
    let cfg = AprsCfg::DEFAULT;
    // What the firmware's APRS task does with a message to us: the RAM table, then the store.
    let hear = |s: &Store, a: &AprsLive, info: &[u8], now: u32| -> bool {
        let rx = a.receive(&frame(info), now, me, &cfg);
        match rx.inbound {
            Some((from, text)) if !s.message_heard(from.as_bytes(), rx.inbound_id.as_bytes(), text.as_bytes()) => s.message_to_us(from.as_bytes(), text.as_bytes(), rx.inbound_id.as_bytes(), rx.inbound_to.as_bytes()),
            _ => false,
        }
    };
    let s = mount(SimFlash::new());
    s.message_in(true, b"VE3KPW-9", b"Running late", 0); // from before APRS-32: no number kept
    let a = AprsLive::new();
    assert!(hear(&s, &a, b":VA7FER-9 :Running late{AB12}", 100));
    assert!(!hear(&s, &a, b":VA7FER-9 :Running late{AB12}", 130), "the RAM table");
    assert_eq!(s.messages(), 2);
    let s = cycle(s);
    let a = AprsLive::new();
    assert!(!hear(&s, &a, b":VA7FER-9 :Running late{AB12}", 5), "the store");
    let mut out = [0u8; FRAME_MAX];
    let t = a.tick(5, Sending::All, true, me, None, &cfg, &mut out);
    assert_eq!((t.what, Frame::parse(&out[..t.len]).unwrap().info), ("Ack", &b":VE3KPW-9 :ackAB12"[..]), "acked all the same");
    assert!(hear(&s, &a, b":VA7FER-9 :Running late{AB13}", 6), "another number");
    assert!(hear(&s, &a, b":VA7FER-9 :Other text{AB12}", 7), "other text");
    assert!(hear(&s, &a, b":VA7FER-9 :Running late", 8), "no number: RAM only");
    assert_eq!(s.messages(), 5);
    assert!(s.message(0).is_some_and(|m| m.inbound && m.text.as_bytes() == b"Running late" && m.state == MsgState::Unread));
}

#[test]
fn waypoints_list_newest_first_and_move_down_on_delete() {
    let s = mount(SimFlash::new());
    for n in 0..200 {
        assert!(s.add_waypoint(&wp(n)));
    }
    // Delete the oldest: everything after it moves down a slot, across two sector boundaries.
    assert!(s.request(Op::DeleteWaypoint(199)));
    assert!(s.request(Op::DeleteWaypoint(0)));
    let s = cycle(s);
    assert_eq!(s.waypoints(), 198);
    assert!(s.waypoint(0).unwrap() == wp(198));
    assert!(s.waypoint(197).unwrap() == wp(1));
    for i in 0..198u16 {
        assert!(s.waypoint(i).unwrap() == wp(198 - i as u32), "slot {i}");
    }
    // Mark here, a heard station and a grid square (GPS-10, GPS-20, GPS-21).
    assert!(s.request(Op::MarkHere));
    assert!(s.request(Op::SaveStation(0)));
    assert!(s.request(Op::AddWaypoint(Pos { lat: 1, lon: 2 }, Buf::from(b"FN25dk"))));
    let w = |i| s.waypoint(i).unwrap();
    assert!(w(0).src == WpSrc::Grid && w(1).src == WpSrc::Aprs && w(1).name.as_bytes() == b"VE3OCB-7" && w(2).src == WpSrc::Marked);
    assert_eq!(w(2).name.as_bytes(), b"Marked 199");
    // 2,032 at most (decision 333).
    while s.waypoints() < ferrite_store::logs::WP_MAX {
        assert!(s.add_waypoint(&wp(1000)));
    }
    assert!(!s.add_waypoint(&wp(1001)) && !s.request(Op::MarkHere));
}

/// APRS-37: the contacts stay in callsign order across the sector boundary (127 to a sector), a saved callsign
/// is renamed rather than added twice, deleting moves the later ones down, and the book holds 1,016 (decision 333).
#[test]
fn contacts_kept_in_callsign_order() {
    let s = mount(SimFlash::new());
    let c = |n: u32, name: &[u8]| {
        let mut call = Buf::<10>::from(b"VE3");
        for d in [n / 1000, n / 100 % 10, n / 10 % 10, n % 10] {
            call.push(&[b'A' + d as u8]);
        }
        Contact { call, name: Buf::from(name) }
    };
    let max = ferrite_store::logs::CONTACT_MAX as u32;
    // Backwards, so every save goes in at the front and moves the rest up.
    for n in (0..max).rev() {
        assert!(s.request(Op::SaveContact(c(n, b"x"))), "contact {n}");
    }
    assert!(!s.request(Op::SaveContact(c(9999, b"full"))), "1,016 at most");
    assert!(s.request(Op::SaveContact(c(5, b"Renamed"))), "a saved one is renamed when full");
    let s = cycle(s);
    assert_eq!(s.contacts() as u32, max);
    for n in 0..max {
        assert!(s.contact(n as u16).unwrap().call == c(n, b"").call, "slot {n}");
    }
    assert!(s.contact_of(c(5, b"").call.as_bytes()).unwrap().name.as_bytes() == b"Renamed");
    assert!(s.request(Op::DeleteContact(c(0, b"").call)));
    assert!(!s.request(Op::DeleteContact(c(0, b"").call)), "not there any more");
    let s = cycle(s);
    assert_eq!(s.contacts() as u32, max - 1);
    assert!(s.contact(0).unwrap().call == c(1, b"").call && s.contact(max as u16 - 2).unwrap().call == c(max - 1, b"").call);
    assert!(s.contact_of(b"VE3ZZZ").is_none());
}

/// APRS-54.1: telemetry is the after-voice byte's bit 1, so the APRS record keeps its length; each is kept
/// apart from the other.
#[test]
fn aprs_telemetry_beside_after_voice() {
    for (after_voice, telemetry) in [(false, true), (true, false), (false, false)] {
        let s = mount(SimFlash::new());
        let a = app();
        let cfg = AprsCfg { after_voice, telemetry, ..AprsCfg::DEFAULT };
        s.autosave(1000, &a);
        assert!(s.request(Op::AprsCfg(cfg)));
        s.autosave(3100, &a);
        let s = cycle(s);
        assert!(s.aprs_cfg() == cfg);
    }
}

#[test]
fn aprs_settings_wait_two_seconds_and_quick_messages_are_kept() {
    let s = mount(SimFlash::new());
    let a = app();
    // APRS-26: the custom path is kept too, and chosen (path 3); APRS-30: and the SSID; APRS-6.1: the message sound; APRS-47 - APRS-49: symbol, TX delay, TX level; APRS-50, APRS-51: comment and ambiguity; APRS-52.4: Mic-E
    let mut comment = [0u8; ferrite_app::aprs::COMMENT_LEN];
    comment.copy_from_slice(b"Ferrite on the trail, QRV 146.52 simplex");
    let mut custom = [0u8; ferrite_app::aprs::CUSTOM_LEN];
    custom[..7].copy_from_slice(b"WIDE2-2");
    let cfg = AprsCfg { f5k: 28_878, beacon: 3, power: 1, path: ferrite_app::aprs::PATH_CUSTOM, digi: 0x23, after_voice: true, custom, ssid: 9, fx25: 32, sound: 3, symbol: 4, txdelay: 45, tx_level: 2, comment, ambiguity: 3, mic_e: 5, telemetry: true, text_every: 3, reply: 0x11, reply_to: *b"VE7ABC-9\0", reply_text: { let mut t = [0u8; ferrite_app::aprs::REPLY_TEXT_LEN]; t[..4].copy_from_slice(b"Away"); t }, retries: 2 };
    s.autosave(1000, &a);
    assert!(s.request(Op::AprsCfg(cfg)));
    assert!(s.aprs_cfg() == cfg);
    let ops = s.nor().ops;
    s.autosave(2500, &a);
    assert_eq!(s.nor().ops, ops, "not written within 2 s of the change");
    s.autosave(3100, &a);
    assert!(s.request(Op::Quick(2, Some(Buf::from(b"QRT, 73")))));
    assert!(s.quick(2).unwrap().as_bytes() == b"QRT, 73" && s.quick(1).is_none());
    let s = cycle(s);
    assert!(s.aprs_cfg() == cfg);
    assert!(s.quick(2).unwrap().as_bytes() == b"QRT, 73");
    assert!(s.request(Op::Quick(2, None)), "back to the factory text");
    assert!(cycle(s).quick(2).is_none());
}

#[test]
fn the_reset_empties_them_all() {
    let s = mount(SimFlash::new());
    s.log(&ev(Src::Same, 1));
    s.message_in(true, b"A", b"B", 0);
    s.add_waypoint(&wp(1));
    s.request(Op::SaveContact(Contact { call: Buf::from(b"VE3KPW"), name: Buf::new() }));
    s.request(Op::AprsCfg(AprsCfg { f5k: 1, beacon: 1, power: 1, ..AprsCfg::DEFAULT }));
    assert!(s.request(Op::Wipe));
    assert_eq!((s.events(), s.messages(), s.waypoints(), s.contacts()), (0, 0, 0, 0));
    assert!(s.aprs_cfg() == AprsCfg::DEFAULT);
    let s = cycle(s);
    assert_eq!((s.events(), s.messages(), s.waypoints(), s.contacts()), (0, 0, 0, 0));
}

/// A chip that held something else in these areas (a radio before Ferrite): mount clears the way.
#[test]
fn old_data_in_the_areas_is_cleared_at_mount() {
    let s = mount(SimFlash::new());
    let mut f = s.into_parts().0;
    let mut noise = 0x1234_5678u32;
    for a in EVENT_LOG.start..EVENT_LOG.start + 3 * 4096 {
        noise = noise.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        ferrite_store::Nor::start_program(&mut f, a, &[(noise >> 16) as u8]).unwrap();
    }
    for a in WAYPOINTS.start + 40..WAYPOINTS.start + 9000 {
        ferrite_store::Nor::start_program(&mut f, a, &[0x5A]).unwrap();
    }
    let s = mount(f);
    assert_eq!((s.events(), s.waypoints()), (0, 0));
    for n in 0..300 {
        assert!(s.add_waypoint(&wp(n)));
        s.log(&ev(Src::Dtmf, n));
    }
    let s = cycle(s);
    assert_eq!((s.events(), s.waypoints()), (300, 300));
    assert!(s.waypoint(0).unwrap() == wp(299) && s.event(0).unwrap().id == 299);
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// What a cut can change.
#[derive(Clone, PartialEq, Debug)]
struct Snap {
    events: Vec<(u32, bool)>,
    msgs: Vec<(Vec<u8>, MsgState)>,
    wps: Vec<Vec<u8>>,
    contacts: Vec<Vec<u8>>,
    cfg: (u16, u8, u8),
    quick: Vec<Option<Vec<u8>>>,
}

fn snap(s: &Store) -> Snap {
    Snap {
        events: (0..s.events()).map(|i| s.event(i).map_or((u32::MAX, false), |e| (e.id, e.unread))).collect(),
        msgs: (0..s.messages()).map(|i| s.message(i).map_or((vec![], MsgState::Failed), |m| (m.text.as_bytes().to_vec(), m.state))).collect(),
        wps: (0..s.waypoints()).map(|i| s.waypoint(i).map_or(vec![], |w| w.name.as_bytes().to_vec())).collect(),
        contacts: (0..s.contacts()).map(|i| s.contact(i).map_or(vec![], |c| [c.call.as_bytes(), c.name.as_bytes()].concat())).collect(),
        cfg: {
            let c = s.aprs_cfg();
            (c.f5k, c.beacon, c.power)
        },
        quick: (0..9).map(|i| s.quick(i).map(|q| q.as_bytes().to_vec())).collect(),
    }
}

/// Runs `op` on a copy to count its flash operations, then on `f` with the power cut at a random one, then
/// mounts again (sometimes cutting the recovery too): the store as it came back, and before and after.
fn cut_during(f: SimFlash, r: &mut Rng, op: &dyn Fn(&Store)) -> (Store, Snap, Snap) {
    let s = mount(f.clone());
    let before = snap(&s);
    let n0 = s.nor().ops;
    op(&s);
    let ops = s.nor().ops - n0;
    let after = snap(&s);
    let s = mount(f);
    if ops > 0 {
        let mut nor = s.nor();
        nor.cut_at = Some(nor.ops + 1 + r.below(ops));
        nor.seed(r.next());
    }
    op(&s);
    let mut f = s.into_parts().0;
    f.power_on();
    if r.below(4) == 0 {
        f.cut_at = Some(f.ops + 1 + r.below(20_000));
        f.seed(r.next());
        f = mount(f).into_parts().0;
        f.power_on();
    }
    (mount(f), before, after)
}

/// A power cut at any flash operation leaves the store as before or after the write, or one of the allowed
/// in-between states: at most the record being written is lost (INBOX-13).
#[test]
fn power_cuts_leave_before_or_after() {
    let mut r = Rng(0x1065_0001);
    let s = mount(SimFlash::new());
    // A log near a sector boundary, a few messages and waypoints, stored APRS settings.
    for n in 0..58 {
        s.log(&ev(Src::Pocsag, n));
    }
    for n in 0..6u32 {
        s.message_in(n.is_multiple_of(2), b"VA3RTX", format!("msg {n}").as_bytes(), 0);
        s.add_waypoint(&wp(n));
    }
    s.request(Op::AprsCfg(AprsCfg { f5k: 7, beacon: 2, power: 0, ..AprsCfg::DEFAULT }));
    s.flush(&app());
    let mut f = s.into_parts().0;
    let (mut old, mut new, mut dup) = (0, 0, 0);
    for i in 0..1500u32 {
        let k = r.below(10);
        let n = 1000 + i;
        let (s, before, after) = cut_during(f, &mut r, &|s| match k {
            0 | 1 => {
                s.log(&ev(Src::Ais, n));
            }
            2 => {
                s.request(Op::Read(r_idx(n, s.events())));
            }
            3 => {
                s.message_in(true, b"VE3KPW-9", format!("hi {n}").as_bytes(), 0);
            }
            4 => {
                s.message_state(b"VA3RTX", b"msg 1", if n % 3 == 0 { MsgState::Acked } else { MsgState::Retrying(2) });
            }
            5 => {
                s.add_waypoint(&wp(n));
            }
            6 => {
                if s.waypoints() > 3 {
                    s.request(Op::DeleteWaypoint(r_idx(n, s.waypoints())));
                }
            }
            // APRS-37: a contact saved, renamed or deleted, all through one sector's rewrite (or an append).
            8 => {
                let call = Buf::from(format!("VE{}", n % 40).as_bytes());
                s.request(Op::SaveContact(Contact { call, name: Buf::from(format!("n{n}").as_bytes()) }));
            }
            9 => {
                if s.contacts() > 2 {
                    s.request(Op::DeleteContact(s.contact(r_idx(n, s.contacts())).unwrap().call));
                }
            }
            _ => {
                s.request(Op::AprsCfg(AprsCfg { f5k: n as u16, beacon: 1, power: 2, ..AprsCfg::DEFAULT }));
                s.flush(&app());
                s.request(Op::Quick((n % 9) as u8, Some(Buf::from(format!("q {n}").as_bytes()))));
            }
        });
        let got = snap(&s);
        if got == before {
            old += 1;
        } else if got == after {
            new += 1;
        } else {
            // The only other outcomes allowed: a waypoint delete cut midway leaves one waypoint twice and none
            // lost; the APRS record's settings and its quick message are two writes, so one may land alone.
            let mut ok = false;
            if k <= 1 {
                // A cut while entering the oldest sector: its records go (as the append would drop them), the
                // new event isn't there yet.
                ok = got.events.as_slice() == &after.events[1..] && got.msgs == before.msgs && got.wps == before.wps;
            }
            if k == 6 {
                let mut g = got.wps.clone();
                g.dedup();
                ok = got.wps.len() == before.wps.len() && (g == after.wps || g.len() == got.wps.len() - 1);
            }
            if k == 7 {
                ok = got.events == before.events && got.msgs == before.msgs && got.wps == before.wps;
            }
            if k == 7 && got.contacts != before.contacts {
                ok = false;
            }
            assert!(
                ok,
                "iteration {i} (kind {k}): neither before nor after: events {}/{}/{}, msgs {:?} / {:?} / {:?}, wps {:?} / {:?} / {:?}",
                before.events.len(),
                after.events.len(),
                got.events.len(),
                before.msgs,
                after.msgs,
                got.msgs,
                before.wps,
                after.wps,
                got.wps
            );
            dup += 1;
        }
        // Whatever happened, the store keeps working.
        assert!(s.log(&ev(Src::Dtmf, 2 * n)) && s.event(0).unwrap().id == 2 * n);
        f = s.into_parts().0;
    }
    eprintln!("cuts: {old} before, {new} after, {dup} in between (allowed)");
}

fn r_idx(n: u32, len: u16) -> u16 {
    (n % len.max(1) as u32) as u16
}

/// INBOX-7.1: reading an unread message from someone marks its log entry read too (so the inbox count goes with
/// it); another entry stays unread, and reading the message again leaves the log alone.
#[test]
fn reading_a_message_reads_its_log_entry() {
    let s = mount(SimFlash::new());
    let entry = |name: &[u8], text: &[u8]| Event { src: Src::Aprs, ago_s: 0, freq: 14_439_000, slot: 2, unread: true, id: 0, x: 0, y: 0, pos: None, name: Buf::from(name), text: Buf::from(text) };
    s.log(&entry(b"VE7TES-7", b"other"));
    s.message_to_us(b"VE7TES-7", b"hello", b"1", b"");
    s.log(&entry(b"VE7TES-7", b"hello"));
    assert!(s.request(Op::MessageRead(0)));
    assert_eq!(s.message(0).map(|m| m.state), Some(MsgState::Read));
    assert_eq!((s.event(0).map(|e| e.unread), s.event(1).map(|e| e.unread)), (Some(false), Some(true)));
    s.log(&entry(b"VE7TES-7", b"hello"));
    assert!(s.request(Op::MessageRead(0)));
    assert_eq!(s.event(0).map(|e| e.unread), Some(true), "already read: a later entry is left alone");
}

/// APRS-46: "Mark all read" reads every message to us and its log entry; "Delete all messages" empties the inbox
/// and sent, and stays empty across a power cycle.
#[test]
fn messages_read_all_and_delete_all() {
    let s = mount(SimFlash::new());
    let entry = |name: &[u8], text: &[u8]| Event { src: Src::Aprs, ago_s: 0, freq: 14_439_000, slot: 2, unread: true, id: 0, x: 0, y: 0, pos: None, name: Buf::from(name), text: Buf::from(text) };
    for (n, t) in [(&b"VE7GPL-7"[..], &b"one"[..]), (b"VA7IF-7", b"two")] {
        s.message_to_us(n, t, b"1", b"");
        s.log(&entry(n, t));
    }
    s.message_in(false, b"VE7GPL-7", b"sent", 0);
    assert!(s.request(Op::MessagesReadAll));
    assert!((0..s.messages()).all(|i| s.message(i).is_some_and(|m| !m.inbound || m.state == MsgState::Read)));
    assert!((0..s.events()).all(|i| s.event(i).is_some_and(|e| !e.unread)), "their log entries too");
    assert!(s.request(Op::ClearMessages));
    assert_eq!(s.messages(), 0);
    assert_eq!(cycle(s).messages(), 0);
}

/// INBOX-6.1: a record's age is real or unknown, never a guessed date. Stamped this power-on, an event's and a
/// message's ages stay right across the clock becoming UTC (GPS); after a power cycle, a record stamped without
/// UTC (as every record from before INBOX-6.1) has no age, and one stamped in UTC gets its age back once the
/// clock is UTC again.
#[test]
fn record_times_are_real_or_unknown() {
    let a = app();
    let s = mount(SimFlash::new());
    s.autosave(10_000, &a);
    assert!(s.log(&ev(Src::Pocsag, 1))); // no clock yet
    assert!(s.message_in(true, b"VE3KPW-9", b"no clock", 0));
    s.autosave(70_000, &a);
    assert_eq!((s.event(0).unwrap().ago_s, s.message(0).unwrap().ago_s), (60, 60));
    s.set_time(1_800_000_000); // GPS: UTC from here
    assert_eq!(s.event(0).unwrap().ago_s, 60, "this power-on's record keeps its age when the clock jumps");
    assert!(s.log(&ev(Src::Pocsag, 2)));
    assert!(s.message_in(true, b"VE3KPW-9", b"with clock", 0));
    s.autosave(100_000, &a);
    assert_eq!((s.event(0).unwrap().ago_s, s.event(1).unwrap().ago_s), (30, 90));
    assert_eq!((s.message(0).unwrap().ago_s, s.message(1).unwrap().ago_s), (30, 90));

    // Power cycle, no GPS yet: neither has a real age.
    let s = cycle(s);
    assert_eq!((s.event(0).unwrap().ago_s, s.event(1).unwrap().ago_s), (AGO_UNKNOWN, AGO_UNKNOWN));
    assert_eq!((s.message(0).unwrap().ago_s, s.message(1).unwrap().ago_s), (AGO_UNKNOWN, AGO_UNKNOWN));
    // A record stored now, before GPS, has its age.
    assert!(s.log(&ev(Src::Pocsag, 3)));
    s.autosave(5_000, &a);
    assert_eq!(s.event(0).unwrap().ago_s, 5);
    // GPS again: the UTC record's age is back (stamped at 1_800_000_000); the old non-UTC one stays unknown;
    // this power-on's keeps its own.
    s.set_time(1_800_000_500);
    assert_eq!(s.event(1).unwrap().ago_s, 500);
    assert_eq!(s.event(2).unwrap().ago_s, AGO_UNKNOWN);
    assert_eq!(s.event(0).unwrap().ago_s, 5);
}

fn st(call: &[u8], comment: &[u8], times: u16) -> Station {
    Station {
        call: Buf::from(call),
        sym: Sym::Car,
        pos: Some(Pos { lat: 454_252_000, lon: -756_901_000 }),
        ago_s: 0,
        via: Buf::from(b"VE3RSB-1*".as_slice()),
        course: Some((92, 54)),
        alt_m: Some(-3),
        times,
        comment: Buf::from(comment),
        msg: Buf::new(),
        msg_min: None,
        msg_to: Default::default(),
        fx25: Some(4),
        device: Some(31),
    }
}

/// APRS-4, APRS-4.2, APRS-4.3: the heard list's rows survive a power cycle: the newest row of each station, newest first, every
/// field back, the times heard too; ages unknown until the clock is UTC again (INBOX-6.1). Clearing the heard
/// list and the reset erase them.
#[test]
fn heard_rows_survive_a_restart() {
    let a = app();
    let s = mount(SimFlash::new());
    s.set_time(1_800_000_000);
    assert!(s.heard_row(&st(b"VE3KPW-9", b"Mobile", 1)));
    assert!(s.heard_row(&st(b"VA3RTX", b"Home", 4)));
    s.autosave(30_000, &a);
    assert!(s.heard_row(&st(b"VE3KPW-9", b"Moved", 6))); // its newer row
    let s = cycle(s);
    assert_eq!(s.heard_stored(), 3);
    let mut got = vec![];
    s.heard_rows(50, &mut |x, _| got.push(x));
    assert_eq!(got.len(), 2, "one per station");
    let want = Station { ago_s: AGO_UNKNOWN, ..st(b"VE3KPW-9", b"Moved", 6) };
    assert!(got[0] == want, "every field back, the via marked used");
    assert_eq!((got[1].call.as_bytes(), got[1].times, got[1].ago_s), (&b"VA3RTX"[..], 4, AGO_UNKNOWN));
    s.set_time(1_800_000_600);
    let mut ages = vec![];
    s.heard_rows(50, &mut |x, _| ages.push(x.ago_s));
    assert_eq!(ages, [570, 600], "UTC rows dated once GPS time is back");
    let mut one = 0;
    s.heard_rows(1, &mut |_, _| one += 1);
    assert_eq!(one, 1, "at most max stations");
    assert!(s.request(Op::ClearHeard));
    assert_eq!(cycle(s).heard_stored(), 0, "clearing the heard list erases the rows");
    let s = mount(SimFlash::new());
    assert!(s.heard_row(&st(b"VE3KPW-9", b"Mobile", 1)));
    assert!(s.request(Op::Wipe));
    assert_eq!(cycle(s).heard_stored(), 0, "the reset erases them");
}

/// What other tasks keep in RAM comes from them through the store: the callsign, the fix, the heard stations (a
/// station with no stored row as the task has it), the beacon, the satellites and the scan's skips.
#[test]
fn other_tasks_records_pass_through() {
    let s = mount(SimFlash::new());
    assert_eq!(s.callsign().unwrap().as_bytes(), b"VA7FER-9");
    assert_eq!(s.fix().map(|f| f.pos), Tasks.fix().map(|f| f.pos));
    assert_eq!(s.stations(), 1);
    assert!(Data::station(&s, 0) == Tasks.station(0) && Data::station(&s, 1).is_none());
    assert!(s.beacon() == Tasks.beacon());
    assert_eq!((s.sats(), s.sat(0).is_none(), s.skips(), s.skip(0)), (0, true, 0, None));
}

/// INBOX-3.1: at power-on the mail badge counts the unread messages to us (a read one or ours doesn't count). On
/// the radio the log entry, the message and the heard row wait in the queue until the Storage task writes them;
/// meanwhile there is no room for a link's channel records (LINK-41.1: only channel writes may queue together).
#[test]
fn the_radio_queue_and_the_mail_badge() {
    use ferrite_app::uistate;
    use ferrite_core::msg::{Device, LiveAll};
    let s = mount(SimFlash::new());
    s.set_deferred(true);
    assert!(s.log(&ev(Src::Aprs, 1)));
    assert!(s.message_in(true, b"VE3KPW-9", b"QSL?", 0));
    assert!(s.message_in(true, b"VA3RTX", b"Read me", 0));
    assert!(s.message_in(false, b"VA3RTX", b"Ours", 0));
    assert!(s.heard_row(&st(b"VE3OCB-7", b"", 1)));
    assert!(s.has_jobs() && !s.idle() && s.busy());
    assert_eq!((s.write_room(), s.events(), s.messages(), s.heard_stored()), (0, 0, 0, 0), "nothing written yet");
    while let Some(j) = s.next_job() {
        assert!(ferrite_store::block(s.run(j)));
    }
    assert!(s.idle() && !s.has_jobs() && s.write_room() > 0);
    assert_eq!((s.events(), s.messages(), s.heard_stored()), (1, 3, 1));
    assert!(s.request(Op::MessageRead(1)));
    while let Some(j) = s.next_job() {
        assert!(ferrite_store::block(s.run(j)));
    }
    let s = cycle(s);
    let mut a = app();
    assert!(!s.restore(&mut a), "no settings stored");
    let (dev, live) = (Device::default(), LiveAll::default());
    let mut l = ferrite_app::term::cmd::Line::new();
    let bar = (0..)
        .map_while(|i| uistate::line(&a, &s, &dev, &live, i, &mut l).then(|| String::from_utf8_lossy(l.as_bytes()).into_owned()))
        .find(|t| t.starts_with("bar"))
        .unwrap();
    assert!(bar.contains(" inbox=1 "), "{bar}");
}

/// APRS-4.5: the heard list keeps only its own fields in RAM for stations restored at power-on; the store fills
/// the path, course, altitude, comment and FX.25 from each station's stored row when it is shown.
#[test]
fn heard_details_come_from_the_rows() {
    use ferrite_app::radio_data::RadioData;
    let s = FlashStore::mount(SimFlash::new(), RadioData::new());
    s.set_time(1_800_000_000);
    assert!(s.heard_row(&st(b"VE3KPW-9", b"Mobile", 1)));
    assert!(s.heard_row(&st(b"VA3RTX", b"Home", 4)));
    s.autosave(30_000, &app());
    let s = FlashStore::mount(s.into_parts().0, RadioData::new());
    s.heard_rows(50, &mut |x, seq| s.extra.aprs.restore(&x, seq, 0));
    assert_eq!(s.extra.aprs.station(1).unwrap().comment.as_bytes(), b"", "RAM alone: no comment");
    let got = Data::station(&s, 1).unwrap();
    let want = Station { ago_s: AGO_UNKNOWN, ..st(b"VE3KPW-9", b"Mobile", 1) };
    let k = |x: &Station| (x.call, x.via, x.course, x.alt_m, x.comment, x.fx25, x.times, x.ago_s, x.pos);
    assert!(k(&got) == k(&want), "every field from the row: via {:?} course {:?} alt {:?} comment {:?} fx25 {:?} times {} pos {}", got.via.as_bytes(), got.course, got.alt_m, got.comment.as_bytes(), got.fx25, got.times, got.pos.is_some());
    assert!(Data::station(&s, 0).unwrap().comment.as_bytes() == b"Home", "newest first");
}

/// APRS-13.1, APRS-13.2, APRS-13.3, APRS-13.5, APRS-46 with the radio's APRS task behind the store: Stop sending
/// keeps the message as cancelled (across a power cycle) and the task tries it no more; Send again queues it as a
/// new message; a fifth is refused before it is saved; "Delete all messages" stops every try; ours still open at
/// power-off end as failed at power-on.
#[test]
fn messages_stop_resend_and_room() {
    use ferrite_app::radio_data::RadioData;
    let mount = |f: SimFlash| {
        let d = RadioData::new();
        d.set_callsign(b"VA7FER-9");
        FlashStore::mount(f, d)
    };
    let send = |s: &FlashStore<SimFlash, RadioData>, t: &[u8]| s.request(Op::Draft(Draft { to: Buf::from(&b"VE3KPW"[..]), text: Buf::from(t) })) && s.request(Op::Send);
    let s = mount(SimFlash::new());
    for t in [&b"one"[..], b"two", b"three", b"four"] {
        assert!(s.send_room() && send(&s, t));
    }
    assert!(!s.send_room());
    assert!(!send(&s, b"five"), "a fifth refused");
    assert_eq!(s.messages(), 4, "and not saved, so never left pending");
    assert!(s.request(Op::StopMessage(3))); // "one", the oldest
    assert_eq!(s.message(3).map(|m| m.state), Some(MsgState::Cancelled));
    assert!(s.send_room(), "the task dropped it");
    assert!(!s.request(Op::StopMessage(3)), "not being sent any more");
    assert!(s.request(Op::ResendMessage(3)));
    assert_eq!(s.message(0).map(|m| (m.text, m.state)), Some((Buf::from(&b"one"[..]), MsgState::Pending)));
    assert_eq!(s.message(4).map(|m| m.state), Some(MsgState::Cancelled), "the stopped one kept");
    assert!(!s.request(Op::ResendMessage(1)), "only failed or cancelled ones");
    assert!(!s.send_room());
    let (f, _) = s.into_parts();
    let s = mount(f);
    assert_eq!(s.message(4).map(|m| m.state), Some(MsgState::Cancelled), "kept across a power cycle");
    assert!((0..4).all(|i| s.message(i).is_some_and(|m| m.state == MsgState::Failed)), "open ones failed at power-on (APRS-13.5)");
    assert!(send(&s, b"six") && send(&s, b"seven"));
    assert!(s.request(Op::ClearMessages));
    assert!(s.send_room() && s.extra.aprs.room(), "deleted: tried no more");
    for t in [&b"a"[..], b"b", b"c", b"d"] {
        assert!(send(&s, t), "room for four again");
    }
}

/// APRS-46.1: Delete message takes one message either way: the list skips it at once and after a power cycle;
/// one of ours being tried is tried no more; one to us unread no longer counts as unread; the indices close up
/// and a new message lands in front. When the ring wraps over a deleted record's slot, the new one there shows.
#[test]
fn delete_one_message() {
    use ferrite_app::radio_data::RadioData;
    let mount = |f: SimFlash| {
        let d = RadioData::new();
        d.set_callsign(b"VA7FER-9");
        FlashStore::mount(f, d)
    };
    let text = |s: &FlashStore<SimFlash, RadioData>| (0..s.messages()).map(|i| s.message(i).unwrap().text).collect::<Vec<_>>();
    let s = mount(SimFlash::new());
    assert!(s.message_in(true, b"VE3KPW-9", b"first", 0));
    assert!(s.request(Op::Draft(Draft { to: Buf::from(&b"VE3KPW"[..]), text: Buf::from(&b"second"[..]) })) && s.request(Op::Send));
    assert!(s.message_in(true, b"VE3ABC", b"third", 0));
    assert_eq!(s.messages(), 3);
    assert!(!s.extra.aprs.room() || s.send_room());
    // ours, still pending: deleted, and the APRS task drops it
    assert_eq!(s.message(1).map(|m| (m.inbound, m.state)), Some((false, MsgState::Pending)));
    assert!(s.request(Op::DeleteMessage(1)));
    assert_eq!(text(&s), [Buf::from(&b"third"[..]), Buf::from(&b"first"[..])], "skipped, the rest close up");
    assert!(s.send_room(), "tried no more");
    // one to us, unread
    assert!(s.request(Op::DeleteMessage(1)));
    assert_eq!(text(&s), [Buf::from(&b"third"[..])]);
    assert!(!s.request(Op::DeleteMessage(1)), "past the end");
    let (f, _) = s.into_parts();
    let s = mount(f);
    assert_eq!(text(&s), [Buf::from(&b"third"[..])], "still gone after a power cycle");
    assert!(s.message_in(true, b"VE3ABC", b"fourth", 0));
    assert_eq!(text(&s), [Buf::from(&b"fourth"[..]), Buf::from(&b"third"[..])], "a new one in front");
    // Wrap the ring: every slot written again, so nothing deleted is left and every record shows.
    for k in 0..520u32 {
        assert!(s.message_in(true, b"VE3ABC", format!("n{k}").as_bytes(), 0));
    }
    let n = s.messages();
    assert!(n >= 465, "the ring of 496 full of new ones (a sector is erased ahead as it wraps): {n}");
    for i in 0..n {
        assert_eq!(s.message(i).unwrap().text.as_bytes(), format!("n{}", 519 - i).as_bytes(), "none hidden: {i}");
    }
    assert!(s.request(Op::DeleteMessage(0)));
    assert_eq!(s.messages(), n - 1);
    assert_eq!(s.message(0).unwrap().text.as_bytes(), b"n518");
}

/// A Discord report on 0.9.7 (three radios on one base call, Ferrite VA7FER-2): frames through the firmware APRS
/// task's path (the APRS task, the heard row, the inbox and the event log), then the station screen as it is shown.
/// APRS-28, APRS-28.1 (ferrite#184): the Message line is the newest message the station was heard sending, to
/// anyone, with its addressee: "6" to TEST after "3" to us showed "3" (the newest to us), and a station that sent
/// only to others showed none. APRS-3: a message to another SSID of ours (APRS-6.3: not ours) is still heard.
/// INBOX-6: the event log takes messages to us and to everyone only, so a message to TEST and a position beacon are
/// in the heard list but not the log.
#[test]
fn heard_shows_every_message_the_log_only_ours() {
    use ferrite_app::aprs_live::uaprs::ax25::{Address, Frame};
    use ferrite_app::aprs_live::FRAME_MAX;
    use ferrite_app::radio_data::RadioData;
    let me = b"VA7FER-2";
    let d = RadioData::new();
    d.set_callsign(me);
    let s = FlashStore::mount(SimFlash::new(), d);
    let cfg = AprsCfg::DEFAULT;
    // firmware/src/tasks/aprs.rs, Aprs::poll: a frame heard
    let hear = |from: &[u8], info: &[u8], now: u32| {
        let mut b = [0u8; FRAME_MAX];
        let n = Frame::new(Address::from_text(from).unwrap(), Address::from_text(b"APDR16").unwrap(), &[], info).unwrap().write(&mut b).unwrap();
        let rx = s.extra.aprs.receive(&b[..n], now, me, &cfg);
        if let Some(st) = Data::station(&s, 0).filter(|_| rx.row) {
            s.heard_row(&st);
        }
        if let Some((from, text)) = rx.inbound.filter(|(f, t)| !s.message_heard(f.as_bytes(), rx.inbound_id.as_bytes(), t.as_bytes())) {
            s.message_to_us(from.as_bytes(), text.as_bytes(), rx.inbound_id.as_bytes(), rx.inbound_to.as_bytes());
            s.log(&Event { src: Src::Aprs, ago_s: 0, freq: Default::default(), slot: 2, unread: true, id: 0, x: 0, y: 0, pos: None, name: Buf::from(from.as_bytes()), text: Buf::from(text.as_bytes()) });
        }
        if let Some((from, to, text)) = rx.notice {
            let mut t = Buf::<64>::from(b"To ");
            t.push(to.as_bytes());
            t.push(b": ");
            t.push(text.as_bytes());
            s.log(&Event { src: Src::Aprs, ago_s: 0, freq: Default::default(), slot: 2, unread: true, id: 0, x: 0, y: 0, pos: None, name: Buf::from(from.as_bytes()), text: t });
        }
    };
    // the station screen's Message line (APRS-28): its station, with the clock at minute 600
    let line = |call: &[u8]| {
        let i = (0..Data::stations(&s)).find(|&i| Data::station(&s, i).unwrap().call.as_bytes() == call).expect("in the heard list (APRS-3)");
        let mut st = Data::station(&s, i).unwrap();
        ferrite_app::screens::aprs::last_message(&mut st, &s, Some(600));
        (st.msg_to.as_bytes().to_vec(), st.msg.as_bytes().to_vec(), st.msg_min)
    };
    let log = || (0..s.events()).map(|i| s.event(i).unwrap().text.as_bytes().to_vec()).collect::<Vec<_>>();
    hear(b"VA7FER-1", b":VA7FER   :3{1", 100);
    assert_eq!(line(b"VA7FER-1"), (b"VA7FER".to_vec(), b"3".to_vec(), Some(600)), "to us, plain call (APRS-6.3)");
    hear(b"VA7FER-1", b":TEST     :6{2", 160);
    assert_eq!(line(b"VA7FER-1"), (b"TEST".to_vec(), b"6".to_vec(), Some(600)), "symptom A: the newest heard, not the newest to us");
    hear(b"VA7FER-4", b":TEST     :6", 220);
    assert_eq!(line(b"VA7FER-4"), (b"TEST".to_vec(), b"6".to_vec(), Some(600)), "symptom A: sent only to others");
    hear(b"VA7FER-1", b":VA7FER-5 :to five{3", 280);
    assert_eq!(line(b"VA7FER-1"), (b"VA7FER-5".to_vec(), b"to five".to_vec(), Some(600)), "symptom B1: another SSID of ours, heard");
    assert_eq!(s.messages(), 1, "APRS-6.3: only the one to us in the inbox");
    hear(b"VA7FER-3", b"!4903.50N/07201.75W-Home", 340);
    assert!(line(b"VA7FER-3").1.is_empty(), "a position beacon: heard, no message");
    assert_eq!(log(), [b"3".to_vec()], "INBOX-6: only the message to us in the event log (B2, B3: by design)");
    hear(b"VA7FER-4", b":CQ       :net", 400);
    assert_eq!(line(b"VA7FER-4"), (b"CQ".to_vec(), b"net".to_vec(), Some(600)));
    assert_eq!(log()[0], b"To CQ: net".to_vec(), "APRS-6.4: to everyone is logged");
    // 57 minutes on: the clock time it came
    s.extra.aprs.receive(&[], 3_700, me, &cfg);
    assert_eq!(line(b"VA7FER-1").2, Some(600 - (3_700 - 280) / 60));
}
