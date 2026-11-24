//! The tone picker (OK on MENU 4 or on the tone cell; manual chapters 16 and 11, TONE-2, TONE-2.1, MENU-17): the
//! RX and TX tones apart, each a mode (Off, CTCSS, DCS-N, DCS-I) and a value in it, so every cross mode is just a
//! pair. ▲▼ pick RX or TX; ◀▶ change the mode; the knob steps the value within the mode, live; digits type a
//! tone (885 is 88.5 Hz, 1000 is 100.0 Hz) or, on a DCS code, the code in octal; # turns it off; OK keeps;
//! BACK puts both back (MENU-20). Typed tones need not be on the lists (TONE-15, as nicFW): any CTCSS from
//! 60.0 to 260.0 Hz and any DCS code D000-D777.
//!
//! On a channel, ▲▼ go on to a second page: the channel's end tone and tail elimination (TONE-10, TONE-17;
//! decision 86), each Global (the radio's, 0 8 8) or its own. The channel editor's grid has no room for them.

use super::forms::picker;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::{ChannelStore, END_TONES, TAILS};
use crate::keys::{KeyEvent, Press};
use crate::slot::{tone_at, tone_custom, tone_index, tone_text, CTCSS, DCS_FLAG, DCS_INV};
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_core::msg::{Param, RadioCmd};
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Tones {
    /// 0 RX, 1 TX.
    pub row: u8,
    /// Both tones when it opened, and a channel's end tone and tail byte (BACK restores them).
    pub old: (u16, u16),
    pub old_ch: u8,
    /// Digits typed so far and when the last came (a pause of [`TYPE_MS`] starts again).
    pub typed: u16,
    pub nt: u8,
    pub at: u32,
}

const TYPE_MS: u32 = 1500;

/// The tone modes, in ◀▶'s order (TONE-2.1): stock's and nicFW's names, CTCSS and DCS normal or inverted.
pub const MODE_NAMES: [&str; 4] = ["Off", "CTCSS", "DCS-N", "DCS-I"];

/// A tone word's mode: 0 off, 1 CTCSS, 2 DCS normal, 3 DCS inverted.
pub fn mode_of(t: u16) -> u8 {
    match t {
        0 => 0,
        _ if t & DCS_FLAG == 0 => 1,
        _ if t & DCS_INV == 0 => 2,
        _ => 3,
    }
}

/// The tone after a change to mode `m` (TONE-2.1): no value is remembered per mode, so a new mode starts at its
/// first value (67.0 Hz, D023), except DCS-N and DCS-I, which keep the code and flip its polarity.
pub fn with_mode(t: u16, m: u8) -> u16 {
    match (mode_of(t), m) {
        (a, b) if a == b => t,
        (2 | 3, 2) => t & !DCS_INV,
        (2 | 3, 3) => t | DCS_INV,
        (_, 1) => CTCSS[0],
        (_, 2) => DCS_FLAG | crate::slot::DCS[0],
        (_, 3) => DCS_FLAG | DCS_INV | crate::slot::DCS[0],
        _ => 0,
    }
}

/// Where a mode's values start in [`tone_at`]'s order, and how many it has: the knob wraps within them.
fn mode_range(m: u8) -> (i32, i32) {
    match m {
        1 => (1, 50),
        2 => (51, 104),
        3 => (155, 104),
        _ => (0, 1),
    }
}

/// `t` stepped `d` values within its own mode (TONE-2.1); off stays off. A custom tone (TONE-15) sits after the
/// standard one below it: down steps to that one, not past it.
pub fn step_in_mode(t: u16, d: i32) -> u16 {
    let (lo, n) = mode_range(mode_of(t));
    if t == 0 {
        return 0;
    }
    let i = tone_index(t) + if tone_custom(t) && d < 0 { 1 } else { 0 };
    tone_at(lo + (i - lo + d).rem_euclid(n))
}

impl Tones {
    pub fn new(app: &App) -> Self {
        let sl = &app.slots[app.active as usize];
        let c = sl.cfg();
        let old_ch = sl.ch.filter(|_| sl.on_channel()).map_or(0, |c| c.tones);
        Tones { row: 1, old: (c.rx_tone, c.tx_tone), old_ch, typed: 0, nt: 0, at: 0 }
    }
}

/// The active slot's channel, when it is on one (the second page's).
fn channel(app: &App) -> Option<crate::chan::Channel> {
    let sl = &app.slots[app.active as usize];
    sl.ch.filter(|_| sl.on_channel())
}

/// Rows: RX and TX tone, and on a channel its end tone and tail.
fn rows(app: &App) -> u8 {
    if channel(app).is_some() { 4 } else { 2 }
}

/// The second page's value (row 2 end tone, row 3 tail) and how many there are.
fn ch_value(app: &App, row: u8) -> (u8, usize) {
    let c = channel(app).unwrap_or(crate::chan::channel(0, 0, 0, "", crate::slot::DEFAULT_CFG));
    if row == 2 { (c.end_tone(), END_TONES.len()) } else { (c.tail(), TAILS.len()) }
}

fn set_ch(app: &mut App, row: u8, v: u8) {
    super::chedit::edit(app, |c| if row == 2 { c.set_end_tone(v) } else { c.set_tail(v) });
    app.send_slot_tones(); // the Radio task sends what this channel now says (TONE-7, TONE-10)
}

fn get(app: &App, rx: bool) -> u16 {
    let c = app.slots[app.active as usize].cfg();
    if rx { c.rx_tone } else { c.tx_tone }
}

/// Sets one tone on the active slot, live (MENU-19), kept for undo and saved on a channel (EDIT-4).
pub(crate) fn set(app: &mut App, rx: bool, t: u16) {
    if get(app, rx) == t {
        return;
    }
    app.keep_undo();
    let a = app.active;
    let c = app.slots[a as usize].cfg_mut();
    if rx {
        c.rx_tone = t;
    } else {
        c.tx_tone = t;
    }
    let param = if rx { Param::RxTone } else { Param::TxTone };
    app.out.push(RadioCmd::SlotParam { slot: a, param, value: t as i32 });
    app.save_channel(a, None);
}

/// "CTCSS", "DCS normal", "DCS inverted", "Off"; "Custom ..." off the lists (TONE-15).
fn kind(t: u16) -> &'static str {
    match t {
        0 => "",
        _ if t & DCS_FLAG == 0 && tone_custom(t) => "CTCSS custom",
        _ if t & DCS_FLAG == 0 => "CTCSS",
        _ if tone_custom(t) => "DCS custom",
        _ if t & DCS_INV == 0 => "DCS normal",
        _ => "DCS inverted",
    }
}

fn text(t: u16, b: &mut Buf<16>) {
    tone_text(t, b);
    if b.is_empty() {
        b.set(b"Off");
    }
}

/// The lowest and highest CTCSS a typed tone may be, 0.1 Hz (TONE-15).
pub const CTCSS_MIN: u16 = 600;
pub const CTCSS_MAX: u16 = 2600;

/// Typed digits to a tone: CTCSS in tenths ("885", "1000") or whole hertz ("100"), a standard tone first, then
/// any tone from 60.0 to 260.0 Hz (TONE-15); on a DCS code, its octal digits, any code D000-D777.
pub fn typed_tone(v: u16, dcs: Option<u16>) -> Option<u16> {
    match dcs {
        Some(inv) => {
            let (mut code, mut x, mut k) = (0u16, v, 1u16);
            while x > 0 {
                if x % 10 > 7 || k > 64 {
                    return None;
                }
                code += (x % 10) * k;
                (x, k) = (x / 10, k * 8);
            }
            Some(DCS_FLAG | inv | code)
        }
        None => {
            let ok = |c: u16| (CTCSS_MIN..=CTCSS_MAX).contains(&c).then_some(c);
            let whole = v.checked_mul(10);
            CTCSS.iter().copied().find(|&c| c == v || Some(c) == whole).or_else(|| ok(v)).or_else(|| whole.and_then(ok))
        }
    }
}

impl View for Tones {
    fn name(self) -> &'static str {
        "Tone picker"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Tones");
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, st: &impl ChannelStore) {
        use Press::*;
        let mut s = self;
        let rx = self.row == 0;
        let n = rows(app);
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => {
                s.row = (s.row % n + if e.key == Key::Down { 1 } else { n - 1 }) % n;
                s.nt = 0;
            }
            (Key::Left | Key::Right, Short | Repeat) if self.row >= 2 => {
                return self.knob(app, if e.key == Key::Right { 1 } else { -1 }, st);
            }
            // TONE-2.1: ◀▶ change the mode, the value starts again in it
            (Key::Left | Key::Right, Short | Repeat) => {
                let t = get(app, rx);
                let m = (mode_of(t) as i32 + if e.key == Key::Right { 1 } else { -1 }).rem_euclid(4) as u8;
                set(app, rx, with_mode(t, m));
                s.nt = 0;
            }
            (Key::Hash, Short) if self.row >= 2 => set_ch(app, self.row, 0), // Global
            (Key::Digit(d), Short) if self.row < 2 => {
                // nt 0: a new number (▲▼ and ◀▶ start one; the old digits stayed and overflowed: found by fuzzing)
                if s.nt == 0 || s.nt >= 4 || app.now.wrapping_sub(s.at) > TYPE_MS {
                    (s.typed, s.nt) = (0, 0);
                }
                (s.typed, s.nt, s.at) = (s.typed * 10 + d as u16, s.nt + 1, app.now);
                let t = get(app, rx);
                let dcs = (t & DCS_FLAG != 0).then_some(t & DCS_INV);
                if let Some(t) = typed_tone(s.typed, dcs) {
                    set(app, rx, t);
                }
            }
            (Key::Hash, Short) => set(app, rx, 0), // MENU-18: off is the default
            (Key::Star, Short) => app.do_undo(),
            (Key::Menu, Short) => return app.back(), // MENU-20: OK keeps
            (Key::Exit, Short) => {
                set(app, true, self.old.0);
                set(app, false, self.old.1);
                if channel(app).is_some_and(|c| c.tones != self.old_ch) {
                    super::chedit::edit(app, |c| c.tones = self.old_ch);
                }
                app.undo = None;
                return app.back();
            }
            _ => return,
        }
        app.screen = Screen::Tones(s);
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        if self.row >= 2 {
            let (v, n) = ch_value(app, self.row);
            return set_ch(app, self.row, (v as i32 + d).rem_euclid(n as i32) as u8);
        }
        let rx = self.row == 0;
        set(app, rx, step_in_mode(get(app, rx), d)); // TONE-2.1: the value, within the mode
    }

    fn own_arrows(self) -> bool {
        true // TONE-2.1: ◀▶ change the mode (on the second page they are the knob)
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        app.editor_head(lcd, cx);
        let (rx, tx) = (get(app, true), get(app, false));
        let page = self.row / 2;
        if page == 1 {
            return self.render_ch(app, lcd, cx);
        }
        for (i, (label, t)) in [("RX tone", rx), ("TX tone", tx)].into_iter().enumerate() {
            let f = self.row as usize == i;
            if app.dirty(R_ROW + i, Mix::new().h(&page).h(&t).h(&f).get()) {
                let y = 46 + i as i32 * 36;
                let (bg, border) = if f { (FOCUS, BLUE) } else { (BG, LINE) };
                // 33 px: the 9 px capitals centre (DISP-19)
                ferrite_ui::canvas::banded(lcd, 6, y, 228, 33, BG, |lcd| {
                    lcd.fill_r(6, y, 228, 33, 2, bg);
                    let mut b = Buf::<16>::new();
                    text(t, &mut b);
                    w::field(lcd, 14, 70, y + 2, y + 30, y + 21, label.as_bytes(), &w::st(&UI_12, if f { BLUE } else { SUB }, bg, Align::Left));
                    // the value is 49 px at most ("T 123.4"); the kind takes the rest: "CTCSS custom" is 84 px
                    w::field(lcd, 84, 54, y + 2, y + 30, y + 21, b.as_bytes(), &w::st(&UI_12B, TEXT, bg, Align::Left));
                    w::field(lcd, 138, 88, y + 2, y + 30, y + 21, kind(t).as_bytes(), &w::st(&UI_12, SUB, bg, Align::Right));
                    if f {
                        w::knob_mark(lcd, 14 + text4_width(label.as_bytes(), &UI_12) + 4, y + 9, BLUE, bg);
                    }
                    lcd.frame_r(6, y, 228, 33, 2, border);
                });
            }
        }
        let t = if self.row == 0 { rx } else { tx };
        // TONE-2.1: the modes (◀▶) over the values in this one (the knob). Each drawn off screen, in one pass (DISP-25);
        // the value box reaches down to 212, where the second page's taller picker ends.
        let m = mode_of(t);
        if app.dirty(R_INFO, Mix::new().h(&page).h(&m).get()) {
            let names: [&[u8]; 4] = MODE_NAMES.map(|n| n.as_bytes());
            ferrite_ui::canvas::banded(lcd, 8, 122, 224, 17, BG, |lcd| w::chips_sel(lcd, 8, 122, 224, &names, Some(m as usize), BG));
        }
        if app.dirty(R_SUB, Mix::new().h(&page).h(&t).get()) {
            ferrite_ui::canvas::banded(lcd, 40, 140, 160, 72, BG, |lcd| {
                if m == 0 {
                    w::field(lcd, 40, 160, 146, 200, 177, b"No tone", const { &w::st(&UI_12, DIM, BG, Align::Center) });
                } else {
                    let (lo, n) = mode_range(m);
                    picker(lcd, 40, 146, 160, 3, 18, tone_index(t) - lo, n, |i, b| text(tone_at(lo + i), b), BG);
                }
            });
        }
        let mut typed = Buf::<32>::new();
        if self.nt > 0 && app.now.wrapping_sub(self.at) <= TYPE_MS {
            let _ = ufmt::uwrite!(typed, "Typed {}", self.typed);
        }
        if app.dirty(R_NOTE, Mix::new().h(&typed).get()) {
            let s: &[u8] = if typed.is_empty() { b"885 is 88.5 Hz; any 60-260 Hz" } else { typed.as_bytes() };
            w::field(lcd, 8, 224, 216, 233, 229, s, &w::st(&UI_12, if typed.is_empty() { DIM } else { TEXT }, BG, Align::Center));
        }
        let help = if rows(app) == 4 { txt!("RX and TX apart; further down, end tone and tail.") } else { txt!("RX and TX are set apart: any cross mode is a pair.") };
        app.help_txt(lcd, 250, 46, help, b"MENU 4");
    }

    fn hints(self, app: &App) -> Hints {
        let rows = if self.row < 2 && rows(app) == 2 { "RX/TX" } else { "Row" };
        if self.row < 2 {
            // TONE-2.1: ◀▶ Mode always shows on the tone rows; Undo takes OK's place once there is one
            Hints::new(&[(ARROWS_UD, rows), (ARROWS_LR, "Mode"), if app.undo.is_some() { ("*", "Undo") } else { ("OK", "Keep") }])
        } else if app.undo.is_some() {
            Hints::new(&[(ARROWS_UD, rows), ("*", "Undo"), ("OK", "Keep")])
        } else {
            Hints::new(&[(ARROWS_UD, rows), ("Knob", "Change"), ("OK", "Keep")])
        }
    }
}

impl Tones {
    /// The second page (on a channel): its end tone and tail elimination, Global or its own (decision 86).
    fn render_ch(self, app: &mut App, lcd: &mut impl Panel, _cx: &Cx<impl ChannelStore>) {
        for (i, row) in [2u8, 3].into_iter().enumerate() {
            let f = self.row == row;
            let (v, _) = ch_value(app, row);
            let label = if row == 2 { "End tone" } else { "Tail" };
            let name = if row == 2 { END_TONES[v as usize] } else { TAILS[v as usize] };
            if app.dirty(R_ROW + i, Mix::new().h(&1u8).h(&v).h(&f).get()) {
                let y = 46 + i as i32 * 36;
                let (bg, border) = if f { (FOCUS, BLUE) } else { (BG, LINE) };
                ferrite_ui::canvas::banded(lcd, 6, y, 228, 33, BG, |lcd| {
                    lcd.fill_r(6, y, 228, 33, 2, bg);
                    w::field(lcd, 14, 70, y + 2, y + 30, y + 21, label.as_bytes(), &w::st(&UI_12, if f { BLUE } else { SUB }, bg, Align::Left));
                    w::field(lcd, 84, 150, y + 2, y + 30, y + 21, name.as_bytes(), &w::st(&UI_12B, TEXT, bg, Align::Left));
                    if f {
                        w::knob_mark(lcd, 14 + text4_width(label.as_bytes(), &UI_12) + 4, y + 9, BLUE, bg);
                    }
                    lcd.frame_r(6, y, 228, 33, 2, border);
                });
            }
        }
        let (v, n) = ch_value(app, self.row);
        let end = self.row == 2;
        if app.dirty(R_SUB, Mix::new().h(&1u8).h(&self.row).h(&v).get()) {
            picker(lcd, 40, 122, 160, 5, 18, v as i32, n as i32, |i, b| b.set(if end { END_TONES[i as usize] } else { TAILS[i as usize] }.as_bytes()), BG);
        }
        if app.dirty(R_NOTE, Mix::new().h(&1u8).h(&end).get()) {
            // the radio's own setting for this row: End tone 0 8 4, Tail removal 0 8 8 2
            let note: &[u8] = if end { b"Global: the radio's, MENU 0 8 4" } else { b"Global: radio's, MENU 0 8 8 2" };
            w::field(lcd, 8, 224, 216, 233, 229, note, const { &w::st(&UI_12, DIM, BG, Align::Center) });
        }
        app.help_txt(lcd, 250, 46, txt!("This channel's end tone and tail removal."), b"MENU 4");
    }
}
