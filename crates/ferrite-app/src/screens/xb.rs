//! The crossband repeater (manual chapter 19, XB-1 to XB-16; XbStart, XbIdle and XbRunning mock-ups). One
//! screen in three states: the start page (each side's slot, the settings grid, the pre-flight checks: XB-2),
//! then, once # starts it, ready (listening both) and passing (one direction, its timeout), both with the last
//! 30 minutes of activity (XB-16). BACK stops (XB-14), and so does leaving by any route (hold BACK, a
//! timeout). Local PTT transmits on its own slot and the repeater carries on after (XB-14); each side is simplex
//! (XB-18). MENU 0 3 5. The radio side is the `tx` build's (`ferrite_core::xb`, decision 93); a refusal from the
//! radio comes back in `XbLive::stopped` and the screen goes back to its start page saying why.
//!
//! XB-20 (decision 161): the link starts and stops it too, in every build: `xb start` / `xb stop` (the terminal
//! and the link's `CMD`), and remote keys' # on the start page, through the same checks as the radio's #
//! ([`try_start`]). While it runs any key but PTT and SIDE1 stops it ([`key_stops`]), the radio's or a remote one,
//! and every start and stop is logged with who did it ([`XbLog`]: the cable log and the readback).

use super::activity::{mmss, Mode};
use super::parts;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::slot::{band_name, tone_text};
use crate::{App, Screen};
use ferrite_core::freq::{band, mhz};
use ferrite_core::keys::Key;
use ferrite_core::msg::{ModeLive, RadioCmd, XbCfg, XbLive, SLOTS, XB_COLS};
use ferrite_core::txplan;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// XB-10's duty limit, until the bench sets it.
pub const DUTY_LIMIT: u8 = 50;
const TIMEOUTS: [u8; 6] = [3, 6, 12, 18, 30, 60]; // 10 s units: 30 s to 10 min
const HANGS: [u8; 7] = [0, 5, 10, 15, 20, 25, 30]; // 0.1 s

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Crossband {
    /// Start page focus: 0 side 1, 1 side 2, then the grid 2-7 (power, timeout, hang, tone gate, ID, speaker).
    pub cell: u8,
}

/// The settings grid: label, and its value from the settings.
const GRID: [&str; 6] = ["Power", "Timeout", "Hang", "Tone gate", "ID", "Speaker"];

fn grid_value(c: &XbCfg, i: usize, b: &mut Buf<16>) {
    b.clear();
    match i {
        0 => b.set(if c.power == 0 { b"Low" } else { b"Mid" }),
        1 => {
            let s = c.timeout_10s as u32 * 10;
            if s < 60 {
                let _ = uwrite!(b, "{} s", s);
            } else {
                let _ = uwrite!(b, "{} min", s / 60);
            }
        }
        2 => {
            let _ = uwrite!(b, "{}.{} s", c.hang_ds / 10, c.hang_ds % 10);
        }
        3 => b.set(if c.flags & XbCfg::TONE_GATE != 0 { b"On" } else { b"Off (carrier)" }),
        4 => b.set(if c.flags & XbCfg::ID != 0 { b"CW 10 m" } else { b"Off" }),
        _ => b.set(if c.flags & XbCfg::SPEAKER != 0 { b"On" } else { b"Off" }),
    }
}

fn step_in(list: &[u8], v: u8, d: i32) -> u8 {
    let i = list.iter().position(|&x| x >= v).unwrap_or(0) as i32;
    list[(i + d).clamp(0, list.len() as i32 - 1) as usize]
}

/// The knob on a cell.
fn change(app: &mut App, cell: u8, d: i32) {
    let c = &mut app.set.xb;
    match cell {
        0 | 1 => {
            // Each side's slot; never both on the same one.
            let other = if cell == 0 { c.side2 } else { c.side1 };
            let mine = if cell == 0 { &mut c.side1 } else { &mut c.side2 };
            let mut s = *mine as i32;
            loop {
                s = (s + d.signum()).rem_euclid(SLOTS as i32);
                if s as u8 != other {
                    break;
                }
            }
            *mine = s as u8;
        }
        2 => c.power = (c.power as i32 + d).clamp(0, 1) as u8, // XB-3: never above mid
        3 => c.timeout_10s = step_in(&TIMEOUTS, c.timeout_10s, d),
        4 => c.hang_ds = step_in(&HANGS, c.hang_ds, d),
        5 => c.flags ^= XbCfg::TONE_GATE,
        6 => c.flags ^= XbCfg::ID,
        _ => c.flags ^= XbCfg::SPEAKER,
    }
}

/// One pre-flight line: passed (tick), a warning (amber "!"), or a refusal (red "x").
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Check {
    Ok,
    Warn,
    Fail,
}

/// XB-2's checks, in order, with their text; the first failure is why # refuses.
fn checks(app: &App, batt: u8, out: &mut [(Check, Buf<40>); 4]) -> Option<usize> {
    let c = app.set.xb;
    let (f1, f2) = (app.slot(c.side1).freq(), app.slot(c.side2).freq());
    let (chip1, chip2) = (band(f1).1, band(f2).1);
    let mut fail = None;
    let mut put = |i: usize, ok: Check, s: &[u8], fail: &mut Option<usize>| {
        out[i] = (ok, Buf::from(s));
        if ok == Check::Fail && fail.is_none() {
            *fail = Some(i);
        }
    };
    let hf = f1 < mhz(64) || f2 < mhz(64);
    let mut b = Buf::<40>::new();
    if hf {
        // the stop's own wording (XB-14's checks), without the bands: "6 m and UHF, HF needs the relay" ran past
        // the line's 206 px (ferrite#144); the longest band pair with "one chip" or "two chips" fits
        put(0, Check::Fail, ferrite_core::xb::Stop::Hf.text().as_bytes(), &mut fail);
    } else {
        let _ = uwrite!(b, "{} and {}, ", band_name(f1), band_name(f2));
        b.push(if chip1 == chip2 { b"one chip" } else { b"two chips" });
        put(0, if chip1 == chip2 { Check::Fail } else { Check::Ok }, b.as_bytes(), &mut fail);
    }
    // Simplex only (XB-18): each side transmits on its own receive frequency, never through a shift or split.
    let tx = |s: u8| {
        let f = app.slot(s).freq();
        app.radio.tx_built && txplan::check(f, app.set.tx_region(), app.set.extend_tx).is_ok()
    };
    let shifted = [c.side1, c.side2].iter().any(|&s| app.slot(s).cfg().offset != 0 || app.slot(s).cfg().split);
    let sq0 = app.sq_level(c.side1) == 0 || app.sq_level(c.side2) == 0;
    if !app.radio.tx_built {
        // Decision 93: the repeater keys only in the `tx` build.
        put(1, Check::Fail, ferrite_core::xb::Stop::NoTx.text().as_bytes(), &mut fail);
    } else if !tx(c.side1) || !tx(c.side2) {
        let which = if !tx(c.side1) { 1 } else { 2 };
        b.clear();
        let _ = uwrite!(b, "Side {} can't transmit there", which);
        put(1, Check::Fail, b.as_bytes(), &mut fail);
    } else if sq0 {
        put(1, Check::Fail, b"Squelch is 0 on a side", &mut fail);
    } else if shifted {
        put(1, Check::Warn, b"Simplex: shifts are ignored", &mut fail);
    } else {
        put(1, Check::Ok, b"Transmit allowed on both", &mut fail);
    }
    if c.flags & XbCfg::ID == 0 {
        put(2, Check::Ok, b"ID off", &mut fail);
    } else if app.set.callsign.as_bytes().is_empty() {
        put(2, Check::Fail, b"ID needs a callsign (setup)", &mut fail);
    } else {
        b.clear();
        b.push(b"ID as ");
        b.push(app.set.callsign.as_bytes());
        put(2, Check::Ok, b.as_bytes(), &mut fail);
    }
    // A rough figure: about 4 h on a full battery at low, 2.5 h at mid.
    b.clear();
    let hours = batt as u32 * if c.power == 0 { 40 } else { 25 } / 1000;
    let _ = uwrite!(b, "Battery {}%: about {} h at {}", batt, hours.max(1), if c.power == 0 { "Low" } else { "Mid" });
    // Always a warning: it runs unattended.
    put(3, Check::Warn, b.as_bytes(), &mut fail);
    fail
}

pub(crate) fn running(app: &App) -> bool {
    app.radio.mode == Mode::Xb
}

/// Who started or stopped the repeater (XB-20.6), a byte: [`BY_KEYS`] the radio's own keys, or [`BY_REMOTE`] (remote
/// keys) or [`BY_CMD`] (`xb start` / `xb stop`) with the port in the low bits ([`crate::remote::BY_CABLE`],
/// [`crate::remote::BY_BT`], [`crate::remote::BY_TERM`]).
pub const BY_KEYS: u8 = 0;
pub const BY_REMOTE: u8 = 0x10;
pub const BY_CMD: u8 = 0x20;
/// In [`crate::screens::activity::RadioUi::xb_by`]: SIDE1 held, listening (XB-15).
pub const LISTEN: u8 = 0x80;

/// The readback's and the log's word for who: `radio`, `keys:cable`, `cmd:bt`, ...
pub fn by_word(by: u8) -> &'static str {
    match by & !LISTEN {
        0x10 => "keys:cable",
        0x11 => "keys:bt",
        0x12 => "keys:term",
        0x20 => "cmd:cable",
        0x21 => "cmd:bt",
        0x22 => "cmd:term",
        _ => "radio",
    }
}

/// Why it stopped, for the log: a key ([`XB_KEY`]), `xb stop` ([`XB_CMD`]), its screen closed ([`XB_SCREEN`]), or
/// the radio refused it (`XB_REFUSED | ferrite_core::xb::Stop` code).
pub const XB_KEY: u8 = 0;
pub const XB_CMD: u8 = 1;
pub const XB_SCREEN: u8 = 2;
pub const XB_REFUSED: u8 = 0x80;

/// A start or a stop for the cable log (XB-20.6): the firmware takes it with [`App::take_xb_log`] and prints it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XbLog {
    pub on: bool,
    pub by: u8,
    pub why: u8,
}

impl XbLog {
    /// The cable log's text after "xb: ".
    pub fn text(self, b: &mut Buf<96>) {
        if self.on {
            b.push(b"started by ");
        } else {
            b.push(b"stopped by ");
        }
        b.push(by_word(self.by).as_bytes());
        if !self.on {
            b.push(b": ");
            b.push(match self.why {
                XB_KEY => b"a key".as_slice(),
                XB_CMD => b"xb stop",
                XB_SCREEN => b"its screen closed",
                w => ferrite_core::xb::Stop::from_code(w & !XB_REFUSED).map_or(b"refused".as_slice(), |s| s.text().as_bytes()),
            });
        }
    }
}

impl App {
    /// XB-20.6: the last start or stop of the crossband repeater not yet logged.
    pub fn take_xb_log(&mut self) -> Option<XbLog> {
        self.radio.xb_log.take()
    }
}

/// XB-20.5: while the repeater runs, any key but PTT (XB-14) and SIDE1 (XB-15) stops it, the radio's or a remote one,
/// lit or dark, locked or not; that press does nothing else (true: it was taken).
pub(crate) fn key_stops(app: &mut App, k: Key) -> bool {
    if !running(app) || matches!(k, Key::Ptt(_) | Key::Side(1)) {
        return false;
    }
    let by = if app.remote_in { BY_REMOTE | app.rkeys_by() } else { BY_KEYS };
    stop(app, by, XB_KEY);
    true
}

/// Stops the repeater (the reconciler unkeys first, XB-14), saying who and why for the log.
pub(crate) fn stop(app: &mut App, by: u8, why: u8) {
    app.radio.xb_on = false;
    app.radio.xb_log = Some(XbLog { on: false, by, why }); // the reconciler's stop keeps it
    app.show_toast(b"Repeater stopped", if by & BY_REMOTE != 0 || by & BY_CMD != 0 { b"from the link" } else { b"" }, false);
    app.reconcile_mode();
}

impl View for Crossband {
    fn own_side1(self) -> bool {
        true
    }

    /// KEY-19: ◀▶▲▼ move between the start screen's cells.
    fn own_arrows(self) -> bool {
        true
    }

    fn name(self) -> &'static str {
        "Crossband repeater"
    }

    fn mockup(self) -> Option<&'static str> {
        Some("XbStart.png")
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Repeater");
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        use Press::*;
        if running(app) {
            return; // XB-20.5: every key but PTT and SIDE1 stopped it as it went down ([`key_stops`])
        }
        // Start page: ◀▶ along a row, ▲▼ between the sides and the grid's two rows.
        const NEXT: [[u8; 4]; 8] = [
            // left, right, up, down
            [0, 1, 0, 2],
            [0, 1, 1, 4],
            [2, 3, 0, 5],
            [2, 4, 0, 6],
            [3, 4, 1, 7],
            [5, 6, 2, 5],
            [5, 7, 3, 6],
            [6, 7, 4, 7],
        ];
        let n = NEXT[self.cell as usize % 8];
        let to = |i: usize| Screen::Crossband(Crossband { cell: n[i] });
        match (e.key, e.press) {
            (Key::Left, Short | Repeat) => app.screen = to(0),
            (Key::Right, Short | Repeat) => app.screen = to(1),
            (Key::Up, Short | Repeat) => app.screen = to(2),
            (Key::Down, Short | Repeat) => app.screen = to(3),
            (Key::Hash, Short) => {
                // XB-20.2: remote keys start it as the radio's do (LINK-19's one exception to "never transmit")
                let by = if app.remote_in { BY_REMOTE | app.rkeys_by() } else { BY_KEYS };
                let _ = try_start(app, by);
            }
            (Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        if !running(app) {
            change(app, self.cell, d);
        }
    }

    /// XB-14: local PTT transmits on the slot it keys (KEY-7: PTT B slot B, PTT A the active slot), interrupting a
    /// pass; the Radio task holds the repeater meanwhile and it carries on after, back on this screen.
    fn ptt(self, app: &mut App, down: bool) {
        if down {
            super::nav::transmit(app);
        }
    }

    /// XB-15: SIDE1 held listens to the receiving side (the Radio task's monitor on both sides, its speaker
    /// whatever the Speaker setting); let go, off again.
    fn raw(self, app: &mut App, key: Option<Key>, _code: Option<u8>, down: bool) {
        if key == Some(Key::Side(1)) && (running(app) && down || !down && app.radio.xb_by & LISTEN != 0) {
            listen(app, down);
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let run = running(app);
        if app.dirty(18, Mix::new().h(&run).get()) {
            lcd.fill(0, w::SBAR_H, 240, w::HINT_Y - w::SBAR_H, BG);
            app.drawn.hash[1..18].fill(0);
        }
        if run {
            let x = match cx.live.mode {
                ModeLive::Xb(x) => x,
                _ => XbLive::default(),
            };
            render_running(app, lcd, &x);
        } else {
            render_start(self, app, lcd, cx.dev.battery_pct);
        }
    }

    fn hints(self, app: &App) -> Hints {
        if running(app) {
            Hints::new(&[("Any key", "Stop"), ("SIDE1", "Listen")])
        } else {
            Hints::new(&[("Knob", "Change"), ("#", "Start"), ("BACK", "Back")])
        }
    }
}

/// XB-15: the monitor on both sides (the receiving one's audio to the speaker), or off.
pub(crate) fn listen(app: &mut App, on: bool) {
    app.radio.xb_by = app.radio.xb_by & !LISTEN | if on { LISTEN } else { 0 };
    let c = app.set.xb;
    for slot in [c.side1, c.side2] {
        app.out.push(RadioCmd::Monitor { slot, on });
    }
}

/// # on the start page, remote keys' # or `xb start` (XB-20): refuse saying why (XB-2), or start; `by` is who, for
/// the log. The start page must be up.
pub(crate) fn try_start(app: &mut App, by: u8) -> Result<(), Buf<40>> {
    let mut out = [(Check::Ok, Buf::new()); 4];
    // The battery line is a warning only; its level doesn't matter here.
    if let Some(i) = checks(app, 100, &mut out) {
        // the reason alone: after "Can't start:" most were cut (the page says "Can't start yet")
        app.show_toast(b"", out[i].1.as_bytes(), false);
        return Err(out[i].1);
    }
    app.radio.xb_on = true; // MODE-2: the reconciler starts it
    app.radio.xb_by = by;
    app.reconcile_mode();
    Ok(())
}

/// The repeater's start: the region and Extend TX for the Radio task's own checks on every pass (decision 93),
/// then the start.
pub(crate) fn send_start(app: &mut App) {
    let region = txplan::Region::ALL.iter().position(|&r| r == app.set.tx_region()).unwrap_or(0) as u8;
    app.out.push(RadioCmd::TxPlan { tx: app.slot(app.set.xb.side1).freq(), region, extend: app.set.extend_tx, confirmed: false });
    app.out.push(RadioCmd::Crossband(Some(app.set.xb)));
}

/// A side's card: letter, band, "Side n"; frequency; tone; a last line. `border` marks focus or direction.
fn side_card(app: &App, lcd: &mut impl Panel, x: i32, y: i32, slot: u8, side: u8, border: u16, tag: (&[u8], u16), last: &[u8], fill: u16) {
    let (wd, h) = (104, 70);
    ferrite_ui::canvas::banded(lcd, x, y, wd, h, BG, |lcd| {
        lcd.fill_r(x, y, wd, h, 3, fill);
        lcd.frame_r(x, y, wd, h, 3, border);
        let f = app.slot(slot).freq();
        w::badge(lcd, x + 6, y + 7, slot, true, fill);
        lcd.text4_rows(x + 24, y + 18, band_name(f).as_bytes(), &UI_12, SUB, fill, y + 4, y + 22);
        let _ = side;
        w::field(lcd, x + 50, 48, y + 4, y + 22, y + 18, tag.0, &w::st(&UI_12B, tag.1, fill, Align::Right));
        let fq = w::freq3(f);
        w::field(lcd, x + 6, wd - 10, y + 20, y + 42, y + 40, fq.as_bytes(), &w::st(&DIG_26, TEXT, fill, Align::Left));
        let mut t = Buf::<16>::new();
        tone_text(app.slot(slot).cfg().tx_tone, &mut t);
        if t.is_empty() {
            t.set(b"no tone");
        }
        w::field(lcd, x + 6, wd - 10, y + 42, y + 55, y + 53, t.as_bytes(), &w::st(&UI_12, TEXT, fill, Align::Left));
        w::field(lcd, x + 6, wd - 10, y + 55, y + 68, y + 66, last, &w::st(&UI_12, SUB, fill, Align::Left));
    });
}

fn render_start(s: Crossband, app: &mut App, lcd: &mut impl Panel, batt: u8) {
    let c = app.set.xb;
    if app.dirty(1, 1) {
        w::field(lcd, 8, 160, 21, 39, 34, b"Crossband repeater", const { &w::st(&UI_12B, TEXT, BG, Align::Left) });
        w::field(lcd, 168, 64, 21, 39, 34, b"Transmit", const { &w::st(&UI_12, SUB, BG, Align::Right) });
        lcd.fill(0, 40, 240, 1, LINE);
        parts::arrow2(lcd, 111, 81, 18, SUB);
    }
    // The two sides; the knob picks the focused side's slot.
    for side in 0..2u8 {
        let slot = if side == 0 { c.side1 } else { c.side2 };
        let foc = s.cell == side;
        let f = app.slot(slot).freq();
        if app.dirty(2 + side as usize, Mix::new().h(&slot).h(&foc).h(&f).any(app.slot(slot).cfg()).get()) {
            let mut label = Buf::<8>::new();
            let _ = uwrite!(label, "Side {}", side + 1);
            let mut last = Buf::<16>::new();
            if foc {
                last.set(b"knob: slot");
            } else {
                let _ = uwrite!(last, "slot {}", w::LETTERS[slot as usize] as char);
            }
            let (border, fill, tc) = if foc { (BLUE, FOCUS, BLUE) } else { (LINE, SURFACE, SUB) };
            side_card(app, lcd, 6 + side as i32 * 124, 46, slot, side, border, (label.as_bytes(), tc), last.as_bytes(), fill);
        }
    }
    // The settings grid: 3 x 2 cells, label over value.
    for (i, label) in GRID.iter().enumerate() {
        let mut v = Buf::<16>::new();
        grid_value(&c, i, &mut v);
        let (col, row) = (i as i32 % 3, i as i32 / 3);
        let x = 6 + col * 77;
        app.grid_cell(lcd, 3 + i, x, 121 + row * 32, 74, 29, label.as_bytes(), v.as_bytes(), s.cell as usize == 2 + i, false);
    }
    // XB-2's pre-flight checks, then the button.
    let mut out = [(Check::Ok, Buf::new()); 4];
    let fail = checks(app, batt, &mut out);
    if app.dirty(10, Mix::new().any(&out).get()) {
        ferrite_ui::canvas::banded(lcd, 6, 187, 228, 74, BG, |lcd| {
            lcd.fill_r(6, 187, 228, 74, 3, SURFACE);
            for (i, (k, t)) in out.iter().enumerate() {
                let y = 190 + 17 * i as i32;
                match k {
                    Check::Ok => parts::tick(lcd, 13, y + 5, GREEN),
                    Check::Warn => {
                        lcd.text4_rows(15, y + 13, b"!", &UI_12B, AMBER, SURFACE, y, y + 17);
                    }
                    Check::Fail => {
                        lcd.text4_rows(14, y + 13, b"x", &UI_12B, RED, SURFACE, y, y + 17);
                    }
                }
                let col = if *k == Check::Fail { RED } else { TEXT };
                w::field(lcd, 26, 206, y, y + 17, y + 13, t.as_bytes(), &w::st(&UI_12, col, SURFACE, Align::Left));
            }
        });
        let (bg, fg, t): (u16, u16, &[u8]) = if fail.is_some() { (LINE, SUB, b"Can't start yet") } else { (GREEN, BLUE_FG, b"# Start") };
        ferrite_ui::canvas::banded(lcd, 6, 266, 228, 24, BG, |lcd| {
            lcd.fill_r(6, 266, 228, 24, 2, bg);
            w::field(lcd, 16, 208, 268, 288, 283, t, &w::st(&UI_12B, fg, bg, Align::Center));
        });
    }
}

fn render_running(app: &mut App, lcd: &mut impl Panel, x: &XbLive) {
    let c = app.set.xb;
    let passing = matches!(x.state, 1 | 2);
    let (inp, outp) = if x.state == 2 { (c.side2, c.side1) } else { (c.side1, c.side2) };
    // The state line: READY listening both and the run time, or PASSING A → B and the pass time.
    let mut t = Buf::<16>::new();
    if passing {
        mmss(x.pass_s as u32, &mut t);
    } else {
        let m = x.run_s / 60;
        let _ = uwrite!(t, "{}h {}m", m / 60, m % 60);
    }
    if app.dirty(1, Mix::new().h(&x.state).h(&t).get()) {
        let (border, fill) = if passing { (GREEN, rgb565(0x10, 0x22, 0x16)) } else { (LINE, SURFACE) };
        ferrite_ui::canvas::banded(lcd, 6, 25, 228, 22, BG, |lcd| {
            lcd.fill_r(6, 25, 228, 22, 2, fill);
            lcd.frame_r(6, 25, 228, 22, 2, border);
            parts::dot(lcd, 14, 32, 7, if passing { RED } else { GREEN }, fill);
            if passing {
                let wd = lcd.text4_rows(26, 41, b"PASSING ", &UI_12B, GREEN, fill, 27, 46);
                dir_label(lcd, 26 + wd, 41, inp, outp, GREEN, fill);
            } else {
                let wd = lcd.text4_rows(26, 41, b"READY ", &UI_12B, TEXT, fill, 27, 46);
                let state: &[u8] = match x.state {
                    3 => b"hang",
                    4 => b"timed out",
                    _ => b"listening both",
                };
                lcd.text4_rows(26 + wd, 41, state, &UI_12, SUB, fill, 27, 46);
            }
            w::field(lcd, 180, 48, 27, 45, 41, t.as_bytes(), &w::st(&UI_12, TEXT, fill, Align::Right));
        });
    }
    // The sides: IN (S-meter) and OUT (its power level, or "Power reduced") while passing; what opens each
    // while ready. No drive meter: it only ever reads full (TX-12).
    let lit_in = (w::meter_level(x.rssi) as u32 * 8 / 20) as u8;
    for side in 0..2u8 {
        let slot = if side == 0 { c.side1 } else { c.side2 };
        let is_in = passing && slot == inp;
        let is_out = passing && slot == outp;
        let key = (x.state, slot, if is_in { lit_in } else { 0 }, is_out && x.reduced);
        if !app.dirty(2 + side as usize, Mix::new().h(&key).get()) {
            continue;
        }
        let px = 6 + side as i32 * 124;
        let (border, tag): (u16, (&[u8], u16)) = if is_in {
            (GREEN, (b"IN", GREEN))
        } else if is_out {
            (RED, (b"OUT", RED))
        } else {
            (LINE, (b"", SUB))
        };
        let mut last = Buf::<16>::new();
        if !passing {
            // What opens this side: its receive tone (named only when it differs from the TX tone above).
            let cfg = app.slot(slot).cfg();
            if c.flags & XbCfg::TONE_GATE == 0 || cfg.rx_tone == 0 {
                last.set(b"on carrier");
            } else if cfg.rx_tone == cfg.tx_tone {
                last.set(b"on its tone");
            } else {
                let mut tn = Buf::<16>::new();
                tone_text(cfg.rx_tone, &mut tn);
                last.push(b"needs ");
                last.push(tn.as_bytes());
            }
        }
        side_card(app, lcd, px, 53, slot, side, border, tag, last.as_bytes(), SURFACE);
        let (ly, base) = (53 + 56, 53 + 65);
        if is_in {
            lcd.text4_rows(px + 6, base, b"S", &UI_12, SUB, SURFACE, ly, ly + 12);
            parts::segs(lcd, px + 18, ly + 3, 8, lit_in, 6, 8, 6, GREEN, SURFACE);
        } else if is_out {
            let mut p = Buf::<16>::new();
            let col = if x.reduced {
                p.set(b"Power reduced");
                AMBER
            } else {
                p.set(b"TX ");
                let mut l = Buf::<16>::new();
                grid_value(&c, 0, &mut l);
                p.push(l.as_bytes());
                SUB
            };
            lcd.text4_rows(px + 6, base, p.as_bytes(), &UI_12, col, SURFACE, ly, ly + 12);
        }
    }
    if app.dirty(4, Mix::new().h(&passing).h(&(x.state == 2)).get()) {
        ferrite_ui::canvas::banded(lcd, 110, 75, 20, 14, BG, |lcd| {
            if passing {
                parts::arrow(lcd, 112, 82, 16, if x.state == 1 { 3 } else { 2 }, GREEN);
            } else {
                parts::arrow2(lcd, 111, 82, 18, SUB);
            }
        });
    }
    // The last pass, or this pass's timeout (XB-9).
    let mut line = Buf::<40>::new();
    if passing {
        mmss(x.pass_s as u32, &mut line);
        line.push(b" / ");
        mmss(c.timeout_10s as u32 * 10, &mut line);
    } else if x.last_dir != 0 {
        let mut a = Buf::<16>::new();
        super::scan::ago(x.idle_s as u32 * 1000, &mut a);
        if a.as_bytes() != b"now" {
            a.push(b" ago");
        }
        let _ = uwrite!(line, ", {}, ", super::activity::txt(a.as_bytes()));
        // (the direction is drawn before it)
        mmss(x.pass_s as u32, &mut line);
    } else {
        line.set(b"No passes yet");
    }
    if app.dirty(5, Mix::new().h(&line).get()) {
        ferrite_ui::canvas::banded(lcd, 0, 127, 240, 17, BG, |lcd| {
            if passing {
                w::field(lcd, 8, 50, 127, 144, 140, b"Timeout", const { &w::st(&UI_12, SUB, BG, Align::Left) });
                parts::bar(lcd, 60, 132, 88, 6, x.pass_s as u32, c.timeout_10s as u32 * 10, SUB, BG);
                w::field(lcd, 150, 82, 127, 144, 140, line.as_bytes(), const { &w::st(&UI_12, TEXT, BG, Align::Right) });
            } else if x.last_dir != 0 {
                let wd = lcd.text4_rows(8, 140, b"Last ", &UI_12, SUB, BG, 127, 144);
                let (a, b) = if x.last_dir == 2 { (c.side2, c.side1) } else { (c.side1, c.side2) };
                let dw = dir_label(lcd, 8 + wd, 140, a, b, SUB, BG);
                lcd.text4_rows(8 + wd + dw, 140, line.as_bytes(), &UI_12, SUB, BG, 127, 144);
            } else {
                lcd.text4_rows(8, 140, line.as_bytes(), &UI_12, SUB, BG, 127, 144);
            }
        });
    }
    strip(app, lcd, 6, x, c.side1, c.side2, passing);
    // Four cells, each always in its place (XB-17): passes, duty, next ID, last pass ("now" while passing).
    let v = cells(&c, x);
    let cells = CELLS.iter().zip(v.iter().map(|b| b.as_bytes()));
    for (i, (l, val)) in cells.enumerate() {
        let (col, row) = (i as i32 % 2, i as i32 / 2);
        app.grid_cell(lcd, 7 + i, 6 + col * 116, 210 + row * 34, 112, 30, l, val, false, false);
    }
}

/// The running page's four cells (XB-17): their labels, then their values from the repeater's report.
pub(crate) const CELLS: [&[u8]; 4] = [b"Passes", b"Duty, 10 min", b"Next ID", b"Last pass"];

pub(crate) fn cells(c: &XbCfg, x: &XbLive) -> [Buf<16>; 4] {
    let passing = matches!(x.state, 1 | 2);
    let mut v = [Buf::<16>::new(), Buf::<16>::new(), Buf::<16>::new(), Buf::<16>::new()];
    let _ = uwrite!(v[0], "{} \u{b7} {} min", x.passes, x.busy_s / 60);
    let _ = uwrite!(v[1], "{}% of {}%", x.duty, DUTY_LIMIT);
    if c.flags & XbCfg::ID != 0 {
        let _ = uwrite!(v[2], "CW in {} min", x.next_id_s.div_ceil(60));
    } else {
        v[2].set(b"Off");
    }
    if passing {
        v[3].set(b"now");
    } else if x.last_dir != 0 {
        super::scan::ago(x.idle_s as u32 * 1000, &mut v[3]);
        if v[3].as_bytes() != b"now" {
            v[3].push(b" ago");
        }
    } else {
        v[3].set(b"none yet");
    }
    v
}

/// The start page for the readback (LINK-19): each side's slot, the grid's six settings and XB-2's four checks
/// (0 ok, 1 warning, 2 fails), as drawn.
pub(crate) fn start_shown(app: &App, batt: u8) -> ([u8; 2], [Buf<16>; 6], [(u8, Buf<40>); 4]) {
    let c = app.set.xb;
    let mut grid = [Buf::<16>::new(); 6];
    for (i, v) in grid.iter_mut().enumerate() {
        grid_value(&c, i, v);
    }
    let mut out = [(Check::Ok, Buf::new()); 4];
    checks(app, batt, &mut out);
    let checks = out.map(|(k, t)| (k as u8, t));
    ([c.side1, c.side2], grid, checks)
}

/// The grid's labels.
pub(crate) fn grid_names() -> &'static [&'static str; 6] {
    &GRID
}

/// "A → B" with a drawn arrow; returns its width.
fn dir_label(lcd: &mut impl Panel, x: i32, base: i32, from: u8, to: u8, c: u16, bg: u16) -> i32 {
    let a = [w::LETTERS[from as usize % 3], b' '];
    let mut px = x + lcd.text4_rows(x, base, &a, &UI_12B, c, bg, base - 13, base + 4);
    lcd.fill(px, base - 13, 14, 17, bg);
    parts::arrow(lcd, px, base - 4, 12, 3, c);
    px += 14;
    let b = [b' ', w::LETTERS[to as usize % 3]];
    px += lcd.text4_rows(px, base, &b, &UI_12B, c, bg, base - 13, base + 4);
    px - x
}

/// The last 30 minutes (XB-16): side 1 → 2 above the line, 2 → 1 below, each in the colour of the slot it was heard
/// on; the current pass red.
fn strip(app: &mut App, lcd: &mut impl Panel, region: usize, x: &XbLive, s1: u8, s2: u8, passing: bool) {
    if !app.dirty(region, Mix::new().h(&x.strip).h(&passing).h(&s1).h(&s2).get()) {
        return;
    }
    let (y, h) = (146, 60);
    ferrite_ui::canvas::banded(lcd, 6, y, 228, h, BG, |lcd| {
        lcd.fill_r(6, y, 228, h, 3, SURFACE);
        dir_label(lcd, 12, y + 13, s1, s2, SUB, SURFACE);
        w::field(lcd, 150, 78, y + 2, y + 16, y + 13, b"Last 30 min", const { &w::st(&UI_12, SUB, SURFACE, Align::Right) });
        dir_label(lcd, 12, y + h - 4, s2, s1, SUB, SURFACE);
        w::field(lcd, 150, 78, y + h - 16, y + h - 1, y + h - 4, b"now", const { &w::st(&UI_12, SUB, SURFACE, Align::Right) });
        let mid = y + h / 2;
        lcd.fill(12, mid, 216, 1, LINE);
        for (i, &v) in x.strip.iter().enumerate() {
            if v == 0 {
                continue;
            }
            let bx = 13 + i as i32 * 216 / XB_COLS as i32;
            let bh = (v.unsigned_abs() as i32 * 14 / 30).clamp(3, 14);
            let now = i == XB_COLS - 1 && passing;
            if v > 0 {
                lcd.fill(bx, mid - bh, 3, bh, if now { RED } else { w::slot_colour(s1) });
            } else {
                lcd.fill(bx, mid + 1, 3, bh, if now { RED } else { w::slot_colour(s2) });
            }
        }
    });
}
