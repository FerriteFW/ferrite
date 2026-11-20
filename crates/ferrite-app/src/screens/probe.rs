//! Tone search and the frequency counter (manual chapter 22; no mock-ups). Both run on the Radio task and
//! report through `ModeLive::Probe`.
//!
//! Tone search (MENU 0 3 5): the active slot's frequency and signal, the tone found or "searching"
//! with the time, the last hits (TSRCH-2). OK sets the RX tone, then asks before setting TX too (TSRCH-4); on a
//! channel the tone is loaded into the slot, not saved to the channel, and # saves it there (TSRCH-9). BACK leaves the tones as they
//! were (TSRCH-5).
//!
//! Counter (MENU 0 3 2): the strongest nearby transmitter's frequency and level, a hold, the last hits
//! (TSRCH-6); OK copies the frequency to the active slot's VFO (TSRCH-7). It only receives (TSRCH-8). It counts
//! 64–1300 MHz on both BK4829s without moving the relay (TSRCH-10); # holds the reading while it keeps listening
//! (TSRCH-11).

use super::activity::{mmss, Hit, HITS};
use super::scan::ago;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::slot::tone_text;
use crate::{App, Screen};
use ferrite_core::freq::Freq10;
use ferrite_core::keys::Key;
use ferrite_core::msg::{LiveAll, ModeLive, Param, RadioCmd};
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// The slot card both screens open with: letter, VFO/channel and band, S-meter text; the frequency.
fn slot_card(app: &mut App, lcd: &mut impl Panel, live: &LiveAll, store: &impl ChannelStore, s: u8) {
    let lv = live.slots[s as usize];
    let mut head = Buf::<32>::new();
    app.head_text(s, live, store, &mut head);
    let st = w::s_text(lv.rssi);
    let (y, h) = (25, 70);
    if app.dirty(1, 1) {
        lcd.fill(6, y, 228, h, BG);
        lcd.fill_r(6, y, 228, h, 3, SURFACE);
        lcd.frame_r(6, y, 228, h, 3, BLUE);
    }
    if app.dirty(2, Mix::new().h(&head).h(&st).h(&lv.squelch_open).get()) {
        w::badge(lcd, 14, y + 8, s, true, SURFACE);
        w::field(lcd, 34, 140, y + 7, y + 24, y + 20, w::fit(head.as_bytes(), &UI_12, 140), const { &w::st(&UI_12, SUB, SURFACE, Align::Left) });
        let c = if lv.squelch_open { GREEN } else { DIM };
        w::field(lcd, 174, 52, y + 7, y + 24, y + 20, st.as_bytes(), &w::st(&UI_12B, c, SURFACE, Align::Right));
    }
    let f = app.slot(s).freq();
    if app.dirty(3, Mix::new().h(&f).get()) {
        w::freq(lcd, 15, 226, y + 60, f, &DIG_46, &DIG_28, TEXT, SUB, SURFACE);
    }
}

/// "100.0" or "023" (digits for the digit font) and what it is ("CTCSS, Hz", "DCS, normal").
fn tone_parts(t: u16, d: &mut Buf<16>, kind: &mut Buf<16>) {
    d.clear();
    if t & crate::slot::DCS_FLAG == 0 {
        let _ = uwrite!(d, "{}.{}", t / 10, t % 10);
        kind.set(b"CTCSS, Hz");
    } else {
        let c = t & 0x1FF;
        let _ = uwrite!(d, "{}{}{}", c >> 6, (c >> 3) & 7, c & 7);
        kind.set(if t & crate::slot::DCS_INV != 0 { b"DCS, inverted" } else { b"DCS, normal" });
    }
}

/// The last-hits list at y: when, what (by `what`), level.
fn hit_rows(app: &mut App, lcd: &mut impl Panel, y: i32, region: usize, what: impl Fn(&Hit, &mut Buf<32>)) {
    if app.dirty(region, 1) {
        w::field(lcd, 8, 224, y, y + 16, y + 12, b"Last hits", const { &w::st(&UI_12B, SUB, BG, Align::Left) });
    }
    for i in 0..HITS {
        let hit = app.radio.hits[i];
        let (mut a, mut t, mut sig) = (Buf::<16>::new(), Buf::<32>::new(), Buf::<4>::new());
        if hit.what != 0 {
            ago(hit.age_ms(app.now), &mut a);
            what(&hit, &mut t);
            sig = w::s_text(hit.rssi());
        } else if i == 0 {
            t.set(b"None yet");
        }
        let ry = y + 18 + 18 * i as i32;
        if app.dirty(region + 1 + i, Mix::new().h(&a).h(&t).h(&sig).get()) {
            w::field(lcd, 8, 52, ry, ry + 18, ry + 13, a.as_bytes(), const { &w::st(&UI_12, SUB, BG, Align::Left) });
            w::field(lcd, 60, 140, ry, ry + 18, ry + 13, t.as_bytes(), const { &w::st(&UI_12, TEXT, BG, Align::Left) });
            w::field(lcd, 200, 32, ry, ry + 18, ry + 13, sig.as_bytes(), const { &w::st(&UI_12B, GREEN, BG, Align::Right) });
        }
    }
}

// ---- Tone search ----

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ToneSearch {
    pub slot: u8,
    /// The RX tone is set and the screen asks about TX (TSRCH-4).
    pub ask_tx: bool,
}

/// MENU 0 3 5 (TSRCH-1): search on the active slot.
pub fn open_tone(app: &mut App) -> Screen {
    let slot = app.active; // MODE-2: the reconciler starts the search on it
    app.radio.t0 = app.now;
    app.radio.clear_hits();
    Screen::ToneSearch(ToneSearch { slot, ask_tx: false })
}

fn found(live: &LiveAll) -> u16 {
    match live.mode {
        ModeLive::Probe { tone, .. } => tone,
        _ => 0,
    }
}

impl ToneSearch {
    /// The tone the screen shows as found (0: searching): the search's, or while asking about TX the one set
    /// (the UI readback reads it too, LINK-19).
    pub(crate) fn shown(self, app: &App, live: &LiveAll) -> u16 {
        if self.ask_tx { app.radio.hits[0].what as u16 } else { found(live) }
    }
}

/// One tone on a slot, live, without saving a channel's record (TSRCH-9; the tone hint's too, TSRCH-16).
pub(crate) fn set_tone(app: &mut App, slot: u8, t: u16, tx: bool) {
    let c = app.slots[slot as usize].cfg_mut();
    if tx {
        c.tx_tone = t;
    } else {
        c.rx_tone = t;
    }
    let param = if tx { Param::TxTone } else { Param::RxTone };
    app.out.push(RadioCmd::SlotParam { slot, param, value: t as i32 });
}

impl View for ToneSearch {
    fn name(self) -> &'static str {
        "Tone search"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Search");
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        let t = app.radio.hits[0].what as u16;
        match (e.key, e.press, self.ask_tx) {
            (Key::Menu, Press::Short, false) if t != 0 => {
                set_tone(app, self.slot, t, false);
                app.screen = Screen::ToneSearch(ToneSearch { ask_tx: true, ..self });
            }
            (Key::Menu, Press::Short, true) => {
                set_tone(app, self.slot, t, true);
                done(app, self.slot, b"RX and TX tone set");
            }
            (Key::Exit, Press::Short, true) => done(app, self.slot, b"RX tone set"),
            // TSRCH-9: on a channel # saves the found tone into its record (with TX too if just set).
            (Key::Hash, Press::Short, _) if t != 0 && app.slot(self.slot).on_channel() => {
                if !self.ask_tx {
                    set_tone(app, self.slot, t, false);
                }
                app.save_channel(self.slot, None);
                app.back();
                app.show_toast(b"Tone saved", b"in the channel", false);
            }
            (Key::Exit, Press::Short, false) => app.back(), // TSRCH-5: tones unchanged
            _ => {}
        }
    }

    fn tick(self, app: &mut App, live: &LiveAll) {
        let t = found(live);
        if t != 0 && !self.ask_tx {
            let rssi = live.slots[self.slot as usize].rssi;
            app.radio.push_hit(Hit::new(t as u32, app.now, rssi), false);
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        slot_card(app, lcd, cx.live, cx.store, self.slot);
        // The result, or "searching" and the time (TSRCH-2).
        let t = self.shown(app, cx.live);
        let secs = app.now.wrapping_sub(app.radio.t0) / 1000;
        let (mut d, mut kind) = (Buf::<16>::new(), Buf::<16>::new());
        if t != 0 {
            tone_parts(t, &mut d, &mut kind);
        } else {
            kind.set(b"Searching ");
            let mut m = Buf::<16>::new();
            mmss(secs, &mut m);
            kind.push(m.as_bytes());
        }
        if app.dirty(4, Mix::new().h(&d).h(&kind).get()) {
            ferrite_ui::canvas::banded(lcd, 0, 100, 240, 60, BG, |lcd| {
                if t != 0 {
                    w::field(lcd, 8, 224, 102, 118, 114, b"Found", const { &w::st(&UI_12, SUB, BG, Align::Left) });
                    lcd.text4_rows(8, 152, d.as_bytes(), &DIG_46, GREEN, BG, 118, 154);
                    let x = 8 + text4_width(d.as_bytes(), &DIG_46) + 8;
                    w::field(lcd, x, 232 - x, 138, 155, 151, kind.as_bytes(), const { &w::st(&UI_12, SUB, BG, Align::Left) });
                } else {
                    w::field(lcd, 8, 224, 120, 140, 134, kind.as_bytes(), const { &w::st(&UI_12B, AMBER, BG, Align::Left) });
                    w::field(lcd, 8, 224, 140, 156, 152, b"Listening for CTCSS and DCS", const { &w::st(&UI_12, SUB, BG, Align::Left) });
                }
            });
        }
        // The slot's tones now; while asking, the question.
        let c = *app.slot(self.slot).cfg();
        let (mut rx, mut tx) = (Buf::<16>::new(), Buf::<16>::new());
        tone_text(c.rx_tone, &mut rx);
        tone_text(c.tx_tone, &mut tx);
        let mut now = Buf::<40>::new();
        if self.ask_tx {
            now.set(b"RX set. Set the TX tone too?"); // 224 px: 32 characters
        } else {
            let _ = uwrite!(now, "Now: RX {} \u{b7} TX {}", or_none(&rx), or_none(&tx));
        }
        if app.dirty(5, Mix::new().h(&now).get()) {
            let c = if self.ask_tx { AMBER } else { SUB };
            w::field(lcd, 8, 224, 162, 180, 175, now.as_bytes(), &w::st(&UI_12, c, BG, Align::Left));
            lcd.fill(0, 184, 240, 1, LINE);
        }
        // Last hits only once two different tones were heard: one would repeat the result above.
        if app.radio.hits[1].what != 0 {
            hit_rows(app, lcd, 190, 6, |h, b| {
                let mut t = Buf::<16>::new();
                tone_text(h.what as u16, &mut t);
                b.set(t.as_bytes());
            });
        }
        if app.slot(self.slot).on_channel() && app.dirty(11, 1) {
            w::field(lcd, 8, 224, 266, 284, 279, b"On a channel: OK loads, # saves", const { &w::st(&UI_12, DIM, BG, Align::Left) });
        }
    }

    fn hints(self, app: &App) -> Hints {
        let ch = app.slot(self.slot).on_channel();
        if self.ask_tx && ch {
            Hints::new(&[("OK", "TX too"), ("#", "Save"), ("BACK", "RX only")])
        } else if self.ask_tx {
            Hints::new(&[("OK", "TX too"), ("BACK", "RX only")])
        } else if app.radio.hits[0].what != 0 && ch {
            Hints::new(&[("OK", "Apply"), ("#", "Save"), ("BACK", "Cancel")])
        } else if app.radio.hits[0].what != 0 {
            Hints::new(&[("OK", "Apply"), ("BACK", "Cancel")])
        } else {
            Hints::new(&[("BACK", "Cancel")])
        }
    }
}

fn or_none(b: &Buf<16>) -> &str {
    if b.is_empty() { "none" } else { core::str::from_utf8(b.as_bytes()).unwrap_or("") }
}

/// Tones applied: home, with what happened (TSRCH-9: a channel's record is left alone).
fn done(app: &mut App, slot: u8, what: &[u8]) {
    applied(app, slot, what);
}

/// One level back (KEY-5) with what was set, and on a channel that it wasn't saved (TSRCH-9, TSRCH-16).
pub(crate) fn applied(app: &mut App, slot: u8, what: &[u8]) {
    let ch = app.slot(slot).on_channel();
    app.back();
    app.show_toast(what, if ch { b"loaded, not saved" } else { b"" }, false);
}

// ---- Counter ----

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Counter {
    /// TSRCH-11: the reading is held: new signals go to the list only.
    pub hold: bool,
    /// The held reading: frequency (0 none) and dBm.
    pub held: Freq10,
    pub held_rssi: i16,
}

/// TSRCH-10: what the counter reports: 64–1300 MHz, outside chip 1's 630–756 MHz gap. 18–64 MHz would need the
/// band relay, which the counter never moves (SAFE-20).
pub fn countable(f: Freq10) -> bool {
    use ferrite_core::freq::mhz;
    (mhz(64)..=mhz(1300)).contains(&f) && !(mhz(630) + 1..mhz(756)).contains(&f)
}

/// MENU 0 3 2 (TSRCH-6).
pub fn open_counter(app: &mut App) -> Screen {
    app.radio.clear_hits(); // MODE-2: the reconciler starts the counter
    Screen::Counter(Counter { hold: false, held: 0, held_rssi: 0 })
}

impl Counter {
    /// What the screen shows: frequency (0: listening), level, and whether it is the live reading (else the last
    /// hit's); the held one while held (TSRCH-11). The UI readback reads it too (LINK-19).
    pub(crate) fn shown(self, app: &App, live: &LiveAll) -> (Freq10, i16, bool) {
        let (f, rssi, now) = match live.mode {
            ModeLive::Probe { freq, rssi, .. } if countable(freq) => (freq, rssi, true),
            _ => (app.radio.hits[0].what, app.radio.hits[0].rssi(), false),
        };
        if self.hold { (self.held, self.held_rssi, now) } else { (f, rssi, now) }
    }
}

impl View for Counter {
    /// SQL-9.3: the counter has no audio to monitor; SIDE1 does nothing here.
    fn own_side1(self) -> bool {
        true
    }

    fn name(self) -> &'static str {
        "Frequency counter"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Counter");
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        match (e.key, e.press) {
            (Key::Hash, Press::Short) => {
                // TSRCH-11: freeze the reading shown now (the newest hit), or release it.
                let (held, held_rssi) = if self.hold { (0, 0) } else { (app.radio.hits[0].what, app.radio.hits[0].rssi()) };
                app.screen = Screen::Counter(Counter { hold: !self.hold, held, held_rssi })
            }
            (Key::Menu, Press::Short) => {
                // TSRCH-7: to the active slot's VFO; the held reading while held.
                let f = if self.hold { self.held } else { app.radio.hits[0].what };
                if f == 0 {
                    return app.show_toast(b"Nothing heard yet", b"", false);
                }
                let a = app.active as usize;
                app.slots[a].vfo = f;
                app.slots[a].vfo_mode = true;
                app.go_home();
                app.send_slot(a as u8);
                let fq = w::freq3(f);
                app.show_toast(b"VFO set", fq.as_bytes(), false);
            }
            (Key::Exit, Press::Short) => app.back(),
            _ => {}
        }
    }

    fn tick(self, app: &mut App, live: &LiveAll) {
        if let ModeLive::Probe { freq, rssi, .. } = live.mode {
            // TSRCH-11: a hold freezes the reading, not the listening; the list keeps filling.
            if countable(freq) {
                app.radio.push_hit(Hit::new(freq, app.now, rssi), false);
            }
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let (f, rssi, live) = self.shown(app, cx.live);
        if app.dirty(1, Mix::new().h(&self.hold).get()) {
            w::field(lcd, 8, 170, 22, 40, 35, b"Strongest nearby signal", const { &w::st(&UI_12, SUB, BG, Align::Left) });
            ferrite_ui::canvas::banded(lcd, 178, 22, 62, 18, BG, |lcd| {
                if self.hold {
                    w::tag(lcd, 232, 23, b"HOLD", AMBER, BG);
                }
            });
        }
        // DISP-31: and whether it is live (white) or the last reading (grey): it stayed white once stale
        if app.dirty(2, Mix::new().h(&f).h(&(live || self.hold)).get()) {
            ferrite_ui::canvas::banded(lcd, 0, 42, 240, 52, BG, |lcd| {
                if f == 0 {
                    w::field(lcd, 8, 224, 56, 76, 70, b"Listening...", const { &w::st(&UI_12B, AMBER, BG, Align::Left) });
                } else {
                    w::freq(lcd, 8, 232, 88, f, &DIG_46, &DIG_28, if live || self.hold { TEXT } else { SUB }, SUB, BG);
                }
            });
        }
        let lit = if f == 0 { 0 } else { w::meter_level(rssi) };
        // DISP-31: the level's text and colour as well as its bars (the text stayed "+30" over a "+20" reading)
        let st = if f == 0 { Buf::new() } else { w::s_text(rssi) };
        if app.dirty(3, Mix::new().h(&lit).h(&st).h(&live).get()) {
            w::meter(lcd, 8, 104, None, lit, if live { GREEN } else { DIM }, BG);
            w::field(lcd, 208, 24, 98, 116, 112, st.as_bytes(), const { &w::st(&UI_12B, TEXT, BG, Align::Right) });
        }
        if app.dirty(4, 1) {
            w::field(lcd, 8, 224, 122, 140, 135, b"Key the other radio close by.", const { &w::st(&UI_12, SUB, BG, Align::Left) });
            w::field(lcd, 8, 224, 140, 158, 153, b"Receive only: never transmits.", const { &w::st(&UI_12, DIM, BG, Align::Left) });
            lcd.fill(0, 164, 240, 1, LINE);
        }
        hit_rows(app, lcd, 170, 5, |h, b| b.set(w::freq3(h.what).as_bytes()));
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[("OK", "To VFO"), ("#", if self.hold { "Release" } else { "Hold" }), ("BACK", "Back")])
    }
}
