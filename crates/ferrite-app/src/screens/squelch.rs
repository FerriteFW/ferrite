//! Squelch from home (manual chapter 12, SQL-7 and SQL-10; no mock-up, the status bar name "Squelch" is
//! inferred): hold SIDE1 and turn the knob or press ▲▼. The level 0-9 and the active slot's live signal show
//! while SIDE1 is held; the change is live (SQL-7); letting SIDE1 go returns home. Beside the live signal, the
//! noise floor (SQL-12, as nicFW's floor readout): the signal while the squelch is closed, smoothed, so the gap
//! between a station and the noise shows where to set the level.

use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_core::msg::LiveAll;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use super::parts;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Squelch {
    /// The noise floor, dBm (SQL-12); 0 until one is read.
    pub floor: i16,
}

/// SIDE1 held + knob or ▲▼: one level up or down (stops at 0 and 9), and this screen while SIDE1 is held.
pub fn step(app: &mut App, d: i32) {
    app.keys.used(Key::Side(1));
    // SQL-7.1: the hold's monitor (SIDE1 held past the long press, before the knob turned) ends as the level is set,
    // so the squelch opens and closes with the level as it turns (it stayed open: "Open: you hear it" whatever the
    // level, a review of Nov 20, 2026). A monitor turned on with a tap stays.
    if app.mon_held != 0 {
        let slot = app.mon_held - 1;
        app.mon_held = 0;
        if app.monitor & 1 << slot == 0 {
            app.out.push(ferrite_core::msg::RadioCmd::Monitor { slot, on: false });
        }
    }
    app.step_squelch(d);
    if !matches!(app.screen, Screen::Squelch(_)) {
        app.screen = Screen::Squelch(Squelch { floor: 0 });
    }
}

impl Squelch {
    /// SQL-12's line: the noise floor, and the signal above it while the squelch is open (empty until read).
    pub(crate) fn floor_text(self, rssi: i16, open: bool) -> Buf<40> {
        let mut fl = Buf::<40>::new();
        if self.floor != 0 {
            let _ = ufmt::uwrite!(fl, "Noise floor {} dBm", self.floor);
            if open && rssi > self.floor {
                let _ = ufmt::uwrite!(fl, ", signal +{} dB", rssi - self.floor);
            }
        }
        fl
    }
}

impl View for Squelch {
    fn own_side1(self) -> bool {
        true
    }

    fn name(self) -> &'static str {
        "Squelch"
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        if let (Key::Up | Key::Down, Press::Short | Press::Repeat) = (e.key, e.press) {
            step(app, if e.key == Key::Up { 1 } else { -1 });
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        step(app, d);
    }

    fn tick(self, app: &mut App, live: &LiveAll) {
        if !app.keys.is_held(Key::Side(1)) {
            app.screen = Screen::Home;
            return;
        }
        // SQL-12: the floor follows the signal while the squelch is closed (an eighth of each reading)
        let lv = live.slots[app.active as usize];
        // SQL-12.1: from the RSSI as read (not held down while closed), while there's no carrier (level 0 too)
        if !lv.carrier && lv.rssi_raw < 0 {
            let floor = if self.floor == 0 { lv.rssi_raw } else { (self.floor * 7 + lv.rssi_raw) / 8 };
            app.screen = Screen::Squelch(Squelch { floor });
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let a = app.active;
        let sq = app.sq_level(a);
        let lv = cx.live.slots[a as usize];
        if app.dirty(1, Mix::new().h(&a).h(&sq).get()) {
            w::badge(lcd, 8, 30, a, true, BG);
            w::field(lcd, 28, 200, 28, 46, 41, b"Squelch, active slot", const { &w::st(&UI_12, SUB, BG, Align::Left) });
            ferrite_ui::canvas::banded(lcd, 0, 52, 240, 60, BG, |lcd| {
                lcd.text4_rows(100, 104, &[b'0' + sq], &DIG_46, TEXT, BG, 60, 106);
                // 0-9: the level's segments.
                for i in 0..10 {
                    let c = if i < sq { BLUE } else { LINE };
                    lcd.fill_r(20 + i as i32 * 20, 122, 16, 10, 1, c);
                }
                w::field(lcd, 8, 30, 136, 152, 148, b"open", const { &w::st(&UI_12, SUB, BG, Align::Left) });
                w::field(lcd, 172, 60, 136, 152, 148, b"tight", const { &w::st(&UI_12, SUB, BG, Align::Right) });
            });
        }
        // The live signal: what the level lets through (SQL-7).
        let lit = w::meter_level(lv.rssi_raw); // SQL-12.1: the signal under the threshold too
        if app.dirty(2, Mix::new().h(&lit).h(&lv.squelch_open).get()) {
            lcd.fill(0, 170, 240, 1, LINE);
            let (t, c): (&[u8], u16) = if lv.squelch_open { (b"Open: you hear it", GREEN) } else { (b"Closed", SUB) };
            w::field(lcd, 8, 160, 178, 196, 191, t, &w::st(&UI_12B, c, BG, Align::Left));
            w::field(lcd, 168, 64, 178, 196, 191, w::s_text(lv.rssi).as_bytes(), const { &w::st(&UI_12B, TEXT, BG, Align::Right) });
            w::meter(lcd, 20, 204, None, lit, if lv.squelch_open { GREEN } else { DIM }, BG);
        }
        // SQL-12: the floor and how far the signal is above it
        let fl = self.floor_text(lv.rssi, lv.squelch_open);
        if app.dirty(4, Mix::new().h(&fl).get()) {
            w::field(lcd, 8, 224, 216, 234, 229, fl.as_bytes(), const { &w::st(&UI_12, SUB, BG, Align::Left) });
        }
        if app.dirty(3, 1) {
            let mut b = Buf::<40>::new();
            b.push(b"Per band: MENU 0 9 9 1");
            parts::wrap(lcd, 8, 224, 240, 17, 1, b.as_bytes(), &UI_12, DIM, BG);
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[("Knob", "Level"), ("SIDE1", "Release: done")])
    }
}
