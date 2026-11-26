//! BOOT-19 (decision 469): power on opens the tool that was open at power off, after the PIN; never after the
//! reset or a failed check; crossband at its ready screen.

use super::*;
use crate::persist::{Persist, SliceSource, Sink};
use crate::screens::activity::Mode;
use crate::screens::nav::Dest;
use ferrite_core::txplan::Region;

struct V(Vec<u8>);
impl Sink for V {
    fn put(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
}

/// The radio turned off and the settings saved, as the power-off flush does; then a fresh App on what was stored,
/// before anything opens (the firmware's `App::new` and the store's settings at power on).
fn cycle(t: &mut T) -> T {
    t.app.power_off();
    let mut v = V(Vec::new());
    t.app.settings().save(&mut v);
    let set = crate::tests::set();
    set.load(&mut SliceSource(&v.0));
    let mut app = App::new(set, crate::tests::alert());
    while app.pop_cmd().is_some() {}
    T { app, t: 1000, live: LiveAll::default() }
}

/// BOOT-19: the band scope, HF mode and ACARS open again at power on, each starting its radio mode; HF mode sends
/// its start (the SSB patch loads as on any opening).
#[test]
fn power_on_resumes_the_tool() {
    for (d, mode) in [(Dest::BandScope, Mode::Scope), (Dest::HfMode, Mode::Hf), (Dest::Acars, Mode::Acars)] {
        let mut t = T::new();
        t.open(d);
        assert_eq!(t.app.radio.mode, mode);
        let want = core::mem::discriminant(&t.app.screen());
        let mut t = cycle(&mut t);
        assert_ne!(t.app.set.resume_tool, 0, "{d:?}: stored");
        t.app.power_on(true);
        assert_eq!(core::mem::discriminant(&t.app.screen()), want, "{d:?}: opened again");
        assert_eq!(t.app.radio.mode, mode, "{d:?}: its mode runs");
        if d == Dest::HfMode {
            assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::Hf { on: true, .. })), "HF mode starts");
        }
        t.tap(Key::Exit);
        assert_eq!(t.app.screen(), Screen::Home, "BACK from it is home");
    }
}

/// BOOT-19: every tool in MENU 0 3's list is found again from its screen.
#[test]
fn every_tool_maps_to_itself() {
    for (i, &d) in TOOLS.iter().enumerate() {
        let mut t = T::new();
        t.app.set_region(Some(Region::Us), false);
        t.open(d);
        let t = cycle(&mut t);
        assert_eq!(t.app.set.resume_tool as usize, i + 1, "{d:?}");
    }
}

/// BOOT-19.1: crossband comes back at its ready screen, not running: nothing transmits on its own at power on.
#[test]
fn crossband_resumes_ready_not_running() {
    let mut t = T::new();
    t.app.radio.tx_built = true;
    t.app.set_region(Some(Region::Us), false);
    t.open(Dest::Crossband);
    t.tap(Key::Hash);
    assert_eq!(t.app.radio.mode, Mode::Xb, "running before power off");
    let mut t = cycle(&mut t);
    t.app.radio.tx_built = true;
    t.app.power_on(true);
    assert!(matches!(t.app.screen(), Screen::Crossband(_)));
    assert_ne!(t.app.radio.mode, Mode::Xb, "ready, not running");
    assert!(!t.cmds().iter().any(|c| matches!(c, RadioCmd::Crossband(Some(_)) | RadioCmd::Transmit { on: true, .. })));
}

/// BOOT-19, BOOT-17: with a PIN at power on the tool opens only once the PIN is right; a wrong one keeps the PIN.
#[test]
fn resume_comes_after_the_pin() {
    let mut t = T::new();
    (t.app.set.pin, t.app.set.pin_len, t.app.set.pin_mode) = (1234, 4, 1);
    t.open(Dest::BandScope);
    let mut t = cycle(&mut t);
    t.app.power_on(true);
    assert!(matches!(t.app.screen(), Screen::Pin(_)), "the PIN first");
    assert_ne!(t.app.radio.mode, Mode::Scope, "no sweep behind the PIN");
    for d in [9, 9, 9, 9] {
        t.tap(Key::Digit(d));
    }
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Pin(_)), "a wrong PIN opens nothing");
    for d in [1, 2, 3, 4] {
        t.tap(Key::Digit(d));
    }
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Scope(_)), "then the scope: {:?}", t.app.screen());
    assert_eq!(t.app.radio.mode, Mode::Scope);
}

/// BOOT-19.3: the power-on reset (BOOT-14) and a failed self-test open no tool, even after BACK and the PIN.
#[test]
fn no_resume_after_reset_or_failed_check() {
    let mut t = T::new();
    t.open(Dest::Acars);
    let mut t = cycle(&mut t);
    t.app.power_on_reset();
    t.tap(Key::Exit); // BACK on the reset's first question: on, as at power on (no PIN: home)
    assert_eq!(t.app.screen(), Screen::Home, "the reset: home");
    // With a PIN: BACK, then the PIN, still home.
    (t.app.set.pin, t.app.set.pin_len, t.app.set.pin_mode) = (1234, 4, 1);
    t.app.power_on_reset();
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Pin(_)));
    for d in [1, 2, 3, 4] {
        t.tap(Key::Digit(d));
    }
    t.tap(Key::Menu);
    assert_eq!(t.app.screen(), Screen::Home, "the reset, then the PIN: home");
    // A failed check: home.
    t.app.set.pin_mode = 0;
    t.app.power_on(false);
    assert_eq!(t.app.screen(), Screen::Home, "a failed check: home");
    assert_ne!(t.app.radio.mode, Mode::Acars);
}

/// BOOT-19.2: a tool closed back to home before power off isn't reopened; a menu over a tool isn't a tool.
/// BOOT-19.5: it is set only as the radio turns off; an older record (without the byte) reads home.
#[test]
fn closed_tool_clears_and_old_record_reads_home() {
    let mut t = T::new();
    t.open(Dest::BandScope);
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home);
    let mut t = cycle(&mut t);
    assert_eq!(t.app.set.resume_tool, 0, "closed: cleared");
    t.app.power_on(true);
    assert_eq!(t.app.screen(), Screen::Home);
    // Scope stored, then the next power off from a menu: cleared too.
    t.open(Dest::BandScope);
    let mut t = cycle(&mut t);
    assert_eq!(t.app.set.resume_tool, 1);
    t.open(Dest::Group(3));
    let t = cycle(&mut t);
    assert_eq!(t.app.set.resume_tool, 0, "a menu isn't a tool");
    // An older firmware's record is one byte shorter: it loads as home.
    let mut s = crate::settings::Settings::DEFAULT;
    s.resume_tool = 1;
    let mut v = V(Vec::new());
    s.save(&mut v);
    let mut old = crate::settings::Settings::DEFAULT;
    old.load(&mut SliceSource(&v.0[..v.0.len() - 1]));
    assert_eq!(old.resume_tool, 0);
}

/// BOOT-19.4: a tool that refuses at power on (Low Power Mode has no band scope, PWR-24.4) shows its own message
/// and home stays.
#[test]
fn refused_tool_stays_home() {
    let mut t = T::new();
    t.open(Dest::BandScope);
    let mut t = cycle(&mut t);
    t.app.lpm = crate::lpm::LPM_ON;
    t.app.power_on(true);
    assert_eq!(t.app.screen(), Screen::Home);
    assert_ne!(t.app.radio.mode, Mode::Scope);
    assert_eq!(t.app.toast.map(|x| std::string::String::from_utf8_lossy(x.head.as_bytes()).into_owned()).as_deref(), Some("Low Power Mode"));
}
