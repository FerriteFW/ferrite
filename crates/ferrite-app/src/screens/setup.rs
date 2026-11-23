//! First-run setup (manual chapter 4, SETUP-1 to SETUP-12; SetupCall, SetupRegion and SetupSummary mock-ups):
//! eight pages, one question each, every page with a default (OK, OK, OK gives a working radio). OK next, BACK
//! previous, # skips to the summary. Each answer writes its setting at once (SETUP-5). Also the Extend TX
//! warning ([`ExtendTx`], A19), shown once before Extend TX turns on (SETUP-8).
//!
//! The pages are data ([`PAGES`]: heading, text, what they ask); the drawing is the same few pieces for each.
//! Setup draws its own top bar ("Setup 2 of 8" and the progress dashes) in place of the status bar.
//!
//! Runs on first boot ([`App::first_boot`], BOOT-12) and from MENU 0 9 7 1; the callsign page alone is also the
//! callsign's editor (MENU 0 8 6 1).

use super::groups::set_extend;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::settings::*;
use crate::slot::Item;
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// What a page asks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ask {
    Nothing,
    Callsign,
    Region,
    Owner,
    Time,
    Look,
    BatterySave,
    Summary,
}

/// The eight pages: heading, text, what they ask (SETUP-2).
const PAGES: [(&str, crate::text::Txt, Ask); 8] = [
    (
        "Welcome to Ferrite",
        txt!("A few questions, one per page. Each has a default, so OK on every page gives a working radio. OK goes on, BACK goes back, # skips the rest (where you type, # changes the letters)."),
        Ask::Nothing,
    ),
    (
        "Your callsign",
        txt!("For APRS, CW ID and the power-on message. May be left empty (MENU 0 8 6 1)."),
        Ask::Callsign,
    ),
    ("Region", txt!("Where you may transmit, and defaults like weather alerts."), Ask::Region),
    (
        "Owner card",
        txt!("Optional. Boot screen (MENU 0 7 3 3) can show it; later, edit it in the web app."),
        Ask::Owner,
    ),
    ("Time", txt!("Time zone, clock and units. The GPS sets the time once it has a fix."), Ask::Time),
    ("Look", txt!("What home shows, and the colours."), Ask::Look),
    (
        "Battery Save",
        txt!("The receivers sleep between checks: deeper lasts longer, but hears a call later."),
        Ask::BatterySave,
    ),
    ("All set", txt!(""), Ask::Summary),
];
const LAST: u8 = PAGES.len() as u8 - 1;

/// The region page's choices: all eight regions on one screen, none chosen until OK on one (decided Oct 6, 2026;
/// the mock-up's "Other ..." row is gone). Extend TX is the row after them.
const REGION_ROWS: u8 = REGION_LIST.len() as u8;
/// Battery Save levels and their trade-off (chapter 33's table).
const SAVE: [(&str, &str, crate::text::Txt); 4] = [
    ("Off", "always on", txt!("Receivers always on: hears everything, shortest battery life.")),
    ("Level 1", "38 % on", txt!("On 300 ms, off 500 ms: a signal can be heard up to 0.5 s late.")),
    ("Level 2", "20 % on", txt!("On 180 ms, off 700 ms: a signal can be heard up to 0.7 s late.")),
    ("Level 3", "18 % on", txt!("On 180 ms, off 800 ms: a signal can be heard up to 0.8 s late.")),
];
/// Owner card fields.
const OWNER: [&str; 3] = ["Name", "Phone", "E-mail"];
/// Multi-tap letters per digit key (KEY-12); the digit itself comes last.
const TAPS: [&[u8]; 10] = [b" 0", b".-/@1", b"ABC2", b"DEF3", b"GHI4", b"JKL5", b"MNO6", b"PQRS7", b"TUV8", b"WXYZ9"];
/// A second tap on the same key within this long picks the next letter.
const TAP_MS: u32 = 1000;

/// Flags.
const FIRST: u8 = 1; // first boot: BACK on page 1 does nothing
const SINGLE: u8 = 2; // one page on its own (the callsign from the menu): OK and BACK return
const LOWER: u8 = 4; // owner card: lower-case letters
const DIGITS: u8 = 8; // the digit keys type digits

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Setup {
    pub page: u8,
    pub row: u8,
    flags: u8,
    /// Multi-tap: the last key (digit + 1, 0 none), how many taps, and when (ms / 16, wrapping at 4 s).
    tap_key: u8,
    tap_n: u8,
    tap_at: u8,
}

/// The callsign: setup's callsign page on its own (SETUP-5: the same setting; MENU 0 8 6 1, and the fix for a
/// transmit refused for no callsign). OK and BACK return to the screen it was opened from.
pub(crate) fn callsign(app: &mut App) {
    app.open(super::nav::Dest::Setup);
    app.screen = Screen::Setup(Setup::single(1));
}

impl Setup {
    /// From the start (MENU 0 9 7 1).
    pub fn new(first: bool) -> Self {
        Setup { page: 0, row: 0, flags: if first { FIRST } else { 0 }, tap_key: 0, tap_n: 0, tap_at: 0 }
    }

    /// One page on its own, as a setting's editor.
    pub fn single(page: u8) -> Self {
        Setup { page, flags: SINGLE, ..Setup::new(false) }
    }

    fn with(self, app: &mut App, f: impl FnOnce(&mut Setup)) {
        let mut s = self;
        f(&mut s);
        app.screen = Screen::Setup(s);
    }

    fn ask(self) -> Ask {
        PAGES[self.page as usize].2
    }

    fn rows(self) -> u8 {
        match self.ask() {
            Ask::Region => REGION_ROWS + 1,
            Ask::Owner | Ask::Time => 3,
            Ask::Look => 2,
            Ask::BatterySave => 4,
            _ => 1,
        }
    }

    /// The row a page opens on: the current value.
    fn first_row(app: &App, page: u8) -> u8 {
        match PAGES[page as usize].2 {
            Ask::Region => crate::settings::region_row(app.set.region),
            Ask::BatterySave => app.bsave,
            _ => 0,
        }
    }

    fn go(self, app: &mut App, page: u8) {
        let row = Setup::first_row(app, page);
        self.with(app, |s| (s.page, s.row, s.tap_key, s.flags) = (page, row, 0, s.flags & (FIRST | SINGLE)));
    }

    fn finish(self, app: &mut App) {
        app.set.setup_done = true;
        app.region_steps(); // VFO-18: 5 kHz on 2 m in the US and Canada
        if self.flags & SINGLE != 0 {
            app.back();
        } else {
            app.go_home();
        }
    }

    /// A digit on a text page: types it (digits mode) or cycles the key's letters (KEY-12).
    fn tap(self, app: &mut App, d: u8) {
        let owner = self.ask() == Ask::Owner;
        let now = (app.now / 16) as u8;
        let again = self.tap_key == d + 1 && now.wrapping_sub(self.tap_at) as u32 * 16 < TAP_MS;
        let mut set: &[u8] = TAPS[d as usize];
        if !owner {
            // Callsigns: capitals, digits, - and / only (SETUP-12).
            set = match d {
                0 => b"0",
                1 => b"-/1",
                _ => set,
            };
        }
        let digits = self.flags & DIGITS != 0;
        let n = if again && !digits { (self.tap_n + 1) % set.len() as u8 } else { 0 };
        let mut c = if digits { b'0' + d } else { set[n as usize] };
        if self.flags & LOWER != 0 {
            c = c.to_ascii_lowercase();
        }
        let ok = match (owner, again && !digits) {
            (false, true) => {
                app.set.callsign.set_last(c);
                true
            }
            (false, false) => app.set.callsign.push(c),
            (true, true) => {
                app.set.owner[self.row as usize % 3].set_last(c);
                true
            }
            (true, false) => app.set.owner[self.row as usize % 3].push(c),
        };
        if ok {
            self.with(app, |s| (s.tap_key, s.tap_n, s.tap_at) = (d + 1, n, now));
        }
    }

    fn delete(self, app: &mut App) {
        match self.ask() {
            Ask::Callsign => app.set.callsign.pop(),
            Ask::Owner => app.set.owner[self.row as usize % 3].pop(),
            _ => {}
        }
        self.with(app, |s| s.tap_key = 0);
    }

    /// The knob or ◀▶ on the focused row.
    fn turn(self, app: &mut App, d: i32) {
        let cyc = |v: u8, n: usize| (v as i32 + d).rem_euclid(n as i32) as u8;
        let s = &mut app.set;
        match self.ask() {
            // The knob moves through the regions; OK chooses (nothing is chosen for you).
            Ask::Region if self.row < REGION_ROWS => {
                let r = cyc(self.row, REGION_ROWS as usize);
                self.with(app, |st| st.row = r);
            }
            Ask::Region => set_extend(app, !app.set.extend_tx),
            Ask::Time => match self.row {
                0 => s.tz = cyc(s.tz, TIME_ZONES.len()),
                1 => s.clock = cyc(s.clock, CLOCKS.len()),
                _ => s.units = cyc(s.units, UNITS.len()),
            },
            Ask::Look => match self.row {
                0 => s.layout = cyc(s.layout, LAYOUTS.len()),
                _ => s.theme = cyc(s.theme, THEMES.len()),
            },
            Ask::BatterySave => {
                let r = cyc(self.row, SAVE.len());
                app.change(Item::BatterySave, r as i32 - app.bsave as i32);
                self.with(app, |st| st.row = r);
            }
            _ => {}
        }
    }
}

impl View for Setup {
    /// KEY-19: ◀ deletes in text; otherwise ◀▶ are the knob.
    fn own_arrows(self) -> bool {
        true
    }

    fn name(self) -> &'static str {
        "Setup"
    }

    fn mockup(self) -> Option<&'static str> {
        match self.page {
            0 => Some("SetupWelcome.png"),
            1 => Some("SetupCall.png"),
            2 => Some("SetupRegion.png"),
            3 => Some("SetupOwner.png"),
            4 => Some("SetupTime.png"),
            5 => Some("SetupLook.png"),
            6 => Some("SetupBattery.png"),
            7 => Some("SetupSummary.png"),
            _ => None,
        }
    }

    fn bar(self) -> bool {
        false
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        use Press::*;
        let single = self.flags & SINGLE != 0;
        let ask = self.ask();
        let text = matches!(ask, Ask::Callsign | Ask::Owner);
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) if self.rows() > 1 => {
                let n = self.rows();
                let row = if e.key == Key::Up { (self.row + n - 1) % n } else { (self.row + 1) % n };
                self.with(app, |s| (s.row, s.tap_key) = (row, 0));
                if let (Ask::BatterySave, r) = (ask, row) {
                    app.change(Item::BatterySave, r as i32 - app.bsave as i32);
                }
            }
            (Key::Left, Short | Repeat) if text => self.delete(app),
            (Key::Left | Key::Right, Short | Repeat) => self.turn(app, if e.key == Key::Right { 1 } else { -1 }),
            (Key::Digit(d), Short) if text => self.tap(app, d),
            // KEY-12: * (marked _) is the space on the owner card (a callsign has none, SETUP-12).
            (Key::Star, Short) if ask == Ask::Owner => {
                let _ = app.set.owner[self.row as usize % 3].push(b' ');
                self.with(app, |s| s.tap_key = 0);
            }
            (Key::Hash, Short) if text => {
                // KEY-12: # (marked ^) changes the letters: callsign letters / digits; owner card ABC / abc / 123.
                let next = match (ask, self.flags & (LOWER | DIGITS)) {
                    (Ask::Callsign, 0) => DIGITS,
                    (Ask::Callsign, _) => 0,
                    (_, 0) => LOWER,
                    (_, LOWER) => DIGITS,
                    _ => 0,
                };
                self.with(app, |s| (s.flags, s.tap_key) = (s.flags & (FIRST | SINGLE) | next, 0));
            }
            // OK on a region chooses it and goes on: no region is chosen for you (SETUP-3).
            (Key::Menu, Short) if ask == Ask::Region && self.row < REGION_ROWS => {
                app.set.region = crate::settings::region_at(self.row);
                app.region_start(); // VFO-25: off the US factory frequencies
                app.region_steps(); // VFO-18
                if single {
                    self.finish(app);
                } else {
                    self.go(app, self.page + 1);
                }
            }
            (Key::Menu, Short) if single => self.finish(app),
            (Key::Menu, Short) if ask == Ask::Region && self.row == REGION_ROWS => self.turn(app, 1),
            (Key::Menu, Short) if self.page == LAST => self.finish(app),
            (Key::Menu, Short) => self.go(app, self.page + 1),
            (Key::Exit, Short) if single => app.back(),
            (Key::Exit, Short) if self.page == 0 && self.flags & FIRST == 0 => app.back(),
            (Key::Exit, Short) if self.page > 0 => self.go(app, self.page - 1),
            (Key::Hash, Short) if !single && self.page == LAST => self.finish(app),
            (Key::Hash, Short) if !single => self.go(app, LAST), // SETUP-4: the rest keep their defaults
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        self.turn(app, d);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let (page, ask) = (self.page, self.ask());
        let single = self.flags & SINGLE != 0;
        app.new_page(lcd, Mix::new().h(&page).get());
        // Top bar: "Setup 2 of 8" and eight dashes, the pages so far in blue.
        if app.dirty(R_LINE, Mix::new().h(&page).h(&single).get()) {
            ferrite_ui::canvas::banded(lcd, 0, 0, 240, 19, BG, |lcd| {
                let x = 8 + lcd.text4(8, 14, b"Setup", &UI_12B, TEXT, BG);
                if !single {
                    let mut t = Buf::<12>::new();
                    let _ = uwrite!(t, " {} of {}", page + 1, PAGES.len());
                    lcd.text4(x, 14, t.as_bytes(), &UI_12, SUB, BG);
                    for i in 0..8 {
                        lcd.fill_r(154 + 10 * i, 8, 7, 3, 1, if i <= page as i32 { BLUE } else { LINE });
                    }
                }
                lcd.fill(0, 19, 240, 1, LINE);
            });
        }
        // Heading and text.
        let (head, text, _) = PAGES[page as usize];
        if app.dirty(R_TOP, Mix::new().h(&page).get()) {
            w::field(lcd, 10, 220, 24, 50, 45, head.as_bytes(), const { &w::st(&UI_15B, TEXT, BG, Align::Left) });
            let text = crate::text::load(text); // UPD-22: read when the page is drawn
            if !text.is_empty() {
                w::wrap(lcd, 10, 220, 50, if ask == Ask::Nothing { 200 } else { 94 }, 63, 15, text.as_bytes(), &UI_12, SUB, BG);
            }
        }
        match ask {
            Ask::Nothing => {}
            Ask::Callsign => {
                let call = app.set.callsign;
                let mode = self.flags & DIGITS;
                if app.dirty(R_ROW, Mix::new().h(&call).h(&mode).get()) {
                    text_box(lcd, 120, 46, b"Callsign", call.as_bytes(), true, true);
                    let long = call.as_bytes().len() > super::transmit::AX25_CALL;
                    let tip: &[u8] = if long {
                        b"over 6 characters: no APRS" // SETUP-13: CW ID sends it whole, AX.25 can't
                    } else if mode != 0 {
                        "digits \u{b7} # letters".as_bytes()
                    } else {
                        "digits type \u{b7} # letters/digits".as_bytes()
                    };
                    w::field(lcd, 10, 220, 170, 186, 182, tip, &w::st(&UI_12, if long { AMBER } else { SUB }, BG, Align::Left));
                    w::key_line(lcd, 10, 230, 200, "\x1b", "delete", BG);
                }
            }
            Ask::Region => {
                let (region, ext, row) = (app.set.region, app.set.extend_tx, self.row);
                for i in 0..REGION_ROWS {
                    let r = crate::settings::region_at(i);
                    let name = REGION_LIST[r as usize - 1];
                    let note = if region == r { "chosen" } else { "" };
                    let focused = row == i;
                    if app.dirty(R_ROW + i as usize, Mix::new().h(&focused).h(&(region == r)).get()) {
                        choice(lcd, 94 + 19 * i as i32, 17, name.as_bytes(), note.as_bytes(), focused); // 17: odd, DISP-20
                    }
                }
                if app.dirty(R_ROW + REGION_ROWS as usize, Mix::new().h(&ext).h(&(row == REGION_ROWS)).get()) {
                    lcd.fill(10, 246, 220, 1, LINE);
                    let sel = row == REGION_ROWS;
                    let bg = if sel { FOCUS } else { BG };
                    ferrite_ui::canvas::banded(lcd, 0, 247, 240, 23, BG, |lcd| {
                        lcd.fill_r(4, 248, 232, 21, 1, bg);
                        w::field(lcd, 10, 120, 249, 267, 262, b"Extend TX", &w::st(&UI_12, if sel { BLUE } else { TEXT }, bg, Align::Left));
                        w::field(lcd, 130, 100, 249, 267, 262, ON_OFF[ext as usize].as_bytes(), &w::st(&UI_12, SUB, bg, Align::Right));
                    });
                    w::wrap(lcd, 10, 220, 270, 300, 283, 15, crate::text::load(txt!("On: transmit outside the amateur bands too. Warns first.")).as_bytes(), &UI_12, SUB, BG);
                }
            }
            Ask::Owner => {
                for i in 0..3 {
                    let v = app.set.owner[i];
                    let f = self.row as usize == i;
                    if app.dirty(R_ROW + i, Mix::new().h(&v).h(&f).get()) {
                        text_box(lcd, 112 + 42 * i as i32, 38, OWNER[i].as_bytes(), v.as_bytes(), f, false);
                    }
                }
                let mode = self.flags & (LOWER | DIGITS);
                if app.dirty(R_ROW + 3, Mix::new().h(&mode).get()) {
                    let m = ["ABC", "abc", "123"][(mode / 4) as usize];
                    let mut t = Buf::<48>::new();
                    let _ = uwrite!(t, "{} \u{b7} # changes", m);
                    w::field(lcd, 10, 220, 240, 258, 254, t.as_bytes(), const { &w::st(&UI_12, SUB, BG, Align::Left) });
                    w::key_line(lcd, 10, 230, 272, "\x1b", "delete", BG);
                }
            }
            Ask::Time => {
                let s = *app.set;
                let mut tz = Buf::<16>::new();
                s.tz_text(&mut tz);
                if cx.dev.gps == ferrite_core::msg::GpsState::Fix {
                    tz.push(b", GPS");
                }
                let vals: [&[u8]; 3] = [tz.as_bytes(), CLOCKS[s.clock as usize % 2].as_bytes(), UNITS[s.units as usize % 3].as_bytes()];
                for (i, (name, v)) in ["Time zone", "Clock", "Units"].iter().zip(vals).enumerate() {
                    let f = self.row as usize == i;
                    if app.dirty(R_ROW + i, Mix::new().h(&v).h(&f).get()) {
                        choice(lcd, 112 + 27 * i as i32, 23, name.as_bytes(), v, f);
                    }
                }
            }
            Ask::Look => {
                let (layout, theme, row) = (app.set.layout, app.set.theme, self.row);
                if app.dirty(R_ROW, Mix::new().h(&layout).h(&(row == 0)).get()) {
                    for i in 0..3 {
                        preview(lcd, 10 + 75 * i, 100, i as u8, layout == i as u8, row == 0);
                    }
                    w::field(lcd, 10, 220, 196, 212, 208, LAYOUTS[layout as usize % 3].as_bytes(), const { &w::st(&UI_12, TEXT, BG, Align::Center) });
                }
                if app.dirty(R_ROW + 1, Mix::new().h(&theme).h(&(row == 1)).get()) {
                    choice(lcd, 222, 23, b"Theme", THEMES[theme as usize % THEMES.len()].as_bytes(), row == 1);
                }
            }
            Ask::BatterySave => {
                let b = app.bsave;
                for (i, (name, on, _)) in SAVE.iter().enumerate() {
                    if app.dirty(R_ROW + i, Mix::new().h(&(b as usize == i)).h(&(self.row as usize == i)).get()) {
                        choice(lcd, 124 + 27 * i as i32, 23, name.as_bytes(), on.as_bytes(), b as usize == i);
                    }
                }
                if app.dirty(R_ROW + 4, Mix::new().h(&b).get()) {
                    w::wrap(lcd, 10, 220, 236, 280, 250, 14, crate::text::load(SAVE[b as usize % 4].2).as_bytes(), &UI_12, TEXT, BG);
                }
            }
            Ask::Summary => {
                let n = cx.store.count();
                let vals = summary(app, n);
                for (i, (l, v)) in SUMMARY.iter().zip(&vals).enumerate() {
                    let c = if i == 6 && n == 0 { SUB } else { TEXT };
                    app.kv(lcd, i, 53 + 24 * i as i32, 24, &w::KV_STATUS, l.as_bytes(), v.as_bytes(), c, false);
                }
                let no_call = app.set.callsign.is_empty();
                if app.dirty(R_HELP, Mix::new().h(&no_call).get()) {
                    w::wrap(lcd, 10, 220, 222, 299, 236, 15, summary_note(app).as_bytes(), &UI_12, SUB, BG);
                }
            }
        }
    }

    fn hints(self, _: &App) -> Hints {
        if self.flags & SINGLE != 0 {
            Hints::new(&[("OK", "Done"), ("BACK", "Back")])
        } else if self.page == LAST {
            Hints::new(&[("OK", "Start"), ("BACK", "Previous")])
        } else if matches!(self.ask(), Ask::Callsign | Ask::Owner) {
            // KEY-12: where you type, # is the letters' mode, not Skip
            Hints::new(&[("OK", "Next"), ("BACK", "Previous"), ("#", "Mode")])
        } else {
            Hints::new(&[("OK", "Next"), ("BACK", "Previous"), ("#", "Skip")])
        }
    }
}

/// The summary's rows (SETUP-10).
const SUMMARY: [&str; 7] = ["Callsign", "Region", "Extend TX", "Time zone", "Home", "Battery Save", "Channels"];

/// The summary's values and its note (SETUP-10: the transmit state, no channels yet and how to add them), with
/// `n` channels stored.
fn summary(app: &App, n: u16) -> [Buf<32>; 7] {
    let s = *app.set;
    let mut v = [Buf::<32>::new(); 7];
    v[0].set(if s.callsign.is_empty() { b"none".as_slice() } else { s.callsign.as_bytes() });
    v[1].set(s.region_name().as_bytes());
    v[2].set(ON_OFF[s.extend_tx as usize].as_bytes());
    let mut tz = Buf::<16>::new();
    s.tz_text(&mut tz);
    v[3].set(tz.as_bytes());
    if s.clock_gps {
        v[3].push(b", from GPS");
    }
    let _ = uwrite!(v[4], "{} \u{b7} {}", LAYOUTS[s.layout as usize % 3], THEMES[s.theme as usize % THEMES.len()]);
    let mut bs = Buf::<16>::new();
    app.item_text(Item::BatterySave, true, &mut bs);
    v[5].set(bs.as_bytes());
    if n == 0 {
        v[6].set(b"none yet");
    } else {
        let _ = uwrite!(v[6], "{}", n);
    }
    v
}

/// The summary's note (from the text pack, UPD-22: read when drawn).
fn summary_note(app: &App) -> Buf<160> {
    let mut note = Buf::<160>::new();
    if app.set.callsign.is_empty() {
        note.push(crate::text::load(txt!("No callsign: automatic transmissions stay off. ")).as_bytes());
    }
    note.push(crate::text::load(txt!("Add channels with hold 9 on a VFO, or load them with the web app. Run setup again: MENU 0 9 7 1.")).as_bytes());
    note
}

/// A row of a setup page for the readback: name, value, focused.
pub(crate) type RowOut<'a> = &'a mut dyn FnMut(&[u8], &[u8], bool);

impl Setup {
    /// What the page shows, for the UI readback (LINK-19): its heading, then each choice or field as a row
    /// (name, value, focused) through `row`, and the summary's note; `n` channels stored. Returns the page (1-8),
    /// how many pages, the heading and the note.
    pub(crate) fn shown(self, app: &App, n: u16, row: RowOut) -> (u8, u8, &'static str, Buf<160>) {
        let mut note = Buf::<160>::new();
        match self.ask() {
            Ask::Nothing => {}
            Ask::Callsign => row(b"Callsign", app.set.callsign.as_bytes(), true),
            Ask::Region => {
                for i in 0..REGION_ROWS {
                    let r = crate::settings::region_at(i);
                    row(REGION_LIST[r as usize - 1].as_bytes(), if app.set.region == r { b"chosen" } else { b"" }, self.row == i);
                }
                row(b"Extend TX", ON_OFF[app.set.extend_tx as usize].as_bytes(), self.row == REGION_ROWS);
            }
            Ask::Owner => {
                for (i, name) in OWNER.iter().enumerate() {
                    row(name.as_bytes(), app.set.owner[i].as_bytes(), self.row as usize == i);
                }
            }
            Ask::Time => {
                let st = *app.set;
                let mut tz = Buf::<16>::new();
                st.tz_text(&mut tz);
                let vals: [&[u8]; 3] = [tz.as_bytes(), CLOCKS[st.clock as usize % 2].as_bytes(), UNITS[st.units as usize % 3].as_bytes()];
                for (i, (name, v)) in ["Time zone", "Clock", "Units"].iter().zip(vals).enumerate() {
                    row(name.as_bytes(), v, self.row as usize == i);
                }
            }
            Ask::Look => {
                row(b"Home", LAYOUTS[app.set.layout as usize % 3].as_bytes(), self.row == 0);
                row(b"Theme", THEMES[app.set.theme as usize % THEMES.len()].as_bytes(), self.row == 1);
            }
            Ask::BatterySave => {
                for (i, (name, on, _)) in SAVE.iter().enumerate() {
                    row(name.as_bytes(), if app.bsave as usize == i { b"chosen" } else { on.as_bytes() }, self.row as usize == i);
                }
            }
            Ask::Summary => {
                let vals = summary(app, n);
                for (l, v) in SUMMARY.iter().zip(&vals) {
                    row(l.as_bytes(), v.as_bytes(), false);
                }
                note = summary_note(app);
            }
        }
        (self.page + 1, PAGES.len() as u8, PAGES[self.page as usize].0, note)
    }
}

/// A choice in a rounded box: name at the left, a note at the right; the chosen one blue-edged (SetupRegion).
fn choice(lcd: &mut impl Panel, y: i32, h: i32, name: &[u8], note: &[u8], sel: bool) {
    let (bg, edge) = if sel { (FOCUS, BLUE) } else { (BG, LINE) };
    ferrite_ui::canvas::banded(lcd, 0, y - 2, 240, h + 4, BG, |lcd| {
        lcd.fill_r(10, y, 220, h, 1, bg);
        lcd.frame_r(10, y, 220, h, 1, edge);
        let base = y + h / 2 + 5;
        w::field(lcd, 17, 120, y + 2, y + h - 2, base, name, &w::st(&UI_12, TEXT, bg, Align::Left));
        w::field(lcd, 137, 86, y + 2, y + h - 2, base, note, &w::st(&UI_12, SUB, bg, Align::Right));
    });
}

/// A text field in a box: label above, the text with a cursor when focused (SetupCall); `big` for the callsign.
fn text_box(lcd: &mut impl Panel, y: i32, h: i32, label: &[u8], text: &[u8], focused: bool, big: bool) {
    let edge = if focused { BLUE } else { LINE };
    ferrite_ui::canvas::banded(lcd, 0, y - 1, 240, h + 2, BG, |lcd| {
        lcd.frame_r(10, y, 220, h, 2, edge);
        lcd.text4_rows(18, y + 14, label, &UI_12, SUB, BG, y + 1, y + 18);
        let (f, base) = if big { (&UI_15B, y + 37) } else { (&UI_12, y + 30) };
        let tw = lcd.text4_rows(18, base, w::fit(text, f, 196), f, TEXT, BG, y + 18, y + h - 1);
        if focused {
            let ch = if big { 16 } else { 13 };
            lcd.fill(19 + tw, base - ch + 2, if big { 6 } else { 5 }, ch, TEXT);
        }
    });
}

/// A small picture of a home layout (0 watch, 1 single + scope, 2 watch + activity), 68 x 90.
fn preview(lcd: &mut impl Panel, x: i32, y: i32, which: u8, chosen: bool, focus: bool) {
    let edge = if chosen { if focus { BLUE } else { SUB } } else { LINE };
    ferrite_ui::canvas::banded(lcd, x, y, 68, 90, BG, |lcd| {
        lcd.frame_r(x, y, 68, 90, 2, edge);
        let cards: &[(i32, i32)] = match which {
            0 => &[(6, 36), (45, 18), (66, 18)],
            1 => &[(6, 36)],
            _ => &[(6, 36), (45, 18)],
        };
        for &(dy, h) in cards {
            lcd.fill(x + 5, y + dy, 58, h, SURFACE);
        }
        if which == 1 {
            // the scope's bars
            for (i, h) in [6, 10, 4, 18, 30, 12, 5, 8, 22, 6, 4, 9].into_iter().enumerate() {
                lcd.fill(x + 7 + 5 * i as i32, y + 84 - h, 3, h, BLUE);
            }
        } else if which == 2 {
            // the activity lines
            for i in 0..3 {
                lcd.fill(x + 7, y + 70 + 6 * i, 40 - 8 * i, 2, SUB);
            }
        }
        // the active card's frequency bar
        lcd.fill(x + 9, y + 14, 34, 6, if chosen { TEXT } else { SUB });
    });
}

/// The Extend TX warning (A19, SETUP-8, TX-2), shown once before Extend TX turns on. OK turns it on, BACK keeps
/// it off; both return where it was asked from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExtendTx;

/// The warning's lines: text and colour.
const WARNING: [(&str, u16); 4] = [
    ("Transmit wherever the radio can, not only on your region's amateur bands.", TEXT),
    ("This may be illegal where you are, and it can interfere with services people rely on.", AMBER),
    ("You are responsible for every transmission.", TEXT),
    ("Emergency frequencies still ask for a second PTT.", SUB),
];

impl View for ExtendTx {
    fn name(self) -> &'static str {
        "Extend TX"
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        match (e.key, e.press) {
            (Key::Menu, Press::Short) => {
                (app.set.extend_tx, app.set.extend_warned) = (true, true);
                app.back();
                app.show_toast(b"Extend TX", b"on", false);
            }
            (Key::Exit, Press::Short) => app.back(),
            _ => {}
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, _: &Cx<impl ChannelStore>) {
        if app.dirty(R_TOP, 1) {
            w::warn_icon(lcd, 10, 30, AMBER, BG);
            w::field(lcd, 30, 200, 24, 48, 41, b"Extend TX", const { &w::st(&UI_15B, AMBER, BG, Align::Left) });
            let mut y = 60;
            for (t, c) in WARNING {
                let n = w::wrap(lcd, 10, 220, y, y + 60, y + 14, 15, t.as_bytes(), &UI_12, c, BG);
                y += 15 * n + 10;
            }
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[("OK", "Turn on"), ("BACK", "Keep off")])
    }
}
