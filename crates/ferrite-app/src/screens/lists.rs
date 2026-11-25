//! What the list and data screens (group C: event log, APRS, GPS, waypoints, channel browser, zones, scan
//! lists) share: walking a list read on demand, the list page (header, rows, empty text), two-line rows,
//! the confirm box, the compass, and the numbers they show (distance, bearing, age, position,
//! grid square), in integer maths (no float on the radio).
//!
//! Lists never copy their items: the state is a selected index and the top row's, items are read from the
//! store as rows are drawn, and the whole page is skipped while nothing it shows changed ([`App::gate`]).

use super::shared::*;
use crate::data::Pos;
use crate::App;
use core::hash::Hash;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// A region of their own: the gate (nothing read from the store while the page shows the same thing). The page
/// is [`App::page`].
pub(crate) const R_GATE: usize = 17;

/// The list header's line (count at the left, what the list is at the right), y 20-40.
pub(crate) const HEAD_Y: i32 = 20;
/// Rows start under the header's line.
pub(crate) const LIST_Y: i32 = 41;
/// Two-line rows: 32 px, eight on a page (the AprsHeard mock-up).
pub(crate) const ROW2_H: i32 = 32;
pub(crate) const ROWS2: u16 = 8;
/// One-line rows: 23 px; nine on a page above a help box, eleven without one.
pub(crate) const ROW1_H: i32 = 23;
pub(crate) const ROWS1: u16 = 9;
pub(crate) const ROWS1_FULL: u16 = 11;

impl App {
    /// Opens a screen that has no [`super::nav::Dest`] of its own (a screen's options, a page reached only
    /// from another), remembering this one for BACK as [`App::open`] does.
    pub(crate) fn open_screen(&mut self, s: crate::Screen) {
        self.push_back();
        self.screen = s;
        self.reconcile_mode(); // MODE-2
    }

    /// VFO-25: a slot still on its factory frequency (A 146.520, B 443.475: US frequencies) that the chosen region
    /// refuses moves to 145.500 or 433.500, in band in every region. Users who chose the UK in setup found every
    /// key-up refused (Nov 17, 2026). A frequency the user tuned is never touched.
    pub(crate) fn region_start(&mut self) {
        use ferrite_core::freq::mhz;
        let region = self.set.tx_region();
        for (i, (factory, start)) in [(mhz(146) + 52_000, mhz(145) + 50_000), (mhz(443) + 47_500, mhz(433) + 50_000)].into_iter().enumerate() {
            let s = &mut self.slots[i];
            if s.vfo_mode && s.vfo == factory && ferrite_core::txplan::check(factory, region, false).is_err() {
                s.vfo = start;
            }
        }
    }

    /// The region changed (setup, 0 9 4): a VFO on 2 m or 1.25 m still on a region default step takes the new
    /// region's (VFO-18), and a VFO in FM or NFM on an FM band the region's mode (SETUP-7). A step the user picked
    /// is left alone.
    pub(crate) fn region_steps(&mut self) {
        use crate::slot::{band_near, band_step, BAND_125M, BAND_VHF, BANDS, STEP_5K};
        self.send_si_opts(); // BCR-32: FM de-emphasis and band, MW spacing follow the region
        let region = self.set.region();
        for i in 0..self.slots.len() {
            let s = &mut self.slots[i];
            let was = s.cfg;
            let b = band_near(s.vfo);
            if matches!(b, BAND_VHF | BAND_125M) && (s.cfg.step == BANDS[b].step || s.cfg.step == STEP_5K) {
                s.cfg.step = band_step(b, region);
            }
            // SETUP-7: a VFO still on its band's FM default follows the region's (wide or NFM)
            if BANDS[b].mode == 0 && s.cfg.mode <= 1 {
                s.cfg.mode = crate::slot::band_mode(b, region);
                s.cfg.narrow = crate::slot::mode_narrow(s.cfg.mode);
                s.cfg.narrow = crate::slot::mode_narrow(s.cfg.mode);
            }
            if s.cfg != was && s.vfo_mode {
                self.send_slot(i as u8);
            }
        }
    }

    /// "Go" (INBOX-10, MENU-26): the active slot to a VFO on that frequency, then home.
    pub(crate) fn go_freq(&mut self, f: ferrite_core::freq::Freq10) {
        use crate::slot::{band_near, band_step};
        let a = self.active as usize;
        let region = self.set.region();
        let s = &mut self.slots[a];
        let b = band_near(f);
        if band_near(s.vfo) != b || !s.vfo_mode {
            s.cfg.mode = crate::slot::band_mode(b, region);
                s.cfg.narrow = crate::slot::mode_narrow(s.cfg.mode);
            s.cfg.step = band_step(b, region);
            (s.cfg.offset, s.cfg.split) = (0, false);
        }
        (s.vfo_mode, s.vfo, s.mem[b]) = (true, f, f);
        self.send_slot(a as u8);
        self.go_home();
        let mut t = Buf::<16>::new();
        mhz_text(f, &mut t);
        let mut h = Buf::<20>::new();
        let _ = uwrite!(h, "Go: {}", w::LETTERS[a] as char);
        self.show_toast(h.as_bytes(), t.as_bytes(), false);
    }

    /// False while the page shows exactly what `key` says (state, the store's version, the minute): the
    /// screen then reads nothing from the store and draws nothing.
    pub(crate) fn gate<K: Hash>(&mut self, key: K) -> bool {
        self.dirty(R_GATE, Mix::new().any(&key).get())
    }
}

/// A list's position: the selected index and the top row's, among `n` items of which `keep` says which show
/// (a filter or a view; `|_| true` for all). Indexes are the store's, so a filtered list needs no RAM.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Cursor {
    pub sel: u16,
    pub top: u16,
}

impl Cursor {
    /// The next shown index after (d > 0) or before `from`, if any.
    fn next(from: u16, d: i32, n: u16, keep: &impl Fn(u16) -> bool) -> Option<u16> {
        if d > 0 {
            (from + 1..n).find(|&i| keep(i))
        } else {
            (0..from.min(n)).rev().find(|&i| keep(i))
        }
    }

    /// The first shown index.
    pub fn first(n: u16, keep: &impl Fn(u16) -> bool) -> Cursor {
        let i = (0..n).find(|&i| keep(i)).unwrap_or(0);
        Cursor { sel: i, top: i }
    }

    /// Moves `d` shown items (the knob: any number; ▲▼: one), wrapping at the ends, and scrolls so the
    /// selection stays on a page of `vis` rows.
    /// Once round the list, the steps left are taken modulo the shown items it found: a fast turn over a sparse
    /// view (a band view on a full radio: a few channels among 4000 records) went round it again for each, a pass
    /// over the records each time (sim/tests/limits.rs, Nov 17, 2026).
    pub fn step(self, d: i32, n: u16, vis: u16, keep: &impl Fn(u16) -> bool) -> Cursor {
        let mut sel = self.sel;
        let mut left = d.unsigned_abs().min(n as u32);
        let mut taken = 0u32;
        while left > 0 {
            left -= 1;
            taken += 1;
            sel = match Self::next(sel, d, n, keep) {
                Some(i) => i,
                None if d > 0 => match (0..n).find(|&i| keep(i)) {
                    Some(i) => i,
                    None => return self,
                },
                None => match (0..n).rev().find(|&i| keep(i)) {
                    Some(i) => i,
                    None => return self,
                },
            };
            if sel == self.sel {
                left %= taken; // round once: `taken` shown items
            }
        }
        Cursor { sel, top: self.top }.shown(n, vis, keep)
    }

    /// Scrolls so `sel` is on the page.
    pub fn shown(self, n: u16, vis: u16, keep: &impl Fn(u16) -> bool) -> Cursor {
        let (sel, mut top) = (self.sel, self.top);
        if sel < top || !keep(top) {
            top = sel;
        }
        // back from sel, a page's rows at most and not past top: the page's top when that many were found. (It
        // counted every row from top to sel and stepped top on: a wrap to a full radio's last channel read 4000
        // records twice, sim/tests/limits.rs.)
        let (mut t, mut k) = (sel.min(n.saturating_sub(1)), 1);
        while k < vis && t > top {
            match (top..t).rev().find(|&i| keep(i)) {
                Some(i) => (t, k) = (i, k + 1),
                None => break,
            }
        }
        if k == vis {
            top = top.max(t);
        }
        Cursor { sel, top }
    }

    /// DISP-24: whether there are shown items above the page or below its `rows` (a full page of `vis`).
    /// Cheap for a dense filter: each side stops at the first kept item found.
    pub fn more(self, n: u16, vis: u16, rows: usize, keep: &impl Fn(u16) -> bool) -> bool {
        if (0..self.top.min(n)).any(keep) {
            return true;
        }
        if rows < vis as usize {
            return false;
        }
        // the last index on the page, then any kept one after it
        let mut seen = 0;
        let mut i = self.top;
        while i < n {
            if keep(i) {
                seen += 1;
                if seen == rows {
                    break;
                }
            }
            i += 1;
        }
        Self::next(i, 1, n, keep).is_some()
    }

    /// The indexes on the page, top first, through `f(row, index)`.
    pub fn rows(self, n: u16, vis: u16, keep: &impl Fn(u16) -> bool, mut f: impl FnMut(usize, u16)) -> usize {
        let mut row = 0;
        let mut i = self.top;
        while row < vis as usize && i < n {
            if keep(i) {
                f(row, i);
                row += 1;
            }
            i += 1;
        }
        row
    }
}

/// A two-line row (APRS heard, events, messages, waypoints): an outlined tag, a title (callsigns in
/// the bigger bold font), a right-hand value and an arrow on the first line; a sub line under it; a dot for
/// unread.
#[derive(Clone, Copy, Hash, Default)]
pub struct Row2<'a> {
    pub tag: &'a [u8],
    pub title: &'a [u8],
    pub big: bool,
    pub right: &'a [u8],
    /// A bearing arrow at the right edge, degrees from north; Some(None) a dash (no position).
    pub arrow: Option<Option<u16>>,
    pub sub: &'a [u8],
    pub sub_right: &'a [u8],
    pub sub_c: u16,
    /// The tag column's width (0: [`TAG_W`]); a list with longer tags sets it so its titles line up.
    pub tag_w: u8,
    pub dot: bool,
    pub selected: bool,
}

/// The tag column: wide enough for "iGate", "Pager" and "Selcall".
pub(crate) const TAG_W: i32 = 46;

/// A two-line row at y, off-screen in bands with its part of the scroll cue (DISP-25).
pub(crate) fn row2(lcd: &mut impl Panel, y: i32, r: &Row2, cue: w::Cue) {
    ferrite_ui::canvas::banded(lcd, 0, y, 240, ROW2_H, BG, |b| {
        row2_now(b, y, r);
        cue.draw(b);
    });
}

fn row2_now(lcd: &mut impl Panel, y: i32, r: &Row2) {
    let h = ROW2_H;
    let bg = if r.selected { BLUE } else { BG };
    let (fg, sub) = if r.selected { (BLUE_FG, BLUE_FG) } else { (TEXT, SUB) };
    lcd.fill(0, y, 240, h - 1, bg);
    lcd.fill(0, y + h - 1, 240, 1, LINE);
    let mut x = 8; // the side margin (DISP-2)
    if !r.tag.is_empty() {
        // 15 px tall so the 9 px capitals centre (DISP-19), the ink centred across
        let tw = text4_width(r.tag, &UI_12) + 8;
        let tx = x + text4_ink(r.tag, &UI_12).map_or(4, |(l, rr)| (tw - (rr - l) + 1) / 2 - l);
        lcd.fill(x, y + 4, tw, 15, bg);
        lcd.text4_rows(tx, y + 16, r.tag, &UI_12, sub, bg, y + 5, y + 18);
        lcd.frame_r(x, y + 4, tw, 15, 1, if r.selected { BLUE_FG } else { DIM });
        x += if r.tag_w > 0 { r.tag_w as i32 } else { TAG_W }.max(tw + 4);
    }
    // right side of line 1: arrow, value
    let mut xr = 232;
    match r.arrow {
        Some(Some(deg)) => {
            arrow(lcd, 224, y + 12, 6, deg as i32, fg, bg);
            xr = 214;
        }
        Some(None) => {
            lcd.fill(219, y + 12, 9, 2, fg);
            xr = 214;
        }
        None => {}
    }
    if !r.right.is_empty() {
        let rw = text4_width(r.right, &UI_12B);
        lcd.text4_rows(xr - rw, y + 17, r.right, &UI_12B, fg, bg, y + 2, y + 20);
        xr -= rw + 6;
    }
    // the big title on the tag's baseline: its capitals end 3 px above the sub line's (ferrite#33: 1 px at y + 18)
    let (f, base) = if r.big { (&UI_15B, y + 16) } else { (&UI_12B, y + 17) };
    let mut tx = x;
    if r.dot {
        lcd.fill_r(x, y + 8, 6, 6, 3, if r.selected { BLUE_FG } else { BLUE });
        tx += 10;
    }
    lcd.text4_rows(tx, base, w::fit(r.title, f, xr - tx), f, fg, bg, y + 1, y + 21);
    // line 2
    let srw = text4_width(r.sub_right, &UI_12);
    if srw > 0 {
        lcd.text4_rows(232 - srw, y + 29, r.sub_right, &UI_12, sub, bg, y + 18, y + h - 1);
    }
    let sc = if r.selected || r.sub_c == 0 { sub } else { r.sub_c };
    lcd.text4_rows(x, y + 29, w::fit(r.sub, &UI_12, 232 - x - srw - 6), &UI_12, sc, bg, y + 18, y + h - 1);
}

impl App {
    /// Two-line row `i` of a list page, drawn when it changed.
    pub(crate) fn row2(&mut self, lcd: &mut impl Panel, i: usize, r: &Row2) {
        if self.dirty(R_ROW + i, hash(r)) {
            row2(lcd, LIST_Y + i as i32 * ROW2_H, r, self.cue());
        }
    }

    /// One-line row `i` (name, value, dot) of a list page.
    pub(crate) fn row1(&mut self, lcd: &mut impl Panel, i: usize, name: &[u8], value: &[u8], dot: bool, selected: bool, grey: bool) {
        let r = w::Row { num: None, name, value, dot, selected, grey };
        self.list_row(lcd, i, LIST_Y + i as i32 * ROW1_H, ROW1_H, &r);
    }

    /// Rows `from..` of a page that has fewer rows than before: cleared.
    pub(crate) fn rows_end(&mut self, lcd: &mut impl Panel, from: usize, rows: u16, h: i32) {
        for i in from..rows as usize {
            if self.dirty(R_ROW + i, 1) {
                lcd.fill(0, LIST_Y + i as i32 * h, 240, h, BG);
            }
        }
    }

    /// DISP-24: the scroll cue beside a list page drawn by [`Cursor::rows`] (`rows` of `vis`, `h` each, from
    /// [`LIST_Y`]); the thumb's place is the top index's share of `n` (for a filtered list, of the whole store).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn list_cue(&mut self, lcd: &mut impl Panel, cur: Cursor, n: u16, vis: u16, h: i32, rows: usize, keep: &impl Fn(u16) -> bool) {
        let more = cur.more(n, vis, rows, keep);
        self.scroll_cue(lcd, (0, vis as usize), LIST_Y, vis as i32 * h, more, cur.top as u32, vis as u32, n as u32);
    }

    /// The list header: a count or title at the left, what the list holds at the right, a line under it.
    pub(crate) fn list_head(&mut self, lcd: &mut impl Panel, left: &[u8], right: &[u8]) {
        if self.dirty(R_TOP, Mix::new().h(&left).h(&right).get()) {
            head(lcd, left, right);
        }
    }

    /// What an empty list says, in the middle of the page.
    pub(crate) fn empty(&mut self, lcd: &mut impl Panel, line1: &[u8], line2: &[u8]) {
        if self.dirty(R_NOTE, Mix::new().h(&line1).h(&line2).get()) {
            Self::empty_now(lcd, line1, line2);
        }
    }

    pub(crate) fn empty_now(lcd: &mut impl Panel, line1: &[u8], line2: &[u8]) {
        w::field(lcd, 8, 224, 130, 147, 143, line1, const { &w::st(&UI_12B, SUB, BG, Align::Center) });
        w::field(lcd, 8, 224, 152, 169, 165, line2, const { &w::st(&UI_12, DIM, BG, Align::Center) });
    }

    /// The box asking before a destructive action (MENU-23): what goes, and the keys.
    /// [`Self::confirm_box`] with its second line from the text pack (UPD-22), read only when the box is drawn.
    pub(crate) fn confirm_box_t(&mut self, lcd: &mut impl Panel, what: &[u8], name: crate::text::Txt) {
        if self.dirty(R_HELP, Mix::new().h(&what).h(&name.0).get()) {
            self.confirm_box_draw(lcd, what, crate::text::load(name).as_bytes());
        }
    }

    fn confirm_box_draw(&mut self, lcd: &mut impl Panel, what: &[u8], name: &[u8]) {
        let (y, h) = (104, 92);
        lcd.fill_r(16, y, 208, h, 3, SURFACE);
        lcd.frame_r(16, y, 208, h, 3, RED);
        w::field(lcd, 24, 192, y + 12, y + 30, y + 26, what, const { &w::st(&UI_12B, TEXT, SURFACE, Align::Center) });
        w::field(lcd, 24, 192, y + 34, y + 52, y + 48, name, const { &w::st(&UI_12, TEXT, SURFACE, Align::Center) });
        w::field(lcd, 24, 192, y + 62, y + 80, y + 76, "OK confirms \u{b7} BACK keeps it".as_bytes(), const { &w::st(&UI_12, SUB, SURFACE, Align::Center) });
    }
}

pub(crate) fn head(lcd: &mut impl Panel, left: &[u8], right: &[u8]) {
    head_at(lcd, HEAD_Y, left, right);
}

/// [`head`] at `y` (a page with something above its list: ACARS's strip, ACARS-5).
pub(crate) fn head_at(lcd: &mut impl Panel, y: i32, left: &[u8], right: &[u8]) {
    let rw = text4_width(right, &UI_12);
    // in bands: the old line goes straight to the new (DISP-25)
    ferrite_ui::canvas::banded(lcd, 0, y, 240, 21, BG, |lcd| {
        lcd.text4_rows(8, y + 14, w::fit(left, &UI_12, 224 - rw - 12), &UI_12, SUB, BG, y, y + 20);
        w::field(lcd, 232 - rw, rw + 8, y, y + 20, y + 14, right, const { &w::st(&UI_12, SUB, BG, Align::Left) });
        lcd.fill(0, y + 20, 240, 1, LINE);
    });
}

/// Record text (ASCII) into ufmt.
pub(crate) struct Txt<'a>(pub &'a [u8]);
impl ufmt::uDisplay for Txt<'_> {
    fn fmt<W: ufmt::uWrite + ?Sized>(&self, f: &mut ufmt::Formatter<'_, W>) -> Result<(), W::Error> {
        f.write_str(core::str::from_utf8(self.0).unwrap_or("?"))
    }
}

// ---- shapes ----

/// sin of an angle in tenths of a degree, x 4096 (Bhaskara's approximation, within 0.2 %).
pub fn sin10(d10: i32) -> i32 {
    let d = d10.rem_euclid(3600) as i64;
    let (x, s) = if d <= 1800 { (d, 1) } else { (d - 1800, -1) };
    let p = x * (1800 - x);
    (s * 4 * 4096 * p / (4_050_000 - p)) as i32
}

pub fn cos10(d10: i32) -> i32 {
    sin10(d10 + 900)
}

/// A filled navigation arrow (a dart with a notched tail) centred at (cx, cy), pointing `deg` clockwise from
/// up, `r` from the centre to the tip; its box (2r + 1 square) painted bg first.
pub fn arrow(lcd: &mut impl Panel, cx: i32, cy: i32, r: i32, deg: i32, c: u16, bg: u16) {
    lcd.fill(cx - r, cy - r, 2 * r + 1, 2 * r + 1, bg);
    // the dart in unit coordinates x 16: tip, right wing, notch, left wing
    let pts = [(0, -16), (11, 14), (0, 6), (-11, 14)];
    let (s, co) = (sin10(deg * 10) as i64, cos10(deg * 10) as i64);
    let p = pts.map(|(x, y)| {
        let (x, y) = (x as i64 * r as i64, y as i64 * r as i64);
        // rotate clockwise on screen (y down): x' = x cos - y sin, y' = x sin + y cos
        (((x * co - y * s) / 4096 / 16) as i32 + cx, ((x * s + y * co) / 4096 / 16) as i32 + cy)
    });
    for t in [[p[0], p[1], p[2]], [p[0], p[2], p[3]]] {
        tri_fill(lcd, t, c, cx - r, cy - r, 2 * r + 1);
    }
}

/// A filled triangle, clipped to the box x0, y0, size.
fn tri_fill(lcd: &mut impl Panel, t: [(i32, i32); 3], c: u16, x0: i32, y0: i32, size: i32) {
    let edge = |a: (i32, i32), b: (i32, i32), x: i32, y: i32| (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0);
    let area = edge(t[0], t[1], t[2].0, t[2].1);
    if area == 0 {
        return;
    }
    for y in y0..y0 + size {
        let mut run: Option<i32> = None;
        for x in x0..=x0 + size {
            let inside = x < x0 + size && {
                let e = [edge(t[0], t[1], x, y), edge(t[1], t[2], x, y), edge(t[2], t[0], x, y)];
                e.iter().all(|&v| v * area.signum() >= 0)
            };
            match (inside, run) {
                (true, None) => run = Some(x),
                (false, Some(s)) => {
                    lcd.fill(s, y, x - s, 1, c);
                    run = None;
                }
                _ => {}
            }
        }
    }
}

/// A ring: radius r, `t` px thick.
pub fn ring(lcd: &mut impl Panel, cx: i32, cy: i32, r: i32, t: i32, c: u16) {
    let ri = r - t;
    for dy in -r..=r {
        let xo = isqrt((r * r - dy * dy) as u64) as i32;
        if dy.abs() < ri {
            let xi = isqrt((ri * ri - dy * dy) as u64) as i32;
            lcd.fill(cx - xo, cy + dy, xo - xi, 1, c);
            lcd.fill(cx + xi + 1, cy + dy, xo - xi, 1, c);
        } else {
            lcd.fill(cx - xo, cy + dy, 2 * xo + 1, 1, c);
        }
    }
}

/// The compass (AprsStation, waypoint go): a ring, N at the top, the arrow to the target; None draws a dash.
pub fn compass(lcd: &mut impl Panel, cx: i32, cy: i32, r: i32, deg: Option<u16>) {
    ferrite_ui::canvas::banded(lcd, cx - r - 1, cy - r - 1, 2 * r + 3, 2 * r + 3, BG, |lcd| {
        ring(lcd, cx, cy, r, 2, LINE);
        lcd.text4(cx - 4, cy - r + 17, b"N", &UI_12, SUB, BG);
        match deg {
            Some(d) => {
                arrow(lcd, cx, cy, r / 2, d as i32, BLUE, BG);
                lcd.fill_r(cx - 2, cy - 2, 5, 5, 2, TEXT);
            }
            None => lcd.fill(cx - 6, cy, 13, 2, SUB),
        }
    });
}

pub fn isqrt(v: u64) -> u64 {
    if v < 2 {
        return v;
    }
    let mut x = 1u64 << ((64 - v.leading_zeros()).div_ceil(2));
    loop {
        let y = (x + v / x) / 2;
        if y >= x {
            return x;
        }
        x = y;
    }
}

// ---- numbers ----

/// Distance in metres and bearing in degrees from a to b (flat-earth: within 0.5 % to a few hundred km).
pub fn dist_bearing(a: Pos, b: Pos) -> (u32, u16) {
    let mid = ((a.lat as i64 + b.lat as i64) / 2 / 1_000_000) as i32; // tenths of a degree
    let dy = (b.lat as i64 - a.lat as i64).clamp(-2_000_000_000, 2_000_000_000);
    let dx = ((b.lon as i64 - a.lon as i64) * cos10(mid) as i64 / 4096).clamp(-2_000_000_000, 2_000_000_000);
    let d = isqrt((dx * dx) as u64 + (dy * dy) as u64);
    let m = (d * 111_195 / 10_000_000) as u32;
    (m, atan2_deg(dx, dy))
}

/// Degrees clockwise from north of the vector (east, north).
fn atan2_deg(x: i64, y: i64) -> u16 {
    if x == 0 && y == 0 {
        return 0;
    }
    // atan(k/8) in hundredths of a degree, k = 0..8, interpolated
    const T: [i64; 9] = [0, 713, 1404, 2056, 2657, 3201, 3687, 4119, 4500];
    let (ax, ay) = (x.abs(), y.abs());
    let (lo, hi) = if ax < ay { (ax, ay) } else { (ay, ax) };
    let r = lo * 8 * 256 / hi; // 0..2048
    let k = (r / 256) as usize;
    let a = if k >= 8 { T[8] } else { T[k] + (T[k + 1] - T[k]) * (r % 256) / 256 };
    // a is the angle off the major axis
    let from_north = if ax < ay { a } else { 9000 - a };
    let deg = match (x >= 0, y >= 0) {
        (true, true) => from_north,
        (true, false) => 18000 - from_north,
        (false, false) => 18000 + from_north,
        (false, true) => 36000 - from_north,
    };
    ((deg + 50) / 100 % 360) as u16
}

/// "N", "NE" ... for a bearing: eight 45-degree sectors centred on the points (N is 338-22).
pub fn compass_name(deg: u16) -> &'static str {
    ["N", "NE", "E", "SE", "S", "SW", "W", "NW"][((deg as u32 * 2 + 45) / 90 % 8) as usize]
}

/// "850 m", "4.2 km", "23 km".
pub fn dist_text(m: u32, b: &mut Buf<12>) {
    b.clear();
    let _ = match m {
        0..=999 => uwrite!(b, "{} m", m),
        1000..=9_949 => uwrite!(b, "{}.{} km", (m + 50) / 1000, (m + 50) % 1000 / 100),
        _ => uwrite!(b, "{} km", (m + 500) / 1000),
    };
}

/// A distance for a narrow column (the zones page, CHAN-28): at most three digits and never wrapping. Under 10:
/// "0.4km", "2.3km"; under 100: "23.4km"; under 1000: "123km" (no decimal); then "far". In 0 6 4's units:
/// Imperial gives miles the same way ("2.3mi"), Nautical nautical miles ("2.3nm").
pub fn dist_short(m: u32, units: u8, b: &mut Buf<12>) {
    b.clear();
    // metres in one of the unit, x 1000
    let (u, per) = match units {
        1 => ("mi", 1_609_344u64),
        2 => ("nm", 1_852_000),
        _ => ("km", 1_000_000),
    };
    // tenths of the unit, rounded
    let tenths = ((m as u64 * 10_000 + per / 2) / per) as u32;
    let _ = if tenths < 1000 {
        uwrite!(b, "{}.{}{}", tenths / 10, tenths % 10, u)
    } else {
        let whole = ((m as u64 * 1000 + per / 2) / per) as u32;
        if whole < 1000 { uwrite!(b, "{}{}", whole, u) } else { uwrite!(b, "far") }
    };
}

/// Where a channel is from the fix, compass letters first (CHAN-28): "NE 2.3km", "N 123km", "W far".
pub fn place_text(m: u32, deg: u16, units: u8, b: &mut Buf<12>) {
    let mut d = Buf::<12>::new();
    dist_short(m, units, &mut d);
    b.clear();
    let _ = uwrite!(b, "{} {}", compass_name(deg), Txt(d.as_bytes()));
}

/// "0 s", "40 s", "12 min", "3 h", "1 h 20", "2 d" (never "now": it reads "now ago" beside "ago", Alex Oct 22).
pub fn age_text(s: u32, b: &mut Buf<12>) {
    b.clear();
    let m = s / 60;
    let _ = match s {
        crate::data::AGO_UNKNOWN => uwrite!(b, "earlier"), // INBOX-6.1
        0..=59 => uwrite!(b, "{} s", s),
        60..=3599 => uwrite!(b, "{} min", m),
        _ if m < 24 * 60 && !m.is_multiple_of(60) && m < 10 * 60 => uwrite!(b, "{} h {}", m / 60, m % 60),
        _ if m < 24 * 60 => uwrite!(b, "{} h", m / 60),
        _ => uwrite!(b, "{} d", m / 1440),
    };
}

/// The clock minute `ago_s` before `now_min` (minutes of the day), as "14:10"; "--:--" without either (INBOX-6.1).
pub fn clock_text(now_min: Option<u16>, ago_s: u32, b: &mut Buf<12>) {
    b.clear();
    match now_min.filter(|_| ago_s != crate::data::AGO_UNKNOWN) {
        Some(n) => {
            let t = (n as i32 - (ago_s / 60) as i32).rem_euclid(1440);
            let _ = uwrite!(b, "{}:{}{}", t / 60, if t % 60 < 10 { "0" } else { "" }, t % 60);
        }
        None => b.set(b"--:--"),
    }
}

/// A position on one line in 0 6 3's format (GPS-15.5), compact to fit a list row: "45°25.290'N 75°41.832'W"
/// (DDM, the default), "45.42150°N 75.69720°W", "45°25'17"N 75°41'50"W" (DMS to the second), or the grid square
/// ("FN25dk"). The GPS page shows each coordinate in full ([`coord`]).
pub fn pos_text(p: Pos, fmt: u8, b: &mut Buf<32>) {
    b.clear();
    if fmt == 3 {
        b.push(&grid(p));
        return;
    }
    for (v, pos, neg) in [(p.lat, "N", "S"), (p.lon, "E", "W")] {
        if !b.is_empty() {
            b.push(b" ");
        }
        let a = (v as i64).unsigned_abs();
        let deg = (a / 10_000_000) as u32;
        let h = if v >= 0 { pos } else { neg };
        let _ = match fmt {
            0 => {
                let f = (a % 10_000_000 / 100) as u32;
                uwrite!(b, "{}.{}{}\u{b0}{}", deg, Z5(f), f, h)
            }
            2 => {
                let s = ((a % 10_000_000) * 3600 / 10_000_000) as u32; // whole seconds
                uwrite!(b, "{}\u{b0}{}{}'{}{}\"{}", deg, z(s / 60), s / 60, z(s % 60), s % 60, h)
            }
            _ => {
                let mm = ((a % 10_000_000) * 60_000 / 10_000_000) as u32; // thousandths of a minute
                let (mi, f) = (mm / 1000, mm % 1000);
                uwrite!(b, "{}\u{b0}{}{}.{}{}{}'{}", deg, z(mi), mi, f / 100, f / 10 % 10, f % 10, h)
            }
        };
    }
}

/// One coordinate in the 0 6 3 format: "45.42150° N", "45°25.290' N", "45°25'17.4\" N".
pub(crate) fn coord(v: i32, pos: &str, neg: &str, fmt: u8, b: &mut Buf<24>) {
    b.clear();
    let a = (v as i64).unsigned_abs();
    let deg = (a / 10_000_000) as u32;
    let h = if v >= 0 { pos } else { neg };
    let _ = match fmt {
        0 => {
            let f = (a % 10_000_000 / 100) as u32; // 5 decimals
            uwrite!(b, "{}.{}{}\u{b0} {}", deg, Z5(f), f, h)
        }
        2 => {
            let ds = (a % 10_000_000) * 36_000 / 10_000_000; // tenths of a second
            let (mi, se) = ((ds / 600) as u32, (ds % 600) as u32);
            uwrite!(b, "{}\u{b0}{}{}'{}{}.{}\" {}", deg, z(mi), mi, z(se / 10), se / 10, se % 10, h)
        }
        _ => {
            let mm = (a % 10_000_000) * 60_000 / 10_000_000; // thousandths of a minute
            let (mi, f) = ((mm / 1000) as u32, (mm % 1000) as u32);
            uwrite!(b, "{}\u{b0}{}{}.{}{}{}' {}", deg, z(mi), mi, f / 100, f / 10 % 10, f % 10, h)
        }
    };
}

/// Zeros in front of a five-digit fraction.
struct Z5(u32);
impl ufmt::uDisplay for Z5 {
    fn fmt<W: ufmt::uWrite + ?Sized>(&self, f: &mut ufmt::Formatter<'_, W>) -> Result<(), W::Error> {
        let mut d = 10_000;
        while d > 1 && self.0 < d {
            f.write_str("0")?;
            d /= 10;
        }
        Ok(())
    }
}

/// "0" before a single digit.
pub(crate) fn z(v: u32) -> &'static str {
    if v < 10 { "0" } else { "" }
}

/// The Maidenhead grid square, six characters ("FN25dk"). GPS-3.2: a position off the globe is taken at its edge, and
/// the poles and the date line themselves fall in the last square (RR99xx), not past it (ferrite#149).
pub fn grid(p: Pos) -> [u8; 6] {
    let p = p.on_globe();
    let lon = (p.lon as i64 + 1_800_000_000).min(3_599_999_999); // 0..360 degrees, x 1e7
    let lat = (p.lat as i64 + 900_000_000).min(1_799_999_999);
    let d = 10_000_000i64;
    [
        b'A' + (lon / (20 * d)) as u8,
        b'A' + (lat / (10 * d)) as u8,
        b'0' + (lon % (20 * d) / (2 * d)) as u8,
        b'0' + (lat % (10 * d) / d) as u8,
        b'a' + (lon % (2 * d) * 24 / (2 * d)) as u8,
        b'a' + (lat % d * 24 / d) as u8,
    ]
}

/// A typed grid square, 4 or 6 characters in either case ("FN31", "fn31pr"), as the centre of the square
/// (GPS-21); None if it isn't one.
pub fn grid_pos(g: &[u8]) -> Option<Pos> {
    if g.len() != 4 && g.len() != 6 {
        return None;
    }
    let up = |i: usize| g[i].to_ascii_uppercase();
    let letter = |i: usize, max: u8| up(i).checked_sub(b'A').filter(|&v| v < max).map(|v| v as i64);
    let digit = |i: usize| g[i].checked_sub(b'0').filter(|&v| v < 10).map(|v| v as i64);
    let d = 10_000_000i64;
    // degrees x 1e7 from the south-west corner (-180, -90)
    let mut lon = letter(0, 18)? * 20 * d + digit(2)? * 2 * d;
    let mut lat = letter(1, 18)? * 10 * d + digit(3)? * d;
    if g.len() == 6 {
        // subsquares: 5' of longitude, 2.5' of latitude; to their centre
        lon += letter(4, 24)? * 2 * d / 24 + d / 24;
        lat += letter(5, 24)? * d / 24 + d / 48;
    } else {
        lon += d; // the square's centre: 1 degree in, half a degree up
        lat += d / 2;
    }
    Some(Pos { lat: (lat - 90 * d) as i32, lon: (lon - 180 * d) as i32 })
}

/// A frequency as "146.520" (MHz, at least three decimals, more when they aren't zero).
pub fn mhz_text(f: u32, b: &mut Buf<16>) {
    b.clear();
    let khz = f / 100;
    let rest = f % 100;
    let _ = uwrite!(b, "{}.{}{}{}", khz / 1000, khz % 1000 / 100, khz % 100 / 10, khz % 10);
    if rest != 0 {
        let _ = uwrite!(b, "{}", rest / 10);
        if !rest.is_multiple_of(10) {
            let _ = uwrite!(b, "{}", rest % 10);
        }
    }
}

#[cfg(test)]
mod tests {
    /// GPS-3.2 (ferrite#149): a position off the globe (a damaged record, the link) gives the edge's square, never
    /// a panic or letters past R; the poles and the date line fall in the last square.
    #[test]
    fn grid_off_the_globe() {
        use crate::data::Pos;
        let g = |lat: i32, lon: i32| std::string::String::from_utf8(grid(Pos { lat, lon }).to_vec()).unwrap();
        assert_eq!(g(900_000_000, 1_800_000_000), "RR99xx", "the north pole on the date line");
        assert_eq!(g(-900_000_000, -1_800_000_000), "AA00aa", "the south pole, the other side");
        assert_eq!(g(i32::MAX, i32::MAX), "RR99xx");
        assert_eq!(g(i32::MIN, i32::MIN), "AA00aa");
        assert_eq!(g(950_000_000, 0), g(900_000_000, 0), "past the pole: the pole");
        let p = Pos { lat: -950_000_000, lon: 2_000_000_000 }.on_globe();
        assert_eq!((p.lat, p.lon), (-900_000_000, 1_800_000_000));
        let mut s = crate::settings::Settings::DEFAULT;
        s.fixed = Pos { lat: i32::MAX, lon: i32::MIN };
        s.sanitize();
        assert_eq!((s.fixed.lat, s.fixed.lon), (900_000_000, -1_800_000_000), "a loaded record's position kept on the globe");
    }

    /// DISP-24: more above or below, with and without a filter.
    #[test]
    fn more_rows() {
        use super::Cursor;
        let all = |_: u16| true;
        assert!(Cursor::first(20, &all).more(20, 8, 8, &all), "20 rows, 8 shown");
        assert!(!Cursor::first(5, &all).more(5, 8, 5, &all), "all fit");
        let even = |i: u16| i.is_multiple_of(2);
        assert!(Cursor { sel: 14, top: 14 }.more(20, 3, 3, &even), "rows above");
        let c = Cursor { sel: 0, top: 0 };
        assert!(!c.more(6, 3, 3, &even), "0, 2, 4 fill the page and nothing follows");
        assert!(c.more(8, 3, 3, &even), "6 follows");
    }

    use super::*;
    use crate::sample::HERE;

    #[test]
    fn geo() {
        // The sample's VE3KPW-9: 4.2 km at 48 degrees.
        let (m, b) = dist_bearing(HERE, Pos { lat: 454_467_741, lon: -756_572_082 });
        assert!((4150..4250).contains(&m), "{m}");
        assert!((47..=49).contains(&b), "{b}");
        let (m, b) = dist_bearing(HERE, Pos { lat: 452_277_209, lon: -764_557_333 });
        assert!((62_500..63_500).contains(&m), "{m}");
        assert!((249..=251).contains(&b), "{b}");
        assert_eq!(dist_bearing(HERE, HERE), (0, 0));
        assert_eq!(compass_name(48), "NE");
        assert_eq!(compass_name(350), "N");
        assert_eq!(&grid(HERE), b"FN25dk");
        for d in (0..3600).step_by(37) {
            let s = sin10(d) as f64 / 4096.0;
            assert!((s - (d as f64 / 10.0).to_radians().sin()).abs() < 0.003, "{d}");
        }
    }

    /// GPS-15.5: 0 6 3's four names, and a list row's one-line position in its format with the GPS page's symbols.
    #[test]
    fn texts() {
        assert_eq!(crate::settings::COORD_FORMATS, ["Decimal", "DDM", "DMS", "Grid square"], "GPS-15.5");
        let mut b = Buf::<12>::new();
        let s = |b: &Buf<12>| std::string::String::from_utf8(b.as_bytes().to_vec()).unwrap();
        dist_text(4200, &mut b);
        assert_eq!(s(&b), "4.2 km");
        dist_text(23_400, &mut b);
        assert_eq!(s(&b), "23 km");
        dist_text(850, &mut b);
        assert_eq!(s(&b), "850 m");
        age_text(120, &mut b);
        assert_eq!(s(&b), "2 min");
        age_text(4800, &mut b);
        assert_eq!(s(&b), "1 h 20");
        clock_text(Some(14 * 60 + 32), 22 * 60, &mut b);
        assert_eq!(s(&b), "14:10");
        let mut p = Buf::<32>::new();
        pos_text(HERE, 1, &mut p);
        assert_eq!(p.as_bytes(), "45\u{b0}25.290'N 75\u{b0}41.832'W".as_bytes(), "GPS-15.5: DDM, with its symbols");
        pos_text(HERE, 0, &mut p);
        assert_eq!(p.as_bytes(), "45.42150\u{b0}N 75.69720\u{b0}W".as_bytes());
        pos_text(HERE, 2, &mut p);
        assert_eq!(p.as_bytes(), "45\u{b0}25'17\"N 75\u{b0}41'49\"W".as_bytes(), "DMS to the whole second (49.9 s reads 49)");
        pos_text(HERE, 3, &mut p);
        assert_eq!(p.as_bytes(), b"FN25dk", "the grid square, six characters");
        let mut f = Buf::<16>::new();
        mhz_text(14_439_000, &mut f);
        assert_eq!(f.as_bytes(), b"144.390");
        mhz_text(43_998_750, &mut f);
        assert_eq!(f.as_bytes(), b"439.9875");
    }

    #[test]
    fn cursor_walks_a_filtered_list() {
        let keep = |i: u16| i.is_multiple_of(3); // 0, 3, 6, 9, 12, 15, 18
        let c = Cursor::first(20, &keep);
        assert_eq!(c, Cursor { sel: 0, top: 0 });
        let c = c.step(4, 20, 3, &keep);
        assert_eq!(c.sel, 12);
        assert_eq!(c.top, 6, "three rows: 6, 9, 12");
        let c = c.step(-1, 20, 3, &keep);
        assert_eq!((c.sel, c.top), (9, 6));
        let c = Cursor::first(20, &keep).step(-1, 20, 3, &keep);
        assert_eq!((c.sel, c.top), (18, 12), "wraps to the end");
        let mut seen = std::vec::Vec::new();
        c.rows(20, 3, &keep, |_, i| seen.push(i));
        assert_eq!(seen, [12, 15, 18]);
    }
}

#[cfg(test)]
#[path = "tests_c.rs"]
mod tests_c;
