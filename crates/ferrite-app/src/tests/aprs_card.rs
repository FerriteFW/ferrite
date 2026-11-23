//! Home's APRS card for each holder of slot C (APRS-17, decision 181): the header word, the fact right of the
//! frequency, the bottom line, the hint bar's OK and the readback.

use super::*;
use crate::aprs::{AprsCfg, Holder, APRS_SLOT};
use crate::data::Data;
use crate::sample::Sample;
use ferrite_core::msg::Device;
use ferrite_ui::fb::FrameBuffer;
use ferrite_ui::gfx::{AMBER, RED, VIOLET};
use std::format;
use std::string::String;
use std::vec::Vec;

struct A {
    app: App,
    s: Sample,
    t: u32,
    live: LiveAll,
}

impl A {
    fn new() -> Self {
        let mut app = App::new(crate::tests::set(), crate::tests::alert());
        app.set.backlight = 0;
        while app.pop_cmd().is_some() {}
        let mut a = A { app, s: Sample::new(), t: 1000, live: LiveAll::default() };
        a.wait(100);
        a
    }

    fn tick(&mut self) {
        self.app.tick(self.t, &self.live, &self.s);
        while let Some(w) = self.app.pop_write() {
            self.s.apply(w);
        }
        while self.app.pop_cmd().is_some() {}
    }

    fn press(&mut self, k: Key, hold: u32) {
        self.app.key_down(k, self.t, &self.s);
        let end = self.t + hold;
        while self.t < end {
            self.t += 10;
            self.tick();
        }
        self.app.key_up(k, self.t, &self.s);
        self.t += 50;
        self.tick();
    }

    fn wait(&mut self, ms: u32) {
        let end = self.t + ms;
        while self.t < end {
            self.t += 100;
            self.tick();
        }
    }

    /// C active (the large card), toasts gone.
    fn c_active(&mut self) {
        while self.app.active() != APRS_SLOT {
            self.press(Key::Slot, 100);
        }
        self.wait(3500);
    }

    fn card_line(&self) -> String {
        let dev = Device::default();
        let mut l = crate::term::cmd::Line::new();
        (0..)
            .map_while(|i| crate::uistate::line(&self.app, &self.s, &dev, &self.live, i, &mut l).then(|| String::from_utf8_lossy(l.as_bytes()).into_owned()))
            .find(|l| l.starts_with("card C"))
            .expect("card C")
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        self.app.screen().hints(&self.app).as_slice().to_vec()
    }

    fn fb(&mut self) -> FrameBuffer {
        let mut fb = FrameBuffer::new();
        self.app.render(&mut fb, &self.live, &Device::default(), &self.s);
        fb
    }
}

/// Pixels of text drawn in `c` on the card's surface, its blended edges too: each RGB565 channel between the
/// surface's and `c`'s (the surface itself not counted).
fn ink(fb: &FrameBuffer, x: i32, y: i32, w: i32, h: i32, c: u16) -> usize {
    use ferrite_ui::gfx::SURFACE;
    let ch = |v: u16| [v >> 11, (v >> 5) & 63, v & 31];
    let (s, t) = (ch(SURFACE), ch(c));
    let between = |p: u16| p != SURFACE && (0..3).all(|i| ch(p)[i] >= s[i].min(t[i]) && ch(p)[i] <= s[i].max(t[i]));
    (x..x + w).flat_map(|px| (y..y + h).map(move |py| (px, py))).filter(|&(px, py)| between(fb.at(px, py))).count()
}

/// Pixels of `c` in the box.
fn count(fb: &FrameBuffer, x: i32, y: i32, w: i32, h: i32, c: u16) -> usize {
    (x..x + w).flat_map(|px| (y..y + h).map(move |py| (px, py))).filter(|&(px, py)| fb.at(px, py) == c).count()
}

/// APRS-17.1 – APRS-17.4 (decision 181): APRS mode's card: the header "APRS Mode · VHF" with the holder in C's colour, the
/// stations heard right of the frequency, the last station at the bottom with the beacon power; OK leads to the
/// heard list (the APRS page on its "Heard list" row: one more OK opens it). APRS-17.10: "Nothing since power-on"
/// while the list holds only restored stations.
#[test]
fn aprs_card_mode() {
    let mut a = A::new();
    a.app.set_aprs_mode(true);
    a.tick();
    let k = a.app.aprs_card(&a.s).expect("C is the APRS slot");
    assert_eq!(k.holder, Holder::Mode);
    // APRS-17.2 (decision 282): the sample's stations were heard before this power-on (as rows restored from
    // flash, APRS-4): none count as heard since, those within the hour count per hour.
    let hour = (0..a.s.stations()).filter_map(|i| a.s.station(i)).filter(|s| s.ago_s <= 3600).count() as u16;
    assert!(hour > 0, "the sample has stations within the hour");
    assert_eq!(k.counts, Some((0, hour)));
    assert_eq!(k.right.as_bytes(), b"0 heard", "the small card's spot starts on the since-power-on count");
    // APRS-17.10: the list isn't empty, only nothing since power-on.
    assert_eq!(k.line.as_bytes(), b"Nothing since power-on");
    assert_eq!(k.power, "LOW");
    assert!(k.next.is_empty(), "beacons off: no countdown");
    let l = a.card_line();
    assert!(l.contains("head=\"APRS Mode \u{b7} VHF\"") && l.contains("holder=mode") && l.contains("power=\"LOW\"") && l.contains("kind=aprs"), "{l}");
    // The header's holder word is drawn in C's colour (the small card: C is not active).
    let (y, _) = crate::screens::home::card(APRS_SLOT, a.app.active());
    let fb = a.fb();
    assert!(count(&fb, 34, y + 5, 80, 17, VIOLET) > 20, "the holder word in C's colour");
    // C active: OK Heard, and OK opens the APRS page on its Heard list row.
    a.c_active();
    assert_eq!(a.hints()[1], ("OK", "Heard"));
    a.press(Key::Menu, 100);
    assert!(matches!(a.app.screen(), Screen::Options(o) if o.of == crate::screens::opts::Of::AprsCard && o.row == crate::screens::opts::APRS_ROW_HEARD));
    a.press(Key::Menu, 100);
    assert!(matches!(a.app.screen(), Screen::Aprs(_)), "the heard list: {:?}", a.app.screen().name());
}

/// APRS-17.1 – APRS-17.4, KISS-9 (decision 181): KISS's card: "APRS KISS", the port, the frames sent and heard.
#[test]
fn aprs_card_kiss() {
    let mut a = A::new();
    a.app.set.bt = true;
    a.app.set.kiss_port = 2; // 0 5 8 on Bluetooth
    a.tick();
    a.s.kiss_frame(true);
    a.s.kiss_frame(false);
    a.s.kiss_frame(false);
    let k = a.app.aprs_card(&a.s).expect("KISS holds C");
    assert_eq!(k.holder, Holder::Kiss);
    assert_eq!(k.right.as_bytes(), b"Bluetooth");
    assert_eq!(k.line.as_bytes(), "Sent 1 \u{b7} Heard 2".as_bytes());
    let l = a.card_line();
    assert!(l.contains("head=\"APRS KISS \u{b7} VHF\"") && l.contains("holder=kiss") && l.contains("port=\"Bluetooth\"") && l.contains("sent=1") && l.contains("frames=2"), "{l}");
    a.app.set.kiss_port = 1; // the cable
    assert_eq!(a.app.aprs_card(&a.s).unwrap().right.as_bytes(), b"Cable");
    a.c_active();
    assert_eq!(a.hints()[1], ("OK", "Heard"));
}

/// APRS-29.1 (decision 436): timed beacons with APRS mode off give no APRS card (C is the user's slot); with APRS
/// mode on the card is APRS Mode's and the countdown is read back (`next=`), or why there is none.
#[test]
fn aprs_card_beacon() {
    let mut a = A::new();
    a.app.set_aprs(AprsCfg { beacon: 5, ..a.app.aprs_cfg() }); // every 10 min
    a.tick();
    assert_eq!(a.app.aprs_holder(), None);
    assert!(a.app.aprs_card(&a.s).is_none(), "APRS mode off: no APRS card");
    a.app.set_aprs_mode(true);
    a.tick();
    a.s.aprs_tick(10, true, true);
    let k = a.app.aprs_card(&a.s).unwrap();
    assert_eq!(k.holder, Holder::Mode);
    assert!(k.next.as_bytes().contains(&b':'), "beacons on with APRS mode: next= m:ss");
    a.s.fix.set(None);
    let k = a.app.aprs_card(&a.s).unwrap();
    assert_eq!(k.next.as_bytes(), b"no fix");
}

/// APRS-17.5 (decision 313): APRS mode with automatic beacons is still "APRS Mode" in the header (the beacon row
/// says the beacons are on); APRS-17.6: \* (quick mute) on the APRS card shows in the header's state as on every
/// card ("Muted", amber), not by the power.
#[test]
fn aprs_card_beacon_and_mute() {
    let mut a = A::new();
    a.app.set_aprs_mode(true);
    a.app.set_aprs(AprsCfg { beacon: 5, ..a.app.aprs_cfg() }); // every 10 min
    a.tick();
    let l = a.card_line();
    assert!(l.contains("head=\"APRS Mode \u{b7} VHF\""), "{l}");
    a.c_active();
    a.s.aprs_tick(10, true, true);
    a.wait(1000);
    // APRS-15.2: APRS mode muted C as it came on
    assert!(a.card_line().contains("state=\"Muted\""), "{}", a.card_line());
    let (y, _) = crate::screens::home::card(APRS_SLOT, a.app.active());
    let fb = a.fb();
    assert!(count(&fb, 150, y + 7, 76, 17, AMBER) > 20, "Muted, in the header");
    assert_eq!(count(&fb, 120, y + 103, 106, 17, AMBER), 0, "nothing amber by the power");
    assert!(count(&fb, 34, y + 7, 60, 17, VIOLET) > 20, "the holder word still drawn");
    a.press(Key::Star, 100); // \* hears it
    a.wait(3500);
    let fb = a.fb();
    assert_eq!(count(&fb, 150, y + 7, 76, 17, AMBER), 0, "unmuted");
}

/// The APRS card's slots: C active, A on 2 m and watched, so the two share the VHF chip (APRS-21).
fn shared_c() -> A {
    let mut a = A::new();
    a.app.set_aprs_mode(true);
    a.app.set_vfo(0, 14_652_000);
    a.app.watch = 2; // A, B and C
    a.tick();
    a.c_active();
    assert!(a.app.aprs_shared().is_some(), "A shares chip 0 with C");
    a
}

fn aprs_line(a: &A) -> String {
    let dev = Device::default();
    let mut l = crate::term::cmd::Line::new();
    (0..)
        .map_while(|i| crate::uistate::line(&a.app, &a.s, &dev, &a.live, i, &mut l).then(|| String::from_utf8_lossy(l.as_bytes()).into_owned()))
        .find(|l| l.starts_with("aprs_card"))
        .expect("aprs_card line")
}

/// APRS-17.1, APRS-17.6, APRS-21, WATCH-17.3 (decision 313): the header's right is every card's state ("Muted"
/// amber, the RX tag), with who wins the shared VHF chip beside it, "APRS first" grey or "A·B first" amber, and
/// " · VHF" gone; where both don't fit (muted while receiving) the state wins. The readback's `priority=`.
/// HOME-15.3: muted while receiving is "Muted" in amber with the green RX tag right of it.
#[test]
fn aprs_card_header_priority() {
    use ferrite_ui::gfx::SUB;
    let mut a = shared_c();
    let (y, _) = crate::screens::home::card(APRS_SLOT, a.app.active());
    assert_eq!(a.app.aprs_priority(), Some(("APRS first", false)));
    assert!(aprs_line(&a).contains("priority=\"APRS first\""), "{}", aprs_line(&a));
    // muted (APRS-15.2): "APRS Mode", "APRS first" in grey, "Muted" in amber: all three fit
    let fb = a.fb();
    let muted_amber = ink(&fb, 186, y + 7, 40, 17, AMBER);
    assert!(muted_amber > 20, "Muted");
    assert!(ink(&fb, 100, y + 7, 85, 17, SUB) > 40 && count(&fb, 100, y + 7, 85, 17, AMBER) == 0, "APRS first beside it, grey");
    // A·B first: amber
    a.app.set.aprs_share = 1;
    a.wait(200);
    assert_eq!(a.app.aprs_priority(), Some(("A\u{b7}B first", true)));
    assert!(aprs_line(&a).contains("priority=\"A\u{b7}B first\""));
    let fb = a.fb();
    assert!(count(&fb, 100, y + 7, 85, 17, AMBER) > 10, "A·B first in amber");
    // RX muted: the state wins, the priority hides
    a.live.slots[APRS_SLOT as usize].squelch_open = true;
    a.wait(200);
    assert!(a.card_line().contains("state=\"RX muted\""));
    let fb = a.fb();
    let rxm = ink(&fb, 100, y + 7, 94, 17, AMBER);
    assert!(rxm > 20 && count(&fb, 100, y + 7, 60, 17, AMBER) == 0, "Muted alone: {rxm}");
    assert!(count(&fb, 196, y + 7, 30, 17, ferrite_ui::gfx::GREEN) > 20, "the RX tag right of Muted");
    assert_eq!(count(&fb, 196, y + 7, 30, 17, AMBER), 0, "Muted stays left of the tag");
    // unmuted, receiving: the RX tag in green with the priority beside it
    a.live.slots[APRS_SLOT as usize].squelch_open = false;
    a.app.set.aprs_share = 0;
    a.press(Key::Star, 100);
    a.live.slots[APRS_SLOT as usize].squelch_open = true;
    a.wait(3500);
    let fb = a.fb();
    assert!(count(&fb, 190, y + 7, 36, 17, ferrite_ui::gfx::GREEN) > 20, "the RX tag");
    assert!(ink(&fb, 100, y + 7, 90, 17, SUB) > 40, "APRS first beside the tag");
    // C alone on its chip: no priority
    a.app.set_vfo(0, 44_600_000);
    a.wait(200);
    assert!(a.app.aprs_priority().is_none() && !aprs_line(&a).contains("priority="));
}

/// APRS-17.2, APRS-17.2.2 – APRS-17.2.5 (decision 313; M3): the large card: "N heard · H /hr" right of the frequency,
/// the two newest stations heard since power-on (call, age, distance and bearing or "via digi"), unread messages in
/// the second row ("2 new from VE3KPW-9", C's colour) until read, our last beacon's fate, and without our own fix
/// ages only and "no fix" in amber on the beacon row. The readback's `aprs_card` line.
#[test]
fn aprs_card_large_rows() {
    let mut a = A::new();
    a.app.set_aprs_mode(true);
    a.tick();
    a.c_active();
    // 70 s of the simulated APRS task: four stations heard (VE3OTW-10 newest, with a position; VE3BTV without, via
    // a digipeater) and a message for us
    for s in 1..=70 {
        a.s.aprs_tick(s, true, true);
        a.wait(1000);
    }
    let k = a.app.aprs_card(&a.s).unwrap();
    assert_eq!(k.counts.unwrap().0, 4);
    assert_eq!(k.rows_n, 3);
    assert_eq!(k.rows[0].call.as_bytes(), b"VE3OTW-10");
    assert!(k.rows[0].right.as_bytes().ends_with(b"km N") || k.rows[0].right.as_bytes().contains(&b' '), "{:?}", core::str::from_utf8(k.rows[0].right.as_bytes()));
    assert_eq!((k.rows[1].call.as_bytes(), k.rows[1].right.as_bytes()), (b"VE3BTV".as_slice(), b"via digi".as_slice()));
    assert!(k.rows[0].age.as_bytes().ends_with(b" s"));
    // the sample's unread message and the new one, both from VE3KPW-9
    assert_eq!((k.unread, k.unread_from.as_bytes()), (2, b"VE3KPW-9".as_slice()));
    // the sample's last beacon was repeated by VE3RSB-1
    assert_eq!(k.fate.as_bytes(), b"Last beacon digi'd by VE3RSB-1");
    assert!(!k.fate_warn && !k.nofix);
    assert_eq!(k.bottom.as_bytes(), b"Beacon off");
    let l = aprs_line(&a);
    assert!(l.contains("r1=\"VE3OTW-10|") && l.contains("r2=\"VE3BTV|") && l.contains("|via digi\"") && l.contains("unread=2 from=\"VE3KPW-9\"") && l.contains("fate=\"Last beacon digi'd by VE3RSB-1\"") && l.contains("bottom=\"Beacon off\""), "{l}");
    let (y, _) = crate::screens::home::card(APRS_SLOT, a.app.active());
    let fb = a.fb();
    assert!(count(&fb, 15, y + 69, 211, 17, VIOLET) > 40, "the unread row in C's colour");
    assert!(count(&fb, 15, y + 52, 80, 17, ferrite_ui::gfx::TEXT) > 40, "the newest call, bold");
    assert!(count(&fb, 130, y + 34, 96, 17, ferrite_ui::gfx::TEXT) > 30, "4 heard right of the frequency");
    // read: the second station again
    a.s.request(crate::data::Op::MessagesReadAll);
    a.wait(200);
    let fb = a.fb();
    assert_eq!(count(&fb, 15, y + 69, 211, 17, VIOLET), 0, "read: no message row");
    assert!(!aprs_line(&a).contains("unread="));
    // APRS-17.2.5: no fix of our own: ages only, "no fix" in amber on the beacon row
    a.s.fix.set(None);
    a.wait(200);
    let k = a.app.aprs_card(&a.s).unwrap();
    assert!(k.nofix && k.fix_note && k.rows.iter().all(|r| r.right.is_empty()));
    assert!(aprs_line(&a).contains(" nofix=1"));
    let fb = a.fb();
    assert!(ink(&fb, 140, y + 103, 60, 17, AMBER) > 20, "no fix by the power");
    assert_eq!(ink(&fb, 152, y + 52, 74, 34, ferrite_ui::gfx::SUB), 0, "no distance column");
}

/// APRS-17.2.3: a beacon not heard back says so in amber after 30 s (just sent, its age); APRS-17.2.1: the beacon
/// row counts down in whole minutes, then seconds.
#[test]
fn aprs_card_beacon_fate_and_countdown() {
    let mut s = Buf::<16>::new();
    crate::aprs_card::soon_text(272, &mut s);
    assert_eq!(s.as_bytes(), b"5m");
    s.clear();
    crate::aprs_card::soon_text(60, &mut s);
    assert_eq!(s.as_bytes(), b"1m");
    s.clear();
    crate::aprs_card::soon_text(42, &mut s);
    assert_eq!(s.as_bytes(), b"42s");
    let mut a = A::new();
    a.app.set_aprs_mode(true);
    a.app.set_aprs(AprsCfg { beacon: 5, ..a.app.aprs_cfg() }); // every 10 min
    a.tick();
    a.s.aprs_tick(1, true, true); // the first beacon goes at once; heard back 2 s on
    let k = a.app.aprs_card(&a.s).unwrap();
    assert!(k.bottom.as_bytes().starts_with(b"Beacon in ") && k.bottom.as_bytes().ends_with(b"m"), "{:?}", core::str::from_utf8(k.bottom.as_bytes()));
    assert!(k.fate.as_bytes().starts_with(b"Last beacon ") && k.fate.as_bytes().ends_with(b" ago") && !k.fate_warn, "{:?}", core::str::from_utf8(k.fate.as_bytes()));
    a.s.aprs_tick(3, true, true);
    assert_eq!(a.app.aprs_card(&a.s).unwrap().fate.as_bytes(), b"Beacon digi'd by VE3OTW-10", "a long call drops Last");
    // not heard back within 30 s: amber
    let mut f = Buf::<40>::new();
    let b = crate::data::SentBeacon { ago_s: 40, ..Default::default() };
    assert!(crate::aprs_card::fate_text(&b, &mut f));
    assert_eq!(f.as_bytes(), b"Last beacon not heard back");
    let b = crate::data::SentBeacon { ago_s: 12, ..Default::default() };
    assert!(!crate::aprs_card::fate_text(&b, &mut f));
    assert_eq!(f.as_bytes(), b"Last beacon 12 s ago");
}

/// APRS-15.2: APRS mode on mutes C (decoded, not heard), off unmutes it; a radio powered up in APRS mode starts
/// with C muted.
#[test]
fn aprs_mode_mutes_c() {
    let mut a = A::new();
    a.app.set_aprs_mode(true);
    assert_eq!(a.app.mute & 1 << APRS_SLOT, 1 << APRS_SLOT);
    a.app.set_aprs_mode(false);
    assert_eq!(a.app.mute & 1 << APRS_SLOT, 0);
    let set = crate::tests::set();
    set.aprs_mode = true;
    let app = App::new(set, crate::tests::alert());
    assert_eq!(app.mute & 1 << APRS_SLOT, 1 << APRS_SLOT, "at power on");
}

/// KISS-10: KISS on suspends the radio's own APRS sending: APRS mode and automatic beacons wait (kept as set),
/// KISS holds C, 0 5 3 and 0 5 6 1 are greyed with why, and Beacon now and sending a message are refused.
#[test]
fn kiss_suspends_the_radios_own_aprs() {
    let mut a = A::new();
    a.app.set_aprs_mode(true);
    a.app.set_aprs(AprsCfg { beacon: 5, ..a.app.aprs_cfg() });
    a.tick();
    assert!(a.app.aprs_on() && a.app.beacons_on());
    a.app.set.kiss_port = 1; // the cable
    a.tick();
    assert!(!a.app.aprs_on() && !a.app.beacons_on(), "suspended");
    assert!(a.app.set.aprs_mode && a.app.aprs_cfg().beacons_on(), "the settings kept");
    assert_eq!(a.app.aprs_holder(), Some(Holder::Kiss));
    assert_eq!(crate::screens::groups::greyed(&a.app, &[5, 3]), Some("KISS on: the app sends"));
    assert_eq!(crate::screens::groups::greyed(&a.app, &[5, 6, 1]), Some("KISS on: the app sends"));
    a.app.set.kiss_port = 0;
    a.tick();
    assert!(a.app.aprs_on() && a.app.beacons_on(), "back as they were");
    assert_eq!(a.app.aprs_holder(), Some(Holder::Mode));
}

/// APRS-17.7, APRS-17.3, MENU-25 (decision 257): one APRS list in one order from the APRS slot and from the heard
/// list (1 Messages, 2 Heard, 3 Sent (APRS-53), 4 Beacon now, 5 Event log, 6 Clear the heard list, 7 APRS settings;
/// decision 375 took out Beacon on/off);
/// OK 1 on C reads the messages; hold MENU on C opens the same list on the same row as OK; a digit on a row that
/// does something only moves to it.
#[test]
fn one_aprs_list_from_c_and_heard() {
    use crate::screens::opts::{rows, Of};
    let names: Vec<&str> = rows(Of::AprsCard).iter().map(|r| r.0).collect();
    assert_eq!(names, ["Messages", "Heard", "Sent", "Beacon now", "Event log", "Clear the heard list", "APRS settings"]);
    assert!(core::ptr::eq(rows(Of::AprsCard), rows(Of::Heard)), "one list");
    let mut a = A::new();
    a.app.set_aprs_mode(true);
    a.tick();
    a.c_active();
    a.press(Key::Menu, 100);
    a.press(Key::Digit(1), 100);
    assert!(matches!(a.app.screen(), Screen::Msgs(_)), "OK 1: the messages, not {}", a.app.screen().name());
    a.press(Key::Exit, 800);
    a.press(Key::Menu, 800);
    assert!(matches!(a.app.screen(), Screen::Options(o) if o.of == Of::AprsCard && o.row == crate::screens::opts::APRS_ROW_HEARD), "hold MENU: as OK");
    a.press(Key::Digit(4), 100);
    assert!(matches!(a.app.screen(), Screen::Options(o) if o.row == 3), "4: on Beacon now, nothing sent");
    // From the heard list: the same list; its Heard row returns to it.
    a.press(Key::Digit(2), 100);
    assert!(matches!(a.app.screen(), Screen::Aprs(_)), "2: the heard list");
    a.press(Key::Menu, 800);
    assert!(matches!(a.app.screen(), Screen::Options(o) if o.of == Of::Heard));
    a.press(Key::Digit(2), 100);
    assert!(matches!(a.app.screen(), Screen::Aprs(_)));
    a.press(Key::Exit, 100);
    assert!(matches!(a.app.screen(), Screen::Home), "one BACK: no second heard list under it, {}", a.app.screen().name());
}

/// APRS-17.2, APRS-17.2.1: nothing rotates: the small card's bold spot is the heard count at any time, the large
/// card and the readback carry both counts; with automatic beacons on, the small card's bottom right is the
/// countdown ("next m:ss" in C's colour, or why it waits in amber) in place of the power. The widest texts fit.
#[test]
fn aprs_card_counts_and_countdown() {
    use crate::aprs_card::COUNT_FLIP_MS;
    use ferrite_ui::assets::{UI_12, UI_12B};
    use ferrite_ui::gfx::text4_width;
    let mut a = A::new();
    a.app.set_aprs_mode(true);
    a.tick();
    let (_, hour) = a.app.aprs_card(&a.s).unwrap().counts.unwrap();
    for _ in 0..3 {
        assert_eq!(a.app.aprs_card(&a.s).unwrap().right.as_bytes(), b"0 heard", "fixed, at any time");
        a.t += COUNT_FLIP_MS;
        a.tick();
    }
    let l = a.card_line();
    assert!(l.contains(" heard=0") && l.contains(&format!(" hour={hour}")), "the readback carries both: {l}");
    // beacons on: the countdown takes the bottom right of the small card (C is not active); a fresh radio, so the
    // first frame draws every region
    let mut a = A::new();
    a.app.set_aprs_mode(true);
    a.app.set_aprs(AprsCfg { beacon: 5, ..a.app.aprs_cfg() }); // every 10 min
    a.tick();
    let k = a.app.aprs_card(&a.s).unwrap();
    assert!(!k.next.is_empty(), "beacons on: a countdown or a reason");
    let (y, _) = crate::screens::home::card(APRS_SLOT, a.app.active());
    let fb = a.fb();
    let want = if k.next_warn { AMBER } else { VIOLET };
    assert!(count(&fb, 140, y + 51, 86, 15, want) > 10, "the countdown drawn at the bottom right: {:?}", k.next.as_bytes());
    // widths: the spot is 96 px; beside "Beacon in 59m" the last-heard line keeps "VE7ABC-9 · 2 min" ("Last " goes)
    assert!(text4_width(b"50 heard", &UI_12B) <= 96);
    let cd = text4_width(b"Beacon in 59m", &UI_12);
    assert!(text4_width("VE7ABC-9 \u{b7} 2 min".as_bytes(), &UI_12) <= 211 - cd - 8);
    // APRS-17.2: the large card's counts beside the frequency at the compact size: "12 heard · 9 /hr" fits, the
    // widest "99 heard · 99 /hr" drops " /hr"
    let f = text4_width(b"144.390", &ferrite_ui::assets::DIG_26);
    let fits = |n: &str, h: &str| 15 + f + 8 + text4_width(n.as_bytes(), &UI_12B) + text4_width(h.as_bytes(), &UI_12) <= 226;
    assert!(fits("12 heard", " \u{b7} 9 /hr") && fits("3 heard", " \u{b7} 11 /hr"));
    assert!(!fits("99 heard", " \u{b7} 99 /hr") && fits("99 heard", ""));
}

/// HOME-18: an APRS key-up (a data key-up: no Transmit screen) shows on C's card as a red TX word and border,
/// and in the readback's `state="TX"`; gone at the unkey.
#[test]
fn aprs_tx_shows_on_the_card() {
    let mut a = A::new();
    a.app.set_aprs_mode(true);
    a.tick();
    let (y, _) = crate::screens::home::card(APRS_SLOT, a.app.active());
    let red = |a: &mut A| {
        let fb = a.fb();
        count(&fb, 6, y, 228, 30, RED)
    };
    assert_eq!(red(&mut a), 0);
    a.live.tx.keyed = true;
    a.live.tx.slot = APRS_SLOT;
    a.tick();
    assert!(a.card_line().contains("state=\"TX\""), "{}", a.card_line());
    assert!(red(&mut a) > 60, "the TX word and border in red");
    a.live.tx.keyed = false;
    a.tick();
    assert!(!a.card_line().contains("state=\"TX\""));
    assert_eq!(red(&mut a), 0);
}

/// WATCH-16.2: while the watch leaves the APRS slot out (another slot keyed, a relay cancel), its card says
/// "paused"; listening again, it doesn't.
#[test]
fn aprs_paused_shows_on_the_card() {
    let mut a = A::new();
    a.app.set_aprs_mode(true);
    a.tick();
    a.live.dropped = 1 << APRS_SLOT;
    a.tick();
    assert!(a.card_line().contains("state=\"paused\""), "{}", a.card_line());
    a.live.dropped = 0;
    a.tick();
    assert!(!a.card_line().contains("state=\"paused\""));
}

/// APRS-53, APRS-53.2, APRS-53.3: the APRS list's 3 opens the Sent page: our beacons newest first, each with its
/// age and who repeated it, "not heard back" in amber (grey in the readback); a beacon sent shows at the top, and
/// the digipeater heard repeating it is added to its row.
#[test]
fn sent_page_lists_our_beacons() {
    let mut a = A::new();
    a.app.set_aprs_mode(true);
    a.tick();
    a.c_active();
    a.press(Key::Menu, 100);
    a.press(Key::Digit(3), 100);
    assert!(matches!(a.app.screen(), Screen::AprsSent(_)), "3: the Sent page, not {}", a.app.screen().name());
    let rows = |a: &A| {
        let dev = Device::default();
        let mut l = crate::term::cmd::Line::new();
        (0..).map_while(|i| crate::uistate::line(&a.app, &a.s, &dev, &a.live, i, &mut l).then(|| String::from_utf8_lossy(l.as_bytes()).into_owned())).filter(|l| l.starts_with("row ")).collect::<Vec<_>>()
    };
    let r = rows(&a);
    assert_eq!(r.len(), 3, "{r:?}");
    assert!(r[0].starts_with("row 0 \"11 min ago\" \"by VE3RSB-1\" sel"), "newest first: {r:?}");
    assert!(r[1].starts_with("row 0 \"21 min ago\" \"not heard back\"") && r[1].ends_with("grey"), "{r:?}");
    assert!(r[2].starts_with("row 0 \"31 min ago\" \"by VE3RSB-1, VE3OTW-10\""), "{r:?}");
    // APRS-53.2: the second row's line in amber
    let fb = a.fb();
    let y = crate::screens::lists::LIST_Y + crate::screens::lists::ROW2_H;
    assert!(count(&fb, 8, y + 18, 200, 13, AMBER) > 20, "not heard back in amber");
    assert_eq!(count(&fb, 8, y - crate::screens::lists::ROW2_H + 18, 200, 13, AMBER), 0, "heard back: not amber");
    // A beacon sent: at the top, not heard back; two seconds on, the iGate repeats it.
    a.s.request(crate::data::Op::BeaconNow);
    assert_eq!(a.s.aprs_tick(100, true, true).tx, Some("Beacon"));
    assert_eq!(a.s.beacons_sent(), 4);
    assert!(a.s.beacon_sent(0).unwrap().by.iter().all(|b| b.is_empty()));
    a.s.aprs_tick(102, true, true);
    assert_eq!(a.s.beacon_sent(0).unwrap().by[0].as_bytes(), b"VE3OTW-10");
    a.press(Key::Exit, 100);
    assert!(!matches!(a.app.screen(), Screen::AprsSent(_)), "BACK leaves it");
}

/// SQL-19, SQL-19.4: the APRS squelch's counts in the card's readback.
#[test]
fn aprs_squelch_counts_in_the_readback() {
    let mut a = A::new();
    a.app.set_aprs_mode(true);
    a.live.aprs_sq = ferrite_core::aprssq::Counts { opens: 3, short: 1, spikes: 2, settle: 4 };
    a.tick();
    let dev = Device::default();
    let mut l = crate::term::cmd::Line::new();
    let line = (0..)
        .map_while(|i| crate::uistate::line(&a.app, &a.s, &dev, &a.live, i, &mut l).then(|| String::from_utf8_lossy(l.as_bytes()).into_owned()))
        .find(|l| l.starts_with("aprs_sq"))
        .expect("aprs_sq line");
    assert_eq!(line, "aprs_sq opens=3 short=1 spikes=2 settle=4");
}

/// MENU-34: hold ABC on the APRS slot opens the APRS quick page, the same page whatever holds C (KISS, APRS mode;
/// beacons no longer hold it alone, APRS-29.1); it no longer says "Slot C held by" (APRS-19 still refuses the
/// slot's own changes).
#[test]
fn hold_abc_on_the_aprs_slot_opens_its_quick_page() {
    use crate::screens::qgrid::Page;
    let mut a = A::new();
    a.app.set.bt = true;
    a.app.set.kiss_port = 2; // KISS holds C
    a.tick();
    a.c_active();
    a.press(Key::Slot, 800);
    assert!(matches!(a.app.screen(), Screen::QGrid(g) if g.page == Page::Aprs), "{}", a.app.screen().name());
    a.press(Key::Exit, 100);
    a.app.set.kiss_port = 0;
    assert_eq!(a.app.screen(), Screen::Home);
    a.app.set_aprs_mode(true);
    a.tick();
    a.press(Key::Slot, 800);
    assert!(matches!(a.app.screen(), Screen::QGrid(g) if g.page == Page::Aprs), "{}", a.app.screen().name());
    assert_eq!(a.app.active(), APRS_SLOT, "the slot stays");
}
