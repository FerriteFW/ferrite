//! The quick menu (hold \* on home; HOME-17, decided Oct 12, 2026): its keys, how it closes, and the PTT safety.

use super::*;
use crate::lights::BRIGHTNESS;
use crate::screens::qmenu::{lines, IDLE_MS};
use crate::settings::NIGHT_RED;
use ferrite_core::keys::{PTT_A, PTT_B};
use crate::persist::{Persist, Sink};
use ferrite_core::txplan::Region;
use std::string::String;

/// An app that can transmit (the tx build, a region), on home with slot A active.
fn ready() -> T {
    let mut t = T::new();
    t.app.radio.tx_built = true;
    t.app.set_region(Some(Region::Us), false);
    t
}

/// Hold \* on home: the quick menu is up (its release does nothing).
fn open(t: &mut T) {
    t.hold(Key::Star);
    assert!(t.app.qmenu.is_some(), "the quick menu is up");
    assert_eq!(t.app.screen(), Screen::Home);
}

fn down(t: &mut T, k: Key) {
    t.app.key_down(k, t.t, &store());
    t.t += 50;
    t.app.tick(t.t, &t.live, &store());
}

fn up(t: &mut T, k: Key) {
    t.app.key_up(k, t.t, &store());
    t.t += 50;
    t.app.tick(t.t, &t.live, &store());
}

/// The state the box shows for key `i` (0 PTT A, 1 PTT B, 2 PF1, 3 PF2, 4 the knob), whose row is one lower
/// (HOME-17.1: the knob's row is first).
fn shown(t: &T, i: usize) -> String {
    String::from_utf8(lines(&t.app)[(i + 1) % 5].2.as_bytes().to_vec()).unwrap()
}

fn keyed(c: &[RadioCmd]) -> bool {
    c.iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. } | RadioCmd::TxPlan { .. }))
}

/// HOME-17: hold \* on home opens the box over home; nothing else happens (no mute, no reverse) and the \*'s
/// release does nothing.
#[test]
fn hold_star_opens_the_quick_menu() {
    let mut t = T::new();
    let rev = t.app.slot(0).cfg().reverse;
    t.cmds();
    open(&mut t);
    assert_eq!(t.app.slot(0).cfg().reverse, rev);
    assert!(!t.cmds().iter().any(|c| matches!(c, RadioCmd::Mute(_) | RadioCmd::SlotParam { .. })));
    assert_eq!(shown(&t, 0), "off");
    assert_eq!(shown(&t, 4), "7");
    assert!(t.app.toast.is_none(), "no toast: the box is the feedback");
}

/// HOME-17, VFO-7: PTT A toggles reverse on the active slot, at once, and the row says so; it never transmits.
#[test]
fn ptt_a_is_reverse() {
    let mut t = ready();
    open(&mut t);
    t.cmds();
    down(&mut t, Key::Ptt(PTT_A));
    assert!(t.app.slot(0).cfg().reverse, "reversed at the press");
    assert_eq!(shown(&t, 0), "on");
    let c = t.cmds();
    assert!(c.iter().any(|c| matches!(c, RadioCmd::SlotParam { slot: 0, param: ferrite_core::msg::Param::Reverse, value: 1 })), "{c:?}");
    assert!(!keyed(&c), "never keyed: {c:?}");
    up(&mut t, Key::Ptt(PTT_A));
    assert_eq!(t.app.screen(), Screen::Home);
    down(&mut t, Key::Ptt(PTT_A));
    up(&mut t, Key::Ptt(PTT_A));
    assert!(!t.app.slot(0).cfg().reverse, "again: off");
    assert_eq!(shown(&t, 0), "off");
    assert!(!keyed(&t.cmds()));
    assert!(t.app.qmenu.is_some(), "the box stays up");
}

/// VFO-7: reverse listens on the input: the receiver is retuned to the TX side as reverse goes on, and back
/// as it goes off (a Discord report, ferrite#102: only the TX side swapped).
#[test]
fn reverse_retunes_the_receiver_to_the_input() {
    let mut t = ready();
    t.app.slots[0].cfg.offset = 60_000; // +600 kHz
    let rx = t.app.slot(0).freq();
    open(&mut t);
    t.cmds();
    down(&mut t, Key::Ptt(PTT_A));
    up(&mut t, Key::Ptt(PTT_A));
    let tunes = |c: &[RadioCmd]| c.iter().filter_map(|c| match c { RadioCmd::TuneSlot { slot: 0, freq } => Some(*freq), _ => None }).collect::<Vec<_>>();
    let c = t.cmds();
    assert_eq!(tunes(&c), [rx + 60_000], "reversed: listens on the input {c:?}");
    down(&mut t, Key::Ptt(PTT_A));
    up(&mut t, Key::Ptt(PTT_A));
    assert_eq!(tunes(&t.cmds()), [rx], "back on the output");
}

/// HOME-17, DISP-18: PTT B steps the theme in use, night red, daylight, then back to the theme in use; daylight
/// is skipped when it is the theme in use. The brightness row follows night red's own level (DISP-23).
#[test]
fn ptt_b_is_night_then_day() {
    let press = |t: &mut T| {
        down(t, Key::Ptt(PTT_B));
        up(t, Key::Ptt(PTT_B));
    };
    let mut t = ready();
    t.app.set.theme = 0; // dark
    t.app.set.brightness_night = 3;
    open(&mut t);
    press(&mut t);
    assert_eq!(t.app.set.theme, NIGHT_RED);
    assert_eq!((shown(&t, 1).as_str(), shown(&t, 4).as_str()), ("red", "3"));
    press(&mut t);
    assert_eq!(t.app.set.theme, crate::settings::DAYLIGHT);
    assert_eq!(shown(&t, 1).as_str(), "day");
    press(&mut t);
    assert_eq!(t.app.set.theme, 0, "back to dark");
    assert_eq!((shown(&t, 1).as_str(), shown(&t, 4).as_str()), ("off", "7"));
    assert!(!keyed(&t.cmds()));
    // Daylight in use: night red, then back to daylight (no second daylight step).
    t.app.set.theme = crate::settings::DAYLIGHT;
    press(&mut t);
    assert_eq!(t.app.set.theme, NIGHT_RED);
    press(&mut t);
    assert_eq!((t.app.set.theme, shown(&t, 1).as_str()), (crate::settings::DAYLIGHT, "off"));
    // Night red already on (set at 0 7 1 from dark): PTT B steps to daylight, then dark.
    t.app.set.theme = 0;
    crate::screens::groups::change(&mut t.app, &[7, 1], 2);
    assert_eq!(t.app.set.theme, NIGHT_RED);
    press(&mut t);
    assert_eq!(t.app.set.theme, crate::settings::DAYLIGHT);
    press(&mut t);
    assert_eq!(t.app.set.theme, 0);
}

/// HOME-17: PF1 steps GPS (0 6 1: Off, Auto, On in turn, GPS-1), PF2 Bluetooth on/off (0 9 2 1), each at the press;
/// the rows follow.
#[test]
fn pf1_gps_and_pf2_bluetooth() {
    use crate::settings::{GPS_AUTO, GPS_OFF, GPS_ON};
    let mut t = T::new();
    t.app.set.gps = GPS_OFF;
    let bt = t.app.set.bt;
    open(&mut t);
    for (want, text) in [(GPS_AUTO, "auto"), (GPS_ON, "on"), (GPS_OFF, "off")] {
        down(&mut t, Key::Side(1));
        assert_eq!((t.app.set.gps, shown(&t, 2).as_str()), (want, text));
        up(&mut t, Key::Side(1));
    }
    down(&mut t, Key::Side(2));
    assert_eq!(t.app.set.bt, !bt);
    assert_eq!(shown(&t, 3), if bt { "off" } else { "on" });
    up(&mut t, Key::Side(2));
    assert!(t.app.qmenu.is_some());
    assert_eq!(t.app.screen(), Screen::Home, "no scan, no counter, no monitor");
    assert_eq!(t.app.monitor, 0);
    // Held past the long press: still one toggle, and no frequency counter at the release (KEY-22).
    t.press(Key::Side(2), 1200);
    assert_eq!((t.app.set.bt, t.app.screen()), (bt, Screen::Home));
}

/// HOME-17, DISP-16.3, DISP-22, DISP-23: the knob steps the brightness for the theme in use, 1-10 with no wrap, live
/// (both lights), and doesn't tune; saved as any setting is.
#[test]
fn knob_is_brightness() {
    let mut t = T::new();
    t.app.set.backlight = 0;
    t.app.set.keypad_light = 1; // with the screen
    let f = t.freq();
    open(&mut t);
    t.knob(1);
    assert_eq!(t.app.set.brightness, 8);
    assert_eq!((t.app.backlight_duty(), t.app.keypad_duty()), (BRIGHTNESS[7], (BRIGHTNESS[7] as u32 * crate::lights::KEYPAD_MAX as u32 / 1000) as u16), "live, both lights (DISP-22.1)");
    assert_eq!(shown(&t, 4), "8");
    for _ in 0..5 {
        t.knob(1);
    }
    assert_eq!(t.app.set.brightness, 10, "10 at most");
    for _ in 0..12 {
        t.knob(-1);
    }
    assert_eq!(t.app.set.brightness, 1, "1 at least");
    assert_eq!(t.freq(), f, "the knob didn't tune");
    // ◀▶ aren't the knob here: they close the box (any other key).
    t.tap(Key::Right);
    assert!(t.app.qmenu.is_none());
    assert_eq!(t.app.set.brightness, 1);
    // Night red: its own level.
    t.app.set.theme = NIGHT_RED;
    open(&mut t);
    t.knob(1);
    assert_eq!((t.app.set.brightness_night, t.app.set.brightness), (4, 1));
    // Saved: the settings' bytes carry it.
    struct V(Vec<u8>);
    impl Sink for V {
        fn put(&mut self, b: &[u8]) {
            self.0.extend_from_slice(b);
        }
    }
    let mut a = V(Vec::new());
    t.app.settings().save(&mut a);
    t.knob(1);
    let mut b = V(Vec::new());
    t.app.settings().save(&mut b);
    assert_ne!(a.0, b.0);
}

/// HOME-17: BACK, \* again, or any other key closes the box, and that key does nothing else (no tune, no entry, no
/// slot change, no menu, no lock, no repeat while held).
#[test]
fn other_keys_close_it_and_do_nothing_else() {
    for k in [Key::Exit, Key::Star, Key::Digit(5), Key::Menu, Key::Slot, Key::VfoMem, Key::Up, Key::Down, Key::Left, Key::Hash] {
        let mut t = T::new();
        let (f, active, mode) = (t.freq(), t.app.active(), t.app.slot(0).vfo_mode);
        open(&mut t);
        t.cmds();
        t.press(k, 1500); // held: no long press, no repeats
        assert!(t.app.qmenu.is_none(), "{k:?} closes it");
        assert_eq!(t.app.screen(), Screen::Home, "{k:?}");
        assert_eq!((t.freq(), t.app.active(), t.app.slot(0).vfo_mode), (f, active, mode), "{k:?} did nothing else");
        assert!(!t.app.locked(), "{k:?}");
        let c = t.cmds();
        assert!(!c.iter().any(|c| matches!(c, RadioCmd::Mute(_) | RadioCmd::SlotParam { .. })), "{k:?}: {c:?}");
        // and the next press is the key's own again
        t.tap(Key::Star);
        assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::Mute(_))), "{k:?}: \\* is quick mute again");
    }
}

/// HOME-17: 5 s with no key or knob input closes it; each input it takes starts the 5 s again.
#[test]
fn closes_after_5_s_idle() {
    let mut t = T::new();
    open(&mut t);
    t.wait(IDLE_MS - 300);
    assert!(t.app.qmenu.is_some());
    t.knob(1);
    t.wait(IDLE_MS - 300);
    assert!(t.app.qmenu.is_some(), "the knob restarted the wait");
    t.tap(Key::Side(1));
    t.wait(IDLE_MS - 300);
    assert!(t.app.qmenu.is_some(), "so did PF1");
    t.wait(400);
    assert!(t.app.qmenu.is_none(), "5 s idle");
    assert_eq!(t.app.screen(), Screen::Home);
}

/// HOME-17, KEY-14.4: locked (past the lock popup's choice, KEY-13), hold \* does nothing but the lock toast; an
/// auto lock closes the box.
#[test]
fn locked_hold_star_does_nothing() {
    let mut t = T::new();
    t.hold(Key::Hash);
    assert!(t.app.locked());
    t.wait(3_100);
    t.hold(Key::Star);
    assert!(t.app.qmenu.is_none());
    assert_eq!(t.app.toast.map(|x| x.head.as_bytes().to_vec()), Some(b"Locked:".to_vec()));
    t.hold(Key::Hash);
    open(&mut t);
    t.app.lock = Some(crate::settings::Lock::Keypad); // KEY-20's auto lock, at a tick
    t.wait(100);
    assert!(t.app.qmenu.is_none(), "locked: closed");
}

/// HOME-17, PIN-5, APRS-19: channel-only mode refuses hold \* as before; on the APRS slot the box opens and PTT A
/// is refused with the slot's reason (the row shows "-").
#[test]
fn channel_only_and_the_aprs_slot() {
    let mut t = T::new();
    t.tap(Key::VfoMem);
    assert!(crate::screens::groups::change(&mut t.app, &[9, 3, 2], 1));
    t.hold(Key::Star);
    assert!(t.app.qmenu.is_none());
    assert_eq!(t.app.toast.map(|x| x.head.as_bytes().to_vec()), Some(b"Channel-only".to_vec()));
    let mut t = ready();
    t.app.set_aprs_mode(true);
    t.tap(Key::Slot);
    t.tap(Key::Slot);
    assert_eq!(t.app.active(), 2);
    open(&mut t);
    assert_eq!(shown(&t, 0), "-");
    t.cmds();
    down(&mut t, Key::Ptt(PTT_A));
    assert!(!t.app.slot(2).cfg().reverse);
    assert_eq!(t.app.screen(), Screen::Home, "no compose (APRS-18): the box took PTT A");
    assert!(!keyed(&t.cmds()));
}

/// HOME-17 (safety): with the box up, PTT A and PTT B never key, held as long as you like; the box stays up and
/// no screen changes. After it closes, PTT keys as usual.
#[test]
fn popup_ptts_never_key() {
    let mut t = ready();
    open(&mut t);
    t.cmds();
    for k in [PTT_A, PTT_B] {
        down(&mut t, Key::Ptt(k));
        t.wait(1500);
        assert!(t.app.radio.keyed.is_none() && t.app.screen() == Screen::Home, "{k}");
        up(&mut t, Key::Ptt(k));
    }
    assert!(!keyed(&t.cmds()));
    assert!(t.app.qmenu.is_some());
    t.tap(Key::Exit);
    assert!(t.app.qmenu.is_none());
    down(&mut t, Key::Ptt(PTT_A));
    assert!(t.cmds().contains(&RadioCmd::Transmit { slot: 0, on: true }), "after the box: PTT A keys");
    up(&mut t, Key::Ptt(PTT_A));
    down(&mut t, Key::Ptt(PTT_B));
    assert!(t.cmds().contains(&RadioCmd::Transmit { slot: 1, on: true }), "and PTT B");
    up(&mut t, Key::Ptt(PTT_B));
}

/// HOME-17 (safety): a PTT pressed while the box was up never keys while it is still held, however the box closed
/// (5 s idle, BACK, another PTT's feature, the screen going dark) and even if the input repeats its down; it has to
/// be let go and pressed again.
#[test]
fn ptt_held_through_the_close_never_keys() {
    for how in 0..4 {
        for k in [PTT_A, PTT_B] {
            let mut t = ready();
            t.app.set.backlight = if how == 2 { 1 } else { 0 }; // 5 s: dark at the idle close
            open(&mut t);
            t.cmds();
            down(&mut t, Key::Ptt(k));
            match how {
                0 => t.wait(IDLE_MS + 500),
                1 => t.tap(Key::Exit),
                2 => {
                    t.wait(6000);
                    assert!(t.app.dark(), "dark");
                }
                _ => {
                    t.wait(IDLE_MS + 500);
                    t.app.key_down(Key::Ptt(k), t.t, &store()); // a repeated down: the same press
                    t.wait(500);
                }
            }
            assert!(t.app.qmenu.is_none(), "closed ({how})");
            t.wait(2000);
            let c = t.cmds();
            assert!(!keyed(&c) && t.app.radio.keyed.is_none(), "PTT {k} held through close {how}: never keyed {c:?}");
            assert_ne!(t.app.screen().name(), "Transmit refused", "not even refused: it never asked");
            up(&mut t, Key::Ptt(k));
            t.wake();
            down(&mut t, Key::Ptt(k));
            assert!(t.app.radio.keyed.is_some(), "pressed again: keys ({how}, {k})");
            up(&mut t, Key::Ptt(k));
            assert!(t.app.radio.keyed.is_none());
        }
    }
}

/// HOME-17 (safety): PTT A pressed in the box and still held when PTT B (another of its keys) goes down: neither
/// keys; let go of both and each keys again.
#[test]
fn both_ptts_in_the_box() {
    let mut t = ready();
    open(&mut t);
    t.cmds();
    down(&mut t, Key::Ptt(PTT_A));
    down(&mut t, Key::Ptt(PTT_B));
    t.tap(Key::Exit);
    t.wait(1000);
    up(&mut t, Key::Ptt(PTT_B));
    assert!(!keyed(&t.cmds()));
    down(&mut t, Key::Ptt(PTT_B));
    assert!(t.app.radio.keyed == Some(1), "PTT B released and pressed again keys");
    up(&mut t, Key::Ptt(PTT_B));
    up(&mut t, Key::Ptt(PTT_A));
}

/// HOME-17, KEY-7.4, KEY-7.6: the side connector's PTT isn't one of the box's keys: it closes the box and transmits on
/// the active slot as usual. VOX doesn't: the box isn't home, so VOX is held while it is up (VOX-3.1, decision 145) and
/// keys only once it has closed.
#[test]
fn external_ptt_and_vox_close_it_and_key() {
    let mut t = ready();
    open(&mut t);
    t.cmds();
    down(&mut t, Key::PTT);
    assert!(t.app.qmenu.is_none(), "closed");
    assert!(t.cmds().contains(&RadioCmd::Transmit { slot: 0, on: true }), "keyed");
    assert!(matches!(t.app.screen(), Screen::Transmit(_)));
    up(&mut t, Key::PTT);
    assert!(t.app.radio.keyed.is_none());
    // VOX
    let mut t = ready();
    crate::screens::groups::change(&mut t.app, &[8, 2, 1], 1);
    open(&mut t);
    t.cmds();
    t.live.tx.voice = true;
    t.wait(200);
    assert!(t.app.qmenu.is_some(), "VOX leaves the box up");
    assert!(!t.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. })), "VOX held while it is up");
    t.tap(Key::Exit);
    t.wait(200);
    assert!(t.app.qmenu.is_none());
    assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { slot: 0, on: true })), "home: VOX keys");
    t.live.tx.voice = false;
    t.wait(200);
}

/// HOME-17 (safety): the last check: the one key-up path refuses while the box is up, whatever calls it.
#[test]
fn transmit_refuses_while_the_box_is_up() {
    let mut t = ready();
    open(&mut t);
    t.cmds();
    crate::screens::transmit::key(&mut t.app, 0);
    crate::screens::transmit::ptt(&mut t.app, 1);
    assert!(!keyed(&t.cmds()));
    assert!(t.app.radio.keyed.is_none());
    t.tap(Key::Exit);
    crate::screens::transmit::key(&mut t.app, 0);
    assert!(t.cmds().contains(&RadioCmd::Transmit { slot: 0, on: true }), "closed: keys");
}

/// HOME-17: the box closes when home goes (a screen opened by itself, here an alert's).
#[test]
fn closes_when_home_goes() {
    let mut t = T::new();
    open(&mut t);
    t.open(crate::screens::nav::Dest::Status);
    assert!(t.app.qmenu.is_none());
    t.hold(Key::Exit);
    assert!(t.app.qmenu.is_none(), "not back by itself");
}

/// HOME-17, DISP-25: the box is drawn over home whole, then only changed rows; nothing flickers in any pass; when
/// it goes, the screen is exactly what a full redraw of home draws, with no blank between.
#[test]
fn box_draws_without_flicker_and_leaves_home_as_it_was() {
    for layout in 0..3 {
        box_over_layout(layout);
    }
}

fn box_over_layout(layout: u8) {
    let mut t = T::new();
    t.app.set.layout = layout;
    let mut fb = FrameBuffer::new();
    t.render(&mut fb);
    let pass = |t: &mut T, fb: &mut FrameBuffer| {
        fb.begin_pass();
        t.render(fb);
        fb.end_pass(false)
    };
    t.hold(Key::Star);
    let r = pass(&mut t, &mut fb);
    assert_eq!(r.flicker, 0, "layout {layout}: opening");
    assert!(r.changed > 1000, "the box is drawn");
    let (x, y, w, h) = crate::screens::qmenu::BOX;
    assert_eq!(fb.at(x + w / 2, y), BLUE, "the frame");
    // Receiving on A while the box is up: nothing under it draws.
    t.live.slots[0] = SlotLive { freq: t.freq(), squelch_open: true, carrier: true, rssi: -85, ..Default::default() };
    t.wait(200);
    assert_eq!(pass(&mut t, &mut fb).changed, 0, "layout {layout}: home waits under the box");
    t.knob(1);
    let r = pass(&mut t, &mut fb);
    assert_eq!(r.flicker, 0);
    assert!(r.changed > 0 && r.changed < 400, "one row: {}", r.changed);
    t.tap(Key::Side(1));
    assert_eq!(pass(&mut t, &mut fb).flicker, 0);
    t.tap(Key::Exit);
    let r = pass(&mut t, &mut fb);
    assert_eq!(r.flicker, 0, "layout {layout}: closing");
    // What a fresh home draws now (receiving, with the new GPS state).
    let mut fresh = FrameBuffer::new();
    t.app.drawn.screen = None;
    t.app.drawn.sbar = None;
    t.render(&mut fresh);
    let diff = (0..fb.px.len()).filter(|&i| fb.px[i] != fresh.px[i]).count();
    assert_eq!(diff, 0, "layout {layout}: after the box, home as drawn from scratch ({x},{y} {w}x{h})");
}

/// HOME-17, HOME-8: a hold digit's popup still up when the box opens goes at once; the box is drawn over home and,
/// once closed, home is as drawn from scratch.
#[test]
fn a_shortcut_popup_goes_when_the_box_opens() {
    let mut t = T::new();
    let mut fb = FrameBuffer::new();
    t.render(&mut fb);
    t.hold(Key::Digit(2)); // TX power: a popup for 1.2 s
    t.render(&mut fb);
    assert!(t.app.toast.is_some_and(|x| x.center));
    t.hold(Key::Star);
    assert!(t.app.toast.is_none() && t.app.qmenu.is_some());
    t.render(&mut fb);
    let (x, y, w, _) = crate::screens::qmenu::BOX;
    assert_eq!(fb.at(x + w / 2, y), BLUE, "the box's frame");
    t.tap(Key::Exit);
    t.render(&mut fb);
    let mut fresh = FrameBuffer::new();
    t.app.drawn.screen = None;
    t.app.drawn.sbar = None;
    t.render(&mut fresh);
    assert_eq!((0..fb.px.len()).filter(|&i| fb.px[i] != fresh.px[i]).count(), 0);
}

/// HOME-17.1: the knob's row is first (the knob sits on top of the radio), then the side keys top to bottom.
#[test]
fn knob_row_first() {
    let t = T::new();
    let keys: std::vec::Vec<&str> = lines(&t.app).iter().map(|l| l.0).collect();
    assert_eq!(keys, ["Knob", "PTT A", "PTT B", "PF1", "PF2"]);
}

/// HOME-8, DISP-25: a shortcut's popup over home's cards goes with nothing flickering, only what it covered drawn
/// again, and home exactly as drawn from scratch after, in every layout (the other two still clear and redraw).
#[test]
fn a_shortcut_popup_goes_straight_to_home() {
    for layout in 0..3 {
        let mut t = T::new();
        t.app.set.layout = layout;
        let mut fb = FrameBuffer::new();
        t.render(&mut fb);
        t.hold(Key::Digit(2)); // TX power: a popup for 1.2 s
        t.render(&mut fb);
        assert!(t.app.toast.is_some_and(|x| x.center));
        // HOME-8: receiving on A while the popup is up: home waits under it and the popup isn't drawn again
        t.live.slots[0] = SlotLive { freq: t.freq(), squelch_open: true, carrier: true, rssi: -85, ..Default::default() };
        t.wait(200);
        fb.begin_pass();
        t.render(&mut fb);
        assert_eq!(fb.end_pass(false).changed, 0, "layout {layout}: home waits under the popup");
        t.wait(crate::POPUP_MS + 100);
        assert!(t.app.toast.is_none(), "layout {layout}: the popup timed out");
        fb.begin_pass();
        t.render(&mut fb);
        let r = fb.end_pass(false);
        if layout == 0 {
            assert_eq!(r.flicker, 0, "layout {layout}: closing");
        }
        let mut fresh = FrameBuffer::new();
        t.app.drawn.screen = None;
        t.app.drawn.sbar = None;
        t.render(&mut fresh);
        let diff = (0..fb.px.len()).filter(|&i| fb.px[i] != fresh.px[i]).count();
        assert_eq!(diff, 0, "layout {layout}: after the popup, home as drawn from scratch");
    }
}

/// VFO-7, ferrite#225: reverse retunes the receiver to the input in every home layout (MENU 0 7 2: Watch, Single +
/// scope with its strip running, Watch + activity), and the retune is the last word once home again.
#[test]
fn reverse_in_each_layout() {
    for layout in 0..3u8 {
        let mut t = ready();
        t.app.set.layout = layout;
        t.wait(300);
        t.app.slots[0].cfg.offset = 60_000;
        let rx = t.app.slot(0).freq();
        open(&mut t);
        t.cmds();
        down(&mut t, Key::Ptt(PTT_A));
        up(&mut t, Key::Ptt(PTT_A));
        t.tap(Key::Exit);
        t.tap(Key::Exit);
        t.wait(2000);
        let tunes: std::vec::Vec<_> = t.cmds().iter().filter_map(|c| match c { RadioCmd::TuneSlot { slot: 0, freq } => Some(*freq), _ => None }).collect();
        assert_eq!(tunes.last(), Some(&(rx + 60_000)), "layout {layout}: listens on the input");
    }
}
