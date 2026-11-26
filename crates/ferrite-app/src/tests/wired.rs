//! The settings that showed a value but did nothing until decision 243: menu timeout, pause time, the lock's
//! modes (chosen at the lock since decision 304), busy lockout default, S-meter, channel display, scan direction and return, and the APRS symbol, TX delay and
//! TX level.

use super::*;
use crate::screens::groups::change;
use crate::settings::Lock;

/// MENU-28: 0 7 9 sets how long an idle menu stays up; Off keeps it.
#[test]
fn menu_timeout_follows_its_setting() {
    let mut t = T::new();
    t.app.set.menu_timeout = 1; // 10 s
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Quick(_)));
    t.wait(10_200);
    assert_eq!(t.app.screen(), Screen::Home, "home after 10 s");
    t.app.set.menu_timeout = 0; // Off
    t.tap(Key::Menu);
    t.wait(70_000);
    assert!(matches!(t.app.screen(), Screen::Quick(_)), "Off: the menu stays");
}

/// WATCH-6: # pauses watch for 0 1 6 1's time.
#[test]
fn pause_time_follows_its_setting() {
    let mut t = T::new();
    t.app.set.pause_time = 2; // 5 min
    t.live.slots[1] = SlotLive { squelch_open: true, carrier: true, rssi: -90, ..Default::default() };
    t.wait(100);
    t.tap(Key::Hash);
    assert!(t.cmds().contains(&RadioCmd::PauseWatch(300)));
}

/// Hold # (KEY-13), then * for All or 0 for All but PTT, or nothing for Keypad.
fn lock_in(t: &mut T, m: Lock) {
    t.hold(Key::Hash);
    match m {
        Lock::Keypad => {}
        Lock::AllButPtt => t.tap(Key::Digit(0)),
        Lock::All => t.tap(Key::Star),
    }
    assert_eq!(t.app.lock_mode(), Some(m));
}

/// KEY-13, KEY-14.5: hold # locks the keypad at once, and the popup offers the wider modes for 3 s: * locks All, 0
/// All but PTT, neither doing anything else; after that * and 0 are locked keys. The mode isn't a setting.
#[test]
fn lock_mode_chosen_at_the_lock() {
    let mut t = T::new();
    t.hold(Key::Hash);
    assert_eq!(t.app.lock_mode(), Some(Lock::Keypad), "locked at once, the keypad");
    let p = t.app.toast.expect("the lock's popup");
    assert!(p.center && p.head.as_bytes() == b"Keypad locked" && p.body.as_bytes() == crate::LOCK_ASK);
    assert!(ferrite_ui::widgets::popup_fits(b"Keypad locked", crate::LOCK_ASK), "the popup's lines fit");
    for (head, body) in [(&b"All keys locked"[..], &b"PTT works"[..]), (b"All keys locked", b"PTT too")] {
        assert!(ferrite_ui::widgets::popup_fits(head, body));
    }
    // releasing # and doing nothing leaves Keypad: the popup goes, * is then a locked key
    t.wait(3_100);
    assert!(t.app.toast.is_none());
    t.cmds();
    t.tap(Key::Star);
    assert_eq!(t.app.lock_mode(), Some(Lock::Keypad));
    assert!(t.app.toast.is_some_and(|x| x.head.as_bytes() == b"Locked:"), "KEY-14.4");
    t.hold(Key::Hash);
    assert!(!t.app.locked());
    // * within the 3 s: All, confirmed; the key does nothing else (no mute, HOME-15)
    t.hold(Key::Hash);
    t.wait(2_000);
    t.cmds();
    t.tap(Key::Star);
    assert_eq!(t.app.lock_mode(), Some(Lock::All));
    assert!(t.app.toast.is_some_and(|x| x.head.as_bytes() == b"All keys locked" && x.body.as_bytes() == b"PTT too"));
    let c = t.cmds();
    assert!(c.iter().all(|c| matches!(c, RadioCmd::Activity | RadioCmd::Beep { .. })), "* consumed: {c:?}");
    t.tap(Key::Digit(0));
    assert_eq!(t.app.lock_mode(), Some(Lock::All), "chosen once: 0 is a locked key now");
    assert!(t.app.toast.is_some_and(|x| x.head.as_bytes() == b"Locked:"));
    // hold # unlocks from any mode; 0 within the time: All but PTT
    t.hold(Key::Hash);
    assert_eq!(t.app.lock_mode(), None);
    t.hold(Key::Hash);
    t.tap(Key::Digit(0));
    assert_eq!(t.app.lock_mode(), Some(Lock::AllButPtt));
    assert!(t.app.toast.is_some_and(|x| x.body.as_bytes() == b"PTT works"));
    assert_eq!(t.app.screen(), Screen::Home, "0 consumed (no home channel, HOME-8)");
    assert_eq!(t.app.set, T::new().app.set, "KEY-14.5: no setting changed");
}

/// KEY-14.1, KEY-14.2, KEY-14.3, TX-18: locked in "All" a radio doesn't key; "Keypad" and "All but PTT" let PTT
/// through.
#[test]
fn lock_covers_ptt() {
    let mut t = T::new();
    t.app.radio.tx_built = true;
    t.app.set_region(Some(ferrite_core::txplan::Region::Us), false);
    let s = store();
    for (m, keys) in [(Lock::Keypad, true), (Lock::AllButPtt, true), (Lock::All, false)] {
        lock_in(&mut t, m);
        t.wait(3_100);
        let at = t.t;
        t.app.key_down(Key::PTT, at, &s);
        assert_eq!(matches!(t.app.screen(), Screen::Transmit(_)), keys, "{}", m.name());
        assert_eq!(t.app.toast.is_some_and(|x| x.head.as_bytes() == b"Locked:"), !keys);
        t.app.key_up(Key::PTT, at + 300, &s);
        t.t = at + 400;
        let bar = t.app.status_bar(&t.live, &ferrite_core::msg::Device::default());
        assert_eq!((bar.locked, bar.lock_ptt), (true, !keys), "KEY-14.3: the padlock red when PTT is locked too");
        t.wait(500);
        t.hold(Key::Hash);
        assert!(!t.app.locked());
    }
}

/// KEY-13, KEY-14.1, KEY-14.2, KEY-14.4: the keypad's lock lets SIDE2 scan (and SIDE1 monitor); All but PTT locks the side keys,
/// SIDE1's squelch too.
#[test]
fn lock_modes_and_the_side_keys() {
    let mut t = T::new();
    let s = store();
    lock_in(&mut t, Lock::Keypad);
    t.cmds();
    t.tap(Key::Side(2));
    assert!(t.cmds().contains(&RadioCmd::Scan { slot: 0, on: true }), "keypad lock: SIDE2 scans");
    t.tap(Key::Side(2));
    t.cmds();
    // SIDE2 + ▲ (a big tuning step) stays locked, and SIDE2's release then does nothing
    let f = t.freq();
    t.app.key_down(Key::Side(2), t.t, &s);
    t.tap(Key::Up);
    t.app.key_up(Key::Side(2), t.t + 100, &s);
    assert_eq!(t.freq(), f);
    assert!(!t.cmds().iter().any(|c| matches!(c, RadioCmd::Scan { .. })), "KEY-14.4: no scan at the release");
    t.hold(Key::Hash);
    lock_in(&mut t, Lock::AllButPtt);
    assert!(t.app.toast.is_some_and(|x| x.head.as_bytes() == b"All keys locked" && x.body.as_bytes() == b"PTT works"));
    t.cmds();
    t.tap(Key::Side(2));
    t.tap(Key::Side(1));
    let sq = t.app.slot(0).cfg().squelch;
    t.app.key_down(Key::Side(1), t.t, &s);
    t.knob(1);
    t.app.key_up(Key::Side(1), t.t + 100, &s);
    let c = t.cmds();
    assert!(!c.iter().any(|c| matches!(c, RadioCmd::Scan { .. } | RadioCmd::Monitor { .. })), "side keys locked: {c:?}");
    assert_eq!(t.app.slot(0).cfg().squelch, sq, "no SIDE1 squelch");
    assert!(t.app.toast_up());
}

/// TX-21.1: 0 8 7 sets the VFOs' busy lockout at once and a new repeater channel's.
#[test]
fn busy_lockout_default_reaches_the_vfos() {
    let mut t = T::new();
    assert!(change(&mut t.app, &[8, 7], 1));
    assert!(t.app.set.busy_default);
    for s in 0..SLOTS as u8 {
        assert_eq!(t.app.slots[s as usize].cfg.busy, 1, "slot {s}'s VFO");
    }
}

/// SQL-12.3: the level as a number for 0 7 3 1 dBm.
#[test]
fn smeter_as_dbm() {
    assert_eq!(ferrite_ui::widgets::dbm_text(-93).as_bytes(), b"-93 dBm");
    assert_eq!(ferrite_ui::widgets::dbm_text(-200).as_bytes(), b"-140 dBm");
    let mut t = T::new();
    assert!(change(&mut t.app, &[7, 3, 1], 1));
    assert!(t.app.set.smeter_dbm);
    let mut fb = FrameBuffer::new();
    t.render(&mut fb); // draws with dBm, then with bars again
    t.app.set.smeter_dbm = false;
    t.render(&mut fb);
}

/// CHAN-33: 0 2 4 shows a channel by its name, its number or its frequency (nothing in the name's place).
#[test]
fn channel_display() {
    let mut t = T::new();
    if !t.app.slot(0).on_channel() {
        t.tap(Key::VfoMem);
    }
    assert!(t.app.slot(0).on_channel());
    let s = store();
    let name = t.app.card_name(0, &t.live, &s);
    assert!(!name.is_empty(), "a named channel");
    t.app.set.ch_display = 1;
    let n = t.app.card_name(0, &t.live, &s);
    assert!(n.as_bytes().starts_with(b"CH "), "{:?}", n.as_bytes());
    t.app.set.ch_display = 2;
    assert!(t.app.card_name(0, &t.live, &s).is_empty(), "the frequency in its place");
}

/// SCAN-26, SCAN-6: Direction "Down" (0 1 6 2) starts the scan going down; Return to "Last hit" (0 1 6 4) leaves
/// a VFO on the scan's last hit when SIDE2 stops it, "Start" where it started.
#[test]
fn scan_direction_and_return() {
    let mut t = T::new();
    if t.app.slot(0).on_channel() {
        t.tap(Key::VfoMem);
    }
    let start = t.freq();
    t.app.set.scan_down = true;
    t.cmds();
    t.tap(Key::Side(2));
    assert!(t.cmds().contains(&RadioCmd::ScanDir { slot: 0, up: false, resume: false }));
    // a hit, held, then SIDE2
    let hit = start + 2_500;
    t.live.slots[0] = SlotLive { scanning: true, held: true, freq: hit, squelch_open: true, carrier: true, ..Default::default() };
    t.wait(200);
    t.app.set.scan_return_hit = true;
    t.tap(Key::Side(2));
    assert_eq!(t.freq(), hit, "last hit");
    // Start: back where it started
    t.live.slots[0] = SlotLive::default();
    t.wait(200);
    t.app.set.scan_return_hit = false;
    let here = t.freq();
    t.tap(Key::Side(2));
    t.live.slots[0] = SlotLive { scanning: true, held: true, freq: here + 5_000, squelch_open: true, carrier: true, ..Default::default() };
    t.wait(200);
    t.tap(Key::Side(2));
    assert_eq!(t.freq(), here, "start");
}

/// SCAN-6 (ferrite#213): stopping a scan with nothing to move to (Return to "Start", or "Last hit" with no hit)
/// retunes the slot where it shows, on a VFO and on a channel: the Radio task's receiver was left on the scan's
/// last step, so the display said the start and the audio was the step's (or none, a channel's tone unmatched).
#[test]
fn scan_stop_retunes_the_start() {
    let mut t = T::new();
    for round in 0..4 {
        if round == 2 {
            t.tap(Key::VfoMem); // the other mode: VFO and channel both
        }
        t.app.set.scan_return_hit = round % 2 == 1;
        let start = t.freq();
        t.tap(Key::Side(2));
        t.live.slots[0] = SlotLive { scanning: true, freq: start + 7_500, ..Default::default() };
        t.wait(200);
        t.cmds();
        t.tap(Key::Side(2));
        t.live.slots[0] = SlotLive { freq: start + 7_500, ..Default::default() };
        t.wait(200);
        assert_eq!(t.freq(), start, "round {round}: shows the start");
        let c = t.cmds();
        assert!(c.contains(&RadioCmd::TuneSlot { slot: 0, freq: start }), "round {round}: retuned to {start}: {c:?}");
    }
}

/// APRS-47, APRS-48, APRS-49: the symbol, TX delay and TX level step and reset in the menu, and the levels are
/// the calibrated 3.0 kHz one's neighbours.
#[test]
fn aprs_symbol_delay_and_level() {
    use crate::aprs::{SYMBOLS, TXDELAY_DEFAULT};
    let mut t = T::new();
    assert_eq!(SYMBOLS[t.app.aprs_cfg().symbol as usize].0, "Person");
    assert_eq!(t.app.aprs_cfg().symbol_chars(), (b'/', b'['), "what beacons always sent");
    change(&mut t.app, &[5, 5, 2], 1);
    assert_eq!(t.app.aprs_cfg().symbol_chars(), (b'/', b'>'), "Car");
    change(&mut t.app, &[5, 9, 3], 2);
    assert_eq!(t.app.aprs_cfg().txdelay_10ms(), TXDELAY_DEFAULT + 10, "50 ms a step");
    change(&mut t.app, &[5, 9, 3], -100);
    assert_eq!(t.app.aprs_cfg().txdelay_10ms(), 10, "100 ms at least");
    change(&mut t.app, &[5, 9, 4], 1);
    assert_eq!(t.app.aprs_cfg().tx_level, 2);
    crate::screens::groups::reset(&mut t.app, &[5, 9, 4]);
    assert_eq!(t.app.aprs_cfg().tx_level, 1);
    let a = ferrite_core::audio::AFSK_AMPS;
    assert!(a[0] < a[1] && a[1] == ferrite_core::audio::AFSK_AMP && a[1] < a[2]);
    assert_eq!(ferrite_core::audio::afsk_tx(300).amp, ferrite_core::audio::afsk_tx_at(300, 1).amp);
}

/// SQL-9.3: SIDE1 is the monitor key on every screen that doesn't give it a job of its own (it worked on home
/// only: Satellites, the menus and the rest ignored it). A tap latches it, a hold opens it while held, and the
/// screens with their own SIDE1 (the squelch screen, the key test) keep theirs.
#[test]
fn side1_monitors_on_every_screen() {
    use crate::screens::nav::Dest;
    let mon = |c: &[RadioCmd]| c.iter().find_map(|c| if let RadioCmd::Monitor { slot, on } = c { Some((*slot, *on)) } else { None });
    for dest in [Dest::Satellites, Dest::Scan, Dest::About] {
        let mut t = T::new();
        t.open(dest);
        t.cmds();
        let at = t.app.screen();
        t.tap(Key::Side(1));
        assert_eq!(mon(&t.cmds()), Some((0, true)), "{dest:?}: tap latches");
        assert_eq!(t.app.screen(), at, "{dest:?}: the screen stays");
        t.tap(Key::Side(1));
        assert_eq!(mon(&t.cmds()), Some((0, false)), "{dest:?}: tap again ends it");
        // SQL-9.1: held, open while held and closed at the release
        t.hold(Key::Side(1));
        let c = t.cmds();
        assert!(c.contains(&RadioCmd::Monitor { slot: 0, on: true }) && c.contains(&RadioCmd::Monitor { slot: 0, on: false }), "{dest:?}: held: {c:?}");
    }
    // The quick settings menu (MENU) too
    let mut t = T::new();
    t.tap(Key::Menu);
    t.cmds();
    t.tap(Key::Side(1));
    assert_eq!(mon(&t.cmds()), Some((0, true)), "the menu");
    assert!(matches!(t.app.screen(), Screen::Quick(_)), "the menu stays open");
    // The key test keeps SIDE1 as a key under test, the scope listens with it; the counter has no audio
    for dest in [Dest::KeyTest, Dest::Counter, Dest::BandScope] {
        let mut t = T::new();
        t.open(dest);
        t.cmds();
        t.tap(Key::Side(1));
        assert_eq!(mon(&t.cmds()), None, "{dest:?}: its own");
    }
}

/// SQL-11, SQL-9.3: locked (the keypad lock lets the side keys through), SIDE1 still monitors off home.
#[test]
fn side1_monitors_while_locked_off_home() {
    let mut t = T::new();
    t.open(crate::screens::nav::Dest::Satellites);
    lock_in(&mut t, Lock::Keypad);
    t.cmds();
    t.tap(Key::Side(1));
    assert!(t.cmds().contains(&RadioCmd::Monitor { slot: 0, on: true }));
}

/// SAT-1: SIDE2 (scan) does nothing in satellite mode: the satellites own both chips, so no scan starts, tapped
/// or held, and the screen stays.
#[test]
fn side2_does_nothing_in_satellite_mode() {
    let mut t = T::new();
    t.open(crate::screens::nav::Dest::Satellites);
    t.cmds();
    let at = t.app.screen();
    t.tap(Key::Side(2));
    t.hold(Key::Side(2));
    let c = t.cmds();
    assert!(!c.iter().any(|c| matches!(c, RadioCmd::Scan { .. })), "no scan: {c:?}");
    assert_eq!(t.app.screen(), at);
}

/// WATCH-6, MODE-5.1 (ferrite#134): in single + scope with the strip paused, a watched slot receiving makes # the
/// watch's pause again, as everywhere else; it resumed the strip, which stopped the watch and APRS.
#[test]
fn hash_pauses_the_watch_while_the_strip_is_paused() {
    let mut t = T::new();
    t.app.set_layout(1);
    t.app.strip_paused = true;
    t.live.slots[1] = SlotLive { squelch_open: true, carrier: true, rssi: -90, ..Default::default() };
    t.wait(100);
    assert!(t.app.strip_pausable() && t.app.watched_rx());
    let _ = t.cmds();
    t.tap(Key::Hash);
    assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::PauseWatch(p) if *p > 0)), "the watch paused");
    assert!(t.app.strip_paused, "the strip stays paused");
}
