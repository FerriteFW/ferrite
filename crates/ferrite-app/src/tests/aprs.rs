//! APRS mode on slot C (APRS-13, APRS-15 to APRS-20), beacon modes (APRS-24) and the simulator's APRS task.

use super::*;
use crate::aprs::{beacon_due, AprsCfg, LastBeacon, APRS_SLOT, BEACON_STEPS, SMART};
use crate::data::{Fix, Pos};
use crate::data::{Data, Draft, MsgState, Op};
use crate::sample::Sample;
use ferrite_core::txplan::Region;

/// An app on the sample records, kept for the whole test.
struct A {
    app: App,
    s: Sample,
    t: u32,
    live: LiveAll,
}

impl A {
    fn new() -> Self {
        let mut app = App::new(crate::tests::set(), crate::tests::alert());
        app.set.backlight = 0;
        while app.pop_cmd().is_some() {}
        let mut a = A { app, s: Sample::new(), t: 1000, live: LiveAll::default() };
        a.wait(100);
        a
    }

    fn tick(&mut self) {
        self.app.tick(self.t, &self.live, &self.s);
        while let Some(w) = self.app.pop_write() {
            self.s.apply(w);
        }
    }

    fn press(&mut self, k: Key, hold: u32) {
        self.app.key_down(k, self.t, &self.s);
        let end = self.t + hold;
        while self.t < end {
            self.t += 10;
            self.tick();
        }
        self.app.key_up(k, self.t, &self.s);
        self.t += 50;
        self.tick();
    }

    fn tap(&mut self, k: Key) {
        self.press(k, 100);
    }

    fn wait(&mut self, ms: u32) {
        let end = self.t + ms;
        while self.t < end {
            self.t += 100;
            self.tick();
        }
    }

    fn cmds(&mut self) -> Vec<RadioCmd> {
        let mut v = Vec::new();
        while let Some(c) = self.app.pop_cmd() {
            v.push(c);
        }
        v
    }

    fn mode(&mut self, on: bool) {
        self.app.set_aprs_mode(on);
        self.tick();
    }
}

/// APRS-15, APRS-16: C tunes to the region's APRS frequency at the beacon power and is watched whatever Watch
/// says; off, C goes back exactly as it was.
#[test]
fn aprs_mode_takes_slot_c() {
    let mut a = A::new();
    let own = a.app.slot(APRS_SLOT).freq();
    a.app.set_watch(0);
    a.cmds();
    a.mode(true);
    let c = a.cmds();
    assert!(c.contains(&RadioCmd::TuneSlot { slot: 2, freq: 14_439_000 }), "no region: 144.390");
    assert!(c.contains(&RadioCmd::Watch(0b100)), "C watched with Watch off: {c:?}");
    assert!(c.contains(&RadioCmd::Awake(0b100)), "C's chip stays awake (decision 111)");
    assert!(c.contains(&RadioCmd::SlotParam { slot: 2, param: Param::Power, value: 0 }), "beacon power Low");
    assert!(c.contains(&RadioCmd::SlotParam { slot: 2, param: Param::Mode, value: 0 }), "FM");
    assert_eq!(a.app.slot(APRS_SLOT).freq(), own, "C's own VFO is kept aside");
    // The region's frequency, and 0 5 4 over it.
    for (r, f) in [(Region::Uk, 14_480_000), (Region::Australia, 14_517_500), (Region::NewZealand, 14_457_500), (Region::Japan, 14_464_000)] {
        a.app.set.set_region(Some(r));
        assert_eq!(a.app.aprs_freq(), f);
    }
    a.app.set.set_region(Some(Region::Canada));
    a.cmds();
    a.app.step_aprs_freq(2);
    assert_eq!(a.app.aprs_freq(), 14_440_000);
    assert!(a.cmds().contains(&RadioCmd::TuneSlot { slot: 2, freq: 14_440_000 }));
    a.tick();
    assert_ne!(a.s.aprs_cfg().f5k, 0, "written back to the APRS store");
    a.app.step_aprs_freq(-2);
    assert_eq!(a.app.aprs_cfg().f5k, 0, "back on the region's: follows the region again");
    a.mode(false);
    let c = a.cmds();
    assert!(c.contains(&RadioCmd::TuneSlot { slot: 2, freq: own }), "C back as it was");
    assert!(c.contains(&RadioCmd::Watch(0)));
    assert!(c.contains(&RadioCmd::Awake(0)));
}

/// APRS-19: C can be the active slot, but tuning, V/M, entry, the edit grid and quick settings refuse.
#[test]
fn aprs_slot_is_locked() {
    let mut a = A::new();
    a.mode(true);
    a.tap(Key::Slot);
    a.tap(Key::Slot);
    assert_eq!(a.app.active(), 2);
    a.cmds();
    for k in [Key::Up, Key::Down, Key::Left, Key::VfoMem, Key::Digit(1)] {
        a.tap(k);
        assert!(matches!(a.app.screen(), Screen::Home), "{k:?}");
        assert!(a.app.toast_up());
    }
    // MENU-34: hold ABC is the APRS quick page, not C's edit grid
    a.press(Key::Slot, 800);
    assert!(matches!(a.app.screen(), Screen::QGrid(g) if g.page == crate::screens::qgrid::Page::Aprs), "no edit grid");
    a.tap(Key::Exit);
    assert!(matches!(a.app.screen(), Screen::Home));
    a.app.knob(1, a.t, &a.s);
    assert!(!a.cmds().iter().any(|c| matches!(c, RadioCmd::TuneSlot { .. } | RadioCmd::SlotParam { .. })), "nothing changed C");
    assert!(a.app.greyed(Item::Mode).is_some() && a.app.greyed(Item::Power).is_none() && a.app.greyed(Item::Watch).is_none());
    // Hold MENU on it: its options (APRS-17.1).
    a.press(Key::Menu, 800);
    assert!(matches!(a.app.screen(), Screen::Options(o) if o.of == crate::screens::opts::Of::AprsCard));
    // Off: C changes again.
    a.press(Key::Exit, 800);
    a.mode(false);
    a.tap(Key::Up);
    assert!(a.cmds().iter().any(|c| matches!(c, RadioCmd::TuneSlot { slot: 2, .. })));
}

/// KISS-9 (decision 173): KISS on (Bluetooth with Bluetooth on, or the cable) makes C the APRS slot with APRS mode
/// off: tuned to the APRS frequency, watched and awake, locked with KISS's message, a data key-up's frequency the
/// APRS one; KISS off gives C back unless APRS mode is on.
#[test]
fn kiss_on_makes_c_the_aprs_slot() {
    let mut a = A::new();
    let own = a.app.slot(APRS_SLOT).freq();
    a.app.set_watch(0);
    a.cmds();
    assert!(!a.app.aprs_slot(APRS_SLOT));
    a.app.set.bt = true;
    a.app.set.kiss_port = 2; // 0 5 8 on Bluetooth
    a.tick();
    assert!(a.app.aprs_slot(APRS_SLOT) && !a.app.aprs_on(), "C is the APRS slot, APRS mode still off");
    let c = a.cmds();
    assert!(c.contains(&RadioCmd::TuneSlot { slot: 2, freq: 14_439_000 }), "tuned to APRS: {c:?}");
    assert!(c.contains(&RadioCmd::Watch(0b100)) && c.contains(&RadioCmd::Awake(0b100)) && c.contains(&RadioCmd::AprsSlot(0b100)));
    assert_eq!(crate::screens::transmit::tx_freq(&a.app, APRS_SLOT), 14_439_000, "frames go out on the APRS frequency");
    // Locked as APRS-19, with KISS's message.
    a.tap(Key::Slot);
    a.tap(Key::Slot);
    assert_eq!(a.app.active(), 2);
    assert_eq!(a.app.greyed(Item::Mode), Some("Slot C held by KISS"));
    a.cmds();
    a.tap(Key::Up);
    assert!(a.app.toast_up());
    assert!(!a.cmds().iter().any(|c| matches!(c, RadioCmd::TuneSlot { .. })), "C stays put");
    // APRS mode on and off again while KISS holds C: C stays APRS.
    a.mode(true);
    a.mode(false);
    assert!(a.app.aprs_slot(APRS_SLOT));
    // Bluetooth off: KISS over Bluetooth is off too, and C comes back.
    a.cmds();
    a.app.set.bt = false;
    a.tick();
    let c = a.cmds();
    assert!(!a.app.aprs_slot(APRS_SLOT));
    assert!(c.contains(&RadioCmd::TuneSlot { slot: 2, freq: own }) && c.contains(&RadioCmd::Watch(0)), "{c:?}");
    // The cable's KISS (0 5 8 on Cable) does it too, and APRS mode keeps C when KISS goes off.
    a.app.set.kiss_port = 1;
    a.tick();
    assert!(a.app.aprs_slot(APRS_SLOT));
    assert_eq!(a.app.greyed(Item::Mode), Some("Slot C held by KISS"));
    a.mode(true);
    a.app.set.kiss_port = 0;
    a.tick();
    assert!(a.app.aprs_slot(APRS_SLOT), "APRS mode still has C");
}

/// APRS-29, APRS-21.1, APRS-19, KISS-9 (decision 181): automatic beacons with APRS mode off hold C as the APRS slot
/// (tuned to the APRS frequency, watched, locked; the lock names what holds C, nothing more) but give it no receive
/// priority (not kept awake); manual beacons don't hold it. APRS mode and KISS give priority. The holder's
/// precedence is APRS mode, KISS, beacons.
#[test]
fn beacons_alone_hold_c_without_priority() {
    use crate::aprs::Holder;
    let mut a = A::new();
    let own = a.app.slot(APRS_SLOT).freq();
    a.app.set_watch(0);
    a.cmds();
    a.app.set_aprs(AprsCfg { beacon: 1, ..a.app.aprs_cfg() }); // manual: only "Beacon now"
    a.tick();
    assert_eq!(a.app.aprs_holder(), None, "manual beacons don't hold C");
    a.app.set_aprs(AprsCfg { beacon: 5, ..a.app.aprs_cfg() }); // every 10 min
    a.tick();
    assert_eq!(a.app.aprs_holder(), Some(Holder::Beacon));
    assert!(a.app.aprs_slot(APRS_SLOT) && !a.app.aprs_prio() && !a.app.aprs_on());
    let c = a.cmds();
    assert!(c.contains(&RadioCmd::TuneSlot { slot: 2, freq: 14_439_000 }), "tuned to APRS: {c:?}");
    assert!(c.contains(&RadioCmd::Watch(0b100)), "an ordinary watched slot: {c:?}");
    assert!(c.contains(&RadioCmd::Awake(0)) && c.contains(&RadioCmd::AprsSlot(0b100)), "no priority, but the APRS slot: {c:?}");
    assert!(a.app.toast_up());
    a.tap(Key::Slot);
    a.tap(Key::Slot);
    assert_eq!(a.app.active(), 2);
    assert_eq!(a.app.greyed(Item::Mode), Some("Slot C held by beacons"));
    a.cmds();
    a.tap(Key::Up);
    assert!(a.app.toast_up());
    assert!(!a.cmds().iter().any(|c| matches!(c, RadioCmd::TuneSlot { .. })), "C stays put");
    // APRS mode on: it holds C (first in precedence) with priority; off again, beacons still hold it, no priority.
    a.mode(true);
    assert_eq!(a.app.aprs_holder(), Some(Holder::Mode));
    assert!(a.cmds().contains(&RadioCmd::Awake(0b100)));
    assert_eq!(a.app.greyed(Item::Mode), Some("Slot C held by APRS mode"));
    a.mode(false);
    assert_eq!(a.app.aprs_holder(), Some(Holder::Beacon));
    assert!(a.cmds().contains(&RadioCmd::Awake(0)));
    // KISS on beside the beacons: KISS holds C, with priority; off, the beacons again.
    a.app.set.bt = true;
    a.app.set.kiss_port = 2;
    a.tick();
    assert_eq!(a.app.aprs_holder(), Some(Holder::Kiss));
    assert!(a.cmds().contains(&RadioCmd::Awake(0b100)), "KISS: priority");
    a.app.set.kiss_port = 0;
    a.tick();
    assert_eq!(a.app.aprs_holder(), Some(Holder::Beacon));
    assert!(a.cmds().contains(&RadioCmd::Awake(0)));
    // Beacons off: C comes back.
    a.app.set_aprs(AprsCfg { beacon: 0, ..a.app.aprs_cfg() });
    a.tick();
    assert_eq!(a.app.aprs_holder(), None);
    let c = a.cmds();
    assert!(c.contains(&RadioCmd::TuneSlot { slot: 2, freq: own }) && c.contains(&RadioCmd::AprsSlot(0)), "{c:?}");
}

/// APRS-18: with the APRS slot (C) active, PTT A opens compose from any screen without keying; in compose PTT
/// sends. PTT B keys B as ever.
#[test]
fn ptt_on_aprs_slot_composes_and_ptt_sends() {
    let mut a = A::new();
    a.mode(true);
    a.app.active = 2;
    a.app.open(crate::screens::nav::Dest::AprsHeard);
    a.cmds();
    a.press(Key::Ptt(0), 100);
    assert!(matches!(a.app.screen(), Screen::Compose(_)), "{}", a.app.screen().name());
    assert!(!a.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. })), "no voice");
    a.s.request(Op::Draft(Draft { to: Buf::from(b"VE3KPW-9".as_slice()), text: Buf::from(b"On my way".as_slice()) }));
    let n = a.s.messages();
    a.press(Key::Ptt(0), 100);
    assert_eq!(a.s.messages(), n + 1, "sent");
    assert!(a.s.message(0).is_some_and(|m| !m.inbound && m.state == MsgState::Pending));
    assert!(!matches!(a.app.screen(), Screen::Compose(_)));
    assert!(!a.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. })));
    // The connector's PTT on the APRS slot composes too; PTT B never reaches C.
    a.press(Key::Exit, 800);
    a.press(Key::PTT, 100);
    assert!(matches!(a.app.screen(), Screen::Compose(_)));
    a.press(Key::Exit, 100);
    a.press(Key::Exit, 800);
    a.app.key_down(Key::Ptt(1), a.t, &a.s);
    assert!(!matches!(a.app.screen(), Screen::Compose(_)));
    a.app.key_up(Key::Ptt(1), a.t + 100, &a.s);
}

/// Beacon mode and power (0 5 6 1, 0 5 6 3, APRS-17.1); a mode paused by the old "Beacon on/off" (decision 375
/// removed it) reads as off.
#[test]
fn beacon_settings() {
    let mut a = A::new();
    let mut b = Buf::<24>::new();
    a.app.beacon_mode_text(&mut b);
    assert_eq!(b.as_bytes(), b"Off");
    a.app.step_beacon(1);
    a.app.beacon_mode_text(&mut b);
    assert_eq!(b.as_bytes(), b"Manual");
    a.app.set_aprs(AprsCfg { beacon: 5 | 0x80, ..a.app.aprs_cfg() });
    assert_eq!(a.app.aprs_cfg().mode(), 0, "paused: off");
    a.app.beacon_mode_text(&mut b);
    assert_eq!(b.as_bytes(), b"Off");
    a.app.set_aprs(AprsCfg { power: 2, ..a.app.aprs_cfg() });
    a.mode(true);
    assert!(a.cmds().contains(&RadioCmd::SlotParam { slot: 2, param: Param::Power, value: 2 }));
}

/// APRS-21: the card says the VHF chip is shared while A or B listens on chip 0 too (active or watched).
#[test]
fn aprs_shares_the_vhf_chip() {
    let mut a = A::new();
    for s in &mut a.app.slots[..2] {
        s.vfo_mode = true;
    }
    a.app.slots[0].vfo = 44_600_000;
    a.app.slots[1].vfo = 14_652_000;
    a.app.set_watch(0);
    a.mode(true);
    assert_eq!(a.app.aprs_shared(), None, "A on UHF, B not watched");
    a.app.set_watch(1);
    assert_eq!(a.app.aprs_shared(), Some(1), "B on 2 m, watched");
    a.app.slots[0].vfo = 14_652_000;
    assert_eq!(a.app.aprs_shared(), Some(0), "the active slot listens whatever Watch says");
    a.mode(false);
    assert_eq!(a.app.aprs_shared(), None);
}

/// 0 5 6 1 steps through off, manual, the timed modes and smart, and wraps (APRS-24, decision 272).
#[test]
fn beacon_modes_in_order() {
    let mut a = A::new();
    let mut seen = std::vec::Vec::new();
    for _ in 0..BEACON_STEPS {
        let mut b = Buf::<24>::new();
        a.app.beacon_mode_text(&mut b);
        seen.push(std::string::String::from_utf8(b.as_bytes().to_vec()).unwrap());
        a.app.step_beacon(1);
    }
    assert_eq!(seen[..3], ["Off", "Manual", "Every 1 min"]);
    assert_eq!(seen[9..], ["Every 60 min", "Smart"]);
    assert_eq!(a.app.aprs_cfg().mode(), 0, "wraps to off");
    a.app.step_beacon(-1);
    assert!(a.app.aprs_cfg().smart(), "and back to smart");
    a.app.step_beacon(-1);
    assert_eq!(a.app.aprs_cfg().every_min(), Some(60));
    a.app.step_beacon(2);
    assert_eq!(a.app.aprs_cfg().mode(), 0, "wraps to off");
}

fn fix(lat: i32, speed_kmh: u16, heading: u16) -> Option<Fix> {
    Some(Fix { pos: Pos { lat, lon: -794_000_000 }, three_d: true, alt_m: 0, speed_kmh, heading, hdop_x10: 9, used: 8, age_s: 1, utc_s: 0, phone: false })
}

/// APRS-24: smart beacons on the one curve (decision 271): parked 30 min, a walk every 500 m, 10 km/h on 3 min.
#[test]
fn smart_beacons() {
    let c = AprsCfg { beacon: SMART as u8, ..AprsCfg::DEFAULT };
    assert!(c.smart() && c.every_min().is_none());
    assert!(AprsCfg { beacon: 13, ..AprsCfg::DEFAULT }.smart(), "a saved distance mode (decision 272) is smart");
    let here = 437_000_000;
    let last = Some(LastBeacon::new(0, fix(here, 50, 90)));
    assert!(!beacon_due(&c, last, 300, fix(here, 2, 90)), "parked: 30 min");
    assert!(beacon_due(&c, last, 1800, fix(here, 2, 90)));
    assert!(!beacon_due(&c, last, 350, fix(here, 5, 90)), "walking 5 km/h: every 6 min");
    assert!(beacon_due(&c, last, 360, fix(here, 5, 90)));
    assert!(!beacon_due(&c, last, 170, fix(here, 20, 90)), "a bike: 3 min");
    assert!(beacon_due(&c, last, 180, fix(here, 20, 90)));
    assert!(!beacon_due(&c, last, 170, fix(here, 110, 90)), "fast: every 3 min");
    assert!(beacon_due(&c, last, 180, fix(here, 110, 90)));
    assert!(beacon_due(&c, last, 40, fix(here, 60, 180)), "a 90 degree turn after 30 s");
    assert!(!beacon_due(&c, last, 20, fix(here, 60, 180)), "not within 30 s of the last");
    assert!(beacon_due(&c, last, 1800, None), "no fix: the slow rate");
}

/// The simulator's APRS task: retries on APRS-13's schedule, an ack from a heard station on the second try,
/// failed after five tries, messages for us, beacons waiting for the relay (APRS-20).
#[test]
fn engine_timeline() {
    let s = Sample::new();
    let draft = |to: &[u8]| Draft { to: Buf::from(to), text: Buf::from(b"Test".as_slice()) };
    s.request(Op::Draft(draft(b"VE3KPW-9")));
    assert!(s.request(Op::Send));
    let n = s.aprs_tick(0, true, true);
    assert_eq!(n.tx, Some("Message"));
    assert_eq!(s.message(0).unwrap().state, MsgState::Pending);
    for t in 1..30 {
        assert!(s.aprs_tick(t, true, true).tx != Some("Message"), "{t}");
    }
    assert_eq!(s.aprs_tick(30, true, true).tx, Some("Message"));
    assert_eq!(s.message(0).unwrap().state, MsgState::Retrying(2));
    for t in 31..34 {
        s.aprs_tick(t, true, true);
    }
    assert_eq!(s.message(0).unwrap().state, MsgState::Acked, "a heard station acks the second try");

    // Nobody heard: tried 5 times at 0, 30, 90, 210 and 450 s, failed 240 s after the last.
    let s = Sample::new();
    s.request(Op::Draft(draft(b"NOCALL")));
    s.request(Op::Send);
    let mut tries = Vec::new();
    for t in 0..800 {
        let n = s.aprs_tick(t, true, true);
        if n.tx == Some("Message") {
            tries.push(t);
        }
        let m = (0..s.messages()).filter_map(|i| s.message(i)).find(|m| !m.inbound).unwrap();
        if t == 689 {
            assert_eq!(m.state, MsgState::Retrying(5));
        }
        if t == 690 {
            assert_eq!(m.state, MsgState::Failed);
        }
    }
    assert_eq!(tries, [0, 30, 90, 210, 450]);
    assert!(s.stations() > 11, "new stations heard");
    assert!((0..s.messages()).filter_map(|i| s.message(i)).any(|m| m.inbound && m.state == MsgState::Unread && m.ago_s < 800));

    // A timed beacon waits while the relay is the other way, then goes.
    let s = Sample::new();
    s.request(Op::AprsCfg(AprsCfg { beacon: 2, ..AprsCfg::DEFAULT })); // every 1 min
    assert_eq!(s.aprs_tick(0, true, false).tx, None);
    assert!(s.beacon().waiting);
    let mut sent = None;
    for t in 1..10 {
        let n = s.aprs_tick(t, true, true);
        if n.tx == Some("Beacon") {
            sent = Some(t);
            break;
        }
    }
    assert_eq!(sent, Some(1), "as soon as the relay allows");
    assert!(!s.beacon().waiting);
    assert_eq!(s.beacon().next_s, Some(60));
    // Off: nothing goes.
    for t in 100..400 {
        assert_eq!(s.aprs_tick(t, false, true).tx, None);
    }
}

/// APRS-17.2, APRS-20: the card's lines.
#[test]
fn card_text() {
    let mut a = A::new();
    a.mode(true);
    a.app.set_aprs(AprsCfg { beacon: 2, ..a.app.aprs_cfg() });
    a.tick();
    a.s.aprs_tick(0, true, false);
    let mut b = Buf::<28>::new();
    a.app.set_vfo(0, 1_407_400); // A on HF holds the relay
    a.app.beacon_text(&a.s, &mut b);
    assert_eq!(b.as_bytes(), b"Beacon waiting: HF");
    a.s.aprs_tick(1, true, true);
    a.app.beacon_text(&a.s, &mut b);
    assert!(b.as_bytes().starts_with(b"Beacon in "), "{:?}", core::str::from_utf8(b.as_bytes()));
    let mut l = Buf::<80>::new();
    crate::aprs::last_text(&a.s, &mut l);
    assert!(l.as_bytes().starts_with(b"VE3KPW-9"), "{:?}", core::str::from_utf8(l.as_bytes()));
    let mut fb = FrameBuffer::new();
    let dev = Device::default();
    a.app.render(&mut fb, &a.live, &dev, &a.s);
}

/// APRS-26, APRS-26.1, APRS-27: 0 5 6 2, 0 5 9 1 (with the APRSAT and ARISS presets) and 0 5 7 change the APRS
/// settings, and # puts each back.
#[test]
fn path_digipeater_and_after_voice_settings() {
    use crate::screens::groups::{change, reset};
    let mut a = A::new();
    assert_eq!(&*a.app.aprs_cfg().path(), &[b"WIDE1-1".as_slice(), b"WIDE2-1"]);
    change(&mut a.app, &[5, 9, 1], 1);
    assert_eq!(&*a.app.aprs_cfg().path(), &[b"APRSAT".as_slice()], "APRS-26.1: WIDE1-1,WIDE2-1 then APRSAT");
    change(&mut a.app, &[5, 9, 1], 1);
    assert_eq!(&*a.app.aprs_cfg().path(), &[b"ARISS".as_slice()], "then ARISS");
    change(&mut a.app, &[5, 9, 1], 1);
    assert!(a.app.aprs_cfg().path().is_empty(), "then None");
    change(&mut a.app, &[5, 9, 1], 1);
    assert_eq!(&*a.app.aprs_cfg().path(), &[b"WIDE1-1".as_slice()]);
    reset(&mut a.app, &[5, 9, 1]);
    assert_eq!(a.app.aprs_cfg().path().len(), 2);
    assert!(!a.app.aprs_cfg().digi_on(), "APRS-12: off by default");
    change(&mut a.app, &[5, 7, 1], 1);
    change(&mut a.app, &[5, 7, 2], 1);
    change(&mut a.app, &[5, 7, 3], -1);
    let c = a.app.aprs_cfg();
    assert!(c.digi_on() && c.digi_mycall() && c.digi_wait_s() == 5);
    reset(&mut a.app, &[5, 7, 3]);
    assert!(a.app.aprs_cfg().digi_on() && a.app.aprs_cfg().digi_wait_s() == 0);
    change(&mut a.app, &[5, 6, 2], 1);
    assert!(a.app.aprs_cfg().after_voice);
    // Kept by the APRS store's owner (written back at the next tick).
    a.tick();
    assert!(a.s.aprs_cfg() == a.app.aprs_cfg());
}

/// APRS-45: 0 5 9 5, FX.25 on transmit: 16 check bytes by default (decision 230), then 32, 64 and Off; # puts it
/// back; kept by the APRS store's owner.
#[test]
fn fx25_on_transmit_setting() {
    use crate::screens::groups::{change, reset};
    let mut a = A::new();
    assert_eq!(a.app.aprs_cfg().fx25, 16);
    for (d, want) in [(1, 32), (1, 64), (1, 0), (1, 16), (-1, 0)] {
        change(&mut a.app, &[5, 9, 5], d);
        assert_eq!(a.app.aprs_cfg().fx25, want);
    }
    a.tick();
    assert_eq!(a.s.aprs_cfg().fx25, 0, "written back");
    reset(&mut a.app, &[5, 9, 5]);
    assert_eq!(a.app.aprs_cfg().fx25, 16);
}

/// APRS-54.1: 0 5 6 4, telemetry: Off by default, OK turns it on (the row reads On), # puts it back; kept by
/// the APRS store's owner, apart from beacon after voice.
#[test]
fn telemetry_setting() {
    use crate::screens::groups::{change, reset};
    let mut a = A::new();
    assert!(!a.app.aprs_cfg().telemetry, "off by default");
    change(&mut a.app, &[5, 6, 4], 1);
    assert!(a.app.aprs_cfg().telemetry && !a.app.aprs_cfg().after_voice);
    a.tick();
    assert!(a.s.aprs_cfg().telemetry, "written back");
    reset(&mut a.app, &[5, 6, 4]);
    assert!(!a.app.aprs_cfg().telemetry);
}

/// APRS-27: with APRS mode and beacon after voice on, releasing PTT asks for a beacon; off, it doesn't.
#[test]
fn beacon_after_voice_on_release() {
    let mut a = A::new();
    a.app.radio.tx_built = true;
    a.app.set_region(Some(Region::Us), false);
    a.app.active = 0;
    a.mode(true);
    a.wait(2000);
    let sent = |a: &mut A| {
        let mut any = false;
        for _ in 0..3 {
            a.wait(1000);
            any |= a.s.aprs_tick(a.t / 1000, true, true).tx == Some("Beacon");
        }
        any
    };
    assert!(!sent(&mut a), "no beacon by itself (beacons off)");
    a.press(Key::Ptt(0), 300);
    a.press(Key::Exit, 100);
    assert!(!sent(&mut a), "after voice is off by default");
    a.app.set_aprs(AprsCfg { after_voice: true, ..a.app.aprs_cfg() });
    a.cmds();
    a.press(Key::Ptt(0), 300);
    assert!(a.cmds().contains(&RadioCmd::Transmit { slot: 0, on: true }), "a voice transmission on A");
    assert!(sent(&mut a), "a beacon after the voice transmission");
}

/// APRS-22: without APRS mode, the active slot on a channel flagged APRS decodes; a VFO or APRS mode doesn't.
#[test]
fn aprs_flag_decodes_on_its_slot() {
    let mut a = A::new();
    assert!(!a.app.aprs_flag_decode());
    let mut ch = crate::chan::channel(5, crate::chan::NO_ZONE, ferrite_core::freq::mhz(144) + 39_000, "APRS", crate::slot::DEFAULT_CFG);
    ch.flags |= crate::chan::FLAG_APRS;
    a.app.slots[0].ch = Some(ch);
    a.app.slots[0].vfo_mode = false;
    a.app.active = 0;
    assert!(a.app.aprs_flag_decode());
    a.mode(true);
    assert!(!a.app.aprs_flag_decode(), "APRS mode decodes on C instead");
}

/// APRS-28: the station screen's Message line comes from the newest stored message the station sent us.
#[test]
fn station_shows_its_last_message() {
    let a = A::new();
    // The sample's VE3KPW-9 sent "On my way" 2 minutes ago (and was sent "Where are you?", not shown).
    let mut s = a.s.station(0).unwrap();
    assert_eq!(s.call.as_bytes(), b"VE3KPW-9");
    s.msg = Buf::new();
    s.msg_min = None;
    crate::screens::aprs::last_message(&mut s, &a.s, Some(10 * 60));
    assert_eq!(s.msg.as_bytes(), b"On my way");
    assert_eq!(s.msg_min, Some(10 * 60 - 2));
    // A station that sent us nothing keeps its dash.
    let mut s = a.s.station(2).unwrap();
    s.msg = Buf::new();
    crate::screens::aprs::last_message(&mut s, &a.s, Some(600));
    assert!(s.msg.is_empty());
}

/// APRS-26: 0 5 9 2's custom path: checked, kept, chosen at once, offered by 0 5 9 1 after the presets, and # forgets
/// it (the path going back to the default when it was in use).
#[test]
fn custom_path() {
    use crate::aprs::{path_ok, PATH_CUSTOM, PATH_DEFAULT};
    use crate::screens::groups::{change, reset};
    for good in [b"WIDE2-2".as_slice(), b"WIDE1-1,WIDE2-2", b"VE3KPW", b"RS0ISS"] {
        assert!(path_ok(good), "{:?}", core::str::from_utf8(good));
    }
    for bad in [b"".as_slice(), b"wide1-1", b"WIDE1-1,WIDE2-1,WIDE3-1", b"WIDE1-16", b"WIDE1-0", b"TOOLONGCALL", b"WIDE1-1,", b"WIDE1-1,WIDE2-2X"] {
        assert!(!path_ok(bad), "{:?}", core::str::from_utf8(bad));
    }
    let mut a = A::new();
    // APRS-26: custom keeps value 3 (a radio set to it before APRSAT and ARISS were added still is); unset, it's skipped
    assert!(!a.app.aprs_cfg().path_ok(PATH_CUSTOM), "no custom path: not a choice");
    a.app.set_aprs(crate::aprs::AprsCfg { path: 2, ..a.app.aprs_cfg() });
    change(&mut a.app, &[5, 9, 1], 1);
    assert_eq!(a.app.aprs_cfg().path, 4, "WIDE1-1,WIDE2-1 then APRSAT, Custom skipped");
    change(&mut a.app, &[5, 9, 1], 1);
    assert_eq!(a.app.aprs_cfg().path, 5, "then ARISS");
    change(&mut a.app, &[5, 9, 1], 1);
    assert_eq!(a.app.aprs_cfg().path, 0, "then round to None");
    a.app.set_aprs_custom_path(b"WIDE2-2");
    let c = a.app.aprs_cfg();
    assert_eq!((c.path, &*c.path()), (PATH_CUSTOM, &[b"WIDE2-2".as_slice()][..]));
    assert_eq!(c.path_name(), b"WIDE2-2");
    change(&mut a.app, &[5, 9, 1], 1);
    assert_eq!(a.app.aprs_cfg().path, 4, "Custom then APRSAT");
    change(&mut a.app, &[5, 9, 1], -1);
    assert_eq!(a.app.aprs_cfg().path, PATH_CUSTOM);
    a.app.set_aprs_custom_path(b"WIDE1-1,WIDE2-2");
    assert_eq!(&*a.app.aprs_cfg().path(), &[b"WIDE1-1".as_slice(), b"WIDE2-2"]);
    reset(&mut a.app, &[5, 9, 2]);
    let c = a.app.aprs_cfg();
    assert!(c.custom_text().is_empty());
    assert_eq!(c.path, PATH_DEFAULT, "forgetting the path in use goes back to the default");
}

/// SETUP-13 (decision 174): a callsign over 6 characters (VE7TEST) is kept for CW ID, but APRS refuses to send with
/// it (an AX.25 address holds 6: it went out as VE7TES); a KISS frame, which carries the client's own addresses, goes.
#[test]
fn long_callsign_refuses_aprs_not_kiss() {
    let mut a = A::new();
    a.app.radio.tx_built = true;
    a.app.set_region(Some(Region::Us), false);
    a.mode(true);
    a.app.set_callsign(b"VE7TEST");
    a.cmds();
    assert!(!crate::screens::transmit::data_key(&mut a.app, APRS_SLOT, 0, b"Beacon"), "refused");
    assert!(a.app.toast_up());
    assert!(!a.cmds().iter().any(|c| matches!(c, RadioCmd::TransmitData { on: true, .. })));
    assert!(crate::screens::transmit::data_key(&mut a.app, APRS_SLOT, 0, b"KISS"), "KISS goes");
    a.cmds();
    a.app.set_callsign(b"VE7TES");
    assert!(crate::screens::transmit::data_key(&mut a.app, APRS_SLOT, 0, b"Beacon"), "6 is fine");
    a.cmds();
    a.app.set_callsign(b"VE7TES-9");
    assert!(crate::screens::transmit::data_key(&mut a.app, APRS_SLOT, 0, b"Beacon"), "an SSID typed in isn't counted (APRS-30)");
}

/// APRS-19.1: on the APRS slot, Power (hold 2, the quick settings) is the APRS power (0 5 6 3): it changes the
/// beacon and message power and leaves C's own power for when C is given back.
#[test]
fn power_on_the_aprs_slot_is_the_aprs_power() {
    let mut a = A::new();
    a.app.radio.tx_built = true;
    let own = a.app.slot(APRS_SLOT).cfg().power;
    a.mode(true);
    a.tap(Key::Slot);
    a.tap(Key::Slot);
    assert_eq!(a.app.active(), 2);
    assert_eq!(a.app.greyed(Item::Power), None, "not refused");
    assert_eq!(a.app.greyed(Item::Mode), Some("Slot C held by APRS mode"), "the rest stays locked");
    assert_eq!(a.app.aprs_cfg().power, 0, "Low by default (APRS-20)");
    a.press(Key::Digit(2), 1200);
    assert_eq!(a.app.aprs_cfg().power, 1, "hold 2: Mid");
    assert_eq!(a.app.tuned_cfg(APRS_SLOT).power, 1, "what the radio gets");
    assert_eq!(crate::screens::transmit::power_name(&a.app, APRS_SLOT), "MID");
    assert_eq!(a.app.slot(APRS_SLOT).cfg().power, own, "C's own power untouched");
    a.mode(false);
    assert_eq!(a.app.slot(APRS_SLOT).cfg().power, own);
    assert_eq!(a.app.aprs_cfg().power, 1, "kept as 0 5 6 3");
}

/// APRS-6.1, APRS-6.2: a message to you plays the message sound (0 7 5 5, Packet by default) at its own volume
/// (0 7 5 6, Max by default); Off plays nothing. The sound isn't for the log's other events.
#[test]
fn a_message_to_you_plays_the_message_sound() {
    use ferrite_core::tonegen::{CHIMES, CHIME_DEFAULT, CHIME_NAMES};
    let mut a = A::new();
    assert_eq!(CHIME_NAMES[a.app.aprs_cfg().sound as usize], "Packet", "decision 244");
    a.cmds();
    a.app.notify_message(b"VE7GPL", b"hello");
    assert!(a.cmds().contains(&RadioCmd::Chime { sound: CHIME_DEFAULT, level: 11 }), "APRS-6.2: Max by default");
    a.app.notify(b"Pager", b"a page", false);
    assert!(!a.cmds().iter().any(|c| matches!(c, RadioCmd::Chime { .. })), "only messages to you");
    a.app.set_aprs(AprsCfg { sound: 0, ..a.app.aprs_cfg() });
    a.app.notify_message(b"VE7GPL", b"again");
    assert!(!a.cmds().iter().any(|c| matches!(c, RadioCmd::Chime { .. })), "Off");
    let ms = |i: usize| -> u32 {
        if i == ferrite_core::tonegen::CHIME_PACKET as usize {
            ferrite_core::tonegen::PACKET.iter().map(|p| p.1 as u32).sum::<u32>() * ferrite_core::tonegen::PACKET_REPEAT as u32
        } else {
            CHIMES[i].iter().map(|x| x.1 as u32).sum()
        }
    };
    assert!(CHIMES[0][0].1 == 0 && (1..CHIMES.len()).all(|i| (100..=600).contains(&ms(i))), "short sounds");
}

/// APRS-17.8: hold V/M on the APRS slot sends a beacon; a tap sends nothing (a bump never transmits) and says
/// "Hold V/M to send a beacon".
/// In compose V/M sends one and leaves the draft as it is.
#[test]
fn vm_beacons() {
    let mut a = A::new();
    a.mode(true);
    a.app.set_callsign(b"VE7TES");
    a.wait(200);
    while a.app.active() != APRS_SLOT {
        a.tap(Key::Slot);
    }
    a.wait(3500);
    a.tap(Key::VfoMem);
    assert!(!a.s.beacon_asked(), "a tap: no beacon");
    assert_eq!(a.app.toast.map(|t| t.head), Some(Buf::from(&b"Hold V/M"[..])), "a tap: says how");
    a.press(Key::VfoMem, 800);
    assert!(a.s.beacon_asked(), "hold V/M: a beacon");
    let mut b = A::new();
    b.mode(true);
    b.app.set_callsign(b"VE7TES");
    b.wait(200);
    b.app.open(crate::screens::nav::Dest::Compose);
    b.tap(Key::Digit(2));
    b.tap(Key::VfoMem);
    assert!(b.s.beacon_asked(), "V/M in compose");
    assert!(matches!(b.app.screen(), Screen::Compose(_)), "still composing");
    assert_eq!(b.s.draft().to.as_bytes(), b"A", "the draft kept");
}

/// APRS-13.1, APRS-13.2: hold MENU on one of ours being tried offers Stop sending first; it ends as cancelled.
/// On a cancelled (or failed) one Send again comes first: a new message, five new tries, the old one kept.
#[test]
fn stop_sending_and_send_again() {
    use crate::screens::opts::Of;
    let mut a = A::new();
    a.mode(true);
    a.app.set_callsign(b"VA7FER");
    assert!(a.s.request(Op::Draft(Draft { to: Buf::from(&b"VE3KPW"[..]), text: Buf::from(&b"QSL"[..]) })));
    assert!(a.s.request(Op::Send));
    a.app.open(crate::screens::nav::Dest::AprsMessages);
    a.tick();
    a.press(Key::Menu, 800);
    assert!(matches!(a.app.screen(), Screen::Options(o) if o.of == Of::MsgSending), "Stop sending offered");
    a.tap(Key::Menu);
    assert_eq!(a.s.message(0).map(|m| m.state), Some(MsgState::Cancelled));
    assert_eq!(a.app.toast.map(|t| t.head), Some(Buf::from(&b"Sending stopped"[..])));
    a.press(Key::Menu, 800);
    assert!(matches!(a.app.screen(), Screen::Options(o) if o.of == Of::MsgEnded), "Send again offered");
    a.tap(Key::Menu);
    assert_eq!(a.s.message(0).map(|m| (m.state, m.peer)), Some((MsgState::Pending, Buf::from(&b"VE3KPW"[..]))), "a new one");
    assert_eq!(a.s.message(1).map(|m| m.state), Some(MsgState::Cancelled), "the old one kept");
    // A message to us has neither: Delete leads its options (APRS-46.1).
    let k = (0..a.s.messages()).find(|&i| a.s.message(i).is_some_and(|m| m.inbound)).unwrap();
    a.app.screen = Screen::Msgs(crate::screens::msgs::Msgs { cur: crate::screens::lists::Cursor { sel: k, top: 0 }, open: false });
    a.press(Key::Menu, 800);
    assert!(matches!(a.app.screen(), Screen::Options(o) if o.of == Of::Msg));
}

/// APRS-17.9: Beacon now with neither APRS mode nor beacons holding C says so and asks for nothing (asked for,
/// it would go the moment either came on).
#[test]
fn beacon_now_refused_with_nothing_holding_c() {
    let mut a = A::new();
    a.app.set_callsign(b"VA7FER");
    a.mode(false);
    a.app.set_aprs(AprsCfg { beacon: 0, ..a.app.aprs_cfg() });
    crate::screens::opts::beacon_now(&mut a.app, &a.s);
    assert!(!a.s.beacon_asked(), "nothing asked for");
    assert_eq!(a.app.toast.map(|t| t.body), Some(Buf::from(&b"APRS and beacons off"[..])));
    a.mode(true);
    crate::screens::opts::beacon_now(&mut a.app, &a.s);
    assert!(a.s.beacon_asked(), "with APRS mode: asked for");
}

/// APRS-10.1: a data frame keys on C only while APRS mode, KISS or beacons hold it: never on C's own channel.
#[test]
fn data_frames_key_only_while_c_is_held() {
    let mut a = A::new();
    a.app.set_callsign(b"VA7FER");
    a.mode(true);
    a.mode(false);
    a.cmds();
    a.app.toast = None;
    assert!(!a.app.aprs_c());
    assert!(!crate::screens::transmit::data_key(&mut a.app, APRS_SLOT, 0, b"Beacon"), "given back: refused");
    assert!(!a.cmds().iter().any(|c| matches!(c, RadioCmd::TransmitData { on: true, .. })), "nothing keyed");
    assert!(a.app.toast.is_none(), "quietly: the frame was dropped as it was let go");
    assert_eq!(a.app.aprs_sending(), crate::aprs_live::Sending::Nothing);
}

/// INBOX-3.2: an emptied log has nothing new; reading every message takes their log entries' count with it.
#[test]
fn log_counts_follow_clear_and_read_all() {
    use crate::screens::opts::{Of, Options};
    let mut a = A::new();
    (a.app.inbox, a.app.log_unread) = (2, 5);
    a.app.screen = Screen::Options(Options { of: Of::Messages, row: 3, confirm: false }); // Mark all read
    a.tap(Key::Menu);
    assert_eq!((a.app.inbox, a.app.log_unread), (0, 3));
    a.app.screen = Screen::Options(Options { of: Of::Log, row: 1, confirm: true }); // Clear the log
    a.tap(Key::Menu);
    assert_eq!(a.app.log_unread, 0);
}

/// APRS-13.6: the Messages options' last row, Message retries, shows its value; OK steps 4, off, 1, 2, 3, 4 and
/// says so (ferrite#110).
#[test]
fn message_retries_row_steps_through_its_values() {
    use crate::screens::opts::{rows, Of, Options};
    let mut a = A::new();
    let row = rows(Of::Messages).iter().position(|r| r.0 == "Message retries").expect("a Message retries row") as u8;
    assert_eq!(row as usize, rows(Of::Messages).len() - 1, "last, so the other rows keep their digits");
    assert_eq!(a.app.aprs_cfg().retries, crate::aprs::RETRIES_DEFAULT);
    let mut seen = std::vec::Vec::new();
    for _ in 0..5 {
        a.app.screen = Screen::Options(Options { of: Of::Messages, row, confirm: false });
        a.tap(Key::Menu);
        seen.push(a.app.aprs_cfg().retries);
    }
    assert_eq!(seen, [0, 1, 2, 3, 4]);
    assert_eq!(a.app.aprs_cfg().tries(), 5);
}

/// MODE-5.1, APRS-15.3 (ferrite#120, ferrite#59): C on airband in AM, then APRS mode on the single + scope
/// layout. C goes to the Radio task as FM on the APRS frequency, the card's settings are the APRS slot's (it read
/// AM), and home's strip doesn't sweep while APRS listens (a sweep is all the Radio task does while it runs: no
/// turn for C, nothing decoded; with C active it swept around 118.950), whichever slot is active. APRS mode off:
/// C is back in AM on airband and the strip sweeps again.
#[test]
fn aprs_from_an_am_slot_is_fm_and_the_strip_waits() {
    use crate::screens::activity::Mode;
    let air = ferrite_core::freq::mhz(118) + 95_000;
    let mut a = A::new();
    a.app.active = APRS_SLOT;
    let s = &mut a.app.slots[APRS_SLOT as usize];
    (s.vfo_mode, s.vfo) = (true, air);
    s.cfg_mut().mode = 2;
    a.app.send_slot(APRS_SLOT);
    a.app.set_layout(1);
    a.wait(700);
    let c = a.cmds();
    assert!(c.contains(&RadioCmd::SlotParam { slot: 2, param: Param::Mode, value: 2 }), "AM on airband: {c:?}");
    assert!(c.iter().any(|c| matches!(c, RadioCmd::Sweep { centre, on: true, .. } if *centre == air)), "the strip around it: {c:?}");
    assert_eq!(a.app.radio.mode, Mode::Strip);
    a.mode(true);
    a.wait(700);
    let c = a.cmds();
    let at = |want: RadioCmd| c.iter().rposition(|x| *x == want);
    let tuned = at(RadioCmd::TuneSlot { slot: 2, freq: 14_439_000 }).expect("on the APRS frequency");
    assert!(at(RadioCmd::SlotParam { slot: 2, param: Param::Mode, value: 0 }) > Some(tuned), "FM, after the retune: {c:?}");
    assert!(!c.contains(&RadioCmd::SlotParam { slot: 2, param: Param::Mode, value: 2 }), "never AM again: {c:?}");
    assert!(c.iter().any(|c| matches!(c, RadioCmd::Sweep { on: false, .. })), "the strip stopped: {c:?}");
    assert!(!c.iter().any(|c| matches!(c, RadioCmd::Sweep { on: true, .. })), "and not started again: {c:?}");
    assert!(a.app.strip_blocked() && a.app.radio.mode != Mode::Strip);
    assert_eq!((a.app.tuned_cfg(APRS_SLOT).mode, a.app.slot(APRS_SLOT).cfg().mode), (0, 2), "the card's FM; C's own AM kept aside");
    // A active, on 2 m beside APRS: still no strip
    a.app.active = 0;
    a.wait(700);
    assert!(a.app.strip_blocked() && a.app.radio.mode != Mode::Strip);
    assert!(!a.cmds().iter().any(|c| matches!(c, RadioCmd::Sweep { on: true, .. })));
    // off: C as it was, and the strip again
    a.app.active = APRS_SLOT;
    a.mode(false);
    a.wait(700);
    let c = a.cmds();
    assert!(c.contains(&RadioCmd::TuneSlot { slot: 2, freq: air }) && c.contains(&RadioCmd::SlotParam { slot: 2, param: Param::Mode, value: 2 }), "{c:?}");
    assert!(c.iter().any(|c| matches!(c, RadioCmd::Sweep { centre, on: true, .. } if *centre == air)), "{c:?}");
}
