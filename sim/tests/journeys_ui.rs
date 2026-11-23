//! End-to-end checks of the look and the keys that hold on every screen: the status bar, the hint bar, the
//! screen layout, redraws and key timing (manual chapters 6, 7, 8, 11 and 31). The harness is `common`; a rule
//! counts as covered when its ID is in a test's doc comment.

mod common;

use common::Radio;
use ferrite_core::keys::Key;
use ferrite_sim::simcore::{rgb565_to_rgb, Event, TextItem};
use ferrite_ui::gfx::RED;

/// The text items drawn in the status bar (rows 0-19).
fn bar(r: &mut Radio) -> Vec<TextItem> {
    r.sim.screen_texts().into_iter().filter(|t| t.y1 < 20).collect()
}

/// Every screen of the jump list in turn (0 = home), from a fresh radio each time.
fn each_screen(mut f: impl FnMut(&mut Radio, &str)) {
    let n = ferrite_app::screens::nav::ALL.len() + 1;
    for i in 0..n {
        let mut r = Radio::new();
        r.sim.jump(i);
        r.wait(400);
        let name = r.screen();
        f(&mut r, name);
    }
}

/// The bar is 20 px tall and holds device-wide state only: no slot's power, tones, shift or mode
/// (SBAR-1).
#[test]
fn bar_is_device_wide() {
    let mut r = Radio::new();
    for t in r.sim.screen_texts() {
        assert!(t.y1 < 20 || t.y0 >= 20, "{:?} straddles the bar's edge", t.s);
    }
    for t in bar(&mut r) {
        for slot_word in ["LOW", "MID", "HIGH", "FM", "NFM", "AM", "T 100.0", "-600k", "+5M"] {
            assert_ne!(t.s, slot_word, "per-slot state in the bar");
        }
    }
}

/// The clock sits in the middle of the bar, `Z` after it while the zone is UTC, local time without it
/// otherwise; the Status page names the clock's source (SBAR-2, GPS-8).
#[test]
fn bar_clock() {
    let mut r = Radio::new();
    let clock = bar(&mut r).into_iter().find(|t| t.s.contains(':')).expect("a clock");
    assert_eq!(clock.s, "19:06Z", "UTC by default (decision 71)");
    let mid = (clock.x0 + clock.x1) / 2;
    assert!((mid - 120).abs() <= 3, "centred: {mid}");
    r.sim.app_mut().settings_mut().tz = 9; // UTC-4
    r.wait(1500);
    assert!(bar(&mut r).iter().any(|t| t.s == "15:06"), "local time, no Z: {:?}", bar(&mut r).iter().map(|t| &t.s).collect::<Vec<_>>());
    r.run("menu 0 0; expect text \"Clock\"");
    let src = r.text();
    assert!(src.contains("From GPS") || src.contains("Manual"), "the clock's source on Status:\n{src}");
}

/// Items appear only when they apply and keep their places: turning Bluetooth on doesn't move the clock or the
/// left side (SBAR-4).
#[test]
fn bar_items_keep_their_places() {
    let mut r = Radio::new();
    let before = bar(&mut r);
    r.event(Event::Bluetooth).wait(300);
    let after = bar(&mut r);
    for t in &before {
        assert!(after.iter().any(|a| a.s == t.s && a.x0 == t.x0), "{:?} moved or went when Bluetooth came on", t.s);
    }
}

/// The bar names the screen in 8 characters or fewer, and nothing on home (SBAR-5).
#[test]
fn bar_screen_name() {
    let mut r = Radio::new();
    assert!(bar(&mut r).iter().all(|t| t.x0 > 8 || t.s == "TW" || t.s == "SCN" || t.s == "DW"), "a name on home");
    each_screen(|r, name| {
        if name == "Home" {
            return;
        }
        if let Some(first) = bar(r).into_iter().find(|t| t.x0 == 8) {
            if !first.s.contains(':') {
                assert!(first.s.chars().count() <= 8, "{name}: bar name {:?} longer than 8", first.s);
            }
        }
    });
}

/// One envelope with the count of unread messages to us (INBOX-3.1); the count turns red while an alert waits,
/// and goes when nothing is unread (SBAR-6, INBOX-3, WX-8).
#[test]
fn bar_inbox() {
    let mut r = Radio::new();
    let count = |r: &mut Radio| bar(r).into_iter().find(|t| t.s.chars().all(|c| c.is_ascii_digit()) && t.x0 < 100);
    let c = count(&mut r).expect("the sample has unread messages");
    assert_eq!(c.s, "1", "the sample's one unread message, not its log entries");
    let red = |r: &mut Radio, t: &TextItem| {
        let px = r.sim.screen_rgb();
        (t.y0..=t.y1).any(|y| (t.x0..=t.x1).any(|x| px[(y * 240 + x) as usize] == rgb565_to_rgb(RED)))
    };
    assert!(!red(&mut r, &c), "not red without an alert");
    r.event(Event::Alert).wait(300);
    r.press(Key::Menu).wait(300); // OK acknowledges the alert screen
    r.home();
    let c = count(&mut r).expect("count after the alert");
    assert_eq!(c.s, "1", "an alert is the log's: the count stays the messages'");
    assert!(red(&mut r, &c), "red while the alert is unread");
}

/// OK on a Status row opens that row's setting (SBAR-15).
#[test]
fn status_row_opens_its_setting() {
    let mut r = Radio::new();
    r.run("menu 0 0; expect screen \"Status\"; press DOWN; press OK; wait 300ms");
    assert_ne!(r.screen(), "Status", "OK on Region opened nothing");
    r.expect_text("Region");
}

/// A toast takes the hint bar's place for 3 s and never covers the frequency (HOME-9).
#[test]
fn toast_replaces_the_hint_bar() {
    let mut r = Radio::new();
    r.press(Key::Slot); // B, on a channel
    r.hold(Key::Digit(9)); // copy to VFO: asked, then a toast (KEY-23)
    r.press(Key::Menu);
    let items = r.sim.screen_texts();
    let toast = items.iter().find(|t| t.s.contains("Copied")).expect("the toast");
    assert!(toast.y0 >= 300, "the toast is in the hint bar's rows: {}", toast.y0);
    assert!(items.iter().any(|t| t.s.contains("443.475") || t.s.contains("146.520")), "frequencies still shown");
    r.wait(3200);
    r.expect_no_text("Copied").expect_text("Hold ABC"); // the hint bar back
}

/// A short press fires on release before 700 ms; a long press fires while held at 700 ms, and its release does
/// nothing more (KEY-3).
#[test]
fn short_and_long_press() {
    let mut r = Radio::new();
    r.down(Key::Digit(1)).wait(600);
    r.expect_screen("Home");
    r.up(Key::Digit(1)).wait(100);
    r.expect_screen("Frequency entry");
    r.home();
    r.down(Key::Digit(1)).wait(760);
    r.expect_screen("Scope");
    r.up(Key::Digit(1)).wait(200);
    r.expect_screen("Scope");
}

/// The quick menu (HOME-17): hold * on home brings the box up over home naming each key and its state; PTT A,
/// PTT B, PF1, PF2 and the knob act at once and the rows follow; PTT never keys while it is up, nor a PTT held
/// through its close; BACK closes it; it closes by itself after 5 s; afterwards PTT keys as usual.
#[test]
fn quick_menu_on_hold_star() {
    use ferrite_core::msg::RadioCmd;
    let mut r = Radio::new();
    r.home();
    let keyed = |r: &mut Radio| r.take_cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. }));
    r.hold(Key::Star);
    for t in ["PTT A", "Reverse", "PTT B", "Night red", "PF1", "GPS", "PF2", "Bluetooth", "Knob", "Brightness"] {
        r.expect_text(t);
    }
    let rows = |r: &mut Radio| {
        let mut v: Vec<_> = r.sim.screen_texts().into_iter().filter(|t| t.y0 > 100 && t.y1 < 220 && t.x0 >= 170 && t.x1 <= 200).collect();
        v.sort_by_key(|t| t.y0);
        v.into_iter().map(|t| t.s).collect::<Vec<_>>()
    };
    assert_eq!(rows(&mut r), ["7", "off", "off", "on", "off"], "brightness, reverse, night red, GPS, Bluetooth (HOME-17.1)");
    r.take_cmds();
    r.down(Key::Ptt(ferrite_core::keys::PTT_A)).wait(1500);
    r.expect_screen("Home");
    assert!(!keyed(&mut r), "PTT A in the menu never keys");
    r.up(Key::Ptt(ferrite_core::keys::PTT_A)).wait(100);
    r.press(Key::Side(1)).press(Key::Side(2)).knob(2).wait(300);
    assert_eq!(rows(&mut r), ["9", "on", "off", "off", "on"]);
    // PTT B: night red, and back.
    r.down(Key::Ptt(ferrite_core::keys::PTT_B)).wait(300);
    assert_eq!(r.sim.app().settings().theme, 2, "night red");
    r.up(Key::Ptt(ferrite_core::keys::PTT_B)).press(Key::Ptt(ferrite_core::keys::PTT_B)).wait(300);
    assert_eq!(r.sim.app().settings().theme, 0);
    assert!(!keyed(&mut r));
    // BACK closes it and does nothing else; the card shows the reverse.
    r.press(Key::Exit).wait(300);
    r.expect_no_text("Brightness").expect_screen("Home").expect_text("R-600k");
    // Held through the 5 s close: never keys; pressed again: keys.
    r.hold(Key::Star).down(Key::Ptt(ferrite_core::keys::PTT_A)).wait(7000);
    r.expect_no_text("Brightness");
    assert!(!keyed(&mut r), "held through the close: never keys");
    r.up(Key::Ptt(ferrite_core::keys::PTT_A)).wait(200);
    r.down(Key::Ptt(ferrite_core::keys::PTT_A)).wait(300);
    assert!(keyed(&mut r), "after the menu: PTT A keys");
    r.up(Key::Ptt(ferrite_core::keys::PTT_A)).wait(1000);
}

/// Keys typed faster than a frame are all taken, in order (KEY-6).
#[test]
fn typed_ahead_keys_are_kept() {
    let mut r = Radio::new();
    for d in [4, 4, 6, 1] {
        r.sim.key(Key::Digit(d), true);
        r.sim.key(Key::Digit(d), false);
    }
    r.wait(300);
    r.expect_screen("Frequency entry").expect_text("446.1");
}

/// # on home opens the quick help (decision 376); while a watched slot is heard it pauses watch instead (KEY-17,
/// HOME-10, WATCH-6).
#[test]
fn hash_on_home() {
    let mut r = Radio::new();
    r.run("press HASH; expect screen \"Holds\"; home");
    r.wait(17_000); // B is heard from 20 to 22 s
    assert!(r.sim.radio().live.slots[1].squelch_open, "B heard");
    r.press(Key::Hash);
    r.expect_screen("Home");
}

/// Every screen keeps the layout grid: the status bar in rows 0-19, the hint bar in 300-319, content between,
/// text inside the 8 px side margins (DISP-2), and a hint bar that says what keys do (DISP-3).
#[test]
fn layout_grid_on_every_screen() {
    each_screen(|r, name| {
        let texts = r.sim.screen_texts();
        for t in &texts {
            let band = |a: i32, b: i32| t.y0 >= a && t.y1 <= b;
            assert!(band(0, 19) || band(20, 299) || band(300, 319), "{name}: {:?} at y {}-{} crosses a band", t.s, t.y0, t.y1);
            assert!(t.x0 >= 8 && t.x1 <= 232, "{name}: {:?} at x {}-{} outside the margins", t.s, t.x0, t.x1);
        }
        // The power-off and empty-battery screens take no keys, so their hint bar is rightly empty.
        assert!(name == "Power" || texts.iter().any(|t| t.y0 >= 300), "{name}: an empty hint bar");
    });
}

/// Nothing moves on a quiet, idle screen: no decorative animation, and only changed rectangles are redrawn
/// (DISP-13, DISP-4).
#[test]
fn idle_screen_is_still() {
    let mut r = Radio::new();
    r.sim.radio_mut().quiet = true;
    r.press(Key::Slot).press(Key::Slot); // C, scanning
    r.press(Key::Side(2)).wait(500); // stop its scan
    r.home().press(Key::Slot).wait(2000);
    let a = r.sim.screen_rgb();
    r.wait(2000);
    let b = r.sim.screen_rgb();
    let changed = a.iter().zip(&b).filter(|(x, y)| x != y).count();
    assert_eq!(changed, 0, "{changed} pixels changed on an idle screen");
    r.knob(1).wait(100); // tune: only the frequency's rectangles are redrawn
    assert!(r.sim.last_px() < 240 * 320 / 4, "a tuning step redrew {} pixels", r.sim.last_px());
}

/// A key's meaning doesn't depend on a setting, the theme or the layout: hold 1 opens the band scope and #
/// the quick help whatever the look (DISP-14, NAV-6, KEY-1), but for single + scope, where # pauses and resumes
/// the strip, as its hint says (MODE-5.1, decision 441).
#[test]
fn keys_mean_the_same_whatever_the_look() {
    let mut r = Radio::new();
    r.sim.radio_mut().quiet = true; // nothing heard, so # always means the quick help
    for layout in 0..3 {
        for theme in 0..3 {
            r.sim.app_mut().settings_mut().theme = theme;
            r.wait(200);
            if layout == 1 {
                r.run("hold 1; expect screen \"Scope\"; home; press HASH; expect screen \"Home\"; expect text \"Scope paused\"; press HASH; home");
            } else {
                r.run("hold 1; expect screen \"Scope\"; home; press HASH; expect screen \"Holds\"; home");
            }
        }
        let _ = layout;
        r.event(Event::Layout);
    }
}

/// A number path means one thing whatever slot is active or what it's on: 0 8 2 is VOX from a VFO slot and
/// from a channel slot (MENU-6).
#[test]
fn numbers_mean_one_thing() {
    let mut r = Radio::new();
    r.run("menu 0 8 2; expect text \"VOX\"; expect text \"Tail\"; home; press SLOT");
    r.run("menu 0 8 2; expect text \"VOX\"; expect text \"Tail\"");
}

/// A change in quick settings is kept at once, offered for undo (* Undo) for 5 s, and saved to flash within
/// 2 s, surviving a power cycle (MENU-21, MENU-22).
#[test]
fn change_undo_and_save() {
    let mut r = Radio::new();
    let sq = |r: &Radio| r.sim.app().slot(0).cfg().squelch;
    let before = sq(&r);
    r.run("press OK; knob 1; expect text \"Undo\"");
    assert_eq!(sq(&r), before + 1);
    r.press(Key::Star).wait(200);
    assert_eq!(sq(&r), before, "* undid it");
    r.knob(1).wait(5200);
    r.expect_no_text("Undo");
    r.home().wait(2100);
    r.event(Event::PowerSwitch).wait(2000).event(Event::PowerSwitch).wait(4000);
    assert_eq!(sq(&r), before + 1, "kept across a power cycle");
}

/// DISP-28: a knob step in compose's address book and quick messages repaints about as much as one in the
/// channel list (two rows and the position line), not the whole box (34,560 px a step until Oct 22, 2026).
#[test]
fn compose_lists_repaint_by_row() {
    let per_step = |r: &mut Radio| {
        r.wait(500);
        r.sim.watch_passes(true);
        r.sim.passes.clear();
        for _ in 0..4 {
            r.knob(1).wait(200);
        }
        let px: u64 = r.sim.passes.drain(..).map(|h| h.report.writes).sum();
        r.sim.watch_passes(false);
        px / 4
    };
    let mut r = Radio::new();
    r.run("menu 0 2 1; wait 300ms");
    let channels = per_step(&mut r);
    let mut r = Radio::new();
    r.run("menu 0 5 2; press HASH; wait 300ms; expect text \"Address book\"");
    let book = per_step(&mut r);
    r.run("press OK; wait 300ms; expect text \"Quick messages\"");
    let quick = per_step(&mut r);
    assert!(book <= channels * 5 / 4 && quick <= channels * 5 / 4, "book {book}, quick {quick}, channels {channels} px a step");
}

/// SAT-12.1, SAT-17.1: the alarm picker stays whole while the page under it ticks (the countdown before the rise, the live
/// values while up), and closing it leaves the page exactly as a radio that never opened it; with the pass up the
/// list's time to set and, over an hour away, the pointing page's rise fit their fields (the drop checks the cuts).
#[test]
fn sat_alarm_picker_stays_whole() {
    use ferrite_core::keys::Key;
    let box_px = |r: &Radio| -> Vec<u32> {
        let px = r.sim.screen_rgb();
        (84..234).flat_map(|y| (40..200).map(move |x| (x, y))).map(|(x, y)| px[y * 240 + x]).collect()
    };
    let (mut a, mut b) = (Radio::new(), Radio::new());
    for r in [&mut a, &mut b] {
        r.sim.jump(68);
        r.wait(1000);
        r.press(Key::Menu).wait(800); // the pass page, ISS an hour away
        r.press(Key::Digit(0)).wait(800); // the pointing page: "rises 1h02"
        r.press(Key::Digit(0)).wait(800); // the plot again
    }
    for up in [false, true] {
        if up {
            for r in [&mut a, &mut b] {
                r.press(Key::Exit).wait(500);
                r.wait(64 * 60_000); // ISS up
                r.press(Key::Menu).wait(800);
            }
        }
        a.press(Key::Hash).wait(300); // a press is 200 ms: the twin waits the same 500
        b.wait(500);
        let open = box_px(&a);
        for _ in 0..5 {
            a.wait(1000);
            b.wait(1000);
            assert!(box_px(&a) == open, "up={up}: the picker changed while the page ticked");
        }
        a.press(Key::Exit).wait(300);
        b.wait(500);
        assert!(a.sim.screen_rgb() == b.sim.screen_rgb(), "up={up}: closing it left something behind");
    }
}
