//! The tone picker (chapter 16): each mode as drawn with its kind, typed and custom tones, and a channel's
//! second page (end tone and tail).

use super::*;
use crate::screens::tones::{self, Tones};

fn open(t: &mut T) {
    t.open(Dest::Tones);
    assert_eq!(t.name(), "Tone picker");
    t.reads("title=\"Tones\"");
    assert!(matches!(t.screen(), Screen::Tones(Tones { row: 1, .. })), "TX first");
}

fn tx(t: &T) -> u16 {
    t.app.slot(t.app.active).cfg().tx_tone
}

/// TONE-2.1, TONE-15: \u{25c0}\u{25b6} step the modes; each draws its kind (CTCSS, DCS normal, DCS inverted), a typed
/// tone off the list is custom, and the typed digits show while typing.
#[test]
fn modes_and_kinds() {
    let mut t = T::new();
    open(&mut t);
    t.tap(Key::Right); // CTCSS
    t.shows("CTCSS");
    for d in [1, 2, 3, 4] {
        t.tap(Key::Digit(d));
    }
    t.shows("Typed 1234");
    t.shows("CTCSS custom");
    assert_eq!(tx(&t), 1234);
    t.tap(Key::Right); // DCS-N
    t.shows("DCS normal");
    t.tap(Key::Right); // DCS-I
    t.shows("DCS inverted");
    t.wait(1600);
    t.tap(Key::Digit(8)); // not octal: nothing
    assert_eq!(tones::mode_of(tx(&t)), 3);
    t.wait(1600);
    for d in [7, 7, 6] {
        t.tap(Key::Digit(d));
    }
    t.shows("DCS custom");
    t.knob(1);
    t.tap(Key::Hash); // off
    assert_eq!(tx(&t), 0);
    t.shows("No tone");
}

/// TONE-15: ▲▼ or ◀▶ between digits starts a new number: the digits typed before don't carry on into it (four
/// typed, a row down and back, then one more overflowed: found by fuzzing).
#[test]
fn a_new_row_starts_a_new_number() {
    let mut t = T::new();
    open(&mut t);
    t.tap(Key::Right); // CTCSS
    for d in [9, 9, 9, 9] {
        t.tap(Key::Digit(d));
    }
    t.tap(Key::Down);
    t.tap(Key::Up);
    t.tap(Key::Digit(1));
    t.shows("Typed 1");
    t.tap(Key::Right);
    t.tap(Key::Digit(2));
    t.shows("Typed 2");
}

/// TONE-15: typed digits on DCS are octal; past three, they start again.
#[test]
fn typed_dcs_limits() {
    assert_eq!(tones::typed_tone(8, Some(0)), None, "8 isn't octal");
    assert_eq!(tones::typed_tone(1777, Some(0)), None, "four octal digits are too many");
    assert!(tones::typed_tone(23, Some(0)).is_some());
}

/// TONE-10, TONE-17: on a channel, \u{25bc} goes on to its end tone and tail; the knob and \u{25c0}\u{25b6} change them,
/// # makes one Global; BACK puts the channel's back.
#[test]
fn channel_page() {
    let mut t = T::ready();
    let a = t.app.active;
    t.app.set_channel(a, 0, 0, &t.s);
    t.wait(100);
    let before = t.app.slot(a).ch.unwrap().tones;
    open(&mut t);
    t.shows("further down, end tone and tail");
    t.tap(Key::Down); // end tone
    assert!(matches!(t.screen(), Screen::Tones(Tones { row: 2, .. })));
    t.shows("End tone");
    t.shows("Global: the radio's, MENU 0 8 4"); // End tone's own path; Tail removal's is 0 8 8 2
    t.shows("This channel's end tone and tail removal.");
    t.tap(Key::Right);
    t.shows("Off");
    t.knob(1);
    t.shows("Beep");
    t.reads("hints \"^v\" \"Row\"");
    t.tap(Key::Down); // tail
    t.tap(Key::Left);
    t.shows("On");
    t.tap(Key::Hash);
    t.tap(Key::Down); // wraps to RX
    assert!(matches!(t.screen(), Screen::Tones(Tones { row: 0, .. })));
    t.tap(Key::Up);
    t.tap(Key::Up);
    t.app.undo = None;
    t.draw();
    t.reads("hints \"^v\" \"Row\" \"Knob\" \"Change\" \"OK\" \"Keep\"");
    t.tap(Key::Exit);
    assert_eq!(t.app.slot(a).ch.unwrap().tones, before, "BACK puts the channel's back");
}
