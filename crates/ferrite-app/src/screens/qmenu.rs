//! The quick menu (hold \* on home; manual chapter 6, HOME-17, decision 109): a small box over home that gives the
//! side keys and the knob a feature each while it is up. PTT A reverse, PTT B night red, PF1 GPS, PF2 Bluetooth,
//! the knob brightness. Each press acts at once and the box shows every row's new state.
//!
//! It is latched: it stays up until BACK, \* again or any other key (taken: that key does nothing else), or 5 s
//! with no key or knob input. It is not a screen: home stays under it, held as it was while the box is up (nothing
//! draws under the box, so nothing flickers through it, DISP-25), and when the box goes its rectangle is drawn
//! again from home off the screen, in bands, so the panel goes from the box straight to home.
//!
//! Safety (HOME-17): while it is up PTT A and PTT B are its keys and never key the transmitter, and a PTT pressed
//! while it was up stays dead until it is let go, even if the box closes while it is held ([`App::ptt_block`]).
//! The side connector's PTT (and VOX, which keys as it does) isn't one of its keys: it closes the box and
//! transmits as usual. [`super::transmit::key`] refuses to key while the box is up, as the last check.

use super::shared::*;
use super::{Cx, View};
use crate::chan::ChannelStore;
use crate::settings::{DAYLIGHT, NIGHT_RED};
use crate::slot::Item;
use crate::{App, Screen};
use ferrite_core::keys::{Key, PTT_A, PTT_B};
use ferrite_ui::assets::*;
use ferrite_ui::canvas;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// The box (x, y, w, h): five rows in the middle of the content area, 240 x 320 portrait.
pub(crate) const BOX: (i32, i32, i32, i32) = (32, 102, 176, 116);
const ROW_H: i32 = 20;
const ROWS: usize = 5;
/// Where each row's columns start: the key (bold), the feature; the state is right-aligned to the box's edge.
const KEY_X: i32 = BOX.0 + 10;
const NAME_X: i32 = BOX.0 + 56;
const VAL_R: i32 = BOX.0 + BOX.2 - 10;
/// The state's width: "auto" (GPS-1) the widest.
const VAL_W: i32 = 30;
/// The box closes after this long with no key or knob input.
pub const IDLE_MS: u32 = 5000;

/// The quick menu while it is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuickMenu {
    /// The last key or knob input it took (the 5 s close counts from here).
    last: u32,
    /// What is on the screen: [`DRAWN`] once the box is, and each row's state as drawn, 4 bits a row ([`lines`]).
    drawn: u32,
}

/// [`QuickMenu::drawn`]: the box is on the screen.
const DRAWN: u32 = 1 << 31;

/// The PTT's bit in [`App::ptt_block`]: PTT A and PTT B are the box's keys; the side connector's isn't.
pub(crate) fn ptt_bit(k: u8) -> u8 {
    match k {
        PTT_A => 1,
        PTT_B => 2,
        _ => 0,
    }
}

/// Hold \* on home (HOME-17): the box comes up. Every key held now is taken, \* too, so their releases do nothing.
pub(crate) fn open(app: &mut App) {
    app.keys.kill(None);
    if app.toast.is_some_and(|t| t.center) {
        // a hold digit's popup still up goes now, and home is drawn whole under the box this once
        app.toast = None;
        app.drawn.popup_gone = true;
    }
    app.qmenu = Some(QuickMenu { last: app.now, drawn: 0 });
}

/// The box goes; home is drawn again where it was ([`repaint`]) when it was on the screen.
pub(crate) fn close(app: &mut App) {
    if app.qmenu.take().is_some_and(|q| q.drawn & DRAWN != 0) {
        app.drawn.qmenu_gone = true;
    }
}

/// Every input and tick: the box closes after 5 s with no input, when the screen goes dark, when the keys lock
/// (auto lock) or when home isn't up any more.
pub(crate) fn expire(app: &mut App) {
    if app.qmenu.is_some_and(|q| app.now.wrapping_sub(q.last) >= IDLE_MS) || app.qmenu.is_some() && (app.dark() || app.locked() || !matches!(app.screen, Screen::Home)) {
        close(app);
    }
}

/// A key went down while the box is up: true when the box took it (nothing else may see it). PTT A and PTT B
/// run their features and are blocked from keying until released; PF1 and PF2 run theirs; the side connector's
/// PTT closes the box and goes on as a PTT; any other key closes it and is taken.
pub(crate) fn key_down(app: &mut App, key: Key) -> bool {
    let Some(q) = app.qmenu.as_mut() else { return false };
    if key == Key::PTT {
        close(app);
        return false;
    }
    q.last = app.now;
    app.keys.kill(Some(key));
    match key {
        Key::Ptt(k) => {
            app.ptt_block |= ptt_bit(k);
            if k == PTT_B {
                night_red(app);
            } else {
                app.change(Item::Reverse, 1); // VFO-7: two values, so +1 toggles; refused on the APRS slot
            }
        }
        Key::Side(1) => {
            super::groups::change(app, &[6, 1], 1); // GPS Off, Auto, On in turn (0 6 1, GPS-1)
        }
        Key::Side(2) => {
            super::groups::change(app, &[9, 2, 1], 1); // Bluetooth on/off (0 9 2 1)
        }
        _ => close(app),
    }
    true
}

/// The knob while the box is up: brightness (0 7 4 2) for the theme in use, 1-10 with no wrap, the keypad light
/// with it (DISP-16, DISP-22), live and saved as the setting is. True when the box took it.
pub(crate) fn knob(app: &mut App, d: i32) -> bool {
    let Some(q) = app.qmenu.as_mut() else { return false };
    q.last = app.now;
    super::groups::change(app, &[7, 4, 2], d);
    true
}

/// PTT B (HOME-17): the theme in use, then night red, then daylight, then back to the theme in use (0 7 1; dark
/// when that isn't known: night red was already on at power-on). Daylight is skipped when it is the theme in use.
fn night_red(app: &mut App) {
    let theme = app.day_theme & !DAY_STEP;
    if app.set.theme == NIGHT_RED {
        app.day_theme = if theme == DAYLIGHT { theme } else { theme | DAY_STEP };
        app.set.theme = DAYLIGHT;
    } else if app.set.theme == DAYLIGHT && app.day_theme & DAY_STEP != 0 {
        (app.day_theme, app.set.theme) = (theme, theme);
    } else {
        (app.day_theme, app.set.theme) = (app.set.theme, NIGHT_RED);
    }
}

/// [`App::day_theme`]'s bit 7: daylight is PTT B's step after night red (HOME-17).
pub(crate) const DAY_STEP: u8 = 0x80;

/// A row: the key, its feature, that feature's state now as shown and its colour, and the state as a 4-bit code.
pub(crate) type Line = (&'static str, &'static str, Buf<4>, u16, u8);

/// The box's rows.
pub(crate) fn lines(app: &App) -> [Line; ROWS] {
    let onoff = |on: bool| (Buf::from(if on { b"on" as &[u8] } else { b"off" }), if on { GREEN } else { SUB }, on as u8);
    let rev = if app.greyed(Item::Reverse).is_some() {
        (Buf::from(b"-"), DIM, 2) // APRS-19: the APRS slot has no reverse
    } else {
        onoff(app.slots[app.active as usize].cfg().reverse)
    };
    // HOME-17: PTT B's step: red (night red), day (daylight, the step after it), else off (0 off, 1 red, 2 day)
    let night = if app.set.theme == NIGHT_RED {
        (Buf::from(b"red"), GREEN, 1)
    } else if app.set.theme == DAYLIGHT && app.day_theme & DAY_STEP != 0 {
        (Buf::from(b"day"), GREEN, 2)
    } else {
        onoff(false)
    };
    // GPS-1: off, auto or on (the state code: 0 off, 1 on, 2 auto, as stored)
    let gps = match app.set.gps {
        crate::settings::GPS_AUTO => (Buf::from(b"auto"), TEXT, 2),
        g => onoff(g == crate::settings::GPS_ON),
    };
    let bt = onoff(app.set.bt);
    let level = app.set.brightness();
    let mut b = Buf::<4>::new();
    let _ = uwrite!(b, "{}", level);
    // HOME-17.1: the knob's row first (it sits on top of the radio), then the side keys top to bottom
    [
        ("Knob", "Brightness", b, TEXT, level),
        ("PTT A", "Reverse", rev.0, rev.1, rev.2),
        ("PTT B", "Night/day", night.0, night.1, night.2),
        ("PF1", "GPS", gps.0, gps.1, gps.2),
        ("PF2", "Bluetooth", bt.0, bt.1, bt.2),
    ]
}

/// Row `i`'s band: (y, h).
fn row_box(i: usize) -> (i32, i32) {
    (BOX.1 + 8 + i as i32 * ROW_H, ROW_H)
}

fn row(lcd: &mut impl Panel, i: usize, (key, name, val, fg, _): &Line) {
    let (y, h) = row_box(i);
    let b = y + 15;
    w::field(lcd, KEY_X, NAME_X - KEY_X, y, y + h, b, key.as_bytes(), const { &w::st(&UI_12B, TEXT, TOAST, Align::Left) });
    w::field(lcd, NAME_X, VAL_R - VAL_W - NAME_X, y, y + h, b, name.as_bytes(), const { &w::st(&UI_12, TEXT, TOAST, Align::Left) });
    w::field(lcd, VAL_R - VAL_W, VAL_W, y, y + h, b, val.as_bytes(), &w::st(&UI_12B, *fg, TOAST, Align::Right));
}

/// The box over home: whole when it isn't on the screen yet (the rounded corners keep what is under them), then
/// only the rows whose state changed, each drawn off the screen and sent once (DISP-25).
pub(crate) fn draw(app: &mut App, lcd: &mut impl Panel) {
    let Some(mut q) = app.qmenu else { return };
    let ls = lines(app);
    let (x, y, bw, bh) = BOX;
    if q.drawn & DRAWN == 0 {
        canvas::banded_over(lcd, x, y, bw, bh, |b| {
            b.fill_r(x, y, bw, bh, 4, BLUE);
            b.fill_r(x + 1, y + 1, bw - 2, bh - 2, 3, TOAST);
            for (i, l) in ls.iter().enumerate() {
                row(b, i, l);
            }
        });
        q.drawn = DRAWN;
        for (i, l) in ls.iter().enumerate() {
            q.drawn |= (l.4 as u32 & 15) << (4 * i);
        }
    } else {
        for (i, l) in ls.iter().enumerate() {
            let code = l.4 as u32 & 15;
            if q.drawn >> (4 * i) & 15 != code {
                let (ry, rh) = row_box(i);
                canvas::banded(lcd, x + 1, ry, bw - 2, rh, TOAST, |b| row(b, i, l));
                q.drawn = q.drawn & !(15 << (4 * i)) | code << (4 * i);
            }
        }
    }
    app.qmenu = Some(q);
}

/// The content was cleared (a new screen, a theme): the box isn't on the screen any more.
pub(crate) fn unseen(app: &mut App) {
    if let Some(q) = &mut app.qmenu {
        q.drawn = 0;
    }
    app.drawn.qmenu_gone = false;
}

/// The box is on the screen: the screen under it waits (it would draw through the box).
pub(crate) fn frozen(app: &App) -> bool {
    app.qmenu.is_some()
}

/// The box went: what was under it is drawn again, so the panel goes from the box straight to home (DISP-25).
/// The screen's regions keep what they knew of the rest of the panel, so the next render draws only what changed
/// while the box was up.
///
/// The cards layout (HOME-3's Watch): the gaps between cards under the box go back to the background here, and the
/// cards under it are marked new, so home's render draws each whole, recorded once and replayed into its own bands
/// (DISP-29). Before (0.8.1), every 10-row band of the box ran the whole of home (about 12 passes) and the box went
/// slowly (Alex, Nov 9, 2026). The other layouts still repaint the box's rectangle in bands.
pub(crate) fn repaint<S: ChannelStore>(app: &mut App, lcd: &mut impl Panel, cx: &Cx<S>) {
    app.drawn.qmenu_gone = false;
    let (x, y, bw, bh) = BOX;
    if super::home::uncover(app, lcd, BOX) {
        return;
    }
    let (hash0, meter0, card0, knob0) = (app.drawn.hash, app.drawn.meter, app.drawn.card, app.knob_at.take());
    canvas::banded_part(lcd, x, y, bw, bh, BG, |b| {
        // every band draws the whole of home afresh: what it drew is forgotten before each
        app.drawn.hash = [0; REGIONS];
        app.drawn.meter = [None; ferrite_core::msg::SLOTS];
        app.drawn.card = [0; ferrite_core::msg::SLOTS];
        super::home::Home.render(app, b, cx); // the box is only ever over home
    });
    (app.drawn.hash, app.drawn.meter, app.drawn.card, app.knob_at) = (hash0, meter0, card0, knob0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HOME-17: every row's key, feature and longest state fit their columns in the box (240 px wide screen).
    #[test]
    fn rows_fit_the_box() {
        for (k, n) in [("PTT A", "Reverse"), ("PTT B", "Night/day"), ("PF1", "GPS"), ("PF2", "Bluetooth"), ("Knob", "Brightness")] {
            assert!(text4_width(k.as_bytes(), &UI_12B) <= NAME_X - KEY_X - 4, "{k}");
            assert!(text4_width(n.as_bytes(), &UI_12) <= VAL_R - VAL_W - NAME_X - 4, "{n}");
        }
        for v in ["off", "on", "red", "day", "10"] {
            assert!(text4_width(v.as_bytes(), &UI_12B) <= 24, "{v}");
        }
        assert!(BOX.0 >= 16 && BOX.0 + BOX.2 <= 240 - 16, "inside the screen with room either side");
        assert!(row_box(ROWS - 1).0 + ROW_H <= BOX.1 + BOX.3 - 4, "the rows fit the box");
        assert!(BOX.1 > 20 && BOX.1 + BOX.3 < 300, "in the content area");
    }
}
