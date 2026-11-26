//! The channel editor (edit in place on a channel: hold SLOT on a channel, MENU 8, end of Add repeater; manual
//! chapters 6 and 10, EDIT-1 to EDIT-7, CHAN-5, CHAN-18; HomeEditCh mock-up). Zone, name and frequency at the
//! top, twelve settings in a 3 x 4 grid under them. Arrows pick, the knob changes at once and the channel is
//! saved (EDIT-4); changing zone or number moves the channel (EDIT-5), and Home makes it the one home channel
//! (CHAN-18). OK on the name types it (A7), on the frequency types it (A1), on a setting opens its bigger
//! editor (A5, A6).

use super::edit::other_slots;
use super::entry::{Entry, EntryFor};
use super::forms::field_box;
use super::nav::Dest;
use super::shared::*;
use super::text::TextFor;
use super::{Cx, Hints, View};
use crate::chan::{Channel, ChannelStore, FLAG_APRS, FLAG_RX_ONLY, MAX_CHANNELS};
use crate::chfield::{ChField, Ctx, Op};
use crate::keys::{KeyEvent, Press};
use crate::slot::{chan_label, Item};
use crate::{App, Screen, EDIT_IDLE_MS};
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::{Buf, Zero};
use ferrite_ui::widgets as w;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChEdit {
    /// 0 zone, 1 name, 2 frequency, 3-14 the grid row by row.
    pub cell: u8,
}

/// A cell: a channel field ([`crate::chfield`]: its steps, text and checks), or the number (moves the channel to
/// the next free one: the editor's own).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cell {
    F(ChField),
    Number,
}

/// The twelve settings (EDIT-2; chapter 10's list), row by row.
const CELLS: [Cell; 12] = [
    Cell::F(ChField::It(Item::Mode)),
    Cell::F(ChField::It(Item::RxTone)),
    Cell::F(ChField::It(Item::Tones)),
    Cell::F(ChField::It(Item::Offset)),
    Cell::F(ChField::It(Item::Power)),
    Cell::F(ChField::It(Item::Squelch)),
    Cell::F(ChField::It(Item::BusyLock)),
    Cell::F(ChField::Scan),
    Cell::F(ChField::Flag(FLAG_APRS)),
    Cell::F(ChField::Flag(FLAG_RX_ONLY)),
    Cell::Number,
    Cell::F(ChField::Home),
];

const ZONE: u8 = 0;
const NAME: u8 = 1;
const FREQ: u8 = 2;
const GRID0: u8 = 3;
/// The Offset cell (shift and offset), where hold 7 and 8 open the editor on a channel.
#[cfg(test)]
pub(crate) const OFFSET_CELL: u8 = GRID0 + 3;
/// Grid geometry: 3 columns of 72 at 77 apart, 4 rows of 29 at 34 apart (HomeEditCh).
const GX: i32 = 7;
const GY: i32 = 108;

impl Cell {
    fn label(self) -> &'static str {
        match self {
            Cell::F(f) => f.label(false),
            Cell::Number => "Number",
        }
    }
}

/// Changes the active slot's channel (keeping undo), moves the slot with it and saves it (EDIT-4, EDIT-5).
pub(crate) fn edit(app: &mut App, f: impl FnOnce(&mut Channel)) {
    let a = app.active;
    let Some(old) = app.slots[a as usize].ch.filter(|_| app.slots[a as usize].on_channel()) else { return };
    app.keep_undo();
    let mut c = old;
    f(&mut c);
    if c == old {
        return;
    }
    if app.set.home_ch == Some(old.num) {
        app.set.home_ch = Some(c.num); // CHAN-18: renumbering carries it
    }
    let s = &mut app.slots[a as usize];
    (s.ch, s.zone) = (Some(c), c.zone);
    if c.rx != old.rx {
        app.send_slot(a);
    }
    app.save_channel(a, (c.num != old.num).then_some(old.num));
}

/// A field's change through its rules ([`crate::chfield`]): kept and saved (EDIT-4), Home set (CHAN-18), or the
/// refusal as a toast.
fn apply(app: &mut App, store: &impl ChannelStore, f: ChField, op: Op) {
    let Some(mut c) = app.slots[app.active as usize].ch else { return };
    let mut home = app.set.home_ch;
    let num = c.num;
    if let Err(r) = f.change(&mut c, &mut home, op, Ctx { zones: store.zones(), region: app.set.region(), room: &|z| store.zone_room(z, num) }) {
        return app.refusal_toast(r);
    }
    if home != app.set.home_ch {
        set_home(app, home.is_some());
    }
    let moved = app.slots[app.active as usize].ch.is_some_and(|o| o.zone != c.zone);
    edit(app, |x| *x = c);
    if moved {
        place(app, store);
    }
}

/// Where the channel now sits in its zone (by number, CHAN-10), counting the others in the store.
fn place(app: &mut App, store: &impl ChannelStore) {
    let a = app.active as usize;
    if let Some(c) = app.slots[a].ch {
        let pos = (0..store.zone_len(c.zone)).filter_map(|i| store.zone_at(c.zone, i)).filter(|o| o.num < c.num).count();
        app.slots[a].pos = pos as u16;
    }
}

impl View for ChEdit {
    /// KEY-19: ◀▶ move across the edit grid; elsewhere they are the knob.
    fn own_arrows(self) -> bool {
        true
    }

    fn name(self) -> &'static str {
        "Channel editor"
    }

    fn mockup(self) -> Option<&'static str> {
        Some("HomeEditCh.png")
    }

    fn title(self, app: &App, b: &mut Buf<10>) {
        let _ = uwrite!(b, "EDIT {}", w::LETTERS[app.active as usize] as char); // EDIT-1
    }

    fn timeout(self) -> Option<u32> {
        Some(EDIT_IDLE_MS) // EDIT-6
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        if !app.slots[app.active as usize].on_channel() {
            return app.go_home();
        }
        let cell = self.cell;
        let to = |c: u8| Screen::ChEdit(ChEdit { cell: c });
        let (r, col) = if cell >= GRID0 { ((cell - GRID0) / 3, (cell - GRID0) % 3) } else { (0, 0) };
        match (e.key, e.press) {
            (Key::Up, Short | Repeat) => {
                app.screen = to(match cell {
                    ZONE => ZONE,
                    NAME | FREQ => cell - 1,
                    _ if r == 0 => FREQ,
                    _ => cell - 3,
                })
            }
            (Key::Down, Short | Repeat) => {
                app.screen = to(match cell {
                    ZONE | NAME => cell + 1,
                    FREQ => GRID0,
                    _ if r == 3 => cell,
                    _ => cell + 3,
                })
            }
            (Key::Left, Short | Repeat) if cell >= GRID0 && col > 0 => app.screen = to(cell - 1),
            (Key::Right, Short | Repeat) if cell >= GRID0 && col < 2 => app.screen = to(cell + 1),
            (Key::Left | Key::Right, Short | Repeat) if cell < GRID0 => self.knob(app, if e.key == Key::Right { 1 } else { -1 }, store),
            (Key::Exit | Key::Slot, Short) => app.back(), // EDIT-6: leaves the edit, one level back
            (Key::Star, Short) => {
                app.do_undo();
                place(app, store);
            }
            (Key::Hash, Short) => match self.at() {
                // MENU-18: back to the default
                Some(Cell::F(ChField::It(i))) => app.reset(i),
                Some(Cell::F(f)) => apply(app, store, f, Op::Reset),
                _ => {}
            },
            (Key::Menu, Short) => match cell {
                NAME => {
                    super::text::open(app, TextFor::ChannelName, store);
                }
                FREQ => {
                    app.open(Dest::Entry(0));
                    app.screen = Screen::Entry(Entry::new(EntryFor::ChannelFreq, None));
                }
                _ => {
                    if let Some(Cell::F(ChField::It(i))) = self.at() {
                        app.open(Dest::value(i)); // MENU-17
                    }
                }
            },
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        let Some(ch) = app.slots[app.active as usize].ch else { return };
        match self.at() {
            Some(Cell::F(ChField::It(i))) => app.change(i, d), // the slot's setting, through the channel's rules
            Some(Cell::F(f)) => apply(app, store, f, Op::Step(d)),
            Some(Cell::Number) => {
                // The next free number that way (EDIT-5: renumbering moves the channel).
                let free = (1..MAX_CHANNELS as i32)
                    .map(|k| (ch.num as i32 + d.signum() * k).rem_euclid(MAX_CHANNELS as i32) as u16)
                    .find(|&n| store.get(n).is_none());
                if let Some(n) = free {
                    edit(app, |c| c.num = n);
                    place(app, store);
                }
            }
            None => {}
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let a = app.active;
        let Some(ch) = app.slots[a as usize].ch else { return };
        let cell = self.cell;
        // Top line: letter, number, the zone box, EDIT.
        let zone = cx.store.zone_label(ch.zone, b"None", b"?");
        let zone = zone.as_bytes();
        if app.dirty(R_TOP, Mix::new().h(&a).h(&ch.num).h(&zone).h(&(cell == ZONE)).get()) {
            w::badge(lcd, 7, 26, a, true, BG);
            let mut l = Buf::<32>::new();
            chan_label(ch.num, &mut l);
            // the zone box starts after the number as wide as it is ("CH 1234" with 4,000 channels), so it never
            // covers its last digit
            let lw = text4_width(l.as_bytes(), &UI_12);
            let zx = 27 + lw + 5;
            w::field(lcd, 21, 6, 24, 41, 38, b"", const { &w::st(&UI_12, TEXT, BG, Align::Left) });
            w::field(lcd, 27, zx - 27, 24, 41, 38, l.as_bytes(), const { &w::st(&UI_12, TEXT, BG, Align::Left) });
            field_box(lcd, zx, 23, 190 - zx, 20, b"Zone", zone, cell == ZONE, false);
            lcd.fill(190, 23, 2, 20, BG);
            w::tag(lcd, 232, 25, b"EDIT", BLUE, BG);
        }
        if app.dirty(R_NOTE, Mix::new().h(&ch.name).h(&(cell == NAME)).get()) {
            // 20 tall, as the zone box: the 46 px digits reach up to row 68 (ferrite#33: the box touched them)
            field_box(lcd, 7, 46, 226, 20, b"Name", ch.name(), cell == NAME, false);
        }
        // Regions: the grid takes 1-12, so the frequency uses R_SUB.
        // VFO-18.2: on the airband with the 8.33 kHz step, the channel name
        let rx = if ch.cfg.step == crate::slot::STEP_833 { ferrite_core::freq::shown_833(ch.rx) } else { ch.rx };
        if app.dirty(R_SUB, Mix::new().h(&rx).h(&(cell == FREQ)).get()) {
            w::freq(lcd, 7, 232, 102, rx, &DIG_46, &DIG_28, TEXT, SUB, BG);
            lcd.fill(7, 104, 225, 2, if cell == FREQ { BLUE } else { BG });
        }
        for (i, c) in CELLS.into_iter().enumerate() {
            let mut v = Buf::<16>::new();
            let mut grey = false;
            match c {
                Cell::F(f) => {
                    f.text(&ch, app.set.home_ch, false, &mut v);
                    if let ChField::It(it) = f {
                        grey = app.greyed(it).is_some();
                    }
                }
                Cell::Number => {
                    let _ = uwrite!(v, "{}", Zero(ch.num as u32 + 1, 3));
                }
            }
            let (x, y) = (GX + (i as i32 % 3) * 77, GY + (i as i32 / 3) * 34);
            app.grid_cell(lcd, i, x, y, 72, 29, c.label().as_bytes(), v.as_bytes(), cell == GRID0 + i as u8, grey);
        }
        // The note: why the focused cell is greyed (EDIT-7), or what the focused field does. One line of 226 px:
        // 32 characters.
        let mut note = Buf::<48>::new();
        let mut col = SUB;
        match (cell, self.at()) {
            (ZONE, _) => note.set(b"Knob: move it to another zone"),
            (NAME, _) => note.set(b"OK: type the name, 16 at most"),
            (FREQ, _) => note.set("Knob: step \u{b7} OK: type it".as_bytes()),
            (_, Some(Cell::F(ChField::It(it)))) if app.greyed(it).is_some() => {
                note.set(it.label().as_bytes());
                note.push(b": ");
                note.push(app.greyed(it).unwrap_or("").as_bytes());
                col = AMBER;
            }
            (_, Some(Cell::Number)) => note.set(b"Knob: to the next free number"),
            (_, Some(Cell::F(ChField::Home))) => note.set(b"Hold 0 on home tunes it"),
            _ => {}
        }
        if app.dirty(R_HELP, Mix::new().h(&note).h(&col).get()) {
            w::field(lcd, 7, 226, 242, 259, 255, note.as_bytes(), &w::st(&UI_12, col, BG, Align::Left));
        }
        other_slots(app, lcd, cx.live);
    }

    fn hints(self, app: &App) -> Hints {
        if app.undo.is_some() {
            Hints::new(&[("Knob", "Change"), ("*", "Undo"), ("BACK", "Done")])
        } else if matches!(self.cell, NAME | FREQ) {
            Hints::new(&[(ARROWS_4, "Pick"), ("OK", "Type"), ("BACK", "Done")])
        } else {
            Hints::new(&[(ARROWS_4, "Pick"), ("Knob", "Change"), ("BACK", "Done")])
        }
    }
}

impl ChEdit {
    /// The focused cell: zone, name and frequency above the grid's.
    fn at(self) -> Option<Cell> {
        match self.cell {
            ZONE => Some(Cell::F(ChField::Zone)),
            NAME => Some(Cell::F(ChField::Name)),
            FREQ => Some(Cell::F(ChField::Rx)),
            c => CELLS.get((c - GRID0) as usize).copied(),
        }
    }
}

/// Home on or off for the active slot's channel: one home channel per radio (CHAN-17, CHAN-18).
fn set_home(app: &mut App, on: bool) {
    let Some(ch) = app.slots[app.active as usize].ch else { return };
    let is = app.set.home_ch == Some(ch.num);
    if on != is {
        app.keep_undo();
        app.set.home_ch = if on { Some(ch.num) } else { None };
    }
}
