//! Auto reply's page (APRS-57, MENU 0 5 8 9): on and off, the pattern typed with * as the wildcard, the delay
//! round its four steps, the reply typed in the message editor; and the menu row's value and knob.

use super::*;

fn open() -> T {
    let mut t = T::ready();
    t.open(Dest::AutoReply);
    assert_eq!(t.name(), "Auto reply");
    t
}

/// APRS-57: the four rows, ▲▼ between them: OK flips on and off and steps the delay (the knob and ◀▶ too, both
/// ways), OK on Reply to and Message types them; the settings are kept.
#[test]
fn rows_change_and_type() {
    let mut t = open();
    t.shows("Everyone (*)");
    t.shows("10 s");
    assert!(!t.app.aprs_cfg().reply_on());
    t.tap(Key::Menu);
    assert!(t.app.aprs_cfg().reply_on(), "OK on the first row turns it on");
    t.shows("On, but no message yet"); // on with nothing to send says so
    // the delay: 30 s, then back round to 2 min with ◀ from 10 s
    t.tap(Key::Down);
    t.tap(Key::Down);
    t.tap(Key::Right);
    assert_eq!(t.app.aprs_cfg().reply_delay_s(), 30);
    t.tap(Key::Left);
    t.tap(Key::Left);
    assert_eq!(t.app.aprs_cfg().reply_delay_s(), 120, "round the four");
    // Reply to: the text editor; * types the wildcard (APRS-57.1)
    t.knob(1);
    assert_eq!(t.app.aprs_cfg().reply_delay_s(), 10, "the knob steps it too");
    t.tap(Key::Up);
    t.tap(Key::Menu);
    assert_eq!(t.name(), "Text entry");
    t.shows("Reply to");
    t.tap(Key::Exit); // the * already there, deleted
    t.tap(Key::Hash); // digits
    t.tap(Key::Digit(7));
    t.tap(Key::Star);
    t.tap(Key::Menu);
    assert_eq!(t.name(), "Auto reply");
    assert_eq!(t.app.aprs_cfg().reply_to_text(), b"7*");
    // Message: the message editor, 45 characters; OK keeps it
    t.tap(Key::Down);
    t.tap(Key::Down);
    t.tap(Key::Menu);
    assert_eq!(t.name(), "Auto reply text");
    t.app.set.predictive = false;
    t.tap(Key::Digit(2));
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "Auto reply / kept");
    assert!(!t.app.aprs_cfg().reply_text().is_empty());
    t.shows("One reply per station");
}

/// APRS-57: the menu row shows on or off, the knob turns it on and off, and the reset (#) turns it off keeping
/// the rest.
#[test]
fn menu_row() {
    let mut t = T::ready();
    let mut c = t.app.aprs_cfg();
    c.reply_text[..4].copy_from_slice(b"Away");
    t.app.set_aprs(c);
    assert!(!crate::screens::groups::change(&mut t.app, &[5, 8, 9], 1) || t.app.aprs_cfg().reply_on());
    assert!(t.app.aprs_cfg().reply_on(), "the knob turned it on");
    crate::screens::groups::change(&mut t.app, &[5, 8, 9], 1);
    assert!(!t.app.aprs_cfg().reply_on(), "and off");
    assert_eq!(t.app.aprs_cfg().reply_text(), b"Away", "the text kept");
}
