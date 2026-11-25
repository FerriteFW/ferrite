//! APRS messages (chapter 25): the list's states and keys, stopping and sending again, and compose's less
//! common keys and refusals.

use super::*;
use crate::data::{Data, Draft, MsgState, Op};
use crate::screens::msgs::{self, Compose, Msgs};
use ferrite_ui::text::Buf;

fn sel(t: &T) -> u16 {
    match t.screen() {
        Screen::Msgs(m) => m.cur.sel,
        s => panic!("not the messages: {}", s.name()),
    }
}

/// Compose on a draft to `to` with `text`, the cursor on the text.
fn compose(t: &mut T, to: &[u8], text: &[u8]) {
    let mut d = Draft::default();
    d.to.set(to);
    d.text.set(text);
    t.s.request(Op::Draft(d));
    t.open(Dest::Compose);
    assert_eq!(t.name(), "Compose message");
}

fn draft(t: &T) -> String {
    String::from_utf8_lossy(t.s.draft().text.as_bytes()).into_owned()
}

/// APRS-13: the list shows each of ours with its state (new, acked, "try n/5", failed); its hints; BACK leaves.
#[test]
fn list_shows_states() {
    let mut t = T::new();
    t.open(Dest::AprsMessages);
    t.shows("new");
    t.shows("acked");
    t.shows("try 2/5");
    t.shows("failed");
    t.reads("title=\"Messages\"");
    t.reads("hints \"OK\" \"Open\" \"#\" \"New\" \"Hold MENU\" \"More\"");
    t.tap(Key::Digit(5)); // nothing
    assert_eq!(t.name(), "APRS messages");
    t.tap(Key::Exit);
    assert_eq!(t.screen(), Screen::Home);
}

/// INBOX-3.1: moving onto an unread message while one is open reads it.
#[test]
fn moving_in_an_open_message_reads_the_next() {
    let mut t = T::new();
    t.open(Dest::AprsMessages);
    t.tap(Key::Down);
    t.tap(Key::Menu); // open the second
    assert_eq!(t.name(), "APRS message");
    t.reads("title=\"Message\"");
    t.reads("hints \"OK\" \"Reply\"");
    assert_eq!(t.s.message(0).unwrap().state, MsgState::Unread);
    t.tap(Key::Up);
    assert_eq!(sel(&t), 0);
    assert_eq!(t.s.message(0).unwrap().state, MsgState::Read, "read as it opens");
    t.tap(Key::Exit);
    assert_eq!(t.name(), "APRS messages", "BACK: the list");
}

/// APRS-13.1, APRS-13.2: hold MENU on one being sent or one that ended opens its own options; stopping one
/// being sent cancels it (drawn "cancelled"), and one that ended goes again as the newest.
#[test]
fn stop_and_send_again() {
    let mut t = T::new();
    t.open(Dest::AprsMessages);
    t.taps(&[Key::Down; 4]); // VA3RTX, retrying (DEC-22: two pages are newer)
    t.hold(Key::Menu);
    assert!(matches!(t.screen(), Screen::Options(o) if o.of == crate::screens::opts::Of::MsgSending), "{}", t.name());
    t.tap(Key::Exit);
    // From the list: stop it.
    t.app.screen = Screen::Msgs(Msgs { cur: crate::screens::lists::Cursor { sel: 4, top: 0 }, open: false });
    msgs::stop_or_resend(&mut t.app, &t.s, true);
    assert_eq!(t.toast(), "Not sending / VA3RTX", "the sample's own rows can't change");
    compose(&mut t, b"VE3KPW-9", b"Test");
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "Message queued / VE3KPW-9");
    t.app.screen = Screen::Msgs(Msgs { cur: Default::default(), open: false });
    msgs::stop_or_resend(&mut t.app, &t.s, true);
    assert_eq!(t.toast(), "Sending stopped / VE3KPW-9");
    assert_eq!(t.s.message(0).unwrap().state, MsgState::Cancelled);
    t.draw();
    t.shows("cancelled");
    t.hold(Key::Menu);
    assert!(matches!(t.screen(), Screen::Options(o) if o.of == crate::screens::opts::Of::MsgEnded), "{}", t.name());
    t.tap(Key::Exit);
    // Send again: refused without a callsign, or while KISS holds APRS; then queued as the newest.
    t.app.screen = Screen::Msgs(Msgs { cur: Default::default(), open: false });
    t.s.call.set(None);
    msgs::stop_or_resend(&mut t.app, &t.s, false);
    assert_eq!(t.toast(), "Not sent / MENU 0 8 6 1");
    t.s.call.set(Some(Buf::from(b"VA7FER")));
    t.app.set.bt = true;
    t.app.set.kiss_port = 2;
    msgs::stop_or_resend(&mut t.app, &t.s, false);
    assert_eq!(t.toast(), "Not sent / KISS on: the app sends");
    t.app.set.kiss_port = 0;
    let n = t.s.messages();
    msgs::stop_or_resend(&mut t.app, &t.s, false);
    assert_eq!(t.toast(), "Message queued / VE3KPW-9");
    assert_eq!(t.s.messages(), n + 1);
    assert_eq!(t.s.message(0).unwrap().state, MsgState::Pending);
}

/// KEY-12: multi-tap compose: * cycles the case, # is a space, \u{25b2}\u{25bc} move between the fields, BACK on an
/// empty message goes to the recipient, OK on the recipient to the message; the 67th character is the last.
#[test]
fn multitap_keys() {
    let mut t = T::new();
    t.app.set.predictive = false;
    compose(&mut t, b"VE3KPW-9", b"");
    t.tap(Key::Digit(2)); // a
    t.tap(Key::Hash); // capitals
    t.tap(Key::Digit(3)); // D
    t.tap(Key::Star);
    t.tap(Key::Hash); // digits
    t.tap(Key::Digit(7));
    assert_eq!(draft(&t), "aD 7");
    t.reads("compose");
    t.reads("\"aD 7\"");
    t.tap(Key::Up); // to the recipient
    assert!(matches!(t.screen(), Screen::Compose(c) if c.field == 0));
    t.tap(Key::Menu); // OK: on to the message
    assert!(matches!(t.screen(), Screen::Compose(c) if c.field == 1));
    for _ in 0..4 {
        t.tap(Key::Exit);
    }
    assert_eq!(draft(&t), "");
    t.tap(Key::Exit); // an empty message: BACK goes to the recipient
    assert!(matches!(t.screen(), Screen::Compose(c) if c.field == 0));
    t.tap(Key::Down);
    // TEXT-5: full at 67.
    let mut d = t.s.draft();
    d.text.set(&[b'x'; 67]);
    t.s.request(Op::Draft(d));
    t.tap(Key::Star); // no room: nothing
    t.tap(Key::Digit(2));
    assert_eq!(t.toast(), "67 characters / the APRS limit");
    assert_eq!(draft(&t).len(), 67);
}

/// APRS-17.8: V/M in compose sends a beacon (asked of the APRS task); the draft stays.
#[test]
fn vm_in_compose_is_a_beacon() {
    let mut t = T::new();
    t.app.set_aprs_mode(true);
    compose(&mut t, b"VE3KPW-9", b"Hi");
    t.tap(Key::VfoMem);
    assert!(t.s.beacon_asked());
    assert_eq!(draft(&t), "Hi");
}

/// TEXT-2: in predictive entry * cycles the next word's case; \u{25b2}\u{25bc} go through the candidates; OK takes the
/// word and sends.
#[test]
fn predictive_case_pick_and_ok() {
    let mut t = T::new();
    compose(&mut t, b"VE3KPW-9", b"");
    for k in [4, 6, 6, 3] {
        t.tap(Key::Digit(k)); // home / good / gone...
    }
    t.tap(Key::Down);
    t.tap(Key::Up);
    t.reads("compose");
    let n = t.s.messages();
    t.tap(Key::Menu); // the word goes in, then sends
    assert_eq!(t.s.messages(), n + 1);
    let m = t.s.message(0).unwrap();
    assert!(m.text.as_bytes()[0].is_ascii_uppercase(), "a capital starts it: {:?}", core::str::from_utf8(m.text.as_bytes()));
}

/// TEXT-3: OK with keys that spell nothing doesn't send: "No word"; the same for PTT's send.
#[test]
fn predictive_no_word_does_not_send() {
    let mut t = T::new();
    compose(&mut t, b"VE3KPW-9", b"Hi ");
    for _ in 0..4 {
        t.tap(Key::Digit(9));
    }
    let n = t.s.messages();
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "No word / # to spell it");
    assert_eq!(t.s.messages(), n);
    msgs::send(&mut t.app, &t.s);
    assert_eq!(t.s.messages(), n, "send refuses the same");
    // TEXT-10: # with keys that spell nothing: they go, and multi-tap's abc starts
    t.tap(Key::Hash);
    assert!(!t.app.set.predictive);
    assert!(t.app.text.is_empty());
}

/// TEXT-3, TEXT-12: keys that spell nothing stop a held digit, 1's mark and \u{25c0} too ("No word"); PTT's send takes
/// a word being predicted as it stands.
#[test]
fn predictive_no_word_other_keys() {
    let mut t = T::new();
    compose(&mut t, b"VE3KPW-9", b"Hi ");
    for _ in 0..4 {
        t.tap(Key::Digit(9));
    }
    for k in [Key::Digit(1), Key::Left] {
        t.app.toast = None;
        t.tap(k);
        assert_eq!(t.toast(), "No word / # to spell it", "{k:?}");
    }
    t.app.toast = None;
    t.hold(Key::Digit(7));
    assert_eq!(t.toast(), "No word / # to spell it");
    assert_eq!(draft(&t), "Hi ");
    for _ in 0..4 {
        t.tap(Key::Exit);
    }
    for k in [4, 6, 6, 3] {
        t.tap(Key::Digit(k));
    }
    let n = t.s.messages();
    msgs::send(&mut t.app, &t.s);
    assert_eq!(t.s.messages(), n + 1, "sent with the word in it");
    assert!(t.s.message(0).unwrap().text.as_bytes().starts_with(b"Hi "));
}

/// TEXT-5: holding a digit, or 1's mark, past the limit beeps and says so; a word too long for the room too.
#[test]
fn predictive_limit_paths() {
    let mut t = T::new();
    compose(&mut t, b"VE3KPW-9", &[b'x'; 67]);
    t.hold(Key::Digit(7));
    assert_eq!(t.toast(), "67 characters / the APRS limit");
    t.app.toast = None;
    t.tap(Key::Digit(1));
    assert_eq!(t.toast(), "67 characters / the APRS limit");
    assert_eq!(draft(&t).len(), 67);
    // a word typed with one place left: its keys beep at the second
    compose(&mut t, b"VE3KPW-9", &[b'x'; 66]);
    t.tap(Key::Digit(4));
    t.app.toast = None;
    t.tap(Key::Digit(6));
    assert_eq!(t.toast(), "67 characters / the APRS limit");
}

/// SETUP-13.1, KISS-10, APRS-8: send refuses a callsign over 6, KISS holding APRS, and an empty message.
#[test]
fn send_refusals() {
    let mut t = T::new();
    compose(&mut t, b"VE3KPW-9", b"");
    t.tap(Key::Menu); // empty: the quick list's first goes in (TEXT-6)
    let mut d = t.s.draft();
    d.text.clear();
    t.s.request(Op::Draft(d));
    t.app.set.predictive = false;
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "Nothing to send / write a message");
    t.s.call.set(Some(Buf::from(b"VA7FERXY")));
    msgs::send(&mut t.app, &t.s);
    assert_eq!(t.toast(), "Callsign over 6 / no APRS");
    t.app.set.bt = true;
    t.app.set.kiss_port = 2;
    msgs::send(&mut t.app, &t.s);
    assert_eq!(t.toast(), "Not sent / KISS on: the app sends");
}

/// APRS-39, TEXT-10: a message to a service stays in multi-tap: # goes round abc, ABC, 123 without T9, and the
/// choice of T9 is kept for the next message.
#[test]
fn service_recipient_stays_multitap() {
    let mut t = T::new();
    compose(&mut t, b"WXBOT", b"");
    for _ in 0..4 {
        t.tap(Key::Hash);
        assert!(t.app.set.predictive, "the choice is kept");
    }
    // abc, ABC, 123, abc, ABC (the service's format shows where the mode would)
    assert!(matches!(t.app.screen, Screen::Compose(c) if c.case == 1), "{:?}", t.app.screen);
}

/// TEXT-10 (Daria, an alpha tester): # goes round T9, abc, ABC, 123 and back to T9 on a message, the hint bar
/// naming the next; hold * no longer switches.
#[test]
fn hash_cycles_t9_and_multitap() {
    let mut t = T::new();
    compose(&mut t, b"VE3KPW-9", b"");
    t.shows("Typing: T9");
    for want in ["abc", "ABC", "123", "T9"] {
        t.tap(Key::Hash);
        t.shows(&std::format!("Typing: {want}"));
        assert_eq!(t.app.set.predictive, want == "T9");
    }
    t.hold(Key::Star);
    assert!(t.app.set.predictive, "hold * changes nothing");
    // a word in progress goes in before multi-tap starts
    for k in [4, 6, 6, 3] {
        t.tap(Key::Digit(k));
    }
    t.tap(Key::Hash);
    assert!(!t.app.set.predictive && t.app.text.is_empty());
    let d = t.s.draft();
    assert_eq!(d.text.len(), 4, "the word kept: {:?}", core::str::from_utf8(d.text.as_bytes()));
}

/// APRS-39: GWALL, the graffiti wall (suggested by Daria, an alpha tester): a service, its format shown.
#[test]
fn gwall_is_a_service() {
    let k = crate::screens::contacts::service(b"GWALL").expect("GWALL is a service");
    assert_eq!(crate::screens::contacts::SERVICES[k].1, "Graffiti wall");
    for (call, _, f) in crate::screens::contacts::SERVICES {
        let line = std::format!("Format: {f}");
        let w = ferrite_ui::gfx::text4_width(line.as_bytes(), &ferrite_ui::assets::UI_12);
        assert!(w <= 224, "{call}'s format line is {w} px");
    }
    let mut t = T::new();
    compose(&mut t, b"GWALL", b"");
    t.draw();
    t.shows("Format: message (? for help)");
}

/// APRS-38: on an empty recipient the knob moves through the address book.
#[test]
fn knob_moves_the_address_book() {
    let mut t = T::new();
    compose(&mut t, b"", b"");
    let q = |t: &T| match t.screen() {
        Screen::Compose(c) => c.quick,
        _ => panic!(),
    };
    let q0 = q(&t);
    t.knob(1);
    assert_eq!(q(&t), q0 + 1);
    t.knob(-2);
    assert_ne!(q(&t), q0 + 1, "wraps backwards");
}

/// APRS-50: the comment's editor holds 40 characters and says so at the limit.
#[test]
fn comment_limit() {
    let mut t = T::new();
    t.app.set.predictive = false;
    t.open(Dest::AprsMessages); // the APRS settings load from their owner at the first tick
    t.app.set_aprs_comment(&[b'c'; 40]);
    t.app.screen = Screen::Compose(Compose::comment_edit());
    t.draw();
    assert_eq!(t.name(), "Comment");
    t.reads("title=\"Comment\"");
    t.wait(1100);
    t.tap(Key::Digit(2));
    assert_eq!(t.toast(), "40 characters / the comment limit");
}

/// APRS-46.1, MENU-23: hold MENU on a message, then Delete message and OK again: that one goes (an unread one
/// leaves the mail badge too), the list closes up under the cursor; from an open message, the list again.
#[test]
fn delete_one() {
    let mut t = T::new();
    t.open(Dest::AprsMessages);
    let n = t.s.messages();
    t.app.inbox = 1;
    t.hold(Key::Menu); // the first row: VE3KPW-9, unread
    assert!(matches!(t.screen(), Screen::Options(o) if o.of == crate::screens::opts::Of::Msg), "{}", t.name());
    t.shows("Delete message");
    t.tap(Key::Menu);
    assert_eq!(t.s.messages(), n, "asks first");
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "Message deleted / VE3KPW-9");
    assert_eq!(t.s.messages(), n - 1);
    assert_eq!(t.app.inbox, 0, "the badge's unread one");
    assert!(matches!(t.screen(), Screen::Msgs(m) if m.cur.sel == 0 && !m.open));
    t.draw();
    t.hides("On my way");
    // The last row, open: deleted, and the cursor on the new last one (DEC-22: the sample's three pages among them,
    // the oldest page now last).
    t.app.screen = Screen::Msgs(Msgs { cur: crate::screens::lists::Cursor { sel: n + 1, top: 0 }, open: true });
    t.hold(Key::Menu); // a failed one of ours: Send again, then Delete message
    t.taps(&[Key::Down, Key::Menu, Key::Menu]);
    assert_eq!(t.toast(), "Message deleted / VE3JLH");
    assert!(matches!(t.screen(), Screen::Msgs(m) if m.cur.sel == n && !m.open), "{}", t.name());
}

/// APRS-6.4: a message to everyone shows above the stored messages (its addressee as the tag, unread dot); OK opens
/// it ("To CQ") and reads it without touching the envelope's count; OK again replies to its sender. The stored
/// messages below keep their own actions: hold MENU and delete takes the stored one under the cursor.
#[test]
fn notices_above_the_inbox() {
    let mut t = T::new();
    t.s.add_notice(crate::data::Notice { from: Buf::from(b"VE7GPL-7"), to: Buf::from(b"CQ"), text: Buf::from(b"anyone on?"), ago_s: 60, unread: true });
    t.app.inbox = 1;
    t.open(Dest::AprsMessages);
    t.shows("anyone on?");
    t.shows("VE7GPL-7");
    assert_eq!(sel(&t), 0);
    // Hold MENU on it: the messages' general options (nothing to stop, resend or delete)
    t.hold(Key::Menu);
    assert!(matches!(t.screen(), Screen::Options(o) if o.of == crate::screens::opts::Of::Messages), "{}", t.name());
    t.tap(Key::Exit);
    // OK opens it, read; the envelope keeps its count (a notice isn't in it)
    t.tap(Key::Menu);
    assert!(matches!(t.screen(), Screen::Msgs(m) if m.open), "{}", t.name());
    t.draw();
    t.shows("To CQ");
    t.shows("anyone on?");
    assert!(!t.s.notice(0).unwrap().unread);
    assert_eq!(t.app.inbox, 1);
    // OK again: a reply to its sender
    t.tap(Key::Menu);
    assert_eq!(t.name(), "Compose message");
    assert_eq!(t.s.draft().to.as_bytes(), b"VE7GPL-7");
    // the first stored message is the next row: deleting there deletes it, not the notice
    t.open(Dest::AprsMessages);
    let n = t.s.messages();
    t.tap(Key::Down);
    assert_eq!(sel(&t), 1);
    t.hold(Key::Menu);
    assert!(matches!(t.screen(), Screen::Options(o) if o.of == crate::screens::opts::Of::Msg), "{}", t.name());
    t.taps(&[Key::Menu, Key::Menu]); // Delete message, asked first
    assert_eq!(t.toast(), "Message deleted / VE3KPW-9");
    assert_eq!(t.s.messages(), n - 1);
    assert_eq!(t.s.notices(), 1, "the notice stays");
}
