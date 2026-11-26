//! The channel editor (EDIT-1 to EDIT-7): moving about, OK on each kind of cell, #, the notes, renumbering the
//! home channel, a refused change, and leaving the channel.

use super::*;
use crate::screens::chedit::ChEdit;
use ferrite_core::freq::mhz;

fn cell(t: &T) -> u8 {
    match t.screen() {
        Screen::ChEdit(c) => c.cell,
        s => panic!("not the channel editor: {}", s.name()),
    }
}

/// The editor on A's first channel.
fn editor() -> T {
    let mut t = T::ready();
    let a = t.app.active;
    t.app.set_channel(a, 0, 0, &t.s);
    t.wait(100);
    t.hold(Key::Slot);
    assert_eq!(t.name(), "Channel editor");
    assert_eq!(t.screen().mockup(), Some("HomeEditCh.png"));
    assert_eq!(cell(&t), 0);
    t
}

/// EDIT-2: \u{25b2} on the zone stays; the name and frequency say what OK does, with their own hints; OK on the name
/// types it, on the frequency types that.
#[test]
fn top_cells() {
    let mut t = editor();
    t.shows("Knob: move it to another zone");
    t.tap(Key::Up);
    assert_eq!(cell(&t), 0);
    t.tap(Key::Down);
    t.shows("OK: type the name, 16 at most");
    t.reads("hints \"<^v>\" \"Pick\" \"OK\" \"Type\" \"BACK\" \"Done\"");
    t.tap(Key::Down);
    t.shows("Knob: step \u{b7} OK: type it");
    t.tap(Key::Up);
    assert_eq!(cell(&t), 1);
    t.tap(Key::Menu);
    assert!(matches!(t.screen(), Screen::Text(_)), "{}", t.name());
    t.tap(Key::Exit);
    t.app.screen = Screen::ChEdit(ChEdit { cell: 2 });
    t.tap(Key::Menu);
    assert!(matches!(t.screen(), Screen::Entry(_)), "{}", t.name());
}

/// MENU-17, MENU-18, EDIT-7: OK on a setting opens its editor, on Number nothing; # resets a setting and a flag;
/// a receive-only channel greys its power, saying why in amber; Number and Home say what they do.
#[test]
fn grid_cells() {
    let mut t = editor();
    t.app.screen = Screen::ChEdit(ChEdit { cell: 3 }); // Mode
    t.tap(Key::Menu);
    assert!(matches!(t.screen(), Screen::Value(_)), "{}", t.name());
    t.tap(Key::Exit);
    t.app.screen = Screen::ChEdit(ChEdit { cell: 13 }); // Number
    t.draw();
    t.shows("Knob: to the next free number");
    t.tap(Key::Menu);
    assert_eq!(cell(&t), 13, "nothing to open");
    t.tap(Key::Hash); // nothing to reset
    t.tap(Key::Right);
    t.shows("Hold 0 on home tunes it");
    // Scan: a step, then # back to the default
    t.app.screen = Screen::ChEdit(ChEdit { cell: 10 });
    t.knob(1);
    t.app.undo = None;
    t.tap(Key::Hash);
    let a = t.app.active;
    assert_eq!(t.app.slot(a).ch.unwrap().scan(), 0);
    // Squelch: a step, # back
    t.app.screen = Screen::ChEdit(ChEdit { cell: 8 });
    t.knob(1);
    t.reads("hints \"Knob\" \"Change\" \"*\" \"Undo\" \"BACK\" \"Done\"");
    t.tap(Key::Hash);
    // receive only: power greyed, why in amber
    t.app.screen = Screen::ChEdit(ChEdit { cell: 12 });
    t.knob(1);
    t.app.screen = Screen::ChEdit(ChEdit { cell: 7 }); // power
    t.draw();
    assert!(t.px(240, 262, ferrite_ui::gfx::AMBER) > 0, "the note in amber");
    t.shows("Power: this channel is RX only");
}

/// CHAN-18, EDIT-5: renumbering the home channel carries home with it.
#[test]
fn renumber_home() {
    let mut t = editor();
    t.app.screen = Screen::ChEdit(ChEdit { cell: 14 }); // Home
    t.knob(1);
    let a = t.app.active;
    let num = t.app.slot(a).ch.unwrap().num;
    assert_eq!(t.app.set.home_ch, Some(num));
    t.app.screen = Screen::ChEdit(ChEdit { cell: 13 });
    t.knob(1);
    let new = t.app.slot(a).ch.unwrap().num;
    assert_ne!(new, num);
    assert_eq!(t.app.set.home_ch, Some(new), "home moved with it");
}

/// TX-37: a change the channel's rules refuse says why in a toast.
#[test]
fn refused_change() {
    let mut t = editor();
    let a = t.app.active as usize;
    let mut c = t.app.slots[a].ch.unwrap();
    (c.rx, c.cfg.offset) = (mhz(64) + 1_000, -(mhz(1) as i32)); // TX across 64 MHz: the relay
    t.app.slots[a].ch = Some(c);
    t.app.screen = Screen::ChEdit(ChEdit { cell: 2 });
    t.knob(1);
    assert!(!t.toast().is_empty(), "refused");
    assert_eq!(t.app.slots[a].ch.unwrap().rx, mhz(64) + 1_000, "unchanged");
}

/// EDIT-1: a key with the slot no longer on a channel goes home.
#[test]
fn off_the_channel_goes_home() {
    let mut t = editor();
    let a = t.app.active as usize;
    t.app.slots[a].vfo_mode = true;
    t.tap(Key::Down);
    assert_eq!(t.screen(), Screen::Home);
}

/// CHAN-26.1, CHAN-26.2, VFO-14 (ferrite#84): on a CB channel (18-64 MHz) the editor's Mode cell steps FM, NFM, AM
/// and round, over the modes the frequency doesn't have, never stuck on a refusal; NFM is the narrow filter.
#[test]
fn cb_channel_mode_cycles() {
    let mut t = editor();
    let a = t.app.active as usize;
    let mut c = t.app.slots[a].ch.unwrap();
    (c.rx, c.cfg.offset, c.cfg.split, c.cfg.mode, c.cfg.narrow) = (mhz(27) + 18_500, 0, false, 0, false);
    t.app.slots[a].ch = Some(c);
    t.app.screen = Screen::ChEdit(ChEdit { cell: 3 }); // the grid's first cell: Mode
    let mut seen = std::vec::Vec::new();
    for _ in 0..3 {
        t.knob(1);
        seen.push(t.app.slots[a].ch.unwrap().cfg.mode);
    }
    assert_eq!(seen, [1, 2, 0], "FM, NFM, AM round");
    t.knob(-1);
    assert_eq!(t.app.slots[a].ch.unwrap().cfg.mode, 2, "back over WFM and NAM to AM");
    t.knob(-1);
    let g = t.app.slots[a].ch.unwrap().cfg;
    assert_eq!((g.mode, g.narrow), (1, true), "NFM, narrow");
}
