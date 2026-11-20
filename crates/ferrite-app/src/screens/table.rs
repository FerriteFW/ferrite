//! Label + value pages as data: the Status page, the system pages ([`super::info`]), link status, the GPS
//! position and an APRS station. A page is a [`Table`], a `const` (in the slow flash with every unnamed constant;
//! it is read once per redraw): each row a label and a function that writes the value and returns its colour.
//! One renderer ([`App::table`]) draws any table through the one label + value drawer ([`w::kv`]), and one key
//! handler ([`key`]) moves and opens the rows of a table that has a selection. The screen's state stays its row.
//!
//! Adding a row is one line in its table.

use super::nav::Dest;
use super::shared::*;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_core::msg::{Device, LiveAll};
use ferrite_ui::gfx::{Gfx, BG};
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets::{self as w, KvStyle};

/// What a row's value is written from: the App, the Radio task's snapshot, the radio-wide state, and the
/// page's own thing `x` (a GPS fix, a station; `()` for the radio's own pages).
pub(crate) struct Ctx<'a, C> {
    pub app: &'a App,
    pub live: &'a LiveAll,
    pub dev: &'a Device,
    pub x: &'a C,
}

/// A row's value: written into the buffer, its colour returned.
pub(crate) type Val<C> = fn(&Ctx<'_, C>, &mut Buf<40>) -> u16;

/// Where OK on a row goes.
#[derive(Clone, Copy)]
pub(crate) enum Go {
    /// Not yet: a toast says so.
    Later,
    Open(Dest),
    /// Quick settings at this row.
    Quick(u8),
}

/// A label + value page: its rows, the first row's top, the row height and the row style.
pub(crate) struct Table<C: 'static> {
    pub rows: &'static [(&'static str, Val<C>)],
    /// What OK does on each row; empty for a page without a selection.
    pub go: &'static [Go],
    pub y: i16,
    pub h: i16,
    pub style: KvStyle,
}

impl<C> Table<C> {
    pub(crate) const fn len(&self) -> u8 {
        self.rows.len() as u8
    }
}

/// Where a scrolling table's rows end (the hint bar starts below).
const TABLE_END: i32 = 298;

impl App {
    /// Label + value row `i` (region R_ROW + i) from y, drawn when what it shows changed. Returns where the next
    /// row starts.
    pub(crate) fn kv(&mut self, lcd: &mut impl Panel, i: usize, y: i32, h: i32, st: &KvStyle, label: &[u8], value: &[u8], c: u16, sel: bool) -> i32 {
        let h = w::kv_h(st, value, h);
        if self.dirty(R_ROW + i, Mix::new().h(&label).h(&value).h(&c).h(&sel).h(&y).get()) {
            w::kv(lcd, y, h, st, label, value, c, sel, self.cue());
        }
        y + h
    }

    /// Draws a table's rows, row `sel` selected; returns where the next row would start.
    pub(crate) fn table<C>(&mut self, lcd: &mut impl Panel, t: &Table<C>, live: &LiveAll, dev: &Device, x: &C, sel: Option<u8>) -> i32 {
        let mut b = Buf::<40>::new();
        let mut y = t.y as i32;
        // A table with a selection scrolls to keep it in view when its rows don't all fit above the hint bar.
        let n = t.rows.len();
        // (h 0: rows size themselves, and such a table doesn't scroll)
        let fit = if t.h == 0 { n } else { ((TABLE_END - t.y as i32) / t.h as i32).max(1) as usize };
        let top = match sel {
            Some(s) if n > fit => (s as usize + 1).saturating_sub(fit).min(n - fit),
            _ => 0,
        };
        let shown = if sel.is_some() { fit.min(n) } else { n };
        for (i, (label, f)) in t.rows.iter().enumerate().skip(top).take(shown) {
            b.clear();
            let c = f(&Ctx { app: self, live, dev, x }, &mut b);
            y = self.kv(lcd, i - top, y, t.h as i32, &t.style, label.as_bytes(), b.as_bytes(), c, sel == Some(i as u8));
        }
        // DISP-31: rows that end higher than last time (shorter values, another station) clear what the old ones left
        // below; only on the same screen (a new screen's regions are all forgotten, R_SCROLL among them).
        let was = self.drawn.table_end as i32;
        if self.drawn.hash[R_SCROLL] != 0 && was > y {
            lcd.fill(0, y, 240, was.min(TABLE_END) - y, BG);
        }
        self.drawn.table_end = y as i16;
        // DISP-24: rows above or below the page. Each row draws its own part of the cue (w::kv), so a row's
        // change doesn't send the whole cue again: no rows for the cue's hash.
        let more = shown < n;
        self.scroll_cue(lcd, (0, 0), t.y as i32, y - t.y as i32, more, top as u32, shown as u32, n as u32);
        y
    }
}

/// What a key did on a table with a selection.
pub(crate) enum Act {
    /// Moved to this row.
    Move(u8),
    /// OK: done (the row's [`Go`]).
    Opened,
    Back,
    None,
}

/// ▲▼ (and repeats) move with wrap, OK opens the row's [`Go`], BACK is the caller's.
pub(crate) fn key<C>(app: &mut App, t: &Table<C>, row: u8, e: KeyEvent) -> Act {
    use Press::*;
    let n = t.len();
    match (e.key, e.press) {
        (Key::Up, Short | Repeat) => Act::Move((row + n - 1) % n),
        (Key::Down, Short | Repeat) => Act::Move((row + 1) % n),
        (Key::Menu, Short) => {
            match t.go[row as usize] {
                Go::Open(d) => app.open(d),
                Go::Quick(row) => {
                    app.push_back(); // BACK returns to the table
                    app.screen = Screen::Quick(super::quick::Quick { row });
                }
                Go::Later => app.show_toast(t.rows[row as usize].0.as_bytes(), b"setting comes next", false),
            }
            Act::Opened
        }
        (Key::Exit, Short) => Act::Back,
        _ => Act::None,
    }
}

/// The knob over a table's rows, with wrap.
pub(crate) fn knob<C>(t: &Table<C>, row: u8, d: i32) -> u8 {
    (row as i32 + d).rem_euclid(t.len() as i32) as u8
}
