//! Auto reply (APRS-57; MENU 0 5 8 9): a message to us gets one reply, after a delay, from the stations a pattern
//! takes. Four rows: on or off, who it answers (`*` everyone, `VE7*` every call starting so, or one callsign-SSID),
//! the delay (10 s to 2 min) and the reply. ▲▼ pick a row; the knob (and ◀▶, KEY-19) changes on/off and the
//! delay; OK changes them too, or types the pattern (the text editor) or the reply (the message editor, 45
//! characters).

use super::shared::ARROWS_UD;
use super::{Cx, Hints, View};
use crate::aprs::{AprsCfg, REPLY_DELAY_NAMES};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AutoReply {
    pub row: u8,
}

const ROWS: u8 = 4;
const ON: u8 = 0;
const TO: u8 = 1;
const DELAY: u8 = 2;
const TEXT: u8 = 3;

impl AutoReply {
    pub fn new() -> Self {
        AutoReply { row: 0 }
    }

    /// ◀▶ (or OK) on a row that has values: on/off flips, the delay steps round.
    fn change(self, app: &mut App, d: i32) {
        let c = app.aprs_cfg();
        let reply = match self.row {
            ON => c.reply ^ 1,
            DELAY => {
                let n = REPLY_DELAY_NAMES.len() as i32;
                let i = ((c.reply >> 4) as i32 + d).rem_euclid(n) as u8;
                c.reply & 0x0F | i << 4
            }
            _ => return,
        };
        app.set_aprs(AprsCfg { reply, ..c });
    }
}

impl View for AutoReply {
    fn name(self) -> &'static str {
        "Auto reply"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Reply"); // 8 or fewer (SBAR-5)
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        use Press::*;
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => {
                let row = (self.row as i32 + if e.key == Key::Up { -1 } else { 1 }).rem_euclid(ROWS as i32) as u8;
                app.screen = Screen::AutoReply(AutoReply { row });
            }
            (Key::Menu, Short) => match self.row {
                TO => {
                    app.text = Buf::from(app.aprs_cfg().reply_to_text());
                    app.open(super::nav::Dest::Text(super::text::TextFor::ReplyTo));
                }
                TEXT => {
                    app.text.clear();
                    app.open_screen(Screen::Compose(super::msgs::Compose::reply_edit()));
                }
                _ => self.change(app, 1),
            },
            (Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        self.change(app, d);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, _: &Cx<impl ChannelStore>) {
        app.page(lcd, 0u8);
        let c = app.aprs_cfg();
        app.list_head(lcd, b"Answers messages to you", b"");
        let to: &[u8] = match c.reply_to_text() {
            b"" | b"*" => b"Everyone (*)",
            t => t,
        };
        let text: &[u8] = if c.reply_text().is_empty() { b"None" } else { c.reply_text() };
        let rows: [(&[u8], &[u8]); 4] = [
            (b"Auto reply", if c.reply_on() { b"On" } else { b"Off" }),
            (b"Reply to", to),
            (b"Delay", REPLY_DELAY_NAMES[(c.reply >> 4) as usize % REPLY_DELAY_NAMES.len()].as_bytes()),
            (b"Message", text),
        ];
        for (k, (name, v)) in rows.iter().enumerate() {
            app.row1(lcd, k, name, v, false, k == self.row as usize, false);
        }
        let help = if c.reply_on() && c.reply_text().is_empty() {
            txt!("On, but no message yet: nothing is sent.")
        } else {
            txt!("One reply per station, then none for 30 min.")
        };
        app.help_txt(lcd, 300 - 52, 48, help, b"MENU 0 5 8 9");
    }

    fn hints(self, _: &App) -> Hints {
        match self.row {
            TO | TEXT => Hints::new(&[("OK", "Type"), (ARROWS_UD, "Row"), ("BACK", "Back")]),
            _ => Hints::new(&[("Knob", "Change"), (ARROWS_UD, "Row"), ("BACK", "Back")]),
        }
    }
}
