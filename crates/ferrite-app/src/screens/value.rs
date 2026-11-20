//! The value editor, "a bigger editor" (OK on a quick settings row or an edit grid cell; manual chapters 11
//! and 12, MENU-16 to MENU-22, SQL-7; MenuEdit mock-up). The setting's value large between ‹ ›, its default and
//! whose it is; squelch adds its ten levels and the live signal against the opening point, other settings
//! their list of values. The knob or ◀▶ change it live, digits type a squelch level, # is the default, OK
//! keeps, BACK puts the old value back (MENU-20), PTT keeps it and transmits. Under the box, the next quick
//! rows, as in the mock-up.

use super::forms::picker;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::slot::{band_near, Item, QUICK};
use crate::{App, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets::{self as w, Row};
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Value {
    pub item: Item,
    /// The value when it opened (BACK restores it).
    pub old: i32,
}

impl Value {
    pub fn new(item: Item, app: &App) -> Self {
        Value { item, old: cur(app, item).0 }
    }
}

/// The setting's value index on the active slot (or radio-wide), and how many values it has.
/// VFO-16: Split's TX frequency typed (entry, prefilled with today's); its OK or BACK returns. OK on Split opens it
/// from the value editor and the quick list alike (VFO-26: the toast says "OK sets the TX frequency").
pub(crate) fn split_entry(app: &mut App) {
    let tx = app.slot(app.active).cfg().offset.max(0) as ferrite_core::freq::Freq10;
    app.open(super::nav::Dest::Entry(0));
    app.screen = crate::Screen::Entry(super::entry::Entry::with_freq(super::entry::EntryFor::SplitTx, tx));
}

pub(crate) fn cur(app: &App, item: Item) -> (i32, i32) {
    match item {
        Item::Watch => (app.watch as i32, 3),
        Item::BatterySave => (app.bsave as i32, 4),
        _ => item.get(app.slots[app.active as usize].cfg()),
    }
}

/// Value i of the setting, as text (long form).
pub(crate) fn text_of(app: &App, item: Item, i: i32, b: &mut Buf<16>) {
    b.clear();
    match item {
        Item::Watch => b.set(WATCH_TEXT[i as usize % 3].as_bytes()),
        Item::BatterySave if i == 0 => b.set(b"Off"),
        Item::BatterySave => {
            let _ = uwrite!(b, "Level {}", i);
        }
        Item::Offset if i == 3 => b.set(b"Split"), // VFO-16: its TX frequency is typed (OK)
        _ => {
            let mut c = *app.slots[app.active as usize].cfg();
            let shift = crate::slot::band_shift(band_near(app.slots[app.active as usize].freq()), app.set.region());
            item.set(&mut c, i, shift);
            item.text(&c, true, b);
        }
    }
}

/// VFO-26.6: under the Offset editor's value, where that choice transmits ("TX 147.120").
fn tx_line(app: &App, i: i32, b: &mut Buf<24>) {
    let s = &app.slots[app.active as usize];
    let mut c = *s.cfg();
    let shift = crate::slot::band_shift(band_near(s.freq()), app.set.region());
    Item::Offset.set(&mut c, i, shift);
    b.set(b"TX ");
    b.push(w::freq3(c.tx_of(s.freq())).as_bytes());
}

/// VFO-4.1: what the Mode editor shows after the mode's name: its channel width, or what it is for.
pub(crate) fn mode_width(name: &[u8]) -> &'static str {
    match name {
        b"FM" => " \u{b7} 25 kHz",
        b"NFM" => " \u{b7} 12.5 kHz",
        b"WFM" => " \u{b7} broadcast",
        _ => "",
    }
}

const BOX_Y: i32 = 44;
const BOX_H: i32 = 162;

impl View for Value {
    fn name(self) -> &'static str {
        "Value editor"
    }

    fn mockup(self) -> Option<&'static str> {
        (self.item == Item::Squelch).then_some("MenuEdit.png")
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        let s = self.item.label().as_bytes();
        b.set(&s[..s.len().min(10)]);
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS) // MENU-28
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        use Press::*;
        let item = self.item;
        match (e.key, e.press) {
            (Key::Digit(d), Short) if item == Item::Squelch => app.change(item, d as i32 + 1 - cur(app, item).0), // MENU-17: level d (index d + 1)
            (Key::Hash, Short) => app.reset(item), // MENU-18
            (Key::Star, Short) => app.do_undo(),   // MENU-22
            // VFO-16: OK on Split sets its TX frequency (entry, prefilled); entry's OK or BACK returns here.
            (Key::Menu, Short) if item == Item::Offset && app.slot(app.active).cfg().split => split_entry(app),
            (Key::Menu, Short) => app.back(), // MENU-20: OK keeps
            (Key::Exit, Short) => {
                // MENU-20: BACK restores the old value
                let now = cur(app, item).0;
                if now != self.old {
                    app.change(item, self.old - now);
                    app.undo = None;
                }
                app.back();
            }
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        app.change(self.item, d);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        app.editor_head(lcd, cx);
        let item = self.item;
        let a = app.active;
        let (v, n) = cur(app, item);
        let qi = QUICK.iter().position(|&q| q == item);
        let mut vt = Buf::<16>::new();
        text_of(app, item, v, &mut vt);
        if item == Item::Mode {
            // VFO-4.1: the mode with its width, so FM and NFM read apart (the heading's default stays the name)
            vt.push(mode_width(vt.as_bytes()).as_bytes());
        }
        let grey = app.greyed(item);
        // The box's frame and heading: number, name, default and whose.
        let mut def = Buf::<32>::new();
        let mut dt = Buf::<16>::new();
        let d = match item {
            Item::Watch | Item::BatterySave => 1,
            _ => item.default_of(),
        };
        text_of(app, item, d, &mut dt);
        def.push(b"default ");
        def.push(dt.as_bytes());
        def.push(" \u{b7} ".as_bytes());
        def.push(match item {
            Item::Watch | Item::BatterySave => b"radio".as_slice(),
            _ if app.slots[a as usize].on_channel() => b"channel",
            _ => b"VFO",
        });
        if app.dirty(R_NOTE, Mix::new().any(&item).h(&def).get()) {
            ferrite_ui::canvas::banded(lcd, 0, 41, 240, BOX_H + 6, BG, |b| {
                b.frame_r(6, BOX_Y, 228, BOX_H, 3, BLUE);
                let mut h = Buf::<24>::new();
                if let Some(i) = qi {
                    let _ = uwrite!(h, "{} ", (i + 1) % 10);
                }
                h.push(item.name().as_bytes());
                let hw = w::field(b, 14, 120, BOX_Y + 6, BOX_Y + 24, BOX_Y + 19, h.as_bytes(), const { &w::st(&UI_12B, TEXT, BG, Align::Left) });
                // "default " goes when the whole doesn't fit beside the name ("5 Offset" with "default Simplex ·
                // channel" was 175 px in 148): the value and whose still read as the default there
                let d = def.as_bytes();
                let d = if text4_width(d, &UI_12) > 204 - hw { d.strip_prefix(b"default ".as_slice()).unwrap_or(d) } else { d };
                w::field(b, 22 + hw, 204 - hw, BOX_Y + 6, BOX_Y + 24, BOX_Y + 19, d, const { &w::st(&UI_12, SUB, BG, Align::Right) });
            });
            for r in [R_FREQ, R_SUB, R_INFO] {
                app.drawn.hash[r] = 0; // inside the box just cleared
            }
        }
        // The value, large, between the arrows; greyed with why (MENU-14).
        let mut txl = Buf::<24>::new();
        if item == Item::Offset {
            tx_line(app, v, &mut txl);
        }
        if app.dirty(R_FREQ, Mix::new().h(&vt).h(&grey).h(&txl).get()) {
            let (y0, y1) = (BOX_Y + 28, BOX_Y + 70);
            // Squelch's level bars start at BOX_Y + 68: the band stops above them, as they draw only when they change.
            let bot = if item == Item::Squelch { BOX_Y + 68 } else { y1 };
            ferrite_ui::canvas::banded(lcd, 12, y0, 216, bot - y0, BG, |b| {
                let col = if grey.is_some() { DIM } else { TEXT };
                let numeric = vt.as_bytes().iter().all(|c| c.is_ascii_digit());
                let tw = if numeric {
                    let tw = text4_width(vt.as_bytes(), &DIG_46);
                    b.text4_rows(120 - tw / 2, y1 - 4, vt.as_bytes(), &DIG_46, col, BG, y0, y1);
                    tw
                } else {
                    w::field(b, 40, 160, y0 + 10, y0 + 30, y0 + 25, vt.as_bytes(), &w::st(&UI_12B, col, BG, Align::Center));
                    text4_width(vt.as_bytes(), &UI_12B)
                };
                if !txl.is_empty() {
                    w::field(b, 40, 160, y0 + 30, y0 + 42, y0 + 40, txl.as_bytes(), const { &w::st(&UI_12, SUB, BG, Align::Center) });
                }
                let ax = (tw / 2 + 22).min(96);
                b.tri(120 - ax - 10, y0 + 14, 12, 2, SUB);
                b.tri(120 + ax, y0 + 14, 12, 3, SUB);
            });
        }
        if item == Item::Squelch {
            let l = app.sq_level(a) as i32; // Global shows the slot's level it uses
            self.squelch(app, lcd, cx, l);
        } else if app.dirty(R_SUB, Mix::new().h(&v).h(&n).h(&grey).get()) {
            let y = BOX_Y + 74;
            match grey {
                Some(why) => {
                    ferrite_ui::canvas::banded(lcd, 12, y, 216, 82, BG, |b| {
                        w::field(b, 14, 212, y + 4, y + 21, y + 17, why.as_bytes(), const { &w::st(&UI_12, AMBER, BG, Align::Center) });
                    });
                }
                None => picker(lcd, 40, y, 160, 5, 16, v, n, |i, b| text_of(app, item, i, b), BG),
            }
        }
        // The next quick rows, under the box (the mock-up's 2-5).
        let start = qi.map_or(0, |i| i + 1);
        for k in 0..4 {
            let i = (start + k) % 9;
            let it = QUICK[i];
            let mut val = Buf::<16>::new();
            app.item_text(it, true, &mut val);
            let r = Row {
                num: Some(((i + 1) % 10) as u8),
                name: it.name().as_bytes(),
                value: val.as_bytes(),
                dot: !app.is_default(it),
                selected: false,
                grey: app.greyed(it).is_some(),
            };
            app.list_row(lcd, k, 210 + 22 * k as i32, 22, &r);
        }
    }

    fn hints(self, app: &App) -> Hints {
        // Four pairs don't fit at 12 px (see edit.rs); # Default still works.
        if app.undo.is_some() && cur(app, self.item).0 == self.old {
            Hints::new(&[("Knob", "Change"), ("*", "Undo"), ("BACK", "Back")])
        } else {
            Hints::new(&[("Knob", "Change"), ("OK", "Keep"), ("BACK", "Undo")])
        }
    }
}

impl Value {
    /// Squelch: the ten levels, and the signal now against where audio opens (SQL-7: live).
    fn squelch(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>, v: i32) {
        let y = BOX_Y + 68;
        if app.dirty(R_SUB, Mix::new().h(&v).get()) {
            ferrite_ui::canvas::banded(lcd, 11, y, 220, 41, BG, |b| {
                for i in 0..10 {
                    let x = 15 + i * 22;
                    b.fill_r(x, y, 14, 5, 1, if i <= v { BLUE } else { LINE });
                    w::field(b, x - 4, 22, y + 7, y + 22, y + 19, &[b'0' + i as u8], const { &w::st(&UI_12, SUB, BG, Align::Center) });
                }
                w::field(b, 15, 200, y + 23, y + 40, y + 36, b"Signal now", const { &w::st(&UI_12, SUB, BG, Align::Left) });
            });
        }
        let rssi = cx.live.slots[app.active as usize].rssi;
        // Where audio opens: the levels spread over the bar (the level table has no values yet: SQL-1).
        let open = 15 + 10 + v * 19;
        let level = (w::meter_level(rssi) as i32 * 210 / w::METER_SEGS as i32).clamp(0, 210);
        let st = w::s_text(rssi);
        if app.dirty(R_INFO, Mix::new().h(&level).h(&open).h(&st).get()) {
            let by = y + 42;
            ferrite_ui::canvas::banded(lcd, 12, by - 2, 216, 14, BG, |b| {
                b.fill_r(15, by, 210, 10, 1, LINE);
                if level > 3 {
                    b.fill_r(15, by, level, 10, 1, SUB);
                }
                b.fill(open, by - 2, 2, 14, BLUE);
            });
            let mut t = Buf::<48>::new();
            t.set(b"Noise ");
            t.push(st.as_bytes());
            t.push(" \u{b7} audio opens past the blue line, live".as_bytes());
            super::forms::para(lcd, 15, by + 13, 212, 2, 16, t.as_bytes(), &UI_12, SUB, BG);
        }
    }
}
