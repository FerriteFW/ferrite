//! ACARS (chapter 27b, on trial): the screen's keys, pages and readback over the sample traffic.

use super::*;
use crate::data::Data;
use crate::sample::Sample;
use crate::screens::acars::Acars;
use crate::term::cmd::Line;
use std::string::String;

/// An app on the sample records (ten ACARS messages), kept for the whole test.
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

    fn tap(&mut self, k: Key) {
        self.app.key_down(k, self.t, &self.s);
        self.t += 100;
        self.app.tick(self.t, &self.live, &self.s);
        self.app.key_up(k, self.t, &self.s);
        self.t += 50;
        self.app.tick(self.t, &self.live, &self.s);
    }

    fn wait(&mut self, ms: u32) {
        let end = self.t + ms;
        while self.t < end {
            self.t += 100;
            self.app.tick(self.t, &self.live, &self.s);
        }
    }

    fn acars(&self) -> Acars {
        match self.app.screen() {
            Screen::Acars(a) => a,
            s => panic!("not on ACARS: {s:?}"),
        }
    }

    fn render(&mut self) -> FrameBuffer {
        let mut fb = FrameBuffer::new();
        let dev = Device { clock_min: Some(19 * 60 + 6), gps: GpsState::Fix, battery_pct: 80, ..Default::default() };
        self.app.render(&mut fb, &self.live, &dev, &self.s);
        fb
    }

    /// The readback's `acars` line.
    fn line(&self) -> String {
        let mut l = Line::new();
        let mut i = 0;
        while crate::uistate::line(&self.app, &self.s, &Device::default(), &self.live, i, &mut l) {
            let s = String::from_utf8_lossy(l.as_bytes()).into_owned();
            if s.starts_with("acars") {
                return s;
            }
            i += 1;
        }
        panic!("no acars line")
    }
}

/// ACARS-1, ACARS-9: MENU 0 3 8 opens the list on the newest message (the readback says so); BACK leaves it.
#[test]
fn the_menu_path_opens_the_list() {
    let mut a = A::new();
    for k in [Key::Menu, Key::Digit(0), Key::Digit(3), Key::Digit(8)] {
        a.tap(k);
    }
    let s = a.acars();
    assert!(!s.detail && !s.strip && s.cur.sel == 0);
    a.render();
    assert!(a.line().starts_with("acars page=\"list\" n=10 sel=0 freq=\"131.550\" strip=0 airlines=0"), "{}", a.line());
    a.tap(Key::Exit);
    assert!(!matches!(a.app.screen(), Screen::Acars(_)));
}

/// ACARS-3: ◀▶ on the list step through the frequencies and wrap; the setting keeps the choice.
#[test]
fn arrows_pick_the_frequency() {
    let mut a = A::new();
    a.app.open(crate::screens::nav::Dest::Acars);
    a.tap(Key::Right);
    assert_eq!(a.app.set.acars_freq, 1);
    assert!(a.line().contains("freq=\"131.725\""), "{}", a.line());
    a.tap(Key::Left);
    a.tap(Key::Left);
    assert_eq!(a.app.set.acars_freq as usize, crate::acars::FREQS.len() - 1);
    assert!(a.line().contains("freq=\"136.900\""), "{}", a.line());
    let mut s = Settings::DEFAULT;
    s.acars_freq = 200;
    s.sanitize();
    assert_eq!(s.acars_freq, 0, "a damaged record's index reads the default");
}

/// ACARS-4, ACARS-7: ▲▼ move, OK opens the message, ◀▶ step to the next and back, BACK returns to the list where
/// it was; the readback names its fields.
#[test]
fn ok_opens_a_message_and_arrows_step() {
    let mut a = A::new();
    a.app.open(crate::screens::nav::Dest::Acars);
    for _ in 0..7 {
        a.tap(Key::Down);
    }
    assert_eq!(a.acars().cur.sel, 7);
    a.tap(Key::Menu);
    assert!(a.acars().detail);
    let l = a.line();
    assert!(l.contains("page=\"message\"") && l.contains("flight=\"AC0115\" reg=\"C-FIUA\" label=\"H1\" up=0 crc=1 blocks=2"), "{l}");
    a.tap(Key::Left);
    assert_eq!(a.acars().cur.sel, 6);
    a.tap(Key::Right);
    a.tap(Key::Right);
    assert_eq!(a.acars().cur.sel, 8);
    a.tap(Key::Exit);
    let s = a.acars();
    assert!(!s.detail && s.cur.sel == 8);
}

/// ACARS-7: ▲▼ scroll a long text a line at a time and stop at its end; the next message starts at the top.
#[test]
fn a_long_text_scrolls() {
    let mut a = A::new();
    a.app.open(crate::screens::nav::Dest::Acars);
    a.tap(Key::Menu);
    for _ in 0..20 {
        a.tap(Key::Down);
    }
    a.render();
    a.render();
    // the newest (a link test) has no text: nothing to scroll
    assert_eq!(a.acars().line, 0);
    // 21 lines: a line feed every 10 characters (ACARS text often has its own lines)
    let mut long = [b'W'; 400];
    for k in (9..400).step_by(10) {
        long[k] = b'\n';
    }
    let f = ferrite_core::acars::Fields { mode: b'2', reg: b"C-FTJO", ack: ferrite_core::acars::NAK, label: *b"H1", block: b'1', msgno: b"M09A", flight: b"AC0001", text: &long[..210], more: false };
    let mut b = [0u8; 300];
    let n = ferrite_core::acars::encode(&f, &mut b);
    a.s.acars().unwrap().frame(&b[..n], 0, a.t / 1000).unwrap();
    a.tap(Key::Right);
    a.tap(Key::Left);
    assert_eq!(a.acars().cur.sel, 0);
    a.tap(Key::Down);
    a.tap(Key::Down);
    a.render();
    assert_eq!(a.acars().line, 2);
    for _ in 0..20 {
        a.tap(Key::Down);
    }
    a.render();
    let end = a.acars().line;
    assert!(end > 2 && end < 21, "held at the text's end: {end}");
    a.tap(Key::Right);
    assert_eq!(a.acars().line, 0);
}

/// ACARS-5: # puts slot B's strip over the list (one row fewer) and takes it away; the selection stays shown.
#[test]
fn hash_toggles_the_strip() {
    let mut a = A::new();
    a.app.open(crate::screens::nav::Dest::Acars);
    for _ in 0..9 {
        a.tap(Key::Down);
    }
    a.tap(Key::Hash);
    let s = a.acars();
    assert!(s.strip && s.cur.top > 0, "the 10th row is off a 9-row page (compact rows, ACARS-4): {s:?}");
    let fb = a.render();
    // the strip's background under the status bar
    assert_eq!(fb.at(120, 22), ferrite_ui::gfx::SURFACE);
    a.tap(Key::Hash);
    assert!(!a.acars().strip);
}

/// ACARS-4: with no messages the list says so; ACARS-6: without the airline table the raw code is shown.
#[test]
fn an_empty_list_and_no_airline_table() {
    let mut a = A::new();
    a.s.acars().unwrap().clear();
    a.app.open(crate::screens::nav::Dest::Acars);
    a.render();
    assert!(a.line().contains("n=0"));
    a.tap(Key::Menu);
    assert!(!a.acars().detail, "nothing to open");
    // the sample store has no flash: no table, so the flight's own code
    let mut b = [0u8; 300];
    let f = ferrite_core::acars::Fields { mode: b'2', reg: b"C-FTJO", ack: ferrite_core::acars::NAK, label: *b"H1", block: b'1', msgno: b"M09A", flight: b"AC0001", text: b"HELLO", more: false };
    let n = ferrite_core::acars::encode(&f, &mut b);
    a.s.acars().unwrap().frame(&b[..n], 0, 1).unwrap();
    a.render();
    assert!(a.line().contains("airlines=0") && a.line().contains("n=1"));
}

/// ACARS-11: the screen runs the Radio task's ACARS mode on the frequency picked: AcarsRx(131.550) as it opens, ◀▶
/// retunes, BACK stops it (AcarsRx(0)). The decoder runs once the radio says it listens there (the header and the
/// readback say so), and a frame heard goes into the list.
#[test]
fn the_screen_tunes_the_radio_and_frames_come_in() {
    use ferrite_core::msg::{ModeLive, RadioCmd};
    let mut a = A::new();
    a.app.open(crate::screens::nav::Dest::Acars);
    a.wait(200);
    let mut cmds = Vec::new();
    while let Some(c) = a.app.pop_cmd() {
        cmds.push(c);
    }
    assert!(cmds.contains(&RadioCmd::AcarsRx(13_155_000)), "{cmds:?}");
    assert!(!a.app.acars_decoding(&a.live), "not before the radio listens");
    a.render();
    assert!(a.line().ends_with("radio=0"), "{}", a.line());
    a.live.mode = ModeLive::Acars { freq: 13_155_000, rssi: -110, open: false };
    a.wait(100);
    assert!(a.app.acars_decoding(&a.live));
    a.render();
    assert!(a.line().ends_with("radio=1"), "{}", a.line());
    let n = a.s.data().acars().unwrap().len();
    let mut b = [0u8; 64];
    let k = ferrite_core::acars::encode(
        &ferrite_core::acars::Fields { mode: b'2', reg: b"C-GKOA", ack: ferrite_core::acars::NAK, label: *b"Q0", block: b'A', ..Default::default() },
        &mut b,
    );
    a.app.acars_heard(&a.s, &b[..k - 1]);
    assert_eq!(a.s.data().acars().unwrap().len(), n + 1, "the frame is in the list");
    a.tap(Key::Right);
    let mut cmds = Vec::new();
    while let Some(c) = a.app.pop_cmd() {
        cmds.push(c);
    }
    assert!(cmds.contains(&RadioCmd::AcarsRx(13_172_500)), "◀▶ retunes: {cmds:?}");
    assert!(!a.app.acars_decoding(&a.live), "the radio still on the old frequency");
    a.tap(Key::Exit);
    let mut cmds = Vec::new();
    while let Some(c) = a.app.pop_cmd() {
        cmds.push(c);
    }
    assert!(cmds.contains(&RadioCmd::AcarsRx(0)), "BACK stops it: {cmds:?}");
}

/// ACARS-4.2: * shows only the messages with text (the samples' link test and ack hidden), the header counting
/// them; * again shows all. The choice is kept in the settings; OK and ▲▼ skip the hidden ones.
#[test]
fn star_hides_the_messages_without_text() {
    let mut a = A::new();
    a.app.open(crate::screens::nav::Dest::Acars);
    a.render();
    assert!(a.line().contains("n=10 sel=0") && a.line().contains("text=0"), "{}", a.line());
    a.tap(Key::Star);
    assert!(a.app.set.acars_text);
    a.render();
    // the newest (C-FJZS's link test) is hidden: the first shown is selected
    let s = a.acars();
    assert_eq!(s.cur.sel, 1, "{}", a.line());
    assert!(a.line().contains("text=1"), "{}", a.line());
    let mut seen = std::vec::Vec::new();
    for _ in 0..12 {
        let i = a.acars().cur.sel;
        seen.push(i);
        assert!(a.s.data().acars().unwrap().get(i, &mut [0u8; 1]).unwrap().1 > 0, "message {i} has text");
        a.tap(Key::Down);
    }
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), 8, "eight of the ten have text: {seen:?}");
    a.tap(Key::Star);
    assert!(!a.app.set.acars_text);
    assert_eq!(a.acars().cur.sel, 0);
}

/// ACARS-3: the header follows the radio: opened before the radio listens it says so, and it redraws as soon as
/// the radio reports (it waited for the next message or minute before).
#[test]
fn the_header_redraws_when_the_radio_listens() {
    use ferrite_core::msg::ModeLive;
    let mut a = A::new();
    a.app.open(crate::screens::nav::Dest::Acars);
    a.wait(200);
    a.render();
    a.render();
    let head = |fb: &FrameBuffer| {
        let y = crate::screens::lists::HEAD_Y as usize;
        fb.writes[y * 240..(y + 20) * 240].iter().filter(|&&w| w > 0).count()
    };
    assert_eq!(head(&a.render()), 0, "nothing changed: nothing drawn");
    a.live.mode = ModeLive::Acars { freq: 13_155_000, rssi: -110, open: false };
    assert!(head(&a.render()) > 0, "the header drawn again");
}

/// ACARS-4.3: a data report (an H1 #DFB from the aircraft's data unit, as the black radio heard WS1057 send) is
/// in the list as its kind and size, its page names the kind and keeps the raw text; * hides it with the messages
/// without text (ACARS-4.2).
#[test]
fn a_data_report_shows_its_kind_not_its_hex() {
    let mut a = A::new();
    a.app.open(crate::screens::nav::Dest::Acars);
    let mut b = [0u8; 256];
    let f = ferrite_core::acars::Fields {
        mode: b'2',
        reg: b"C-FKWJ",
        ack: ferrite_core::acars::NAK,
        label: *b"H1",
        block: b'5',
        msgno: b"D01A",
        flight: b"WS1057",
        text: b"#DFB50101C-FKWJB737-800261125WS1057KLASCYVR0710WJX79B",
        more: false,
    };
    let k = ferrite_core::acars::encode(&f, &mut b);
    a.app.acars_heard(&a.s, &b[..k - 1]);
    let (m, _) = a.s.data().acars().unwrap().get(0, &mut []).unwrap();
    assert!(m.is_data() && m.has_text());
    a.render();
    a.tap(Key::Menu);
    a.render();
    assert!(a.line().contains("flight=\"WS1057\"") && a.line().contains("data=1"), "{}", a.line());
    a.tap(Key::Exit);
    a.tap(Key::Star);
    a.render();
    assert_ne!(a.acars().cur.sel, 0, "hidden under *: {}", a.line());
}

/// ACARS-4.2, the reboot receiving ACARS (Nov 27, 2026): with * on and no message with text (an empty list, then
/// link tests, acks or data reports heard) the list showed its empty page; before, the render stepped the
/// selection off the hidden message, found none to step to and drew again, for ever (the stack overflowed).
#[test]
fn star_with_no_message_to_show() {
    use ferrite_core::acars::{encode, Fields, NAK};
    let mut a = A::new();
    a.s.acars().unwrap().clear();
    a.app.open(crate::screens::nav::Dest::Acars);
    a.tap(Key::Star);
    assert!(a.app.set.acars_text);
    a.render();
    let mut b = [0u8; 300];
    // a link test (no text), then an ADS-C report (data, not words): neither is shown under *
    let link = Fields { mode: b'2', reg: b"C-FJZS", ack: NAK, label: *b"Q0", block: b'1', ..Default::default() };
    let ads = Fields { mode: b'2', reg: b"C-FTJO", ack: NAK, label: *b"B6", block: b'2', msgno: b"M01A", flight: b"AC0001", text: b"/ADS.C-FTJO0123", more: false };
    for (k, f) in [link, ads].iter().enumerate() {
        let n = encode(f, &mut b);
        a.s.acars().unwrap().frame(&b[..n], 0, 2 + k as u32).unwrap();
        a.render();
        assert!(a.line().contains(&std::format!("n={}", k + 1)), "{}", a.line());
        for key in [Key::Down, Key::Up, Key::Menu, Key::Hash, Key::Hash] {
            a.tap(key);
            a.render();
        }
        assert!(!a.acars().detail, "nothing shown to open");
    }
}
