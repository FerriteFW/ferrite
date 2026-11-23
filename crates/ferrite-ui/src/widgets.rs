//! The pieces the look review's screens are made of (`docs/img/screens/*.png`): status bar, slot letter, tags,
//! chips, frequency, segmented S-meter, list rows with number, name, value and default dot, help box, hint bar,
//! edit cell with the knob mark, toast. Each one paints its whole box, background included, so any one of them
//! can be redrawn alone (HOME-7). Numbers are pixels on the 240 x 320 portrait screen.

use crate::assets::*;
use crate::font4::Font4;
use crate::gfx::*;
use crate::panel::Panel;
use crate::text::Buf;
use ferrite_core::freq::{self, Freq10};
use ferrite_core::msg::GpsState;

/// Status bar rows 0-19 (the line is row 19), content 20-299, hint bar 300-319 (HOME-1).
pub const SBAR_H: i32 = 20;
pub const HINT_Y: i32 = 300;

/// Hint-bar arrows: these bytes in a key name draw as triangles.
pub const UP: &str = "\x18";
pub const DOWN: &str = "\x19";
pub const RIGHT: &str = "\x1a";
pub const LEFT: &str = "\x1b";

pub const LETTERS: [u8; 3] = *b"ABC";

pub fn slot_colour(slot: u8) -> u16 {
    [BLUE, AMBER, VIOLET][slot as usize % 3]
}

/// The longest start of s (whole UTF-8 characters) that fits in w pixels.
pub fn fit<'a>(s: &'a [u8], f: &Font4, w: i32) -> &'a [u8] {
    if text4_width(s, f) <= w {
        return s;
    }
    let mut end = 0;
    for i in 1..=s.len() {
        if i == s.len() || s[i] & 0xC0 != 0x80 {
            if text4_width(&s[..i], f) > w {
                break;
            }
            end = i;
        }
    }
    &s[..end]
}

/// [`fit`] inside a widget: the text the caller gave is cut, which [`crate::clipcheck`] notes (a deliberate cut is
/// fitted by the caller first).
fn cut<'a>(s: &'a [u8], f: &Font4, w: i32) -> &'a [u8] {
    let r = fit(s, f, w);
    #[cfg(feature = "clipcheck")]
    if r.len() < s.len() {
        crate::clipcheck::note("cut", s, w, text4_width(s, f));
    }
    r
}

/// How [`field`] draws its text: font, colours and alignment. A call with constant ones passes
/// `const { &st(..) }`, one address in place of four arguments (the flash audit's #7, Nov 1).
pub struct Style {
    pub f: &'static Font4,
    pub fg: u16,
    pub bg: u16,
    pub align: Align,
}

/// A [`Style`].
pub const fn st(f: &'static Font4, fg: u16, bg: u16, align: Align) -> Style {
    Style { f, fg, bg, align }
}

/// Text in the box x..x+w, y0..y1: aligned, cut to fit, the rest of the box painted bg. The workhorse for
/// redrawing a value in place without clearing it first (no flicker). A thin shim: one body for every panel.
#[inline]
#[cfg_attr(feature = "clipcheck", track_caller)]
pub fn field(lcd: &mut impl Panel, x: i32, w: i32, y0: i32, y1: i32, baseline: i32, s: &[u8], st: &Style) -> i32 {
    field_at(lcd, x, w, y0, y1, baseline, s, st)
}

#[inline(never)]
#[allow(clippy::too_many_arguments)]
#[cfg_attr(feature = "clipcheck", track_caller)]
fn field_at(lcd: &mut dyn Panel, x: i32, w: i32, y0: i32, y1: i32, baseline: i32, s: &[u8], st: &Style) -> i32 {
    let (f, fg, bg) = (st.f, st.fg, st.bg);
    clip_site!();
    let s = cut(s, f, w);
    let tw = text4_width(s, f);
    let tx = match st.align {
        Align::Left => x,
        Align::Right => x + w - tw,
        // the ink in the middle (an odd leftover pixel goes on the left, where the glyphs' anti-aliasing is
        // lighter), kept inside the box
        Align::Center => match text4_ink(s, f) {
            Some((l, r)) => (x + (w - (r - l) + 1) / 2 - l).clamp(x, (x + w - tw).max(x)),
            None => x + (w - tw) / 2,
        },
    };
    lcd.fill(x, y0, tx - x, y1 - y0, bg);
    lcd.text4_rows(tx, baseline, s, f, fg, bg, y0, y1);
    // the box's rows above and below the font's line box
    let top = baseline - f.ascent as i32;
    lcd.fill(tx, y0, tw, top - y0, bg);
    lcd.fill(tx, top + f.line_h as i32, tw, y1 - top - f.line_h as i32, bg);
    lcd.fill(tx + tw, y0, x + w - tx - tw, y1 - y0, bg);
    tw
}

/// The 14 x 14 slot letter: filled for the active slot, outlined for the others.
pub fn badge(lcd: &mut impl Panel, x: i32, y: i32, slot: u8, filled: bool, bg: u16) {
    crate::canvas::banded(lcd, x, y, 14, 14, bg, |b| badge_now(b, x, y, slot, filled, bg));
}

fn badge_now(lcd: &mut impl Panel, x: i32, y: i32, slot: u8, filled: bool, bg: u16) {
    let c = slot_colour(slot);
    let (fg, back) = if filled { (BLUE_FG, c) } else { (c, bg) };
    // 13 x 13: the 9 px letter sits 1 px from the top and bottom inside (an even box can't centre an odd
    // letter), and its 5 px strokes 3 px from each side (the glyph has a 1 px left bearing). The 14 px clear
    // wipes a box drawn at the old size.
    lcd.fill(x, y, 14, 14, bg);
    lcd.fill_r(x, y, 13, 13, 1, back);
    lcd.text4_rows(x + 3, y + 11, &[LETTERS[slot as usize % 3]], &UI_12B, fg, back, y + 1, y + 12);
    if !filled {
        lcd.frame_r(x, y, 13, 13, 1, c);
    }
}

/// An outlined tag ("RX", "EDIT") ending at x_right; returns its width. Night red fills it, black text on solid
/// red (DISP-18: a filled highlight, where the other themes have the outline's colour).
#[cfg_attr(feature = "clipcheck", track_caller)]
pub fn tag(lcd: &mut impl Panel, x_right: i32, y: i32, s: &[u8], c: u16, bg: u16) -> i32 {
    clip_site!();
    // 15 px tall so the 9 px capitals sit 2 px from the top and bottom inside (DISP-19)
    let w = text4_width(s, &UI_12B) + 10;
    let x = x_right - w;
    let night = crate::theme::get() == crate::theme::NIGHT_RED;
    crate::canvas::banded(lcd, x, y, w, 16, bg, |b| {
        let back = if night { c } else { bg };
        field(b, x, w, y, y + 15, y + 12, s, &st(&UI_12B, c, back, Align::Center));
        b.frame_r(x, y, w, 15, 1, c);
    });
    w
}

/// A card's border, radius 3. Night red, where every border is the same red, draws the active card's 2 px
/// (DISP-18: a heavier outline marks it), the second line just outside, in the gap around the card, so the card's
/// content keeps its room; an inactive card's gap is `around` (the screen's ground).
#[allow(clippy::too_many_arguments)]
pub fn card_border(lcd: &mut impl Panel, x: i32, y: i32, w: i32, h: i32, c: u16, active: bool, around: u16) {
    lcd.frame_r(x, y, w, h, 3, c);
    if crate::theme::get() == crate::theme::NIGHT_RED {
        lcd.frame_r(x - 1, y - 1, w + 2, h + 2, 4, if active { c } else { around });
    }
}

/// An outlined chip ("NFM", "T 100.0"); returns its width.
#[cfg_attr(feature = "clipcheck", track_caller)]
pub fn chip(lcd: &mut impl Panel, x: i32, y: i32, s: &[u8], fg: u16, bg: u16) -> i32 {
    clip_site!();
    let w = text4_width(s, &UI_12) + 10;
    field(lcd, x, w, y, y + 17, y + 13, s, &st(&UI_12, fg, bg, Align::Center));
    lcd.frame_r(x, y, w, 17, 1, LINE);
    w
}

/// The big card's chip row in fixed slots (HOME-4): mode, tone, shift and power each keep their own x and width, so
/// a value coming or going never moves its neighbours; power is pinned to the right edge. Widths fit the widest
/// text each can show in UI_12 (7 px a character, 4 px padding each side): "NFM", "T 254.1", "R-12.5M" (a
/// reversed shift) and "No TX". x..x+w, all inside 15..226.
pub const SLOT_MODE: (i32, i32) = (15, 36);
pub const SLOT_TONE: (i32, i32) = (54, 57);
pub const SLOT_SHIFT: (i32, i32) = (114, 57);
pub const SLOT_POWER: (i32, i32) = (176, 50);

/// A chip in a fixed slot (x, w) at y..y+17: the slot is cleared first, then a chip of the slot's full width when
/// `s` isn't empty, so a shorter value never leaves pixels behind.
#[cfg_attr(feature = "clipcheck", track_caller)]
pub fn chip_slot(lcd: &mut impl Panel, slot: (i32, i32), y: i32, s: &[u8], fg: u16, frame: u16, bg: u16) {
    clip_site!();
    crate::canvas::banded(lcd, slot.0, y, slot.1, 17, bg, |b| chip_slot_now(b, slot, y, s, fg, frame, bg));
}

fn chip_slot_now(lcd: &mut impl Panel, slot: (i32, i32), y: i32, s: &[u8], fg: u16, frame: u16, bg: u16) {
    let (x, w) = slot;
    lcd.fill(x, y, w, 17, bg);
    if !s.is_empty() {
        field(lcd, x + 1, w - 2, y + 1, y + 16, y + 13, cut(s, &UI_12, w - 4), &st(&UI_12, fg, bg, Align::Center));
        lcd.frame_r(x, y, w, 17, 1, frame);
    }
}

/// A row of chips in the box x..x+w, y..y+17.
#[cfg_attr(feature = "clipcheck", track_caller)]
pub fn chips(lcd: &mut impl Panel, x: i32, y: i32, w: i32, items: &[&[u8]], bg: u16) {
    clip_site!();
    chips_sel(lcd, x, y, w, items, None, bg)
}

/// [`chips`] with one of them selected (accent text and outline): a row of choices such as MENU's Recent.
#[cfg_attr(feature = "clipcheck", track_caller)]
pub fn chips_sel(lcd: &mut impl Panel, x: i32, y: i32, w: i32, items: &[&[u8]], sel: Option<usize>, bg: u16) {
    clip_site!();
    let mut cx = x;
    for (i, s) in items.iter().enumerate().filter(|(_, s)| !s.is_empty()) {
        if cx + text4_width(s, &UI_12) + 10 > x + w {
            break;
        }
        let cw = chip(lcd, cx, y, s, if sel == Some(i) { BLUE } else { TEXT }, bg);
        if sel == Some(i) {
            lcd.frame_r(cx, y, cw, 17, 1, BLUE);
        }
        cx += cw + 4;
    }
    lcd.fill(cx, y, x + w - cx, 17, bg);
}

/// "146.520" in `big` and "00" in `small` on one baseline, painting only the digits' rows (the big font's line
/// box would reach into its neighbours) and clearing to x_end. The small digits (10 Hz) only where they end by
/// x_end: 1000 MHz and up on home's big card leaves them out. Returns where the text ends.
pub fn freq(
    lcd: &mut impl Panel,
    x: i32,
    x_end: i32,
    baseline: i32,
    f: Freq10,
    big: &'static Font4,
    small: &'static Font4,
    fg: u16,
    fg_small: u16,
    bg: u16,
) -> i32 {
    let mut b = [0u8; 16];
    let n = freq::format(f, &mut b);
    let split = n.saturating_sub(3);
    let h = digit_h(big);
    let (y0, y1) = (baseline - h, baseline + 1);
    let w = lcd.text4_rows(x, baseline, &b[..split], big, fg, bg, y0, y1);
    let sx = x + w + 4;
    lcd.fill(x + w, y0, 4, y1 - y0, bg);
    let hs = digit_h(small);
    let tail = &b[split + 1..n];
    let tail = if sx + text4_width(tail, small) <= x_end { tail } else { &[] };
    lcd.fill(sx, y0, text4_width(tail, small), h - hs, bg);
    let ws = lcd.text4_rows(sx, baseline, tail, small, fg_small, bg, baseline - hs, y1);
    lcd.fill(sx + ws, y0, x_end - sx - ws, y1 - y0, bg);
    sx + ws
}

/// [`freq`] centred between `x` and `x_end`, the space left of it cleared too. Returns where the text ends.
#[allow(clippy::too_many_arguments)]
pub fn freq_centred(
    lcd: &mut impl Panel,
    x: i32,
    x_end: i32,
    baseline: i32,
    f: Freq10,
    big: &'static Font4,
    small: &'static Font4,
    fg: u16,
    fg_small: u16,
    bg: u16,
) -> i32 {
    let mut b = [0u8; 16];
    let n = freq::format(f, &mut b);
    let split = n.saturating_sub(3);
    let w = text4_width(&b[..split], big) + 4 + text4_width(&b[split + 1..n], small);
    let sx = (x + (x_end - x - w) / 2).max(x);
    let (y0, y1) = (baseline - digit_h(big), baseline + 1);
    lcd.fill(x, y0, sx - x, y1 - y0, bg);
    self::freq(lcd, sx, x_end, baseline, f, big, small, fg, fg_small, bg)
}

/// Height of a digit above the baseline.
pub fn digit_h(f: &Font4) -> i32 {
    f.glyph('0' as u32).map_or(f.ascent as i32, |g| -(g.y as i32))
}

/// "146.520" (three decimals), for compact lines.
pub fn freq3(f: Freq10) -> Buf<12> {
    let mut b = [0u8; 16];
    let n = freq::format(f, &mut b);
    Buf::from(&b[..n.saturating_sub(3)])
}

/// The S-meter: 20 segments 8 px wide on a 10 px pitch, 8 px tall. Only segments between the old and new
/// level are drawn (None: all of them).
pub const METER_SEGS: u8 = 20;
pub fn meter(lcd: &mut impl Panel, x: i32, y: i32, old: Option<u8>, lit: u8, on: u16, bg: u16) {
    let (a, b) = match old {
        Some(o) => (o.min(lit), o.max(lit)),
        None => (0, METER_SEGS),
    };
    if old.is_none() {
        // the whole meter in one band: the gaps' background and every segment (DISP-25)
        crate::canvas::banded(lcd, x, y, METER_SEGS as i32 * 10, 8, bg, |band| meter_segs(band, x, y, 0, METER_SEGS, lit, on));
    } else {
        meter_segs(lcd, x, y, a, b, lit, on);
    }
}

fn meter_segs(lcd: &mut impl Panel, x: i32, y: i32, a: u8, b: u8, lit: u8, on: u16) {
    // DISP-18: night red outlines a DIM fill, which would read as empty: lit segments are solid there
    let on = if crate::theme::outlined(on) { SUB } else { on };
    for i in a..b.min(METER_SEGS) {
        lcd.fill_r(x + i as i32 * 10, y, 8, 8, 1, if i < lit { on } else { LINE });
    }
}

/// dBm to lit segments: S0 (-127 dBm) to S9 (-73 dBm) over the first 17, then 20 dB per segment to S9 + 60.
pub fn meter_level(dbm: i16) -> u8 {
    let d = dbm as i32;
    if d <= -127 {
        0
    } else if d <= -73 {
        ((d + 127) * 17 / 54) as u8
    } else {
        (17 + (d + 73) / 20).min(METER_SEGS as i32) as u8
    }
}

/// SQL-12.3: the level as a number, "-93 dBm".
pub fn dbm_text(dbm: i16) -> Buf<12> {
    let mut b = Buf::new();
    let _ = ufmt::uwrite!(b, "{} dBm", dbm.clamp(-140, 20));
    b
}

/// "S7", "S9", "+20".
pub fn s_text(dbm: i16) -> Buf<4> {
    let mut b = Buf::new();
    let d = dbm as i32;
    if d < -63 {
        let s = ((d + 127).max(0) / 6).min(9);
        let _ = ufmt::uwrite!(b, "S{}", s);
    } else {
        let _ = ufmt::uwrite!(b, "+{}", ((d + 73) / 10 * 10).min(60));
    }
    b
}

/// One line of text with keys in bold: `key` (arrows as triangles), then `label`. Returns the width.
pub fn key_label(lcd: &mut impl Panel, x: i32, baseline: i32, key: &str, label: &str, bg: u16, draw: bool) -> i32 {
    key_label_in(lcd, x, baseline, key, label, (KEYS, SUB, bg), draw)
}

/// [`key_label`] in other colours: the key's, the label's and the background (Low Power Mode's amber band).
#[allow(clippy::too_many_arguments)]
pub fn key_label_in(lcd: &mut impl Panel, x: i32, baseline: i32, key: &str, label: &str, (kc, lc, bg): (u16, u16, u16), draw: bool) -> i32 {
    let (y0, y1) = (baseline - 13, baseline + 4);
    let mut px = x;
    for part in key.as_bytes().split_inclusive(|&c| (0x18..=0x1b).contains(&c)) {
        let (text, arrow) = match part.last() {
            Some(&c) if (0x18..=0x1b).contains(&c) => (&part[..part.len() - 1], Some(c)),
            _ => (part, None),
        };
        if draw {
            px += lcd.text4_rows(px, baseline, text, &UI_12B, kc, bg, y0, y1);
        } else {
            px += text4_width(text, &UI_12B);
        }
        if let Some(c) = arrow {
            if draw {
                lcd.fill(px, y0, 7, y1 - y0, bg);
                let dir = [0, 1, 3, 2][(c - 0x18) as usize];
                lcd.tri(px, baseline - 7, 6, dir, kc);
            }
            px += 7;
        }
    }
    if !label.is_empty() {
        if draw {
            lcd.fill(px, y0, 4, y1 - y0, bg);
            px += 4;
            px += lcd.text4_rows(px, baseline, label.as_bytes(), &UI_12, lc, bg, y0, y1);
        } else {
            px += 4 + text4_width(label.as_bytes(), &UI_12);
        }
    }
    px - x
}

/// A line like "Hold V/M save this VFO as a channel" at x, the rest of the row to x_end cleared.
pub fn key_line(lcd: &mut impl Panel, x: i32, x_end: i32, baseline: i32, key: &str, label: &str, bg: u16) {
    let w = key_label(lcd, x, baseline, key, label, bg, true);
    lcd.fill(x + w, baseline - 13, x_end - x - w, 17, bg);
}

/// The hint bar holds at most this many key + label pairs (KEY-9): more overflow 240 px in the 12 px font.
pub const HINT_MAX: usize = 3;

/// The hint bar's width for these pairs, with the smallest gap (8 px) between them; it fits when this is at
/// most 224 (8 px margins).
pub fn hint_width(lcd: &mut impl Panel, items: &[(&str, &str)]) -> i32 {
    let n = items.len().min(HINT_MAX);
    let w: i32 = items[..n].iter().map(|(k, l)| key_label(lcd, 0, 0, k, l, BG, false)).sum();
    w + 8 * (n as i32 - 1).max(0)
}

/// The hint bar (KEY-9): up to [`HINT_MAX`] key + label pairs, the first at the left margin, the last at the
/// right, the one between centred in the gap.
pub fn hint_bar(lcd: &mut impl Panel, items: &[(&str, &str)]) {
    // Off-screen, sent once (DISP-25): the key boxes were drawn over their own blank.
    crate::canvas::banded(lcd, 0, HINT_Y, 240, 20, BG, |b| hint_bar_now(b, items));
}

fn hint_bar_now(lcd: &mut impl Panel, items: &[(&str, &str)]) {
    let base = HINT_Y + 15; // text rows HINT_Y + 2 .. HINT_Y + 19
    lcd.fill(0, HINT_Y, 240, 1, RULE);
    lcd.fill(0, HINT_Y + 1, 240, 1, BG);
    lcd.fill(0, HINT_Y + 19, 240, 1, BG);
    let n = items.len().min(HINT_MAX);
    let mut w = [0i32; HINT_MAX];
    for i in 0..n {
        w[i] = key_label(lcd, 0, base, items[i].0, items[i].1, BG, false);
    }
    let total: i32 = w[..n].iter().sum();
    let gap = if n > 1 { (224 - total) / (n as i32 - 1) } else { 0 };
    let (mut x, mut end) = (8, 0);
    for i in 0..n {
        if i == n - 1 && n > 1 {
            x = 232 - w[i];
        }
        lcd.fill(end, HINT_Y + 2, x - end, 17, BG);
        key_label(lcd, x, base, items[i].0, items[i].1, BG, true);
        end = x + w[i];
        x = end + gap;
    }
    lcd.fill(end, HINT_Y + 2, 240 - end, 17, BG);
}

/// The manual's QR code (HELP-2) at x, y, `scale` px a module, with the 4-module quiet zone: inverted, the code's
/// dark modules in the theme's text colour on its background (decided Oct 8, 2026), so night red stays red (daylight,
/// dark on white, is a plain code). Phone cameras read inverted codes. Dark modules go out as runs along each row. Returns the side in px.
pub fn manual_qr(lcd: &mut impl Panel, x: i32, y: i32, scale: i32) -> i32 {
    use crate::qr::{MANUAL_QR, QR_SIZE};
    let side = (QR_SIZE + 8) * scale;
    lcd.fill(x, y, side, side, BG);
    let (x0, y0) = (x + 4 * scale, y + 4 * scale);
    for (r, &bits) in MANUAL_QR.iter().enumerate() {
        let mut c = 0;
        while c < QR_SIZE {
            let dark = |c: i32| bits >> (QR_SIZE - 1 - c) & 1 != 0;
            if !dark(c) {
                c += 1;
                continue;
            }
            let start = c;
            while c < QR_SIZE && dark(c) {
                c += 1;
            }
            lcd.fill(x0 + start * scale, y0 + r as i32 * scale, (c - start) * scale, scale, TEXT);
        }
    }
    side
}

/// The envelope: 12 x 9.
pub fn envelope(lcd: &mut impl Panel, x: i32, y: i32, c: u16) {
    lcd.frame(x, y, 12, 9, c);
    lcd.line(x + 1, y + 1, x + 5, y + 5, c);
    lcd.line(x + 6, y + 5, x + 10, y + 1, c);
}

/// A bell, 11 x 11 (alert tone; with `slash`, alerts paused: SBAR-18).
pub fn bell(lcd: &mut impl Panel, x: i32, y: i32, c: u16, bg: u16, slash: bool) {
    lcd.fill(x, y, 11, 11, bg);
    lcd.fill_r(x + 2, y + 1, 7, 8, 3, c);
    lcd.fill(x + 1, y + 7, 9, 2, c);
    lcd.fill(x + 4, y + 9, 3, 2, c);
    lcd.fill_r(x + 4, y + 3, 3, 4, 1, bg);
    if slash {
        lcd.line(x, y, x + 10, y + 10, bg);
        lcd.line(x + 1, y, x + 10, y + 9, c);
    }
}

/// A warning triangle, 13 x 12, with its "!" in bg.
pub fn warn_icon(lcd: &mut impl Panel, x: i32, y: i32, c: u16, bg: u16) {
    for i in 0..12 {
        let n = i / 2;
        lcd.fill(x + 6 - n, y + i, 2 * n + 1, 1, c);
    }
    lcd.fill(x + 6, y + 4, 1, 4, bg);
    lcd.fill(x + 6, y + 9, 1, 2, bg);
}

/// A progress bar: `pct` of w filled in `c`, the rest LINE.
pub fn progress(lcd: &mut impl Panel, x: i32, y: i32, w: i32, h: i32, pct: u8, c: u16) {
    let f = w * pct.min(100) as i32 / 100;
    lcd.fill(x, y, f, h, c);
    lcd.fill(x + f, y, w - f, h, LINE);
}

/// Text wrapped on words into lines `step` apart from `baseline`, in the box x..x+w, rows y0..y1; the rest of
/// the box painted bg. Returns the lines used.
#[cfg_attr(feature = "clipcheck", track_caller)]
pub fn wrap(lcd: &mut impl Panel, x: i32, w: i32, y0: i32, y1: i32, baseline: i32, step: i32, s: &[u8], f: &'static Font4, fg: u16, bg: u16) -> i32 {
    clip_site!();
    let (mut rest, mut base, mut n) = (s, baseline, 0);
    let top = baseline - f.ascent as i32;
    lcd.fill(x, y0, w, top - y0, bg);
    let mut y = top;
    while y < y1 {
        let mut cut = rest.len();
        if text4_width(rest, f) > w {
            cut = 0;
            for (i, &c) in rest.iter().enumerate() {
                if c == b' ' && text4_width(&rest[..i], f) <= w {
                    cut = i;
                }
            }
            if cut == 0 {
                cut = fit(rest, f, w).len();
            }
        }
        let line = &rest[..cut];
        let bottom = (base - f.ascent as i32 + step).min(y1);
        field(lcd, x, w, y, bottom, base, line, &st(f, fg, bg, Align::Left));
        if !line.is_empty() {
            n += 1;
        }
        rest = rest[cut..].strip_prefix(b" ").unwrap_or(&rest[cut..]);
        y = bottom;
        base += step;
    }
    #[cfg(feature = "clipcheck")]
    if !rest.is_empty() {
        crate::clipcheck::note("lines", s, w * n, text4_width(s, f));
    }
    n
}

/// A toast in place of the hint bar (HOME-9): bold head, then body.
/// The largest box [`popup`] draws (x, y, w, h), for clearing it.
pub const POPUP_BOX: (i32, i32, i32, i32) = (24, 128, 192, 64);

/// A hold-digit shortcut's popup in the middle of the content area (HOME-8): what ran, and under it the new
/// value when it has one.
#[cfg_attr(feature = "clipcheck", track_caller)]
pub fn popup(lcd: &mut impl Panel, head: &[u8], body: &[u8]) {
    clip_site!();
    let (x, _, w, _) = POPUP_BOX;
    let (y, h) = if body.is_empty() { (138, 44) } else { (128, 64) };
    // Off-screen: the box keeps whatever was under its rounded corners (DISP-25).
    crate::canvas::banded_over(lcd, x, y, w, h, |b| popup_now(b, head, body));
}

fn popup_now(lcd: &mut impl Panel, head: &[u8], body: &[u8]) {
    let (x, _, w, _) = POPUP_BOX;
    let (y, h) = if body.is_empty() { (138, 44) } else { (128, 64) };
    lcd.fill_r(x, y, w, h, 4, BLUE);
    lcd.fill_r(x + 1, y + 1, w - 2, h - 2, 3, TOAST);
    let hb = if body.is_empty() { y + 28 } else { y + 26 };
    lcd.text4_aligned(120, hb, Align::Center, cut(head, &UI_15B, w - 16), &UI_15B, TEXT, TOAST);
    if !body.is_empty() {
        lcd.text4_aligned(120, y + 50, Align::Center, cut(body, &UI_12, w - 16), &UI_12, TEXT, TOAST);
    }
}

/// Whether a toast's head and body fit the hint bar's one line as [`toast`] lays them out, with nothing cut
/// (HOME-9, INBOX-1); the toast tests check every toast against it.
pub fn toast_fits(head: &[u8], body: &[u8], icon: bool) -> bool {
    let x = if icon { 26 } else { 8 };
    let hw = text4_width(head, &UI_12B);
    if hw > 232 - x {
        return false;
    }
    body.is_empty() || text4_width(body, &UI_12) <= 232 - (x + hw + 7)
}

/// The room [`toast`] leaves its body after `head`, in pixels (0: none): to [`fit`] a body that isn't ours (a
/// message, an alert's event) to it.
pub fn toast_room(head: &[u8], icon: bool) -> i32 {
    let x = if icon { 26 } else { 8 } + text4_width(head, &UI_12B);
    if x < 225 {
        232 - (x + 7)
    } else {
        0
    }
}

/// Whether a shortcut popup's head and body fit their lines as [`popup`] lays them out (HOME-8).
pub fn popup_fits(head: &[u8], body: &[u8]) -> bool {
    let w = POPUP_BOX.2 - 16;
    text4_width(head, &UI_15B) <= w && text4_width(body, &UI_12) <= w
}

#[cfg_attr(feature = "clipcheck", track_caller)]
pub fn toast(lcd: &mut impl Panel, head: &[u8], body: &[u8], icon: bool) {
    clip_site!();
    crate::canvas::banded(lcd, 0, HINT_Y, 240, 20, TOAST, |b| toast_now(b, head, body, icon));
}

fn toast_now(lcd: &mut impl Panel, head: &[u8], body: &[u8], icon: bool) {
    lcd.fill(0, HINT_Y, 240, 1, BLUE);
    lcd.fill(0, HINT_Y + 1, 240, 19, TOAST);
    let mut x = 8;
    if icon {
        envelope(lcd, x, HINT_Y + 6, BLUE);
        x += 18;
    }
    let base = HINT_Y + 15;
    x += lcd.text4_rows(x, base, cut(head, &UI_12B, 232 - x), &UI_12B, TEXT, TOAST, HINT_Y + 1, HINT_Y + 20);
    if !body.is_empty() && x < 225 {
        x += if head.is_empty() { 0 } else { 7 }; // a body alone starts where a head would
        lcd.text4_rows(x, base, cut(body, &UI_12, 232 - x), &UI_12, TEXT, TOAST, HINT_Y + 1, HINT_Y + 20);
    }
}

/// A menu row: number, name (with a dot when not default, MENU-12), value at the right. Selected rows are
/// filled blue; greyed rows (MENU-14) draw their name dim.
pub struct Row<'a> {
    pub num: Option<u8>,
    pub name: &'a [u8],
    pub value: &'a [u8],
    pub dot: bool,
    pub selected: bool,
    pub grey: bool,
}

#[cfg_attr(feature = "clipcheck", track_caller)]
pub fn list_row(lcd: &mut impl Panel, y: i32, h: i32, r: &Row, cue: Cue, big: bool) {
    clip_site!();
    crate::canvas::banded(lcd, 0, y, 240, h, BG, |b| {
        list_row_now(b, y, h, r, big);
        cue.draw(b);
    });
}

fn list_row_now(lcd: &mut impl Panel, y: i32, h: i32, r: &Row, big: bool) {
    let bg = if r.selected { BLUE } else { BG };
    let (fg, sub) = if r.selected { (BLUE_FG, BLUE_FG) } else { (if r.grey { DIM } else { TEXT }, SUB) };
    // The menus' rows (DISP-6) are in the 14 px font, names and values.
    let nf: &Font4 = if big { &UI_14 } else { &UI_12 };
    let base = if big { y + h / 2 + 6 } else { y + h / 2 + 5 };
    let nbase = base;
    let x = if let Some(n) = r.num {
        field(lcd, 0, 25, y, y + h, nbase, &[b'0' + n % 10], &st(nf, sub, bg, Align::Center));
        25
    } else {
        lcd.fill(0, y, 8, h, bg);
        8
    };
    // The name keeps its room (all of it with no value); the value gets the rest.
    let name = cut(r.name, nf, 232 - x - if r.value.is_empty() { 8 } else { 40 });
    let nw = field(lcd, x, text4_width(name, nf), y, y + h, nbase, name, &st(nf, fg, bg, Align::Left));
    let x = x + nw;
    let value = cut(r.value, nf, 232 - x - if r.dot { 18 } else { 8 });
    let vw = text4_width(value, nf);
    let vx = 232 - vw;
    lcd.fill(x, y, vx - x, h, bg);
    if r.dot {
        lcd.fill_r(x + 4, base - 7, 6, 6, 3, if r.selected { BLUE_FG } else { AMBER });
    }
    field(lcd, vx, vw, y, y + h, base, value, &st(nf, sub, bg, Align::Right));
    lcd.fill(232, y, 8, h, bg);
}

/// How a label + value row is laid out. One drawer ([`kv`]) for every "label: value" row: the Status page and
/// the system pages ([`KV_STATUS`]), detail pages ([`KV_DETAIL`]) and a pair on a card ([`KV_PAIR`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KvStyle {
    /// Where the value starts; it runs to x = 232.
    pub vx: i16,
    /// The value against x = 232 instead of at vx.
    pub right: bool,
    /// The baseline below the row's top; 0 centres it in the row.
    pub base: i8,
    /// A LINE under the row, in its last pixel row.
    pub line: bool,
    /// A value too wide for one line goes on to a second (the row is then 36 px tall, else 19).
    pub wrap: bool,
}

/// The Status page: value at the right, a line under each row.
pub const KV_STATUS: KvStyle = KvStyle { vx: 108, right: true, base: 0, line: true, wrap: false };
/// A detail page (the AprsStation mock-up): value from x = 68, on two lines when it needs them.
pub const KV_DETAIL: KvStyle = KvStyle { vx: 68, right: false, base: 13, line: true, wrap: true };
/// A pair on a card ("Last alert   14:28"): no line, 17 px.
pub const KV_PAIR: KvStyle = KvStyle { vx: 90, right: false, base: 13, line: false, wrap: false };

/// A row's height: `h`, or for a wrapping style 19 px, 36 for a value on two lines.
pub fn kv_h(st: &KvStyle, value: &[u8], h: i32) -> i32 {
    if !st.wrap {
        h
    } else if text4_width(value, &UI_12) <= 232 - st.vx as i32 {
        19
    } else {
        36
    }
}

/// A label + value row from y, `h` tall ([`kv_h`]): the label in SUB at x = 8, the value in `vc`; all white on
/// blue when selected. Paints the whole row.
#[allow(clippy::too_many_arguments)]
#[cfg_attr(feature = "clipcheck", track_caller)]
pub fn kv(lcd: &mut impl Panel, y: i32, h: i32, st: &KvStyle, label: &[u8], value: &[u8], vc: u16, selected: bool, cue: Cue) {
    clip_site!();
    crate::canvas::banded(lcd, 0, y, 240, h, BG, |b| {
        kv_now(b, y, h, st, label, value, vc, selected);
        cue.draw(b);
    });
}

#[allow(clippy::too_many_arguments)]
fn kv_now(lcd: &mut impl Panel, y: i32, h: i32, st: &KvStyle, label: &[u8], value: &[u8], vc: u16, selected: bool) {
    let bg = if selected { BLUE } else { BG };
    let (lc, vc) = if selected { (BLUE_FG, BLUE_FG) } else { (SUB, vc) };
    // A value against the right edge may use the room a short label leaves (8 px clear of it).
    let vx = if st.right { (st.vx as i32).min(16 + text4_width(label, &UI_12)) } else { st.vx as i32 };
    let vw = 232 - vx;
    let y1 = y + h - st.line as i32;
    let base = y + if st.base == 0 { h / 2 + 5 } else { st.base as i32 };
    lcd.fill(0, y, 240, y1 - y, bg);
    lcd.text4_rows(8, base, label, &UI_12, lc, bg, y, y1);
    let mut v = fit(value, &UI_12, vw);
    // one line: the value cut to it (two: the first full, the second cut)
    #[cfg(feature = "clipcheck")]
    if !st.wrap && v.len() < value.len() {
        crate::clipcheck::note("cut", value, vw, text4_width(value, &UI_12));
    }
    if st.wrap && v.len() < value.len() {
        // the second line from the last space, comma or dash whose rest fits on it (a position's longitude stays
        // whole: "48°59.530'N" over "123°49.249'W"), else from where the first line is full
        let brk = kv_cut(value, v.len(), vw);
        lcd.text4_rows(vx, base + 17, cut(value[brk..].trim_ascii_start(), &UI_12, vw), &UI_12, vc, bg, y, y1);
        v = &value[..brk];
    }
    let x = if st.right { 232 - text4_width(v, &UI_12) } else { vx };
    lcd.text4_rows(x, base, v, &UI_12, vc, bg, y, y1);
    if st.line {
        lcd.fill(0, y1, 240, 1, LINE);
    }
}

/// A two-line kv value's break (APRS-5.1): after the last space, comma or dash in the first line's `first` bytes
/// whose rest fits the second line, else where the first line is full.
pub(crate) fn kv_cut(value: &[u8], first: usize, vw: i32) -> usize {
    value[..first]
        .iter()
        .enumerate()
        .rev()
        .filter(|&(_, &ch)| ch == b' ' || ch == b',' || ch == b'-')
        .map(|(p, _)| p + 1)
        .find(|&p| text4_width(value[p..].trim_ascii_start(), &UI_12) <= vw)
        .unwrap_or(first)
}

/// DISP-24: the scroll cue at the right edge of a list or table with more rows than fit: a 3 px track (LINE)
/// from `y` for `h`, and a thumb (SUB) for the page, placed at the first shown row's share of all `n` rows (at
/// least 12 px tall); 1 px corners (DISP-19). x 236-238, right of the values (they end at 232).
pub const CUE_X: i32 = 236;

/// A scroll cue on the screen (h 0: none): its track from y, h tall, and the thumb's place in it. A row redrawn
/// beside it draws its part of the cue in the same band, so the cue never blanks under a row's redraw (DISP-25).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cue {
    pub y: i16,
    pub h: i16,
    pub ty: i16,
    pub th: i16,
}

impl Cue {
    /// Rows `top..top + shown` of `n` beside a track from y, h tall: the thumb's share and place.
    pub fn new(y: i32, h: i32, top: u32, shown: u32, n: u32) -> Self {
        let n = n.max(1);
        let th = (h as u32 * shown.min(n) / n).max(12).min(h as u32) as i32;
        let room = (h - th).max(0) as u32;
        let span = n.saturating_sub(shown).max(1);
        let ty = y + (room * top.min(span) / span) as i32;
        Cue { y: y as i16, h: h as i16, ty: ty as i16, th: th as i16 }
    }

    /// The cue's pixels inside what `lcd` takes (a row's band clips it to the row).
    pub fn draw(self, lcd: &mut impl Panel) {
        if self.h > 0 {
            lcd.fill_r(CUE_X, self.y as i32, 3, self.h as i32, 1, LINE);
            lcd.fill_r(CUE_X, self.ty as i32, 3, self.th as i32, 1, SUB);
        }
    }
}

pub fn scroll_cue(lcd: &mut impl Panel, cue: Cue) {
    // Track and thumb off-screen, sent once (DISP-25); the corners keep the background.
    crate::canvas::banded_over(lcd, CUE_X, cue.y as i32, 3, cue.h as i32, |b| cue.draw(b));
}

/// The help line at the foot of a menu (MENU-15): a rounded box, the text wrapped on words, then the path.
#[cfg_attr(feature = "clipcheck", track_caller)]
pub fn help_box(lcd: &mut impl Panel, y: i32, h: i32, text: &[u8], path: &[u8]) {
    clip_site!();
    crate::canvas::banded(lcd, 0, y, 240, h, BG, |b| help_box_now(b, y, h, text, path));
}

fn help_box_now(lcd: &mut impl Panel, y: i32, h: i32, text: &[u8], path: &[u8]) {
    lcd.fill(0, y, 240, h, BG);
    lcd.fill_r(8, y, 224, h, 2, SURFACE);
    let max = 208;
    let (mut line, mut rest) = (0, text);
    while !rest.is_empty() && line < 2 {
        // the longest run of whole words that fits
        let mut cut = rest.len();
        if text4_width(rest, &UI_12) > max {
            cut = 0;
            for (i, &c) in rest.iter().enumerate() {
                if c == b' ' && text4_width(&rest[..i], &UI_12) <= max {
                    cut = i;
                }
            }
            if cut == 0 {
                cut = fit(rest, &UI_12, max).len();
            }
        }
        let base = y + 13 + line * 15;
        let w = lcd.text4_rows(16, base, &rest[..cut], &UI_12, TEXT, SURFACE, y + 2, y + h - 2);
        rest = rest[cut..].strip_prefix(b" ").unwrap_or(&rest[cut..]);
        if rest.is_empty() && !path.is_empty() {
            let pw = text4_width(path, &UI_12);
            if 16 + w + 7 + pw <= 16 + max {
                lcd.text4_rows(16 + w + 7, base, path, &UI_12, SUB, SURFACE, y + 2, y + h - 2);
            } else if line == 0 {
                lcd.text4_rows(16, base + 15, path, &UI_12, SUB, SURFACE, y + 2, y + h - 2);
            }
        }
        line += 1;
    }
    #[cfg(feature = "clipcheck")]
    if !rest.is_empty() {
        crate::clipcheck::note("lines", text, 2 * max, text4_width(text, &UI_12));
    }
}

/// A setting in the edit grid: label above, value below; the focused one is blue with the knob mark.
#[allow(clippy::too_many_arguments)]
#[cfg_attr(feature = "clipcheck", track_caller)]
pub fn edit_cell(lcd: &mut impl Panel, x: i32, y: i32, w: i32, h: i32, label: &[u8], value: &[u8], focused: bool, grey: bool) {
    clip_site!();
    crate::canvas::banded(lcd, x, y, w, h, BG, |b| edit_cell_now(b, x, y, w, h, label, value, focused, grey));
}

#[allow(clippy::too_many_arguments)]
fn edit_cell_now(lcd: &mut impl Panel, x: i32, y: i32, w: i32, h: i32, label: &[u8], value: &[u8], focused: bool, grey: bool) {
    let (bg, border, lc) = if focused { (FOCUS, BLUE, BLUE) } else { (BG, LINE, SUB) };
    lcd.fill(x, y, w, h, bg);
    // the two lines are laid out for 29 px; a taller cell moves them down to keep them centred
    let (y0, y) = (y, y + (h - 29) / 2);
    let lw = lcd.text4_rows(x + 6, y + 12, label, &UI_12, lc, bg, y0 + 1, y + 15);
    if focused && lw + 14 < w - 6 {
        knob_mark(lcd, x + 6 + lw + 4, y + 4, BLUE, bg);
    }
    let v = cut(value, &UI_12, w - 10);
    lcd.text4_rows(x + 6, y + 26, v, &UI_12, if grey { DIM } else { TEXT }, bg, y + 15, y0 + h - 1);
    let y = y0;
    lcd.frame_r(x, y, w, h, 1, border);
}

/// The knob mark: a small ring, open at the top right, with a tick (the knob changes this).
pub fn knob_mark(lcd: &mut impl Panel, x: i32, y: i32, c: u16, bg: u16) {
    lcd.frame_r(x, y, 8, 8, 3, c);
    lcd.fill(x + 5, y, 3, 3, bg);
    lcd.fill(x + 4, y, 3, 1, c);
    lcd.fill(x + 6, y + 1, 1, 2, c);
}

/// What the status bar shows (manual chapter 8). Compared with what is on screen, so only changed parts redraw.
#[derive(Clone, Copy, PartialEq, Default)]
pub struct StatusBar {
    /// The screen's own name; empty on home (SBAR-5).
    pub name: Buf<10>,
    pub locked: bool,
    /// The lock covers PTT too (KEY-14.3, SBAR-3): the padlock in red.
    pub lock_ptt: bool,
    pub scan: bool,
    /// Watched slots: 3 = TW, 2 = DW, else nothing (SBAR-16).
    pub watch: u8,
    /// VOX on (0 8 2, SBAR-25): `VOX` in amber after TW/DW, as speech can key the radio.
    pub vox: bool,
    /// A speaker mic plugged in (SND-12, SBAR-26): `SPK` after VOX, so it's clear which mic is live.
    pub spk: bool,
    pub inbox: u8,
    pub alert: bool,
    pub clock: Option<u16>,
    pub utc: bool,
    /// The 12 h clock (MENU 0 6 6, SBAR-2.1): `2:32p`; UTC stays 24 h, as it always is (`14:32Z`).
    pub h12: bool,
    /// The callsign, drawn in the clock's place while there's no time (SBAR-2); empty with none set. The slot holds
    /// 6 characters (AX.25's, SETUP-13), so a longer callsign keeps its first 6.
    pub call: Buf<6>,
    pub gps: GpsState,
    /// SBAR-8: Bluetooth on (0 9 2 1), and a device connected.
    pub bt_on: bool,
    pub bt: bool,
    /// 0-4 bars.
    pub battery: u8,
    pub battery_low: bool,
    /// SBAR-9.1: the fill's colour, `ferrite_core::battery::tier`'s (0 green, 1 amber, 2 red).
    pub battery_tier: u8,
    pub battery_cutoff: bool,
    /// The cut-off blink's off phase.
    pub blink_off: bool,
    /// Weather alerts paused (HF mode with SAME on): a slashed bell after the inbox (SBAR-18).
    pub alerts_paused: bool,
    /// Dry TX on (TX-48, SBAR-22): "DRY" left of the Bluetooth and GPS marks.
    pub dry: bool,
    /// Low Power Mode (PWR-24, SBAR-24): "LOW" left of those.
    pub lpm: bool,
    /// SAT-20: a pass alarm set: 0 none; else [`SAT_UP`] while overhead, or the minutes to the rise (up to
    /// [`SAT_UP`] - 1), with [`SAT_TEXT`] while the time shows instead of the satellite (every other 2 s).
    pub sat: u16,
}

/// [`StatusBar::sat`]: the satellite overhead now; the time shown, not the icon.
pub const SAT_UP: u16 = 0x3FFF;
pub const SAT_TEXT: u16 = 0x8000;

impl StatusBar {
    fn left_eq(&self, o: &Self) -> bool {
        (self.name, self.locked, self.lock_ptt, self.scan, self.watch, self.vox, self.spk, self.inbox, self.alert, self.alerts_paused)
            == (o.name, o.locked, o.lock_ptt, o.scan, o.watch, o.vox, o.spk, o.inbox, o.alert, o.alerts_paused)
    }

    fn right_eq(&self, o: &Self) -> bool {
        (self.gps, self.bt_on, self.bt, self.battery, self.battery_low, self.battery_tier, self.battery_cutoff, self.blink_off, self.dry, self.lpm, self.sat)
            == (o.gps, o.bt_on, o.bt, o.battery, o.battery_low, o.battery_tier, o.battery_cutoff, o.blink_off, o.dry, o.lpm, o.sat)
    }

    /// Draws what differs from `old` (everything with None).
    pub fn draw(&self, lcd: &mut impl Panel, old: Option<&StatusBar>) {
        if old.is_none() {
            lcd.fill(0, SBAR_H - 1, 240, 1, RULE);
        }
        // Each half drawn off-screen and sent once (DISP-25): it was blanked and drawn, which flickered.
        if old.is_none_or(|o| !self.left_eq(o)) {
            crate::canvas::banded(lcd, 0, 0, 98, SBAR_H - 1, BG, |b| self.draw_left(b));
        }
        if old.is_none_or(|o| (o.clock, o.utc, o.h12, o.call) != (self.clock, self.utc, self.h12, self.call)) {
            let mut b = Buf::<8>::new();
            let t = match self.clock {
                Some(m) => {
                    let _ = ufmt::uwrite!(b, "{}", crate::text::Clock(m, self.h12 && !self.utc));
                    if self.utc {
                        b.push(b"Z");
                    }
                    b.as_bytes()
                }
                // SBAR-2: no time, the callsign in its place (cut to the slot), or nothing.
                None => self.call.as_bytes(),
            };
            // 98..142: wide enough for `14:32Z` (41 px) or a 6-character callsign, still centred on 120 (SBAR-2).
            field(lcd, 98, 44, 0, SBAR_H - 1, 14, t, const { &st(&UI_12B, KEYS, BG, Align::Center) });
        }
        if old.is_none_or(|o| !self.right_eq(o)) {
            crate::canvas::banded(lcd, 142, 0, 98, SBAR_H - 1, BG, |b| self.draw_right(b));
        }
    }

    fn draw_left(&self, lcd: &mut impl Panel) {
        let (y0, y1, base, gap, x_end) = (0, SBAR_H - 1, 14, 5, 98);
        let mut count = Buf::<4>::new();
        let _ = ufmt::uwrite!(count, "{}", self.inbox.min(99));
        // Icons first, in order (SBAR-3): the name gets what is left of the ~90 px (SBAR-5).
        let icons = if self.locked { 14 + gap } else { 0 }
            + if self.scan { 21 + gap } else { 0 }
            + if self.watch >= 2 { 14 + gap } else { 0 }
            + if self.vox { text4_width(b"VOX", &UI_12) + gap } else { 0 }
            + if self.spk { text4_width(b"SPK", &UI_12) + gap } else { 0 }
            + if self.inbox > 0 { 14 + text4_width(count.as_bytes(), &UI_12B) + gap } else { 0 }
            + if self.alerts_paused { 11 + gap } else { 0 };
        let mut x = 8;
        lcd.fill(0, y0, 8, y1, BG);
        if !self.name.is_empty() {
            let name = fit(self.name.as_bytes(), &UI_12, 90 - icons);
            x += field(lcd, x, text4_width(name, &UI_12), y0, y1, base, name, const { &st(&UI_12, SUB, BG, Align::Left) });
            lcd.fill(x, y0, gap, y1, BG);
            x += gap;
        }
        if self.locked {
            lcd.mask4(x, 2, &ICON_LOCK, if self.lock_ptt { RED } else { SUB }, None, BG);
            lcd.fill(x, 16, 14, 3, BG);
            lcd.fill(x + 14, y0, gap, y1, BG);
            x += 14 + gap;
        }
        for (on, t) in [(self.scan, b"SCN".as_slice()), (self.watch >= 2, if self.watch >= 3 { b"TW" } else { b"DW" })] {
            if on {
                let w = text4_width(t, &UI_12);
                field(lcd, x, w + gap, y0, y1, base, t, const { &st(&UI_12, SUB, BG, Align::Left) });
                x += w + gap;
            }
        }
        // SBAR-25: VOX on, in amber (attention, SBAR-12): speech keys the radio.
        if self.vox {
            let w = text4_width(b"VOX", &UI_12);
            field(lcd, x, w + gap, y0, y1, base, b"VOX", const { &st(&UI_12, AMBER, BG, Align::Left) });
            x += w + gap;
        }
        // SBAR-26: a speaker mic plugged in, in grey (a state, not a warning).
        if self.spk {
            let w = text4_width(b"SPK", &UI_12);
            field(lcd, x, w + gap, y0, y1, base, b"SPK", const { &st(&UI_12, SUB, BG, Align::Left) });
            x += w + gap;
        }
        if self.inbox > 0 {
            let c = if self.alert { RED } else { SUB };
            lcd.fill(x, y0, 14, y1, BG);
            envelope(lcd, x, 5, c);
            x += 14;
            let w = text4_width(count.as_bytes(), &UI_12B);
            field(lcd, x, w, y0, y1, base, count.as_bytes(), &st(&UI_12B, if self.alert { RED } else { TEXT }, BG, Align::Left));
            x += w;
        }
        if self.alerts_paused {
            lcd.fill(x, y0, gap, y1, BG);
            x += gap;
            lcd.fill(x, y0, 11, y1, BG);
            bell(lcd, x, 4, SUB, BG, true);
            x += 11;
        }
        lcd.fill(x, y0, x_end - x, y1, BG);
    }

    fn draw_right(&self, lcd: &mut impl Panel) {
        lcd.fill(142, 0, 98, SBAR_H - 1, BG);
        // Battery at the right edge (SBAR-9): body 17 x 10 and a nub.
        // SBAR-9.1: green above 80 %, amber to 31 %, red at 30 % and below; the outline red with it.
        let c = if self.battery_cutoff || self.battery_tier >= 2 { RED } else { SUB };
        let fillc = match self.battery_tier {
            _ if self.battery_cutoff => RED,
            0 => BATT,
            1 => AMBER,
            _ => RED,
        };
        if !(self.battery_cutoff && self.blink_off) {
            lcd.frame_r(212, 5, 17, 10, 1, c);
            lcd.fill(229, 8, 2, 4, c);
            let bars = if self.battery_cutoff { 1 } else { self.battery.clamp(1, 4) as i32 };
            lcd.fill(214, 7, bars * 13 / 4, 6, fillc);
        }
        let mut x = 212;
        // SBAR-8: on with nothing connected grey, connected blue; night red has only red, so there connected is the
        // rune cut out of a filled badge and on the rune alone.
        if self.bt_on || self.bt {
            x -= 16;
            if crate::theme::get() == crate::theme::NIGHT_RED && self.bt {
                lcd.fill_r(x - 1, 3, 13, 13, 2, BLUE);
                lcd.mask4(x, 4, &ICON_BT, BG, None, BLUE);
            } else {
                lcd.mask4(x, 4, &ICON_BT, if self.bt { BLUE } else { SUB }, None, BG);
            }
        }
        if self.gps != GpsState::Off {
            x -= 17;
            // SBAR-7: searching dim, the ring only; a fix green with its centre filled (a colour and a shape).
            if self.gps == GpsState::Fix {
                lcd.mask4(x, 4, &ICON_GPS, GREEN, None, BG);
                lcd.fill_r(x + 3, 7, 6, 6, 3, GREEN);
            } else {
                lcd.mask4(x, 4, &ICON_GPS, DIM, None, BG);
            }
        }
        // SBAR-22: Dry TX in amber (night red's red, cyberpunk's magenta), a word as DISP-10 asks: 151..175 at most.
        if self.dry {
            let w = text4_width(b"DRY", &UI_12B);
            x -= w + 4;
            field(lcd, x, w, 0, SBAR_H - 1, 14, b"DRY", const { &st(&UI_12B, AMBER, BG, Align::Left) });
        }
        // SBAR-24: Low Power Mode in the battery's fill colour (a saving, not a warning). GPS and Bluetooth are off
        // in it, so with DRY too it ends at 155 or so, inside the right half.
        if self.lpm {
            let w = text4_width(b"LOW", &UI_12B);
            x -= w + 4;
            field(lcd, x, w, 0, SBAR_H - 1, 14, b"LOW", const { &st(&UI_12B, BATT, BG, Align::Left) });
        }
        // SAT-20: the pass alarm: a satellite riding a dashed arc, every other 2 s the time to the rise ("12",
        // "3H") or UP; white over an hour away, amber within it, green overhead. 16 px, left of the others.
        if self.sat != 0 {
            x -= 20;
            let m = self.sat & SAT_UP;
            let c = if m == SAT_UP { GREEN } else if m <= 60 { AMBER } else { TEXT };
            if self.sat & SAT_TEXT != 0 {
                let mut b = Buf::<4>::new();
                if m == SAT_UP {
                    b.set(b"UP");
                } else if m <= 99 {
                    let _ = ufmt::uwrite!(b, "{}", crate::text::Zero(m as u32, 2));
                } else {
                    let _ = ufmt::uwrite!(b, "{}H", (m / 60).min(9));
                }
                field(lcd, x, 18, 0, SBAR_H - 1, 14, b.as_bytes(), &st(&UI_12B, c, BG, Align::Center));
            } else {
                sat_icon(lcd, x + 2, 3, c);
            }
        }
    }
}

/// SAT-20's mark, 15 x 12 from rectangles: a dashed arc (its path's dots, two on, two off) with the satellite's
/// body and panels at the top.
fn sat_icon(lcd: &mut impl Panel, x: i32, y: i32, c: u16) {
    for k in 0..=13 {
        // y = 10 - 16 t (1 - t) along x = 1 + 13 t, in 1/13 steps
        if k % 4 < 2 {
            lcd.fill(x + 1 + k, y + 10 - 16 * k * (13 - k) / 169, 1, 1, DIM);
        }
    }
    lcd.fill(x + 5, y + 2, 5, 4, c);
    lcd.fill(x + 1, y + 3, 3, 2, c);
    lcd.fill(x + 11, y + 3, 3, 2, c);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fb::FrameBuffer;

    /// APRS-5.1: a position in degrees and minutes breaks between latitude and longitude, never inside one (a
    /// lone "W" on the second line, found on the clear radio, Oct 22, 2026).
    #[test]
    fn two_line_value_breaks_between_words() {
        let v = "48\u{b0}59.530'N 123\u{b0}49.249'W".as_bytes();
        let vw = 232 - 104;
        let first = fit(v, &UI_12, vw).len();
        assert!(first < v.len(), "too long for one line, as on the radio");
        let cut = kv_cut(v, first, vw);
        assert_eq!(&v[cut..], "123\u{b0}49.249'W".as_bytes());
    }

    #[test]
    fn meter_levels() {
        assert_eq!(meter_level(-140), 0);
        assert_eq!(meter_level(-85), 13); // S7, as in the HomeA mock-up
        assert_eq!(meter_level(-73), 17);
        assert_eq!(meter_level(0), METER_SEGS);
        assert_eq!(s_text(-85).as_bytes(), b"S7");
        assert_eq!(s_text(-60).as_bytes(), b"+10");
        assert_eq!(s_text(-140).as_bytes(), b"S0");
    }

    #[test]
    fn fit_whole_chars() {
        let s = "A·B·C".as_bytes();
        assert_eq!(fit(s, &UI_12, 1000), s);
        assert_eq!(fit(s, &UI_12, 14), "A·".as_bytes()); // never half a middle dot
        assert_eq!(fit(s, &UI_12, 0), b"");
    }

    #[test]
    fn field_paints_its_box_only() {
        let mut fb = FrameBuffer::new();
        fb.px.fill(0x1234);
        field(&mut fb, 20, 100, 30, 47, 43, b"Narrow", const { &st(&UI_12, TEXT, BG, Align::Right) });
        assert_eq!(fb.at(19, 35), 0x1234);
        assert_eq!(fb.at(120, 35), 0x1234);
        assert_eq!(fb.at(20, 29), 0x1234);
        assert_eq!(fb.at(20, 47), 0x1234);
        assert_eq!(fb.at(21, 31), BG);
        assert!((70..120).any(|x| fb.at(x, 40) != BG), "text at the right");
    }

    #[test]
    fn status_bar_redraws_only_changes() {
        struct Count(FrameBuffer, u64);
        impl Panel for Count {
            fn window(&mut self, a: i32, b: i32, c: i32, d: i32) {
                self.0.window(a, b, c, d)
            }
            fn push(&mut self, c: u16, n: u32) {
                self.1 += n as u64;
                self.0.push(c, n)
            }
            fn push_buf(&mut self, p: &[u16]) {
                self.1 += p.len() as u64;
                self.0.push_buf(p)
            }
        }
        let mut lcd = Count(FrameBuffer::new(), 0);
        let a = StatusBar { clock: Some(14 * 60 + 32), watch: 3, gps: GpsState::Fix, battery: 4, ..Default::default() };
        a.draw(&mut lcd, None);
        assert!(lcd.1 >= 240 * 20);
        lcd.1 = 0;
        a.draw(&mut lcd, Some(&a));
        assert_eq!(lcd.1, 0, "nothing changed, nothing drawn");
        let b = StatusBar { clock: Some(14 * 60 + 33), ..a };
        b.draw(&mut lcd, Some(&a));
        assert!(lcd.1 > 0 && lcd.1 <= 44 * 19, "only the clock (its 44 px field): {} px", lcd.1);
    }

    /// SBAR-22, TX-48: Dry TX's "DRY" sits left of the GPS and Bluetooth marks, inside the right half, in every
    /// theme; night red stays red or black (DISP-18).
    #[test]
    fn status_bar_dry_mark() {
        for t in [crate::theme::DARK, crate::theme::DAYLIGHT, crate::theme::NIGHT_RED, crate::theme::CYBERPUNK] {
            crate::theme::set(t);
            let off = StatusBar { gps: GpsState::Fix, bt: true, battery: 4, ..Default::default() };
            let on = StatusBar { dry: true, ..off };
            let (mut a, mut b) = (FrameBuffer::new(), FrameBuffer::new());
            off.draw(&mut a, None);
            on.draw(&mut b, None);
            let diff: std::vec::Vec<(i32, i32)> =
                (0..SBAR_H).flat_map(|y| (0..240).map(move |x| (x, y))).filter(|&(x, y)| a.at(x, y) != b.at(x, y)).collect();
            assert!(!diff.is_empty(), "theme {t}: DRY drawn");
            assert!(diff.iter().all(|&(x, _)| (142..212 - 16 - 17).contains(&x)), "theme {t}: inside 142..179");
            if t == crate::theme::NIGHT_RED {
                assert!(diff.iter().all(|&(x, y)| b.at(x, y) & 0x07FF == 0), "night red: red or black only");
            }
            // Turning it off redraws the right half only.
            off.draw(&mut b, Some(&on));
            assert!(b.px == a.px, "theme {t}: off again as before");
        }
        crate::theme::set(crate::theme::DARK);
    }

    /// SBAR-26: a speaker mic plugged in draws "SPK" in the left half beside VOX, and nothing else; the screen
    /// name gives way for it (SBAR-5).
    #[test]
    fn status_bar_spk_mark() {
        let off = StatusBar { scan: true, watch: 3, vox: true, inbox: 12, battery: 4, ..Default::default() };
        let on = StatusBar { spk: true, ..off };
        let (mut a, mut b) = (FrameBuffer::new(), FrameBuffer::new());
        off.draw(&mut a, None);
        on.draw(&mut b, None);
        let diff: std::vec::Vec<(i32, i32)> =
            (0..SBAR_H).flat_map(|y| (0..240).map(move |x| (x, y))).filter(|&(x, y)| a.at(x, y) != b.at(x, y)).collect();
        assert!(!diff.is_empty(), "SPK drawn");
        assert!(diff.iter().all(|&(x, _)| x < 98), "inside the left half");
        off.draw(&mut b, Some(&on));
        assert!(b.px == a.px, "off again as before");
    }

    /// SBAR-25: VOX on draws "VOX" in the left half (before the clock's 98) and nothing else, in every theme;
    /// night red stays red or black (DISP-18).
    #[test]
    fn status_bar_vox_mark() {
        for t in [crate::theme::DARK, crate::theme::DAYLIGHT, crate::theme::NIGHT_RED, crate::theme::CYBERPUNK] {
            crate::theme::set(t);
            let off = StatusBar { scan: true, watch: 3, inbox: 12, battery: 4, ..Default::default() };
            let on = StatusBar { vox: true, ..off };
            let (mut a, mut b) = (FrameBuffer::new(), FrameBuffer::new());
            off.draw(&mut a, None);
            on.draw(&mut b, None);
            let diff: std::vec::Vec<(i32, i32)> =
                (0..SBAR_H).flat_map(|y| (0..240).map(move |x| (x, y))).filter(|&(x, y)| a.at(x, y) != b.at(x, y)).collect();
            assert!(!diff.is_empty(), "theme {t}: VOX drawn");
            assert!(diff.iter().all(|&(x, _)| x < 98), "theme {t}: inside the left half");
            if t == crate::theme::NIGHT_RED {
                assert!(diff.iter().all(|&(x, y)| b.at(x, y) & 0x07FF == 0), "night red: red or black only");
            }
            off.draw(&mut b, Some(&on));
            assert!(b.px == a.px, "theme {t}: off again as before");
        }
        crate::theme::set(crate::theme::DARK);
    }

    /// SBAR-7: the GPS mark with a fix is green with its centre filled; searching, dim with the centre empty.
    #[test]
    fn status_bar_gps_fix_stands_out() {
        crate::theme::set(crate::theme::DARK);
        let (mut a, mut b) = (FrameBuffer::new(), FrameBuffer::new());
        StatusBar { gps: GpsState::Searching, ..Default::default() }.draw(&mut a, None);
        StatusBar { gps: GpsState::Fix, ..Default::default() }.draw(&mut b, None);
        let x = 212 - 17;
        let px = |f: &FrameBuffer, c: u16| (0..SBAR_H).flat_map(|y| (x..x + 12).map(move |x| (x, y))).filter(|&(x, y)| f.at(x, y) == c).count();
        assert!(px(&b, GREEN) > 20 && px(&b, DIM) == 0, "a fix: green");
        assert!(px(&a, DIM) > 0 && px(&a, SUB) == 0 && px(&a, GREEN) == 0, "searching: dim");
        assert_ne!(a.at(x + 6, 10), b.at(x + 6, 10), "the centre filled only with a fix");
    }

    /// SBAR-24, PWR-24: Low Power Mode's "LOW" sits left of the battery (GPS and Bluetooth are off in the mode),
    /// inside the right half, with DRY beside it too, in every theme.
    #[test]
    fn status_bar_lpm_mark() {
        for t in [crate::theme::DARK, crate::theme::DAYLIGHT, crate::theme::NIGHT_RED, crate::theme::CYBERPUNK] {
            crate::theme::set(t);
            let off = StatusBar { battery: 2, ..Default::default() };
            for on in [StatusBar { lpm: true, ..off }, StatusBar { lpm: true, dry: true, ..off }] {
                let (mut a, mut b) = (FrameBuffer::new(), FrameBuffer::new());
                off.draw(&mut a, None);
                on.draw(&mut b, None);
                let diff: std::vec::Vec<(i32, i32)> =
                    (0..SBAR_H).flat_map(|y| (0..240).map(move |x| (x, y))).filter(|&(x, y)| a.at(x, y) != b.at(x, y)).collect();
                assert!(!diff.is_empty(), "theme {t}: LOW drawn");
                assert!(diff.iter().all(|&(x, _)| (142..212).contains(&x)), "theme {t}: inside 142..212");
                off.draw(&mut b, Some(&on));
                assert!(b.px == a.px, "theme {t}: off again as before");
            }
        }
        crate::theme::set(crate::theme::DARK);
    }

    #[test]
    fn widgets_draw_inside_their_boxes() {
        let mut fb = FrameBuffer::new();
        fb.px.fill(0x1234);
        freq(&mut fb, 15, 225, 89, 14_652_000, &DIG_46, &DIG_28, TEXT, SUB, SURFACE);
        assert!(fb.at(15, 89 - digit_h(&DIG_46) - 1) == 0x1234, "freq painted above its digits");
        assert!(fb.at(15, 90) == 0x1234, "freq painted below the baseline");
        chips(&mut fb, 15, 97, 210, &[b"NFM", "T 100.0".as_bytes()], SURFACE);
        meter(&mut fb, 15, 124, None, 7, GREEN, SURFACE);
        assert_eq!(fb.at(16, 128), GREEN);
        assert_eq!(fb.at(15 + 8 * 10 + 1, 128), LINE);
        hint_bar(&mut fb, &[("Hold ABC", "Edit"), ("OK", "Quick"), (LEFT, "Band")]);
        hint_bar(&mut fb, &[(UP, ""), ("Knob", "Change")]);
        toast(&mut fb, b"VE3KPW-9", b"On my way", true);
        list_row(&mut fb, 41, 22, &Row { num: Some(1), name: b"Squelch", value: b"4", dot: true, selected: true, grey: false }, Cue::default(), false);
        kv(&mut fb, 20, 18, &KV_STATUS, b"Transmit", b"Ready", GREEN, false, Cue::default());
        help_box(&mut fb, 265, 33, b"Audio opens above this level. Home: SIDE1 + knob.", b"MENU 1");
        edit_cell(&mut fb, 161, 85, 72, 31, b"Tone", b"T 100.0", true, false);
        badge(&mut fb, 14, 33, 2, false, SURFACE);
        tag(&mut fb, 226, 32, b"RX", GREEN, SURFACE);
        // nothing escaped the screen's bands: the status bar rows were never touched
        assert!((0..240).all(|x| fb.at(x, 0) == 0x1234));
    }

    /// SBAR-9.1: the battery's fill is green above 80 %, amber to 31 %, red at 30 % and below, the outline red with
    /// it; at cut-off it is red and blinks (gone in the blink's off phase).
    #[test]
    fn status_bar_battery_colours() {
        let bar = |tier: u8, cutoff: bool, blink_off: bool| {
            let mut fb = FrameBuffer::new();
            StatusBar { battery: 4, battery_tier: tier, battery_cutoff: cutoff, blink_off, ..Default::default() }.draw(&mut fb, None);
            (fb.at(216, 9), fb.at(212, 10))
        };
        let m = crate::theme::map;
        assert_eq!(bar(0, false, false), (m(BATT), m(SUB)));
        assert_eq!(bar(1, false, false), (m(AMBER), m(SUB)));
        assert_eq!(bar(2, false, false), (m(RED), m(RED)));
        assert_eq!(bar(0, true, false), (m(RED), m(RED)));
        assert_eq!(bar(0, true, true), (BG, BG), "the blink's off phase");
    }

    /// SBAR-2: the clock as hh:mm with Z when it is UTC; with no time, the callsign in its place.
    #[test]
    fn status_bar_clock_or_callsign() {
        let field_px = |s: &StatusBar| {
            let mut fb = FrameBuffer::new();
            s.draw(&mut fb, None);
            (0..SBAR_H - 1).flat_map(|y| (98..142).map(move |x| (x, y))).map(|(x, y)| fb.at(x, y)).collect::<std::vec::Vec<u16>>()
        };
        let local = field_px(&StatusBar { clock: Some(14 * 60 + 32), ..Default::default() });
        let utc = field_px(&StatusBar { clock: Some(14 * 60 + 32), utc: true, ..Default::default() });
        let call = field_px(&StatusBar { call: Buf::from(b"VA7FER"), ..Default::default() });
        let none = field_px(&StatusBar::default());
        assert!(none.iter().all(|&c| c == BG), "nothing without either");
        assert!(local != none && utc != local && call != none && call != local);
        let ink = |v: &[u16]| v.iter().filter(|&&c| c != BG).count();
        assert!(ink(&utc) > ink(&local), "the Z added");
        // SBAR-2.1: 12 h draws differently, and not in UTC (which stays 24 h); `12:32p` fits the field.
        let h12 = field_px(&StatusBar { clock: Some(14 * 60 + 32), h12: true, ..Default::default() });
        assert!(h12 != local);
        assert_eq!(field_px(&StatusBar { clock: Some(14 * 60 + 32), utc: true, h12: true, ..Default::default() }), utc);
        assert!(crate::gfx::text4_width(b"12:32p", &UI_12B) <= 44, "the longest fits the 44 px field");
    }

    /// A tag is outlined in its colour; in night red it is filled (red, with black text: DISP-18).
    #[test]
    fn tags_fill_in_night_red() {
        let mut fb = FrameBuffer::new();
        let w = tag(&mut fb, 226, 32, b"RX", GREEN, SURFACE);
        assert_eq!((fb.at(226 - w + 2, 34), fb.at(226 - w, 40)), (crate::theme::map(SURFACE), crate::theme::map(GREEN)));
        crate::theme::set(crate::theme::NIGHT_RED);
        let mut fb = FrameBuffer::new();
        tag(&mut fb, 226, 32, b"RX", GREEN, SURFACE);
        assert_eq!(fb.at(226 - w + 2, 34), crate::theme::NIGHT_INK, "filled red");
        crate::theme::set(crate::theme::DARK);
    }

    /// A row of chips stops at the first that wouldn't fit (never a cut chip); the selected one has a blue outline.
    #[test]
    fn chips_stop_at_the_edge() {
        let mut fb = FrameBuffer::new();
        let items: [&[u8]; 4] = [b"One", b"Two", b"", b"Three hundred"];
        chips_sel(&mut fb, 10, 50, 80, &items, Some(1), SURFACE);
        let w0 = text4_width(b"One", &UI_12) + 10;
        assert_eq!(fb.at(10 + w0 + 4, 58), crate::theme::map(BLUE), "the second chip's outline is blue");
        assert!((50..67).all(|y| (10 + 80..240).all(|x| fb.at(x, y) == 0)), "nothing past the row's width");
    }

    /// HOME-9: a toast's head must fit beside its icon, and the body after it.
    #[test]
    fn toast_fit() {
        assert!(toast_fits(b"VE3KPW-9", b"On my way", true));
        assert!(toast_fits(b"VE3KPW-9", b"", true));
        assert!(!toast_fits(&[b'W'; 40], b"", false), "a head too wide");
        assert!(!toast_fits(b"VE3KPW-9", &[b'W'; 40], false), "a body too wide");
        assert_eq!(dbm_text(-200).as_bytes(), b"-140 dBm");
        assert_eq!(dbm_text(-93).as_bytes(), b"-93 dBm");
    }

    /// A word longer than the box is cut where the line is full (wrapped text, the menu's help box); the help box
    /// keeps two lines at most.
    #[test]
    fn long_words_are_cut_at_the_box() {
        let long = [b'M'; 60];
        let mut fb = FrameBuffer::new();
        let lines = wrap(&mut fb, 10, 100, 100, 160, 113, 15, &long, &UI_12, TEXT, BG);
        assert!(lines >= 2, "{lines} lines");
        assert!((100..160).all(|y| (110..240).all(|x| fb.at(x, y) == 0)), "nothing past the box");
        let mut fb = FrameBuffer::new();
        help_box(&mut fb, 200, 50, &long, b"MENU 1");
        assert!((200..250).any(|y| (16..224).any(|x| fb.at(x, y) == crate::theme::map(TEXT))));
    }
}
