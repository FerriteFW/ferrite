//! Journeys for decision 96: key beeps and alert tones reaching the radio, the build on About. The harness is
//! `common`; a rule counts as covered when its ID is in a test's doc comment.

mod common;

use common::Radio;
use ferrite_core::keys::Key;
use ferrite_sim::simcore::Event;

/// Each key press beeps on the radio (SND-3, SND-11: the fake radio counts the beeps the Radio task would play).
#[test]
fn key_presses_beep() {
    let mut r = Radio::new();
    let before = r.sim.radio().beeps;
    r.press(Key::Up).press(Key::Down).wait(200);
    assert!(r.sim.radio().beeps >= before + 2, "a beep per key");
}

/// A SAME alert sounds on the radio each second until OK (SND-6, INBOX-4).
#[test]
fn alert_sounds_until_ok() {
    let mut r = Radio::new();
    r.event(Event::Alert).wait(500).expect_screen("Alert");
    let at = r.sim.radio().beeps;
    r.wait(3000);
    assert!(r.sim.radio().beeps >= at + 2, "the alert tone repeats");
    assert_eq!(r.sim.radio().last_beep.0, 1050);
    r.press(Key::Menu).wait(200);
    let at = r.sim.radio().beeps;
    r.wait(3000);
    assert_eq!(r.sim.radio().beeps, at, "OK silenced it");
}

/// About shows the version and the build (SELF-5; the tests' build reads 0000000); SELF-5.1: the row is labelled
/// "Unofficial build" on anything but an official release's image (the tests are never one unless FERRITE_OFFICIAL=1).
#[test]
fn about_shows_version_and_build() {
    let mut r = Radio::new();
    r.run("menu 0 9 8; wait 300ms; expect screen \"About\"");
    r.expect_text(ferrite_core::VERSION).expect_text("0000000");
    r.expect_text(if ferrite_core::OFFICIAL { "Version" } else { "Unofficial build" });
}
