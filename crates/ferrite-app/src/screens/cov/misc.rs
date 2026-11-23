//! Smaller screens' less-travelled paths: the contacts' options, the link status and write progress, HF mode's
//! recent list, and the advanced menu's recent settings and state words.

use super::*;
use crate::data::Data;
use crate::screens::opts::Of;

fn contact_options(t: &mut T, sel: u16) {
    t.open(Dest::Contacts);
    t.app.screen = Screen::Contacts(crate::screens::contacts::Contacts { cur: crate::screens::lists::Cursor { sel, top: 0 } });
    t.draw();
    t.app.open_screen(Of::Contacts.screen());
    t.draw();
}

/// APRS-40: a contact's Station opens its page when it is heard, else says it isn't; Message writes to it.
#[test]
fn contact_options_run() {
    let mut t = T::new();
    let heard = (0..t.s.contacts()).find(|&i| {
        let c = t.s.contact(i).unwrap();
        (0..t.s.stations()).any(|k| t.s.station(k).unwrap().call.as_bytes() == c.call.as_bytes())
    });
    let unheard = (0..t.s.contacts()).find(|&i| {
        let c = t.s.contact(i).unwrap();
        !(0..t.s.stations()).any(|k| t.s.station(k).unwrap().call.as_bytes() == c.call.as_bytes())
    });
    if let Some(i) = heard {
        contact_options(&mut t, i);
        t.tap(Key::Digit(3));
        t.tap(Key::Menu);
        assert!(matches!(t.screen(), Screen::Aprs(a) if a.station), "{}", t.name());
    }
    if let Some(i) = unheard {
        contact_options(&mut t, i);
        t.tap(Key::Digit(3));
        t.tap(Key::Menu);
        assert!(t.toast().ends_with("/ not in the heard list"), "{}", t.toast());
    }
    assert!(heard.is_some() || unheard.is_some());
    // Message: compose to it
    contact_options(&mut t, 0);
    t.app.screen = Screen::Contacts(crate::screens::contacts::Contacts { cur: Default::default() });
    crate::screens::contacts::sel(&mut t.app, crate::screens::contacts::Sel::Message, &t.s);
    assert_eq!(t.name(), "Compose message");
    assert_eq!(t.s.draft().to.as_bytes(), t.s.contact(0).unwrap().call.as_bytes());
}

/// GPS-36: Use as fixed position with no waypoint under it says none is selected.
#[test]
fn fixed_from_no_waypoint() {
    let mut t = T::new();
    t.app.screen = Screen::Gps(crate::screens::gps::Gps::new(crate::screens::gps::Page::Waypoints));
    let st = crate::chan::Table { zones: &[], channels: &[] };
    crate::screens::contacts::sel(&mut t.app, crate::screens::contacts::Sel::Fixed, &st);
    assert_eq!(t.toast(), "Waypoint / none selected");
}

/// LINK-7, LINK-9: the link status with Bluetooth and the cable connected; the write progress; PTT transmits
/// from the status page.
#[test]
fn link_pages() {
    let mut t = T::ready();
    t.dev.bt = true;
    t.dev.link.cable = true;
    t.dev.link.write_pct = Some(40);
    t.open(Dest::LinkStatus);
    assert_eq!(t.name(), "Link status");
    t.shows("Connected");
    t.shows("Writing, 40 %");
    t.ptt(true);
    assert!(matches!(t.screen(), Screen::Transmit(_)), "{}", t.name());
    t.ptt(false);
    t.open(Dest::WriteProgress);
    assert_eq!(t.name(), "Writing");
    t.shows("40 %");
    t.ptt(true);
    assert_eq!(t.name(), "Writing", "no PTT while writing");
    t.ptt(false);
}

/// HF-28, HF-28.1: HF mode's recent list: each frequency in MHz with its mode; # clears it after a confirm.
#[test]
fn hf_recent_list() {
    let mut t = T::new();
    t.open(Dest::HfRecent);
    t.shows("Nothing yet");
    t.app.set.hf.recent[0] = 707_400 | 3 << 28;
    t.app.set.hf.recent[1] = 958_000 | 2 << 28;
    t.draw();
    t.shows("7.074 MHz");
    t.shows("USB");
    t.tap(Key::Hash);
    assert!(matches!(t.screen(), Screen::Confirm(_)), "asked first");
    assert!(crate::screens::hf::recent(&t.app.set.hf).next().is_some(), "nothing cleared yet");
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "Recent list / cleared");
    assert!(crate::screens::hf::recent(&t.app.set.hf).next().is_none());
}

/// MENU-8: the advanced menu's recent settings: \u{25c0}\u{25b6} pick among them, the help names the one picked, OK
/// opens its editor; the group rows' state words (scanning, new events, GPS searching).
#[test]
fn advanced_recent_and_states() {
    let mut t = T::ready();
    t.tap(Key::Menu);
    t.knob(1); // squelch: now a recent setting
    t.tap(Key::Down);
    t.knob(1); // power
    t.tap(Key::Exit);
    t.hold(Key::Menu);
    assert_eq!(t.screen().mockup(), Some("MenuTop.png"));
    t.tap(Key::Up); // onto the recent row
    t.tap(Key::Right);
    t.tap(Key::Left);
    t.shows("Changed lately. OK opens ");
    t.tap(Key::Menu);
    assert!(matches!(t.screen(), Screen::Value(_)), "{}", t.name());
    t.tap(Key::Exit);
    t.app.scan_mask = 1;
    t.app.log_unread = 2;
    t.dev.gps = ferrite_core::msg::GpsState::Searching;
    t.draw();
    t.shows("Scanning"); // decision 372: the watch beside it no longer fit
    t.shows("2 new");
    t.shows("Searching");
}
