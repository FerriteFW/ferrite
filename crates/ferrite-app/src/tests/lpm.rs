//! Low Power Mode (PWR-24, decision 234).

use super::*;
use crate::lpm::{LPM_BACKLIGHT, LPM_COMBO_MS};
use crate::screens::nav::Dest;

/// SIDE1 down, then SIDE2, held `ms`, then both let go.
fn side_combo(t: &mut T, ms: u32) {
    let s = store();
    t.app.key_down(Key::Side(1), t.t, &s);
    t.t += 50;
    t.app.key_down(Key::Side(2), t.t, &s);
    let end = t.t + ms;
    while t.t < end {
        t.t += 10;
        t.app.tick(t.t, &t.live, &s);
    }
    t.app.key_up(Key::Side(2), t.t, &s);
    t.app.key_up(Key::Side(1), t.t, &s);
    t.t += 50;
    t.app.tick(t.t, &t.live, &s);
}

/// PWR-24.2, PWR-24.4, PWR-24.5: 0 7 7 2 On enters (home, Battery Save's cycle, watch to the active slot alone);
/// the overrides apply while in it and none touches a stored setting; Off leaves and everything is as it was. The
/// mode screens are refused, satellite mode too (ferrite#133: it kept both chips awake).
#[test]
fn on_and_off_restore_everything() {
    let mut t = T::new();
    t.app.set.lpm = 0; // the default is Auto (PWR-24.1); these start from Off
    t.app.set.bt = true;
    t.app.set.gps = crate::settings::GPS_ON;
    t.app.set_aprs_mode(true);
    t.app.set.pocsag_on = true;
    t.app.set.keypad_light = 3; // always
    *t.app.set.brightness_mut() = 10;
    t.wait(200);
    t.cmds();
    let before = *t.app.set;
    t.open(Dest::Group(7));
    assert!(crate::screens::groups::change(&mut t.app, &[7, 7, 2], -1)); // Off -> On (back: Off, Auto, On)
    assert!(t.app.low_power());
    assert_eq!(t.app.screen, Screen::Home, "home on entering");
    let c = t.cmds();
    assert!(c.contains(&RadioCmd::LowPower(true)), "{c:?}");
    assert!(c.contains(&RadioCmd::Watch(0)), "the active slot alone: {c:?}");
    assert!(!t.app.aprs_on() && !t.app.aprs_c() && !t.app.bt_on() && !t.app.gps_cfg().on && !t.app.kiss_on());
    assert_eq!(t.app.pager_slot(&t.live), None);
    assert_eq!(t.app.backlight_duty(), LPM_BACKLIGHT, "at most 25 %");
    assert_eq!(t.app.keypad_duty(), 0);
    assert!(t.app.status_bar(&t.live, &Device::default()).lpm, "SBAR-24");
    assert_eq!(Settings { lpm: 0, ..*t.app.set }, Settings { lpm: 0, ..before }, "nothing stored changed but 0 7 7 2");
    // Scan and the mode screens are refused with a toast.
    t.tap(Key::Side(2));
    assert_eq!(t.app.screen, Screen::Home, "no scan");
    crate::screens::entry::tune_vfo(&mut t.app, 0, ferrite_core::freq::mhz(100) + 50_000);
    t.wait(100);
    assert!(!t.app.radio.fm_on, "no broadcast: a slot in the band doesn't play it (BCR-21)");
    t.app.open(Dest::Crossband);
    assert_eq!(t.app.screen, Screen::Home, "no crossband (two VFOs)");
    t.app.open(Dest::BandScope);
    assert_eq!(t.app.screen, Screen::Home, "no band scope (a sweeping receiver)");
    t.app.open(Dest::Counter);
    assert_eq!(t.app.screen, Screen::Home, "no counter (a listening receiver)");
    t.app.open(Dest::Satellites);
    assert_eq!(t.app.screen, Screen::Home, "no satellite mode (both chips awake; ferrite#133)");
    t.app.open(Dest::Group(7));
    assert_ne!(t.app.screen, Screen::Home, "the menus stay");
    t.app.go_home();
    // Off: as before.
    assert!(crate::screens::groups::change(&mut t.app, &[7, 7, 2], 1)); // On -> Off
    assert!(!t.app.low_power());
    let c = t.cmds();
    assert!(c.contains(&RadioCmd::LowPower(false)), "{c:?}");
    assert!(t.app.aprs_on() && t.app.bt_on() && t.app.gps_cfg().on);
    assert_eq!(t.app.backlight_duty(), 1000);
    assert_eq!(*t.app.set, before, "every setting as it was");
}

/// PWR-24.4: SIDE1 + SIDE2 held 1 s leaves (On becomes Off); shorter does nothing; out of the mode the combo does
/// nothing at all; the hold's progress is there for a fill bar.
#[test]
fn side_keys_leave_only() {
    let mut t = T::new();
    t.app.set.lpm = 0; // the default is Auto (PWR-24.1); these start from Off
    t.app.set.backlight = 0; // never dark: every key acts
    side_combo(&mut t, 1500);
    assert!(!t.app.low_power(), "never enters");
    t.app.set_lpm_mode(1);
    assert!(t.app.low_power());
    side_combo(&mut t, 500);
    assert!(t.app.low_power(), "half a second isn't enough");
    // Half way through a hold, the progress is about half.
    let s = store();
    t.app.key_down(Key::Side(1), t.t, &s);
    t.app.key_down(Key::Side(2), t.t, &s);
    t.t += LPM_COMBO_MS / 2;
    t.app.tick(t.t, &t.live, &s);
    assert!(t.app.lpm_leave_progress().is_some_and(|p| (400..=600).contains(&p)), "{:?}", t.app.lpm_leave_progress());
    t.t += LPM_COMBO_MS / 2 + 20;
    t.app.tick(t.t, &t.live, &s);
    assert!(!t.app.low_power(), "left at 1 s, still held");
    t.app.key_up(Key::Side(2), t.t, &s);
    t.app.key_up(Key::Side(1), t.t, &s);
    assert_eq!(t.app.set.lpm, 0, "On became Off");
    assert_eq!(t.app.lpm_leave_progress(), None);
    assert_eq!(t.app.screen, Screen::Home, "neither key did its own thing (no counter, no scan)");
}

/// PWR-24.3: Auto enters below its level (once per crossing), leaves by itself at 80 %; left by the keys it
/// waits for the battery 10 points above its level; no reading yet does nothing.
#[test]
fn auto_follows_the_battery() {
    let mut t = T::new();
    t.app.set.lpm = 0; // the default is Auto (PWR-24.1); these start from Off
    t.app.set.backlight = 0;
    t.app.set_lpm_mode(2);
    t.wait(200);
    assert!(!t.app.low_power(), "no reading yet");
    t.app.battery(25);
    t.wait(200);
    assert!(!t.app.low_power(), "above 20 %");
    t.app.battery(19);
    t.wait(200);
    assert!(t.app.low_power(), "below 20 %");
    side_combo(&mut t, 1200);
    assert!(!t.app.low_power());
    assert_eq!(t.app.set.lpm, 2, "Auto stays Auto");
    t.app.battery(18);
    t.wait(200);
    assert!(!t.app.low_power(), "not again on the same crossing");
    t.app.battery(29);
    t.wait(200);
    t.app.battery(19);
    t.wait(200);
    assert!(!t.app.low_power(), "29 isn't 10 points above 20");
    t.app.battery(30);
    t.wait(200);
    t.app.battery(19);
    t.wait(200);
    assert!(t.app.low_power(), "back above 30 and down again: a new crossing");
    t.app.battery(79);
    t.wait(200);
    assert!(t.app.low_power(), "79 %: still in");
    t.app.battery(80);
    t.wait(200);
    assert!(!t.app.low_power(), "the charger: out at 80 %");
    // 0 7 7 3's level.
    t.app.set.lpm_below = 40;
    t.app.battery(39);
    t.wait(200);
    assert!(t.app.low_power());
}

/// PWR-24.1, PWR-24.3: the menu turns Off -> Auto -> On (Auto never passes On, which would enter at once and leave
/// the menu); choosing Auto decides from the battery now, also coming back from On (Oct 21, 2026, on black: `set`
/// stepped through On at 32 % and Auto then held the mode until 80 %).
/// PWR-24.1, PWR-24.3: Auto at 20 % is the default; On by hand never leaves by itself (Alex: entered manually, "we
/// dont auto exit it").
#[test]
fn on_by_hand_never_leaves_by_itself() {
    let mut t = T::new();
    t.app.set.lpm = 0;
    t.app.battery(30);
    t.wait(200);
    t.app.set_lpm_mode(1);
    assert!(t.app.low_power());
    t.app.battery(100);
    t.wait(200);
    assert!(t.app.low_power(), "On by hand: no exit at 80 % or 100 %");
    assert_eq!(crate::settings::Settings::DEFAULT.lpm, 2, "Auto by default");
    assert_eq!(crate::settings::Settings::DEFAULT.lpm_below, 20, "at 20 %");
}

#[test]
fn auto_is_chosen_without_passing_on() {
    let mut t = T::new();
    t.app.set.lpm = 0; // the default is Auto (PWR-24.1); these start from Off
    t.app.battery(32);
    t.wait(200);
    assert!(crate::screens::groups::change(&mut t.app, &[7, 7, 2], 1));
    assert_eq!(t.app.set.lpm, 2, "Off, then Auto");
    assert!(!t.app.low_power(), "32 % is above 20 %");
    assert!(crate::screens::groups::change(&mut t.app, &[7, 7, 2], 1));
    assert_eq!(t.app.set.lpm, 1, "then On");
    assert!(t.app.low_power());
    assert!(crate::screens::groups::change(&mut t.app, &[7, 7, 2], -1));
    assert_eq!(t.app.set.lpm, 2, "back to Auto");
    assert!(!t.app.low_power(), "Auto at 32 %: out");
    assert!(crate::screens::groups::change(&mut t.app, &[7, 7, 2], 1));
    assert!(crate::screens::groups::change(&mut t.app, &[7, 7, 2], 1));
    assert_eq!(t.app.set.lpm, 0, "On, then Off");
}

/// PWR-24.1: an older record (zeros where the two bytes now are) loads as Off and 20 %.
#[test]
fn older_record_reads_off_and_twenty() {
    let mut s = Settings { lpm: 0, lpm_below: 0, ..Settings::DEFAULT };
    s.sanitize();
    assert_eq!((s.lpm, s.lpm_below), (0, 20));
    let mut s = Settings { lpm: 7, lpm_below: 35, ..Settings::DEFAULT };
    s.sanitize();
    assert_eq!((s.lpm, s.lpm_below), (0, 20));
}

/// PWR-24.8: the active slot's last reception (when it closed, how long, the peak), kept for Low Power Mode's home;
/// another slot's reception doesn't replace it, and a new one on the active slot does.
#[test]
fn last_heard_is_kept() {
    let mut t = T::new();
    t.app.set.lpm = 0;
    let a = t.app.active as usize;
    assert_eq!(t.app.last_rx.2, 0, "nothing heard yet");
    t.live.slots[a].squelch_open = true;
    t.live.slots[a].rssi = -100;
    t.wait(500);
    t.live.slots[a].rssi = -80;
    t.wait(1500);
    t.live.slots[a].squelch_open = false;
    t.wait(100);
    let (slot, from, to, peak) = t.app.last_rx;
    assert_eq!(slot as usize, a);
    assert!(to > from && (1900..=2300).contains(&(to - from)), "about 2 s: {}", to - from);
    assert_eq!(peak, -80, "the peak");
    let other = (a + 1) % 3;
    t.live.slots[other].squelch_open = true;
    t.wait(500);
    t.live.slots[other].squelch_open = false;
    t.wait(100);
    assert_eq!(t.app.last_rx.0 as usize, a, "another slot's reception doesn't replace it");
}
