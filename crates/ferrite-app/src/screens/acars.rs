//! ACARS (MENU 0 3 8; chapter 27b, on trial; B26): the messages heard, newest first, a two-line row each (its
//! kind, the flight, the time; the airline, the aircraft and the label under it), and one message's page (every
//! field, the label's meaning, the airline's name, the text wrapped and scrolled). # puts the other slot (B) in a
//! one-line strip over the list; * shows only the messages with text (ACARS-4.2). While it is open the radio
//! listens (ACARS-11); the simulator and `acars feed` give it sample traffic.

use super::lists::*;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::acars::FREQS;
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::acars::{data_kind, label, label_text, DataKind, Msg, CUT, GAP, JOIN_MAX, NAK};
use ferrite_core::airlines::{self, Airline};
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// The airline table not looked at yet this time the screen is open (ACARS-6).
const UNCHECKED: u16 = u16::MAX;
/// The strip's height (ACARS-5): the slot's line and a rule under it.
const STRIP_H: i32 = 26;
/// The slot the strip shows: B, on the UHF chip, while the VHF chip listens for ACARS.
const STRIP_SLOT: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Acars {
    pub cur: Cursor,
    /// One message's page (the selected one).
    pub detail: bool,
    /// ACARS-5: slot B's strip over the list.
    pub strip: bool,
    /// The message page's first text line shown.
    pub line: u8,
    /// The airline table's rows ([`UNCHECKED`] until the first draw; 0: none).
    pub air: u16,
}

/// ACARS-1: the list, on the newest message.
pub fn open(_app: &mut App) -> Screen {
    Screen::Acars(Acars { cur: Cursor::default(), detail: false, strip: false, line: 0, air: UNCHECKED })
}

fn count(st: &impl ChannelStore) -> u16 {
    st.data().acars().map_or(0, |a| a.len())
}

/// ACARS-4.2: the messages the list shows: all, or with `only` those with text.
fn kept(st: &impl ChannelStore, only: bool) -> impl Fn(u16) -> bool + '_ {
    move |i| !only || st.data().acars().and_then(|a| a.get(i, &mut [])).is_some_and(|(m, _)| m.has_text() && !m.is_data())
}

/// ACARS-4.3: a data report's kind, "Aircraft data report" or "data (#T1B)", from its label and text's start.
fn data_text(m: &Msg, text: &[u8], b: &mut Buf<32>) -> bool {
    match data_kind(m.label, text) {
        Some(DataKind::Named(n)) => b.set(n.as_bytes()),
        Some(DataKind::Sub(c)) => {
            let _ = uwrite!(b, "data (#{})", Txt(&c));
        }
        None if m.is_data() => b.set(b"data"),
        None => return false,
    }
    true
}

/// ACARS-3: what the radio does, for the list's header: 2 a signal on the frequency shown, 1 listening there,
/// 0 not (yet) listening.
fn radio_state(app: &App, live: &ferrite_core::msg::LiveAll) -> u8 {
    match live.mode {
        ferrite_core::msg::ModeLive::Acars { freq, open, .. } if freq == app.acars_freq() => 1 + open as u8,
        _ => 0,
    }
}

/// Message `i` and its text, made safe to draw: line feeds kept, other control characters as spaces.
fn message(st: &impl ChannelStore, i: u16, text: &mut [u8; JOIN_MAX]) -> Option<(Msg, usize)> {
    let (m, n) = st.data().acars()?.get(i, text)?;
    for c in &mut text[..n] {
        if *c != b'\n' && (*c < 0x20 || *c >= 0x7F) {
            *c = b' ';
        }
    }
    Some((m, n))
}

/// ACARS-6: the flight's airline from the table in the SPI flash (None: no flight, no table, not in it).
fn airline(st: &impl ChannelStore, rows: u16, m: &Msg) -> Option<Airline> {
    let code = m.airline().filter(|_| rows != 0 && rows != UNCHECKED)?;
    airlines::lookup(&mut |a, b| st.data().data_read(a, b), rows, code)
}

/// The airline's name, else its code as sent (empty: no flight).
fn airline_text(st: &impl ChannelStore, rows: u16, m: &Msg, b: &mut Buf<32>) {
    b.clear();
    match airline(st, rows, m) {
        Some(a) => b.set(a.name()),
        None => b.set(&m.airline().unwrap_or_default()),
    }
    if b.as_bytes() == [0, 0] {
        b.clear();
    }
}

/// The frequency picked (ACARS-3), as "131.550".
fn freq_text(app: &App, b: &mut Buf<16>) {
    mhz_text(FREQS[app.set.acars_freq as usize % FREQS.len()], b);
}

/// Text wrapped on words, and at each line feed, into lines `wd` px wide in UI_12; `f(k, line)` for each.
/// Returns the lines.
#[inline(never)]
fn lines(text: &[u8], wd: i32, f: &mut dyn FnMut(i32, &[u8])) -> i32 {
    let mut k = 0;
    for para in text.split(|&c| c == b'\n') {
        let mut rest = para.trim_ascii_end();
        if rest.is_empty() && !para.is_empty() {
            continue;
        }
        loop {
            let mut cut = rest.len();
            if text4_width(rest, &UI_12) > wd {
                cut = rest.iter().enumerate().filter(|&(i, &c)| c == b' ' && text4_width(&rest[..i], &UI_12) <= wd).map(|(i, _)| i).next_back().unwrap_or(0);
                if cut == 0 {
                    cut = w::fit(rest, &UI_12, wd).len().max(1);
                }
            }
            f(k, &rest[..cut]);
            k += 1;
            rest = rest[cut..].strip_prefix(b" ").unwrap_or(&rest[cut..]);
            if rest.is_empty() {
                break;
            }
        }
    }
    k
}

impl Acars {
    fn list_y(self) -> i32 {
        HEAD_Y + if self.strip { STRIP_H } else { 0 }
    }

    fn rows(self) -> u16 {
        if self.strip { ROWS2C - 1 } else { ROWS2C }
    }

    /// ACARS-5: slot B's frequency, mode and state, one line under the status bar.
    fn strip(app: &mut App, lcd: &mut impl Panel, live: &ferrite_core::msg::LiveAll) {
        let s = STRIP_SLOT;
        let mut t = Buf::<48>::new();
        t.push(w::freq3(app.shown_freq(s, live)).as_bytes());
        let cfg = app.slots[s as usize].cfg();
        let _ = uwrite!(t, " \u{b7} {}", crate::slot::MODES[cfg.mode as usize % crate::slot::MODES.len()]);
        let (state, c) = app.state(s, live);
        // (R_HELP: the list's gate is R_LINE's number)
        if app.dirty(R_HELP, Mix::new().h(&t).h(&state).h(&c).get()) {
            let y = HEAD_Y;
            ferrite_ui::canvas::banded(lcd, 0, y, 240, STRIP_H, SURFACE, |lcd| {
                w::badge(lcd, 8, y + 6, s, false, SURFACE);
                lcd.text4(28, y + 17, t.as_bytes(), &UI_12B, TEXT, SURFACE);
                let sw = text4_width(state.as_bytes(), &UI_12);
                lcd.text4(232 - sw, y + 17, state.as_bytes(), &UI_12, c, SURFACE);
                lcd.fill(0, y + STRIP_H - 1, 240, 1, LINE);
            });
        }
    }

    /// ACARS-4: the list.
    fn render_list(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let st = cx.store;
        let n = count(st);
        let only = app.set.acars_text;
        let keep = kept(st, only);
        let shown = (0..n).filter(|&i| keep(i)).count();
        let now_s = app.now / 1000;
        let (y0, vis) = (self.list_y(), self.rows());
        let mut l = Buf::<32>::new();
        let mut f = Buf::<16>::new();
        freq_text(app, &mut f);
        let _ = uwrite!(l, "{} AM \u{b7} {} {}", Txt(f.as_bytes()), shown, if only { "with text" } else if shown == 1 { "msg" } else { "msgs" });
        // ACARS-3: what the radio does: listening on the frequency shown (a signal now: "receiving"), or not
        let state: &[u8] = [&b"not listening"[..], b"listening", b"receiving"][radio_state(app, cx.live) as usize];
        if app.dirty(R_TOP, Mix::new().h(&l).h(&y0).h(state).get()) {
            head_at(lcd, y0, l.as_bytes(), state);
        }
        let top = y0 + 21;
        let rows = self.cur.rows(n, vis, &keep, |row, i| {
            let mut start = [0u8; 48];
            let Some((m, len)) = st.data().acars().and_then(|a| a.get(i, &mut start)) else { return };
            let age = now_s.wrapping_sub(m.at_s);
            let (mut sub, mut name, mut when) = (Buf::<48>::new(), Buf::<32>::new(), Buf::<12>::new());
            if !m.crc_ok() {
                sub.push("CRC bad \u{b7} ".as_bytes());
            }
            let lab = label(m.label);
            if m.is_data() && data_text(&m, &start[..len], &mut name) {
                // ACARS-4.3: a data report: its kind and size, not its hex
                let _ = uwrite!(sub, "{} \u{b7} {} chars", Txt(name.as_bytes()), m.text_len());
            } else if len > 0 {
                // ACARS-4: the text's start, on one line (the airline and the aircraft are on its page)
                let t = start[..len].trim_ascii_start();
                for &c in t.iter().take(48) {
                    sub.push(&[if (0x20..0x7F).contains(&c) { c } else { b' ' }]);
                }
            } else if m.flight().is_empty() {
                // no flight: the aircraft is the title, what the message is under it
                if m.up() {
                    sub.push("from ground \u{b7} ".as_bytes());
                }
                sub.push(lab.meaning.as_bytes());
            } else {
                airline_text(st, self.air, &m, &mut name);
                let _ = uwrite!(sub, "{} \u{b7} {}", Txt(name.as_bytes()), Txt(m.reg()));
            }
            // the clock time it came, or how long ago without a clock
            if cx.dev.clock_min.is_some() {
                clock_text(cx.dev.clock_min, age, &mut when);
            } else {
                age_text(age, &mut when);
            }
            let title = if m.flight().is_empty() { m.reg() } else { m.flight() };
            // ACARS-4.1: a repeat's count before its label
            let mut lab_r = Buf::<8>::new();
            if m.repeats > 0 {
                let _ = uwrite!(lab_r, "{}x ", m.repeats as u16 + 1);
            }
            lab_r.push(&label_text(m.label));
            let r = Row2 {
                tag: lab.tag.as_bytes(),
                title,
                big: true,
                right: when.as_bytes(),
                sub: sub.as_bytes(),
                sub_right: lab_r.as_bytes(),
                sub_c: if m.crc_ok() { 0 } else { RED },
                selected: i == self.cur.sel,
                compact: true,
                ..Row2::default()
            };
            if app.dirty(R_ROW + row, hash(&r) ^ top as u32) {
                row2(lcd, top + row as i32 * ROW2C_H, &r, app.cue());
            }
        });
        for i in rows..vis as usize {
            if app.dirty(R_ROW + i, 1) {
                lcd.fill(0, top + i as i32 * ROW2C_H, 240, ROW2C_H, BG);
            }
        }
        let more = self.cur.more(n, vis, rows, &keep);
        app.scroll_cue(lcd, (0, vis as usize), top, vis as i32 * ROW2C_H, more, self.cur.top as u32, vis as u32, shown as u32);
        if n == 0 {
            app.empty(lcd, b"No messages yet", b"They show here as they're heard");
        } else if shown == 0 {
            app.empty(lcd, b"No messages with text", b"* shows them all");
        }
    }

    /// ACARS-7: one message.
    fn render_detail(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let st = cx.store;
        let mut text = [0u8; JOIN_MAX];
        let Some((m, len)) = message(st, self.cur.sel, &mut text) else {
            return app.empty(lcd, b"No such message", b"The list changed");
        };
        let text = &text[..len];
        let now_s = app.now / 1000;
        let age = now_s.wrapping_sub(m.at_s);
        let air = airline(st, self.air, &m);
        let lab = label(m.label);
        // the heading: the flight (or the aircraft), the airline and aircraft, the label's chip
        let title = if m.flight().is_empty() { m.reg() } else { m.flight() };
        let mut who = Buf::<48>::new();
        match air {
            Some(a) => who.push(a.name()),
            None if m.up() => who.push(b"From a ground station"),
            None => {}
        }
        if !m.flight().is_empty() {
            if !who.is_empty() {
                who.push(" \u{b7} ".as_bytes());
            }
            who.push(m.reg());
        }
        let mut chip = Buf::<40>::new();
        let mut kind = Buf::<32>::new();
        let meaning = if m.is_data() && data_text(&m, text, &mut kind) { kind.as_bytes() } else { lab.meaning.as_bytes() };
        let _ = uwrite!(chip, "{} \u{b7} {}", Txt(&label_text(m.label)), Txt(meaning));
        if app.dirty(R_TOP, Mix::new().h(&title).h(&who).h(&chip).get()) {
            ferrite_ui::canvas::banded(lcd, 0, 20, 240, 70, BG, |lcd| {
                lcd.text4(8, 42, title, &UI_15B, if m.crc_ok() { TEXT } else { RED }, BG);
                lcd.text4(8, 61, w::fit(who.as_bytes(), &UI_12, 224), &UI_12, SUB, BG);
                lcd.fill(0, 89, 240, 1, LINE);
            });
            super::parts::path_chip(lcd, 8, 67, w::fit(chip.as_bytes(), &UI_12B, 210), BG);
        }
        // the rows: label, value, colour (each a line, two for a long airline name)
        let mut rows: [(&[u8], Buf<40>, u16); 5] = Default::default();
        // Airline: its name and ICAO code, else the code as sent
        let b = &mut rows[0].1;
        match (air, m.airline()) {
            (Some(a), _) => {
                let _ = uwrite!(b, "{} \u{b7} {}", Txt(a.name()), Txt(a.icao()));
            }
            (None, Some(c)) => {
                let _ = uwrite!(b, "{} (not in the table)", Txt(&c));
            }
            (None, None) => b.set(b"-"),
        }
        // From: the aircraft or a ground station, and the (first) block
        let _ = uwrite!(rows[1].1, "{} \u{b7} block {}", if m.up() { "ground" } else { "aircraft" }, m.block as char);
        // Message: its number (downlinks) and the blocks joined
        let b = &mut rows[2].1;
        let no = ferrite_core::acars::field(&m.msgno);
        b.set(if no.is_empty() { b"-" } else { no });
        if m.blocks > 1 {
            let _ = uwrite!(b, " \u{b7} {} blocks", m.blocks);
        }
        // Heard: the clock time (else how long ago) and the frequency
        let (mut when, mut f) = (Buf::<12>::new(), Buf::<16>::new());
        if cx.dev.clock_min.is_some() {
            clock_text(cx.dev.clock_min, age, &mut when);
        } else {
            age_text(age, &mut when);
            when.push(b" ago");
        }
        mhz_text(FREQS[m.freq as usize % FREQS.len()], &mut f);
        let _ = uwrite!(rows[3].1, "{} \u{b7} {}", Txt(when.as_bytes()), Txt(f.as_bytes()));
        if m.repeats > 0 {
            // ACARS-4.1: a repeat, the last time
            let _ = uwrite!(rows[3].1, " \u{b7} {}x", m.repeats as u16 + 1);
        }
        // Check: the CRC, blocks missing, parity; the mode and the acknowledgement
        let b = &mut rows[4].1;
        rows[4].2 = if !m.crc_ok() {
            b.push(b"CRC bad");
            RED
        } else if m.flags & (GAP | CUT) != 0 {
            b.push(if m.flags & GAP != 0 { b"blocks missing" } else { b"text cut" });
            AMBER
        } else {
            b.push(b"CRC ok");
            TEXT
        };
        if m.parity > 0 {
            let _ = uwrite!(b, " \u{b7} {} parity", m.parity);
        }
        let _ = uwrite!(b, " \u{b7} mode {}", m.mode as char);
        if m.ack != NAK {
            let _ = uwrite!(b, " \u{b7} ack {}", m.ack as char);
        }
        for (k, l) in [&b"Airline"[..], b"From", b"Message", b"Heard", b"Check"].into_iter().enumerate() {
            rows[k].0 = l;
            if k < 4 {
                rows[k].2 = TEXT;
            }
        }
        // the text, wrapped, from the line scrolled to; its scroll cue first, so the rows never draw another page's
        let ty = 90 + rows.iter().map(|r| w::kv_h(&w::KV_DETAIL, r.1.as_bytes(), 0)).sum::<i32>() + 6;
        let shown = ((w::HINT_Y - 2 - ty) / 17).max(1);
        let total = lines(text, 220, &mut |_, _| {});
        let max = (total - shown).clamp(0, 255) as u8;
        if self.line > max {
            app.screen = Screen::Acars(Acars { line: max, ..self });
        }
        app.scroll_cue(lcd, (0, 0), ty, shown * 17, total > shown, self.line.min(max) as u32, shown as u32, total as u32);
        let mut y = 90;
        for (k, (l, v, c)) in rows.iter().enumerate() {
            y = app.kv(lcd, k, y, 0, &w::KV_DETAIL, l, v.as_bytes(), *c, false);
        }
        let skip = self.line.min(max) as i32;
        if app.dirty(R_NOTE, Mix::new().h(&text).h(&ty).h(&skip).get()) {
            ferrite_ui::canvas::banded(lcd, 0, ty - 6, 240, w::HINT_Y - ty + 6, BG, |lcd| {
                if text.is_empty() {
                    lcd.text4(8, ty + 13, b"No text", &UI_12, DIM, BG);
                }
                lines(text, 220, &mut |k, s| {
                    if (skip..skip + shown).contains(&k) {
                        lcd.text4(8, ty + 13 + (k - skip) * 17, s, &UI_12, TEXT, BG);
                    }
                });
            });
        }
    }
}

impl View for Acars {
    fn name(self) -> &'static str {
        if self.detail { "ACARS message" } else { "ACARS" }
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"ACARS");
    }

    fn own_arrows(self) -> bool {
        true
    }

    fn key(self, app: &mut App, e: KeyEvent, st: &impl ChannelStore) {
        use Press::*;
        let n = count(st);
        let keep = kept(st, app.set.acars_text);
        match (e.key, e.press, self.detail) {
            (Key::Up | Key::Down, Short | Repeat, _) => self.knob(app, if e.key == Key::Up { -1 } else { 1 }, st),
            // ACARS-3: ◀▶ step through the frequencies (the radio retunes: App::reconcile_mode)
            (Key::Left | Key::Right, Short | Repeat, false) => {
                let d = if e.key == Key::Left { -1 } else { 1 };
                app.set.acars_freq = (app.set.acars_freq as i32 + d).rem_euclid(FREQS.len() as i32) as u8;
            }
            // ACARS-7: ◀▶ the newer and older message
            (Key::Left | Key::Right, Short | Repeat, true) if n > 0 => {
                let d = if e.key == Key::Left { -1 } else { 1 };
                let cur = self.cur.step(d, n, self.rows(), &keep);
                app.screen = Screen::Acars(Acars { cur, line: 0, ..self });
            }
            (Key::Menu, Short, false) if self.cur.sel < n && keep(self.cur.sel) => app.screen = Screen::Acars(Acars { detail: true, line: 0, ..self }),
            (Key::Hash, Short, false) => app.screen = Screen::Acars(Acars { strip: !self.strip, cur: self.cur.shown(n, if self.strip { ROWS2C } else { ROWS2C - 1 }, &keep), ..self }),
            // ACARS-4.2: * shows only the messages with text, or all again (kept in the settings)
            (Key::Star, Short, false) => {
                app.set.acars_text = !app.set.acars_text;
                let cur = Cursor::first(n, &kept(st, app.set.acars_text));
                app.screen = Screen::Acars(Acars { cur, ..self });
            }
            (Key::Exit, Short, true) => app.screen = Screen::Acars(Acars { detail: false, ..self }),
            (Key::Exit, Short, false) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, st: &impl ChannelStore) {
        if self.detail {
            // ACARS-7: the text scrolls a line a click (held at its end when drawn)
            app.screen = Screen::Acars(Acars { line: (self.line as i32 + d).clamp(0, 255) as u8, ..self });
        } else {
            let cur = self.cur.step(d, count(st), self.rows(), &kept(st, app.set.acars_text));
            app.screen = Screen::Acars(Acars { cur, ..self });
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let st = cx.store;
        if self.air == UNCHECKED {
            // ACARS-6: the table is checked once each time the screen opens (Console may have written it)
            let me = Acars { air: airlines::check(&mut |a, b| st.data().data_read(a, b)).unwrap_or(0), ..self };
            app.screen = Screen::Acars(me);
            return me.render(app, lcd, cx);
        }
        app.page(lcd, (self.detail, self.strip));
        if self.strip && !self.detail {
            Self::strip(app, lcd, cx.live);
        }
        // everything else changes with the messages, the selection, the frequency or the age shown
        let ver = st.data().acars().map_or(0, |a| a.version());
        let now_s = app.now / 1000;
        // without a clock the list shows ages: by the second for the first minute
        let newest = st.data().acars().and_then(|a| a.get(0, &mut [])).map(|(m, _)| now_s.wrapping_sub(m.at_s));
        let tick = if cx.dev.clock_min.is_some() && !self.detail {
            0
        } else if newest.is_some_and(|a| a < 60) {
            now_s
        } else {
            now_s / 60
        };
        // ACARS-4.2: a selection on a message the filter hides (the list moved under it) goes to the next shown
        if !self.detail && app.set.acars_text {
            let (n, k) = (count(st), kept(st, true));
            if self.cur.sel < n && !k(self.cur.sel) {
                let me = Acars { cur: self.cur.step(1, n, self.rows(), &k), ..self };
                app.screen = Screen::Acars(me);
                return me.render(app, lcd, cx);
            }
        }
        // the header's radio state (ACARS-3) redraws as it changes
        let radio = radio_state(app, cx.live);
        if !app.gate((self, ver, (app.set.acars_freq, app.set.acars_text, radio), cx.dev.clock_min, tick)) {
            return;
        }
        if self.detail {
            self.render_detail(app, lcd, cx)
        } else {
            self.render_list(app, lcd, cx)
        }
    }

    fn hints(self, app: &App) -> Hints {
        if self.detail {
            Hints::new(&[(ARROWS_LR, "Prev/next"), (ARROWS_UD, "Scroll"), ("BACK", "List")])
        } else {
            Hints::new(&[("OK", "Open"), (ARROWS_LR, "Freq"), ("*", if app.set.acars_text { "All" } else { "Text only" })])
        }
    }
}
