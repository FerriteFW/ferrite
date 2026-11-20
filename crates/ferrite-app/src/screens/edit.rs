//! Edit in place (hold SLOT; manual chapters 6 and 9, EDIT-1 to EDIT-7; HomeEdit mock-up): the active slot's
//! settings as a 3 x 3 grid, arrows pick, the knob changes at once. On a channel, hold SLOT opens the channel
//! version instead ([`super::chedit`]: zone, name, frequency and 12 settings).

use super::nav::Dest;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::slot::GRID;
use crate::{App, Screen, EDIT_IDLE_MS};
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Edit {
    /// The focused cell, 0-8, row by row.
    pub cell: u8,
}

impl View for Edit {
    /// KEY-19: ◀▶ move across the edit grid; the knob changes the cell.
    fn own_arrows(self) -> bool {
        true
    }

    fn name(self) -> &'static str {
        "Edit"
    }

    fn mockup(self) -> Option<&'static str> {
        Some("HomeEdit.png")
    }

    fn title(self, app: &App, b: &mut Buf<10>) {
        let _ = uwrite!(b, "EDIT {}", w::LETTERS[app.active as usize] as char);
    }

    fn timeout(self) -> Option<u32> {
        Some(EDIT_IDLE_MS) // EDIT-6
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        use Press::*;
        let cell = self.cell;
        let (r, c) = (cell / 3, cell % 3);
        let mv = |r: u8, c: u8| Screen::Edit(Edit { cell: r * 3 + c });
        match (e.key, e.press) {
            // EDIT-3: arrows pick, the knob changes.
            (Key::Up, Short | Repeat) => app.screen = mv(r.saturating_sub(1), c),
            (Key::Down, Short | Repeat) => app.screen = mv((r + 1).min(2), c),
            (Key::Left, Short | Repeat) => app.screen = mv(r, c.saturating_sub(1)),
            (Key::Right, Short | Repeat) => app.screen = mv(r, (c + 1).min(2)),
            (Key::Exit | Key::Slot, Short) => app.back(), // EDIT-6: leaves the edit, one level back
            (Key::Star, Short) => app.do_undo(),
            (Key::Hash, Short) => app.reset(GRID[cell as usize]),
            // OK on a cell: its bigger editor (MENU-17).
            (Key::Menu, Short) => app.open(Dest::value(GRID[cell as usize])),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        app.change(GRID[self.cell as usize], d);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let (live, cell) = (cx.live, self.cell);
        let a = app.active;
        let mut head = Buf::<32>::new();
        app.head_text(a, live, cx.store, &mut head);
        if app.dirty(R_TOP, Mix::new().h(&a).h(&head).get()) {
            w::badge(lcd, 7, 26, a, true, BG);
            w::field(lcd, 21, 6, 25, 42, 38, b"", const { &w::st(&UI_12, SUB, BG, Align::Left) });
            w::field(lcd, 27, 160, 25, 42, 38, w::fit(head.as_bytes(), &UI_12, 160), const { &w::st(&UI_12, SUB, BG, Align::Left) }); // HOME-4.3
            w::tag(lcd, 232, 25, b"EDIT", BLUE, BG); // EDIT-1
        }
        let f = app.shown_freq(a, live);
        // R_FREQ is grid cell 6's region (R_ROW + 6): the frequency uses R_SUB.
        if app.dirty(R_SUB, Mix::new().h(&f).get()) {
            w::freq(lcd, 7, 232, 76, f, &DIG_46, &DIG_28, TEXT, SUB, BG);
        }
        let cfg = *app.slots[a as usize].cfg();
        for (i, item) in GRID.into_iter().enumerate() {
            let mut v = Buf::<16>::new();
            item.text(&cfg, false, &mut v);
            if item == crate::slot::Item::Offset && cfg.split {
                // "TX 145.920" is 70 px in the 62 px cell: "Split" here; hold 7 shows its TX (VFO-26.6)
                v.set(b"Split");
            }
            let focused = i as u8 == cell;
            let grey = app.greyed(item).is_some();
            let (x, y) = (7 + (i as i32 % 3) * 77, 85 + (i as i32 / 3) * 36);
            app.grid_cell(lcd, i, x, y, 72, 31, item.label().as_bytes(), v.as_bytes(), focused, grey);
        }
        // The note under the grid: why the focused setting is greyed (EDIT-7), or what hold 9 does.
        let why = app.greyed(GRID[cell as usize]);
        let vfo = !app.slots[a as usize].on_channel();
        if app.dirty(R_NOTE, Mix::new().h(&why).h(&vfo).get()) {
            match why {
                Some(t) => {
                    let mut b = Buf::<40>::new();
                    b.push(GRID[cell as usize].label().as_bytes());
                    b.push(b": ");
                    b.push(t.as_bytes());
                    w::field(lcd, 7, 226, 190, 207, 202, b.as_bytes(), const { &w::st(&UI_12, AMBER, BG, Align::Left) });
                }
                None if vfo => w::key_line(lcd, 7, 233, 202, "Hold 9", "save as a channel", BG),
                None => w::key_line(lcd, 7, 233, 202, "Hold 0", "jumps to the home channel", BG),
            }
        }
        other_slots(app, lcd, live);
    }

    fn hints(self, app: &App) -> Hints {
        // Four pairs don't fit at 12 px where the mock-ups' smaller text has them; the knob always stays named
        // (MENU-16) and "* Undo" takes a place while it is offered (EDIT-4, MENU-22).
        if app.undo.is_some() {
            Hints::new(&[("Knob", "Change"), ("*", "Undo"), ("BACK", "Done")])
        } else {
            Hints::new(&[(ARROWS_4, "Pick"), ("Knob", "Change"), ("BACK", "Done")])
        }
    }
}

/// The other two slots under an editor, one line each (HomeEdit, HomeEditCh).
pub(crate) fn other_slots(app: &mut App, lcd: &mut impl Panel, live: &ferrite_core::msg::LiveAll) {
    let a = app.active;
    // The other two slots, one line each.
    for (k, s) in [(a + 1) % 3, (a + 2) % 3].into_iter().enumerate() {
        let mut t = Buf::<48>::new();
        t.push(w::freq3(app.shown_freq(s, live)).as_bytes());
        let sl = &app.slots[s as usize];
        let lv = &live.slots[s as usize];
        if lv.scanning {
            let _ = uwrite!(t, " \u{b7} {} \u{b7} {} hits", crate::slot::band_name(lv.freq), lv.hits);
        } else {
            if let Some(c) = sl.ch.filter(|_| sl.on_channel()) {
                t.push(" \u{b7} ".as_bytes());
                t.push(c.name());
            }
            let (st, _) = app.state(s, live);
            t.push(" \u{b7} ".as_bytes());
            t.push(st.as_bytes());
        }
        if app.dirty(R_LINE + k, Mix::new().h(&s).h(&t).get()) {
            let y = 268 + k as i32 * 17;
            w::badge(lcd, 7, y, s, false, BG);
            // DISP-31: never into the hint bar's line (y 300): the second line's box reached it, and each scan hit
            // erased it
            let y1 = (y + 16).min(w::HINT_Y);
            w::field(lcd, 21, 4, y - 1, y1, y + 11, b"", const { &w::st(&UI_12, SUB, BG, Align::Left) });
            // a long channel name with its state ("443.475 · Club repeater · paused", 224 px) is cut on purpose, with "…"
            w::field(lcd, 25, 211, y - 1, y1, y + 11, w::fit(t.as_bytes(), &UI_12, 211), const { &w::st(&UI_12, SUB, BG, Align::Left) });
        }
    }
}
