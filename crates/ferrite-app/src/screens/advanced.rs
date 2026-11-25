//! Advanced settings (MENU 0, or hold MENU on home; manual chapter 11, MENU-2, MENU-8; MenuTop mock-up): the
//! Recent row and the nine groups, each with a summary value. A digit or OK opens a group ([`Dest::Group`]);
//! 0 opens the Status page (SBAR-13). The Recent row is selectable like the groups (above group 1, once
//! something was changed): ◀▶ pick one of its settings, OK opens that setting's editor (MENU-8, MENU-17).

use super::nav::Dest;
use super::shared::*;
use super::status::Status;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_core::msg::GpsState;
use ferrite_core::VERSION;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets::{self as w, Row};
use ufmt::uwrite;
use ferrite_core::msg::Device;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Advanced {
    /// The highlighted group, 0-8 (MENU 0 1 to MENU 0 9); [`RECENT`] + i: the Recent row, its i-th setting.
    pub row: u8,
    /// Opened from quick settings (BACK returns there by the BACK stack, like everywhere: MENU-24).
    pub from_quick: bool,
}

/// `row` from here on: the Recent row, with its selected setting.
pub const RECENT: u8 = 9;

/// The next row up or down: Recent (when it has anything) sits above group 1, and the list wraps.
fn step(app: &App, row: u8, d: i32) -> u8 {
    let has = app.recent[0].is_some() as i32;
    let n = 9 + has;
    // Positions: 0 Recent (if any), then the groups.
    let pos = if row >= RECENT { 0 } else { row as i32 + has };
    let p = (pos + d).rem_euclid(n);
    if has == 1 && p == 0 { RECENT } else { (p - has) as u8 }
}

/// Advanced settings groups (MENU 0 1-9) and their help lines.
pub(crate) const GROUPS: [(&str, crate::text::Txt); 9] = [
    ("Watch and scan", txt!("Watch, resume, priority, ranges, skips.")),
    ("Channels", txt!("Browse channels and zones, add a repeater.")),
    ("Tools", txt!("FM, scope, counter, HF, crossband, tones, ACARS.")),
    ("Decoders and alerts", txt!("Event log, pagers, DTMF, weather alerts.")),
    ("APRS", txt!("Heard, messages, beacons, your station.")),
    ("GPS and time", txt!("GPS, position, units, time zone, clock.")),
    ("Display, sound, power", txt!("Theme, layout, backlight, sounds, power.")),
    ("Transmit", txt!("Timeout, VOX, mic gain, DTMF, callsign, ID.")),
    ("System", txt!("Keys, links, PIN, region, receiver, reset.")),
];

impl View for Advanced {
    /// KEY-19: ◀▶ pick a Recent setting (MENU-8); elsewhere they are the knob.
    fn own_arrows(self) -> bool {
        true
    }

    fn name(self) -> &'static str {
        "More"
    }

    fn mockup(self) -> Option<&'static str> {
        Some("MenuTop.png")
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let Advanced { row, from_quick } = self;
        // The group's row stays highlighted, so BACK from the group lands on it (MENU-24).
        let open = |app: &mut App, r: u8| {
            app.screen = Screen::Advanced(Advanced { row: r, from_quick });
            app.open(Dest::Group(r + 1));
            super::groups::restore_row(app); // MENU-24: on its last item this visit
        };
        match (e.key, e.press) {
            (Key::Up, Short | Repeat) => app.screen = Screen::Advanced(Advanced { row: step(app, row, -1), from_quick }),
            (Key::Down, Short | Repeat) => app.screen = Screen::Advanced(Advanced { row: step(app, row, 1), from_quick }),
            (Key::Left | Key::Right, Short | Repeat) if row >= RECENT => {
                // MENU-8: pick one of the recent settings
                let n = app.recent.iter().flatten().count().max(1) as i32;
                let i = (row - RECENT) as i32 + if e.key == Key::Right { 1 } else { -1 };
                app.screen = Screen::Advanced(Advanced { row: RECENT + i.rem_euclid(n) as u8, from_quick });
            }
            (Key::Left | Key::Right, Short | Repeat) => self.knob(app, if e.key == Key::Right { 1 } else { -1 }, store),
            (Key::Menu, Short) if row >= RECENT => {
                if let Some(item) = app.recent[(row - RECENT) as usize % 3] {
                    app.open(Dest::value(item)); // MENU-17: its editor; BACK returns here
                }
            }
            (Key::Digit(0), Short) => {
                // SBAR-13: the Status page, one level down
                app.push_back();
                app.screen = Screen::Status(Status { row: 0, from_menu: true });
            }
            (Key::Digit(d), Short) => open(app, d - 1),
            (Key::Menu, Short) => open(app, row),
            (Key::Exit, Short) => app.back(), // MENU-24: up one level
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        app.screen = Screen::Advanced(Advanced { row: step(app, self.row, d), ..self });
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let (row, dev) = (self.row, cx.dev);
        let recent = app.recent;
        let sel = (row >= RECENT).then(|| (row - RECENT) as usize);
        if app.dirty(R_TOP, Mix::new().any(&recent).h(&sel).get()) {
            w::field(lcd, 0, 54, 21, 40, 35, b"", const { &w::st(&UI_12, SUB, BG, Align::Left) });
            w::field(lcd, 8, 46, 21, 40, 35, b"Recent", &w::st(&UI_12, if sel.is_some() { BLUE } else { SUB }, BG, Align::Left));
            let names: [&[u8]; 3] = recent.map(|r| r.map_or(b"".as_slice(), |i| i.label().as_bytes()));
            if names[0].is_empty() {
                w::field(lcd, 54, 178, 21, 40, 35, b"nothing yet", const { &w::st(&UI_12, DIM, BG, Align::Left) });
            } else {
                lcd.fill(54, 21, 178, 1, BG);
                w::chips_sel(lcd, 54, 22, 178, &names, sel, BG);
                lcd.fill(54, 39, 178, 1, BG);
            }
        }
        for (i, (name, _)) in GROUPS.iter().enumerate() {
            let v = group_value(app, cx.store.count(), dev, i);
            let r = Row { num: Some(i as u8 + 1), name: name.as_bytes(), value: v.as_bytes(), dot: false, selected: i as u8 == row, grey: false };
            app.menu_row(lcd, i, 41 + 24 * i as i32, 24, &r);
        }
        let mut p = Buf::<16>::new();
        match recent[sel.unwrap_or(0)].filter(|_| sel.is_some()) {
            Some(item) => {
                let mut t = Buf::<48>::new();
                t.set(b"Changed lately. OK opens ");
                t.push(item.name().as_bytes());
                t.push(b"; arrows pick another.");
                p.set(b"Recent");
                app.help_box(lcd, 260, 36, t.as_bytes(), p.as_bytes());
            }
            None => {
                let _ = uwrite!(p, "MENU 0 {}", row.min(8) + 1);
                app.help_txt(lcd, 260, 36, GROUPS[row.min(8) as usize].1, p.as_bytes());
            }
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[(ARROWS_UD, "Move"), ("OK", "Open"), ("BACK", "Back")]) // digits jump: the rows show them
    }
}

/// Group `i`'s value as drawn (the UI readback reads it too, LINK-19).
pub(crate) fn group_value(app: &App, channels: u16, dev: &Device, i: usize) -> Buf<24> {
    let mut v = Buf::<24>::new();
    match i {
        // decision 372: "Scanning" while a scan runs, else the watch (the two together no longer fit at 14 px)
        0 if app.scan_mask != 0 => v.set(b"Scanning"),
        0 => v.push(WATCH_TEXT[app.watch as usize % 3].as_bytes()),
        1 => {
            let _ = uwrite!(v, "{} used", channels);
        }
        3 => v.set(b"Off"), // no decoder running (a short word: the name is long)
        4 if app.log_unread > 0 => {
            let _ = uwrite!(v, "{} new", app.log_unread);
        }
        5 => match dev.gps {
            GpsState::Off => v.set(b"GPS off"),
            GpsState::Searching => v.set(b"Searching"),
            GpsState::Fix => {
                let _ = uwrite!(v, "{} sats", dev.sats);
            }
        },
        7 => v.set(if dev.tx_built { b"Ready".as_slice() } else { b"Receive only" }),
        8 => v.set(VERSION.as_bytes()),
        _ => {}
    }
    v
}
