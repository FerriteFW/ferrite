//! Home's cards (chapter 6): the broadcast card large and small, a scanning card, a split's header, the
//! clarifier, the APRS card's large rows, and the steps.

use super::*;
use crate::aprs::{AprsCfg, APRS_SLOT};
use ferrite_core::freq::mhz;
use ferrite_core::msg::ModeLive;

const FM1005: ferrite_core::freq::Freq10 = mhz(100) + 50_000;

fn live_fm(t: &mut T, rt: &[u8], ps: &[u8; 8], rssi: u8) {
    let mut r = [0u8; 64];
    r[..rt.len()].copy_from_slice(rt);
    t.live.mode = ModeLive::Fm { freq: 10_050, ps: *ps, rt: r, rssi, snr: 20, stereo: true, pty: 0, heard: true };
    t.live.bcast_heard = true;
}

/// A on 100.5 MHz broadcast, playing.
fn on_fm() -> T {
    let mut t = T::ready();
    t.app.set_vfo(0, FM1005);
    t.wait(100);
    assert_eq!(t.app.bcast_slot(), Some(0));
    t
}

/// BCR-26.1 - BCR-26.3: the large broadcast card: the frequency, STEREO, the SNR, and the radio text whole; the
/// station name alone when there is no radio text; "No RDS" with neither.
#[test]
fn broadcast_card_large() {
    let mut t = on_fm();
    live_fm(&mut t, b"Some radio text that runs on for more than one line of the card", b"STATION ", 40);
    t.wait(100);
    t.shows("100.5");
    t.shows("STEREO");
    t.shows("SNR 20 dB");
    t.shows("Some radio text");
    assert!(t.px(0, 300, ferrite_ui::gfx::GREEN) > 0, "the signal green while it plays");
    live_fm(&mut t, b"", b"STATION ", 40);
    t.wait(100);
    t.shows("STATION");
    live_fm(&mut t, b"", b"        ", 0);
    t.wait(100);
    t.shows("No RDS");
    t.hides("SNR");
}

/// BCR-22, BCR-26.3: on a small card the text is one line; another slot in the band says where the broadcast
/// plays instead.
#[test]
fn broadcast_card_small_and_elsewhere() {
    let mut t = on_fm();
    live_fm(&mut t, b"Some text", b"STATION ", 40);
    t.app.set_vfo(1, mhz(99) + 10_000);
    t.wait(100);
    assert_eq!(t.app.bcast_slot(), Some(1), "the last into the band has it");
    t.shows("Broadcast plays on B"); // on A's large card
    t.shows("99.1");
    t.tap(Key::Slot); // B active: A's card is small
    t.wait(100);
    t.shows("Broadcast plays on B");
    t.shows("Some text");
}

/// BCR-26.9 (ferrite#178): a slot on a saved broadcast channel shows its name beside the frequency, which stays
/// large: in the SNR's place on the large card, in the stereo tag's on the small one. A VFO shows the SNR as before.
#[test]
fn broadcast_channel_shows_its_name() {
    use crate::chan::{ChanWrite, Channel, NO_ZONE};
    let mut t = T::ready();
    let mut name = [0u8; 16];
    name[..11].copy_from_slice(b"CBC Radio 2");
    let ch = Channel { num: 70, zone: NO_ZONE, rx: FM1005, name, cfg: crate::slot::DEFAULT_CFG, flags: 0, tones: 0 };
    t.s.apply(ChanWrite::Put { ch, from: None });
    t.taps(&[Key::VfoMem, Key::Digit(0), Key::Digit(7), Key::Digit(1), Key::Menu]); // channel mode, 071
    t.wait(100);
    assert_eq!(t.app.bcast_slot(), Some(0), "on the broadcast channel");
    live_fm(&mut t, b"Some text", b"STATION ", 40);
    t.wait(100);
    t.shows("100.5");
    t.shows("CBC Radio 2");
    t.shows("STEREO");
    t.hides("SNR"); // the name has its place
    t.tap(Key::Slot); // B active: A's card is small
    t.wait(100);
    t.shows("100.5");
    t.shows("CBC Radio 2");
}

/// HOME-4, HOME-4.1, LINK-19.4: a slot scanning on a small card shows its hits and the last hit's frequency, and
/// the readback carries the last hit as drawn.
#[test]
fn scanning_small_card() {
    let mut t = T::ready();
    let f = t.app.slot(1).freq();
    t.live.slots[1] = ferrite_core::msg::SlotLive { freq: f, scanning: true, hits: 3, last_hit: mhz(146) + 52_000, ..Default::default() };
    t.wait(100);
    t.shows("3 hits");
    t.shows("last 146.520");
    let card = |t: &T| t.readback().into_iter().find(|l| l.starts_with("card B")).unwrap();
    assert!(card(&t).contains("lasthit=\"146.520\""), "LINK-19.4: the readback has the last hit: {}", card(&t));
    t.live.slots[1].hits = 0;
    t.wait(100);
    t.shows("0 hits");
    t.hides("last ");
    assert!(!card(&t).contains("lasthit="), "{}", card(&t));
}

/// HOME-4: a split's TX frequency takes the band's place in the active card's header.
#[test]
fn split_header() {
    let mut t = T::ready();
    let a = t.app.active as usize;
    t.app.slots[a].cfg.split = true;
    t.app.slots[a].cfg.offset = (mhz(147) + 30_000) as i32;
    t.wait(100);
    t.shows("\u{b7} TX 147.300");
}

/// VFO-22: SIDE1 + \u{25c0}\u{25b6} move the clarifier; the active card shows it, in amber, beside the state.
#[test]
fn clarifier_on_the_card() {
    let mut t = T::ready();
    t.app.key_down(Key::Side(1), t.t, &t.s);
    t.tap(Key::Right);
    t.tap(Key::Right);
    t.app.key_up(Key::Side(1), t.t, &t.s);
    t.wait(100);
    assert_ne!(t.app.slot(t.app.active).clar, 0);
    t.shows("CLR +0.2");
    assert!(t.px(25, 50, ferrite_ui::gfx::AMBER) > 0, "the clarifier in amber");
}

/// NAV-3: SIDE2 held, \u{25b2} steps 1 MHz.
#[test]
fn side2_up_is_a_big_step() {
    let mut t = T::ready();
    let f = t.app.slot(0).freq();
    t.app.key_down(Key::Side(2), t.t, &t.s);
    t.tap(Key::Up);
    t.app.key_up(Key::Side(2), t.t, &t.s);
    t.wait(100);
    assert_eq!(t.app.slot(0).freq(), f + 100_000);
    assert_eq!(t.screen(), Screen::Home, "SIDE2's release after a step opens nothing");
    assert_eq!(t.screen().mockup(), Some("HomeA.png"));
}

/// APRS-17.2: APRS mode's large card shows both counts; KISS's shows its port; APRS mode with timed beacons, the
/// countdown (beacons need APRS mode, APRS-29.1).
#[test]
fn aprs_card_large_rows() {
    let mut t = T::ready();
    t.app.set.bt = true;
    t.app.set.kiss_port = 2;
    t.wait(100);
    while t.app.active() != APRS_SLOT {
        t.tap(Key::Slot);
    }
    t.wait(3500);
    t.shows("Bluetooth");
    t.app.set.kiss_port = 0;
    t.app.set_aprs_mode(true);
    t.app.set_aprs(AprsCfg { beacon: 5, ..*t.app.aprs_cfg() });
    t.s.aprs_tick(10, true, true);
    t.wait(100);
    t.shows("Beacon in");
}

/// NAV-1: \u{25b2}\u{25bc} on a channel slot step the zone's channels; a zone with none does nothing.
#[test]
fn channel_step_in_an_empty_zone() {
    let mut t = T::ready();
    let a = t.app.active as usize;
    let ch = crate::chan::ChannelStore::zone_at(&t.s, 0, 0).unwrap();
    t.app.slots[a].ch = Some(ch);
    t.app.slots[a].vfo_mode = false;
    t.app.slots[a].zone = 0;
    let empty = crate::chan::Table { zones: &[b"Empty"], channels: &[] };
    let before = t.app.slots[a].pos;
    t.app.key_down(Key::Up, t.t, &empty);
    t.app.key_up(Key::Up, t.t + 100, &empty);
    assert_eq!(t.app.slots[a].pos, before, "nothing to step to");
    // V/M with no channels at all says so.
    t.app.slots[a].vfo_mode = true;
    t.app.slots[a].ch = None;
    t.app.key_down(Key::VfoMem, t.t, &empty);
    t.app.key_up(Key::VfoMem, t.t + 100, &empty);
    assert_eq!(t.toast(), "No channels yet / ");
}

/// VFO-18.2: on the airband with the 8.33 kHz step every home layout shows the channel name ("118.010" for
/// 118.00833) without 10 Hz digits, and 121.500 as itself; with another step, the frequency.
#[test]
fn airband_833_names() {
    for l in 0..3 {
        let mut t = T::ready();
        t.app.set_layout(l);
        let a = t.app.active;
        t.app.set_vfo(a, mhz(118) + 833);
        t.app.slots[a as usize].cfg_mut().step = crate::slot::STEP_833;
        t.wait(100);
        t.shows("118.010");
        t.hides("118.008 33"); // the single layout's scope strip keeps its centre's frequency (a scale)
        t.hides("118.010 33");
        t.app.set_vfo(a, mhz(121) + 50_000);
        t.app.slots[a as usize].cfg_mut().step = crate::slot::STEP_833;
        t.wait(100);
        t.shows("121.500"); // a block's base keeps its 25 kHz name, not 121.505
        t.hides("121.505");
        t.app.set_vfo(a, mhz(118) + 833);
        t.app.slots[a as usize].cfg_mut().step = 11; // 25 kHz
        t.wait(100);
        t.shows("118.008 33");
        t.hides("118.010");
    }
}


/// The held peak's tests: layout `layout` (0 watch, 1 single), A in the VFO on 145 MHz with its squelch open at
/// -100 dBm (8 segments) long enough to settle, then -73 dBm (17) heard and gone: the peak is held. Returns the
/// meter's left and top.
fn peak_held(layout: u8) -> (T, i32, i32) {
    let mut t = T::ready();
    t.app.set_layout(layout);
    let a = t.app.active;
    t.tune(mhz(145));
    t.live.slots[a as usize] = ferrite_core::msg::SlotLive { freq: mhz(145), squelch_open: true, carrier: true, rssi: -100, ..Default::default() };
    t.wait(300);
    t.live.slots[a as usize].rssi = -73;
    t.wait(100);
    t.live.slots[a as usize].rssi = -100;
    t.wait(100);
    let (x, y) = if layout == 0 { (14, crate::screens::home::card(a, a).0 + 109) } else { (8, 170) };
    (t, x, y)
}

/// Segment `i`'s look on the meter at (x, y): 0 empty, 1 lit (open), 2 the held peak.
fn seg(t: &T, x: i32, y: i32, i: i32) -> u8 {
    use ferrite_ui::gfx::{AMBER, GREEN};
    match t.fb.at(x + 10 * i + 4, y + 4) {
        GREEN => 1,
        AMBER => 2,
        _ => 0,
    }
}

/// The meter's segments, as `seg` reads them.
fn segs(t: &T, x: i32, y: i32) -> std::vec::Vec<u8> {
    (0..20).map(|i| seg(t, x, y, i)).collect()
}

/// A memory channel numbered `num` on `rx`, on the active slot (shown, not tuned through the store).
fn on_channel(t: &mut T, num: u16, rx: ferrite_core::freq::Freq10) {
    let a = t.app.active as usize;
    use crate::chan::ChannelStore;
    t.app.slots[a].ch = Some(crate::chan::Channel { num, rx, ..t.s.get(9).unwrap() });
    t.app.slots[a].vfo_mode = false;
    t.live.slots[a].freq = rx;
}

/// The S-meter holds its peak: after a strong signal falls back, the highest level stays marked above the bar for
/// two seconds (watch and single layouts), the same level again renews the hold, and a higher one takes over.
#[test]
fn meter_holds_its_peak_for_two_seconds() {
    for layout in [0, 1] {
        let (mut t, x, y) = peak_held(layout);
        let a = t.app.active as usize;
        let want = |lit: usize, peak: usize| (0..20).map(|i| if i < lit { 1 } else if i + 1 == peak { 2 } else { 0 }).collect::<std::vec::Vec<u8>>();
        assert_eq!(segs(&t, x, y), want(8, 17), "layout {layout}: the bar at 8, the peak at 17");
        t.wait(1600); // 1.8 s since -73 dBm
        assert_eq!(segs(&t, x, y), want(8, 17), "layout {layout}: held");
        t.wait(300);
        assert_eq!(segs(&t, x, y), want(8, 0), "layout {layout}: gone after 2 s");
        // the same level again renews it
        for _ in 0..2 {
            t.live.slots[a].rssi = -73;
            t.wait(100);
            t.live.slots[a].rssi = -100;
            t.wait(1500);
        }
        assert_eq!(segs(&t, x, y), want(8, 17), "layout {layout}: renewed");
        // a higher one takes over
        t.live.slots[a].rssi = -33; // 19 segments
        t.wait(100);
        t.live.slots[a].rssi = -100;
        t.wait(100);
        assert_eq!(segs(&t, x, y), want(8, 19), "layout {layout}: the new peak");
        // the bar at or above the peak: no mark
        t.live.slots[a].rssi = -33;
        t.wait(100);
        assert_eq!(segs(&t, x, y), want(19, 0), "layout {layout}: the bar is the peak");
    }
}

/// A held peak is the signal's: any change of what the meter shows lets it go, and it doesn't come back. The same
/// frequency on a memory channel, another channel or another scan position is another signal.
#[test]
fn meter_peak_lets_go_on_a_change() {
    type Change = fn(&mut T);
    let changes: [(&str, Change); 11] = [
        ("retune", |t| {
            t.tune(mhz(145) + 2_500);
            t.live.slots[t.app.active as usize].freq = mhz(145) + 2_500;
        }),
        ("memory channel on the same frequency", |t| on_channel(t, 40, mhz(145))),
        ("slot", |t| t.tap(Key::Slot)),
        ("keyed", |t| t.live.tx.keyed = true),
        ("dBm number", |t| t.app.set.smeter_dbm = true),
        ("activity layout", |t| t.app.set_layout(2)),
        ("relay", |t| t.live.slots[t.app.active as usize].relay = true),
        ("unconfirmed retune", |t| t.tune(mhz(146))),
        ("scan on the same frequency", |t| {
            let lv = &mut t.live.slots[t.app.active as usize];
            (lv.scanning, lv.chan) = (true, 3);
        }),
        ("slot turned off", |t| t.app.off |= 1 << t.app.active),
        ("another screen", |t| t.open(Dest::Group(7))),
    ];
    for (what, change) in changes {
        let (mut t, _, _) = peak_held(0);
        let a = t.app.active;
        assert_eq!(t.app.meter_peak(a, &t.live, 8), 17, "{what}: held before");
        let before = (t.live, t.app.active, t.app.set.smeter_dbm, t.app.set.layout, t.app.slots[a as usize], t.app.off);
        change(&mut t);
        t.wait(100);
        for s in 0..3 {
            assert_eq!(t.app.meter_peak(s, &t.live, 8), 0, "{what}: let go (slot {s})");
        }
        // back as it was: the peak stays gone
        if t.screen() != crate::Screen::Home {
            t.app.go_home();
        }
        (t.live, t.app.active, t.app.set.smeter_dbm, t.app.slots[a as usize], t.app.off) = (before.0, before.1, before.2, before.4, before.5);
        t.app.set_layout(before.3);
        t.wait(100);
        assert_eq!(t.app.meter_peak(a, &t.live, 8), 0, "{what}: not back");
    }
    // between two channels on one frequency, and two scan positions on one frequency
    let (mut t, _, _) = peak_held(0);
    let a = t.app.active;
    for (what, first, second) in [("channel", 0u8, 1u8), ("scan position", 2, 3)] {
        let set = |t: &mut T, k: u8| match k {
            0 => on_channel(t, 40, mhz(145)),
            1 => on_channel(t, 41, mhz(145)),
            k => {
                let lv = &mut t.live.slots[t.app.active as usize];
                (lv.scanning, lv.chan, lv.freq) = (true, k as u16, mhz(145));
            }
        };
        set(&mut t, first);
        t.live.slots[a as usize].rssi = -100;
        t.wait(300);
        t.live.slots[a as usize].rssi = -73;
        t.wait(100);
        t.live.slots[a as usize].rssi = -100;
        t.wait(100);
        assert_eq!(t.app.meter_peak(a, &t.live, 8), 17, "{what}: held on the first");
        set(&mut t, second);
        t.wait(100);
        assert_eq!(t.app.meter_peak(a, &t.live, 8), 0, "{what}: let go on the second");
    }
}

/// A new signal's first readings can still be the old one's (the Radio task reads, then retunes, and publishes
/// both): a strong level seen in the first 200 ms after a change makes no peak; a real one after does.
#[test]
fn meter_peak_waits_for_a_new_signal_to_settle() {
    type Change = fn(&mut T);
    let changes: [(&str, Change); 4] = [
        ("retune", |t| {
            t.tune(mhz(145) + 2_500);
            t.live.slots[t.app.active as usize].freq = mhz(145) + 2_500;
        }),
        ("memory channel on the same frequency", |t| on_channel(t, 40, mhz(145))),
        ("another channel on the same frequency", |t| on_channel(t, 41, mhz(145))),
        ("a scan's next position on the same frequency", |t| {
            let lv = &mut t.live.slots[t.app.active as usize];
            (lv.scanning, lv.chan) = (true, lv.chan + 1);
        }),
    ];
    for (what, change) in changes {
        let (mut t, _, _) = peak_held(0);
        let a = t.app.active;
        on_channel(&mut t, 40, mhz(145));
        if what.starts_with("memory") || what.starts_with("retune") {
            t.app.slots[a as usize].vfo_mode = true;
        }
        if what.starts_with("a scan") {
            let lv = &mut t.live.slots[a as usize];
            (lv.scanning, lv.chan) = (true, 1);
        }
        t.wait(2_500); // settled, nothing held
        assert_eq!(t.app.meter_peak(a, &t.live, 8), 0, "{what}: nothing held before");
        t.live.slots[a as usize].rssi = -73; // the old signal's level, published with the new one
        change(&mut t);
        t.wait(100);
        t.live.slots[a as usize].rssi = -100;
        t.wait(100);
        assert_eq!(t.app.meter_peak(a, &t.live, 8), 0, "{what}: no peak from the old signal's reading");
        t.wait(100);
        t.live.slots[a as usize].rssi = -73; // a real one, settled
        t.wait(100);
        t.live.slots[a as usize].rssi = -100;
        t.wait(100);
        assert_eq!(t.app.meter_peak(a, &t.live, 8), 17, "{what}: a settled signal's peak");
    }
}

/// Only a level the open squelch shows makes a peak: not one under a closed squelch (the closed meter is empty,
/// SQL-12.4), nor one read while the receivers sleep; a running scan's 70 ms steps never settle, a scan held on a
/// hit does.
#[test]
fn meter_peak_only_from_an_open_squelch() {
    let (mut t, _, _) = peak_held(0);
    let a = t.app.active as usize;
    t.wait(2100); // the held one gone
    assert_eq!(t.app.meter_peak(a as u8, &t.live, 8), 0);
    t.live.slots[a].squelch_open = false;
    t.live.slots[a].rssi = -73;
    t.wait(100);
    t.live.slots[a].rssi = -121;
    t.wait(100);
    // what stays is the last open level (8), held after the squelch closed; never the closed -73 dBm's 17
    assert_eq!(t.app.meter_peak(a as u8, &t.live, 1), 8, "closed: no new peak");
    t.live.slots[a].squelch_open = true;
    t.live.rx_asleep = true;
    t.live.slots[a].rssi = -73;
    t.wait(100);
    t.live.rx_asleep = false;
    t.live.slots[a].rssi = -100;
    t.wait(100);
    assert_eq!(t.app.meter_peak(a as u8, &t.live, 8), 0, "asleep: no peak");
    // a running scan: a new position every 70 ms, strong on each, never settles
    t.live.slots[a] = ferrite_core::msg::SlotLive { freq: mhz(145), squelch_open: true, scanning: true, chan: 1, rssi: -73, ..Default::default() };
    for k in 2..10 {
        t.wait(100);
        (t.live.slots[a].chan, t.live.slots[a].freq) = (k, mhz(145) + 2_500 * k as u32);
    }
    t.live.slots[a].rssi = -100;
    t.wait(100);
    assert_eq!(t.app.meter_peak(a as u8, &t.live, 8), 0, "a running scan takes no peak");
    // held on a hit: settled, its peak held
    t.wait(300);
    t.live.slots[a].rssi = -73;
    t.wait(100);
    t.live.slots[a].rssi = -100;
    t.wait(100);
    assert_eq!(t.app.meter_peak(a as u8, &t.live, 8), 17, "a scan held on a hit holds its peak");
}

/// No peak on a broadcast card, the APRS card, an off slot or Low Power Mode's home, and none on another slot's
/// card: only the active slot's own meter holds one.
#[test]
fn meter_peak_only_on_the_active_slots_own_meter() {
    let (mut t, _, _) = peak_held(0);
    let a = t.app.active;
    for s in (0..3).filter(|&s| s != a) {
        assert_eq!(t.app.meter_peak(s, &t.live, 0), 0, "slot {s} isn't the peak's");
    }
    let strong_then_weak = |t: &mut T, s: u8| {
        t.live.slots[s as usize].squelch_open = true;
        t.live.slots[s as usize].rssi = -100;
        t.wait(300);
        t.live.slots[s as usize].rssi = -73;
        t.wait(100);
        t.live.slots[s as usize].rssi = -100;
        t.wait(100);
        t.app.meter_peak(s, &t.live, 8)
    };
    // the active slot on broadcast FM
    t.app.set_vfo(a, mhz(100) + 50_000);
    t.live.slots[a as usize].freq = mhz(100) + 50_000;
    t.wait(100);
    assert_eq!(crate::screens::home::card_kind(&t.app, a), 2);
    assert_eq!(strong_then_weak(&mut t, a), 0, "no peak on the broadcast card");
    // the APRS slot active, its card shown
    let mut t = T::ready();
    t.app.set_aprs_mode(true);
    while t.app.active() != APRS_SLOT {
        t.tap(Key::Slot);
    }
    t.live.slots[APRS_SLOT as usize].freq = t.app.tuned_freq(APRS_SLOT);
    t.wait(100);
    assert_eq!(crate::screens::home::card_kind(&t.app, APRS_SLOT), 1);
    assert_eq!(strong_then_weak(&mut t, APRS_SLOT), 0, "no peak on the APRS card");
    // a slot turned off
    let (mut t, _, _) = peak_held(0);
    let a = t.app.active;
    t.app.off |= 1 << a;
    assert_eq!(strong_then_weak(&mut t, a), 0, "no peak on an off slot");
    // Low Power Mode's own home
    let (mut t, _, _) = peak_held(0);
    let a = t.app.active;
    t.app.set.lpm = 0;
    t.open(Dest::Group(7));
    assert!(crate::screens::groups::change(&mut t.app, &[7, 7, 2], -1));
    assert!(t.app.low_power());
    t.app.go_home();
    assert_eq!(strong_then_weak(&mut t, a), 0, "no peak in Low Power Mode");
}

/// A hold running out on the same signal doesn't blind the meter: a burst on the very tick it runs out is held
/// (only a new signal waits to settle).
#[test]
fn meter_peak_is_taken_as_its_hold_runs_out() {
    let (mut t, _, _) = peak_held(0);
    let a = t.app.active;
    t.wait(1800); // the tick before 2 s since -73 dBm
    t.live.slots[a as usize].rssi = -73;
    t.wait(100); // the hold runs out and -73 dBm is read on the same tick
    t.live.slots[a as usize].rssi = -100;
    t.wait(100);
    assert_eq!(t.app.meter_peak(a, &t.live, 8), 17, "taken as the old one ran out");
}

/// The hold runs on the App's millisecond clock, which wraps after 49.7 days: a peak taken just before the wrap is
/// held across it and let go 2 s after, as any other.
#[test]
fn meter_peak_across_the_clock_wrap() {
    let (mut t, _, _) = peak_held(0);
    let a = t.app.active;
    let at = u32::MAX - 500;
    t.app.meter_peak_tick(at - 300, &t.live); // its hold long gone at this clock: let go, settling from here
    t.live.slots[a as usize].rssi = -73;
    t.app.meter_peak_tick(at, &t.live);
    t.live.slots[a as usize].rssi = -100;
    for (dt, held) in [(100u32, 17), (1_000, 17), (1_900, 17), (2_000, 0), (5_000, 0)] {
        let now = at.wrapping_add(dt);
        t.app.meter_peak_tick(now, &t.live);
        t.app.now = now;
        assert_eq!(t.app.meter_peak(a, &t.live, 8), held, "{dt} ms after a peak at {at}");
    }
}
