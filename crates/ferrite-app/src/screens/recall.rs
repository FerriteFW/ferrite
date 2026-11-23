//! HF recent (# in HF mode; chapter 20a, HF-28; C18): the last 10 HF frequencies with their mode, newest first.
//! OK tunes the entry, back in HF mode. The list is HF mode's memory, kept with the settings (HF-29). (FM presets
//! shared this screen until decision 258: saved stations are channels now, BCR-31.)

use super::lists::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Recall {
    pub cur: Cursor,
}

const ALL: fn(u16) -> bool = |_| true;

fn n(app: &App) -> u16 {
    super::hf::recent(&app.set.hf).count() as u16
}

impl View for Recall {
    fn name(self) -> &'static str {
        "HF recent"
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => self.knob(app, if e.key == Key::Up { -1 } else { 1 }, store),
            (Key::Menu, Short) => {
                // HF-28: OK tunes it, back in HF mode
                let hit = super::hf::recent(&app.set.hf).nth(self.cur.sel as usize);
                if let Some((f, m)) = hit {
                    app.back();
                    super::hf::tune_recent(app, f, m);
                }
            }
            // HF-28.1: # clears the list, asked first (MENU-23)
            (Key::Hash, Short) if n(app) > 0 => super::confirm::ask(app, super::confirm::Ask::ClearRecent),
            (Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        let cur = self.cur.step(d, n(app), ROWS1_FULL, &ALL);
        app.screen = Screen::Recall(Recall { cur });
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let data = cx.store.data();
        app.page(lcd, ());
        if !app.gate((self, data.version(), app.set.hf.recent)) {
            return;
        }
        let count = n(app);
        let mut l = Buf::<24>::new();
        let _ = uwrite!(l, "{} of 10", count);
        app.list_head(lcd, l.as_bytes(), b"newest first");
        let rows = self.cur.rows(count, ROWS1_FULL, &ALL, |row, i| {
            let (mut name, mut v) = (Buf::<20>::new(), Buf::<16>::new());
            if let Some((freq, mode)) = super::hf::recent(&app.set.hf).nth(i as usize) {
                let mut f = Buf::<16>::new();
                mhz_text(freq, &mut f);
                let _ = uwrite!(name, "{} MHz", Txt(f.as_bytes()));
                v.set(super::hf::mode_name(mode).as_bytes());
            }
            app.row1(lcd, row, name.as_bytes(), v.as_bytes(), false, i == self.cur.sel, false);
        });
        app.rows_end(lcd, rows, ROWS1_FULL, ROW1_H);
        app.list_cue(lcd, self.cur, count, ROWS1_FULL, ROW1_H, rows, &ALL);
        if count == 0 {
            app.empty(lcd, b"Nothing yet", b"10 s on a frequency adds it");
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[("OK", "Tune"), ("#", "Clear"), ("BACK", "Back")])
    }
}
