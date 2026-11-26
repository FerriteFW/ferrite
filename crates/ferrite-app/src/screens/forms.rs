//! Drawing pieces the entry and editing screens share (group A): wrapped paragraphs, the slot line at the top
//! of an editor, a labelled field box with a cursor, a picker column, progress dashes and the multi-tap
//! keypad. Each paints its whole box, background included, so it can be redrawn alone (HOME-7); callers gate
//! them with `app.dirty(region, hash)`. Candidates for the common display library.

use super::shared::*;
use super::Cx;
use crate::chan::ChannelStore;
use crate::slot::{band_name, chan_label, mode_chip};
use crate::App;
use ferrite_ui::assets::*;
use ferrite_ui::font4::Font4;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;

/// Text wrapped on words into `lines` lines of `lh` pixels from y (the box x..x+w, y..y+lines*lh, cleared to
/// bg). Returns the lines used.
pub(crate) fn para(lcd: &mut impl Panel, x: i32, y: i32, w_: i32, lines: i32, lh: i32, text: &[u8], f: &'static Font4, fg: u16, bg: u16) -> i32 {
    // in bands, so the old text never blanks before the new (DISP-25)
    let mut drawn = 0;
    ferrite_ui::canvas::banded(lcd, x, y, w_, lines * lh, bg, |lcd| {
        let (mut line, mut rest) = (0, text);
        while !rest.is_empty() && line < lines {
            let mut cut = rest.len();
            if let Some(nl) = rest.iter().position(|&c| c == b'\n') {
                cut = nl;
            }
            if text4_width(&rest[..cut], f) > w_ {
                let fits = |i: usize| text4_width(&rest[..i], f) <= w_;
                cut = (1..cut).rev().find(|&i| rest[i] == b' ' && fits(i)).unwrap_or_else(|| w::fit(rest, f, w_).len());
            }
            let base = y + line * lh + f.ascent as i32 + (lh - f.line_h as i32) / 2;
            lcd.text4_rows(x, base, &rest[..cut], f, fg, bg, y + line * lh, y + (line + 1) * lh);
            rest = &rest[cut..];
            rest = rest.strip_prefix(b" ").or_else(|| rest.strip_prefix(b"\n")).unwrap_or(rest);
            line += 1;
        }
        drawn = line;
    });
    drawn
}

impl App {
    /// The active slot at the top of an editor (MENU-13: whose setting this is): letter, VFO or channel and
    /// band, frequency, mode. Region R_TOP, rows 21-40.
    pub(crate) fn editor_head(&mut self, lcd: &mut impl Panel, _cx: &Cx<impl ChannelStore>) {
        let a = self.active;
        let sl = self.slots[a as usize];
        let mut head = Buf::<32>::new();
        if sl.on_channel() {
            chan_label(sl.ch.map_or(0, |c| c.num), &mut head);
        } else {
            head.push("VFO \u{b7} ".as_bytes());
            head.push(band_name(sl.freq()).as_bytes());
        }
        let mut m = Buf::<16>::new();
        mode_chip(sl.cfg(), &mut m);
        let f = sl.freq();
        if self.dirty(R_TOP, Mix::new().h(&a).h(&head).h(&m).h(&f).get()) {
            w::badge(lcd, 8, 23, a, true, BG);
            w::field(lcd, 22, 4, 21, 39, 35, b"", const { &w::st(&UI_12, SUB, BG, Align::Left) });
            // the frequency right after the head, whose width varies: "VFO · Airband" is 91 px (ferrite#144: a
            // fixed 66 cut it); "1300.000 00" still fits before the mode
            let fx = 26 + text4_width(head.as_bytes(), &UI_12).clamp(66, 94);
            w::field(lcd, 26, fx - 26, 21, 39, 35, head.as_bytes(), const { &w::st(&UI_12, SUB, BG, Align::Left) });
            let mut fb = [0u8; 16];
            let n = ferrite_core::freq::format(f, &mut fb);
            w::field(lcd, fx, 200 - fx, 21, 39, 35, &fb[..n], const { &w::st(&UI_12B, TEXT, BG, Align::Left) });
            w::field(lcd, 200, 32, 21, 39, 35, m.as_bytes(), const { &w::st(&UI_12, SUB, BG, Align::Right) });
            lcd.fill(0, 40, 240, 1, LINE);
        }
    }
}

/// A labelled box: small label, then the value in bold, a block cursor after it when typing; blue when
/// focused (the name and zone boxes of the channel editor, the callsign field). From 36 px tall the label
/// sits above the value (SetupCall), else beside it (HomeEditCh).
pub(crate) fn field_box(lcd: &mut impl Panel, x: i32, y: i32, w_: i32, h: i32, label: &[u8], value: &[u8], focused: bool, cursor: bool) {
    let (bg, border) = if focused { (FOCUS, BLUE) } else { (BG, LINE) };
    let lc = if focused { BLUE } else { SUB };
    ferrite_ui::canvas::banded(lcd, x, y, w_, h, BG, |lcd| {
        lcd.fill_r(x, y, w_, h, 2, bg);
        let two = h >= 36 && !label.is_empty();
        let (vx, vb) = if two {
            lcd.text4_rows(x + 8, y + 15, label, &UI_12, lc, bg, y + 2, y + 18);
            (x + 8, y + h - 10)
        } else if label.is_empty() {
            (x + 8, y + h / 2 + 5)
        } else {
            (x + 14 + lcd.text4_rows(x + 8, y + h / 2 + 5, label, &UI_12, lc, bg, y + 2, y + h - 2), y + h / 2 + 5)
        };
        let v = w::fit(value, &UI_12B, x + w_ - vx - if cursor { 14 } else { 4 });
        let vw = lcd.text4_rows(vx, vb, v, &UI_12B, TEXT, bg, vb - 15, vb + 4);
        if cursor {
            lcd.fill(vx + vw + 1, vb - 12, 7, 15, TEXT);
        }
        lcd.frame_r(x, y, w_, h, 2, border);
    });
}

/// A picker column: the value list around the current one, `rows` tall, the current in the middle and
/// filled blue (the bigger editor, the tone picker). `text(i, buf)` writes value i; `n` values, wrapping.
pub(crate) fn picker(lcd: &mut impl Panel, x: i32, y: i32, w_: i32, rows: i32, rh: i32, cur: i32, n: i32, text: impl Fn(i32, &mut Buf<16>), bg: u16) {
    let mid = rows / 2;
    let mut b = Buf::<16>::new();
    for r in 0..rows {
        let ry = y + r * rh;
        let k = r - mid;
        let show = n > rows || (cur + k >= 0 && cur + k < n);
        if !show {
            lcd.fill(x, ry, w_, rh, bg);
            continue;
        }
        text((cur + k).rem_euclid(n), &mut b);
        let base = ry + rh / 2 + 5;
        if k == 0 {
            ferrite_ui::canvas::banded(lcd, x, ry, w_, rh, bg, |lcd| {
                lcd.fill_r(x, ry, w_, rh, 1, BLUE);
                w::field(lcd, x + 8, w_ - 16, ry + 1, ry + rh - 1, base, b.as_bytes(), const { &w::st(&UI_12B, BLUE_FG, BLUE, Align::Center) });
            });
        } else {
            let c = if k.abs() == 1 { TEXT } else { DIM };
            w::field(lcd, x, w_, ry, ry + rh, base, b.as_bytes(), &w::st(&UI_12, c, bg, Align::Center));
        }
    }
}

/// The keypad legend for multi-tap: a 3 x 4 grid of keys, each with what it types (text entry, callsign).
/// `legend(k)` gives key k's characters (0-9); * and # get their labels.
pub(crate) fn keypad(lcd: &mut impl Panel, y: i32, legend: impl Fn(u8) -> &'static [u8], star: &[u8], hash_: &[u8]) {
    const ORDER: [u8; 12] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 0, 11];
    for (i, &k) in ORDER.iter().enumerate() {
        let (cx, cy) = (8 + (i as i32 % 3) * 76, y + (i as i32 / 3) * 24);
        ferrite_ui::canvas::banded(lcd, cx, cy, 72, 22, BG, |lcd| {
            lcd.frame_r(cx, cy, 72, 22, 1, LINE);
            let (key, text): (&[u8], &[u8]) = match k {
                10 => (b"*", star),
                11 => (b"#", hash_),
                d => (core::slice::from_ref(&DIGITS[d as usize]), legend(d)),
            };
            let kw = lcd.text4_rows(cx + 6, cy + 16, key, &UI_12B, TEXT, BG, cy + 1, cy + 21);
            let t = w::fit(text, &UI_12, 72 - 18 - kw);
            lcd.text4_rows(cx + 12 + kw, cy + 16, t, &UI_12, SUB, BG, cy + 1, cy + 21);
        });
    }
}

const DIGITS: [u8; 10] = *b"0123456789";
