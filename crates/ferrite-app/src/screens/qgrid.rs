//! The APRS slot's quick page (MENU-34): hold ABC on the APRS slot opens its most-used APRS settings as a grid,
//! the way hold ABC on any other slot opens the slot's settings (EDIT-1). Each cell is a menu setting by its path, so the
//! value, the steps, the greying and the reset are the menu's own ([`groups::wired`], [`groups::turn_path`],
//! [`groups::reset`]); the help box names the path, so the menu number is learned on the way.

use super::groups;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// Which feature's page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Page {
    /// The APRS slot (whatever holds it).
    Aprs,
}

/// One cell: its short label and the setting's menu path (MENU 0 ...).
type Cell = (&'static str, &'static [u8]);

/// The APRS slot's page, whatever holds C (APRS mode, KISS or beacons alone): turning APRS on, the beacon, and
/// what it sends.
const APRS: [Cell; 9] = [
    ("APRS mode", &[5, 3]),
    ("Beacon", &[5, 6, 1]),
    ("Power", &[5, 6, 3]),
    ("Frequency", &[5, 4]),
    ("Path", &[5, 8, 1]),
    ("Digipeat", &[5, 7, 1]),
    ("Bcn text", &[5, 5, 3]), // the beacon comment (APRS-50); "Beacon text" is two characters too wide
    ("Text rate", &[5, 6, 5]), // APRS-50.1: how often the beacon text goes (0 5 6 5)
    ("Symbol", &[5, 5, 2]),
];

impl Page {
    pub(crate) fn cells(self) -> &'static [Cell] {
        match self {
            Page::Aprs => &APRS,
        }
    }

    /// The page's name: its heading and the status bar's.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Page::Aprs => "APRS",
        }
    }

    /// The page as a screen, its first cell focused.
    pub fn screen(self) -> Screen {
        Screen::QGrid(QGrid { page: self, cell: 0 })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct QGrid {
    pub page: Page,
    /// The focused cell, row by row.
    pub cell: u8,
}

impl QGrid {
    fn at(self) -> Cell {
        let c = self.page.cells();
        c[(self.cell as usize).min(c.len() - 1)]
    }

    /// The focused cell's help: its path ("MENU 0 5 6 1") and its menu help line, or why it is greyed.
    pub(crate) fn help(self, app: &App) -> (Buf<20>, groups::Help) {
        let (_, p) = self.at();
        let mut path = Buf::<20>::new();
        path.push(b"MENU 0");
        for x in p {
            let _ = uwrite!(path, " {}", x);
        }
        let h = match groups::greyed(app, p) {
            Some(why) => groups::Help::Why(why),
            None => groups::Help::Note(groups::entry_of(p).and_then(|e| e.help())),
        };
        (path, h)
    }

    /// Cell `i`'s value as shown, and whether it is greyed (the UI readback reads it too).
    pub(crate) fn value(app: &App, dev: &ferrite_core::msg::Device, p: &[u8]) -> (Buf<24>, bool, bool) {
        let mut v = Buf::<24>::new();
        let dot = match groups::wired(app, dev, p, &mut v) {
            Some(d) => d,
            None => {
                v.set(groups::entry_of(p).map_or("", |e| e.value()).as_bytes());
                false
            }
        };
        (v, dot, groups::greyed(app, p).is_some())
    }
}

/// The room a value has in its cell (6 px in, 4 clear of the border).
const VALUE_W: i32 = 62;

/// A value as its cell shows it: whole when it fits, else cut at the last word break that fits ("WIDE1-1,WIDE2-1"
/// shows "WIDE1-1", "Every 10 min" "Every 10"); true when cut, and the help box then shows it whole (MENU-34).
pub(crate) fn short(v: &[u8]) -> (&[u8], bool) {
    let f = w::fit(v, &UI_12, VALUE_W);
    if f.len() == v.len() {
        return (v, false);
    }
    let cut = if v[f.len()] == b' ' { f.len() } else { f.iter().rposition(|&c| c == b' ' || c == b',').filter(|&i| i > 0).unwrap_or(f.len()) };
    (f[..cut].trim_ascii_end().strip_suffix(b",").unwrap_or(f[..cut].trim_ascii_end()), true)
}

impl View for QGrid {
    /// KEY-19: ◀▶ move across the grid; the knob changes the cell.
    fn own_arrows(self) -> bool {
        true
    }

    fn name(self) -> &'static str {
        "Quick page"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(self.page.name().as_bytes());
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS) // MENU-28
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        use Press::*;
        let n = self.page.cells().len() as u8;
        let c = self.cell.min(n - 1);
        let mv = |c: u8| Screen::QGrid(QGrid { cell: c, ..self });
        let (label, p) = self.at();
        match (e.key, e.press) {
            (Key::Up, Short | Repeat) if c >= 3 => app.screen = mv(c - 3),
            (Key::Down, Short | Repeat) if c + 3 < n => app.screen = mv(c + 3),
            (Key::Left, Short | Repeat) if !c.is_multiple_of(3) => app.screen = mv(c - 1),
            (Key::Right, Short | Repeat) if c % 3 < 2 && c + 1 < n => app.screen = mv(c + 1),
            (Key::Digit(d), Short) if d >= 1 && d <= n => app.screen = mv(d - 1), // MENU-5: a digit picks
            (Key::Menu, Short) => groups::open_path(app, p, label, true), // MENU-17: its editor, or a toggle
            (Key::Hash, Short) => groups::reset(app, p),                  // MENU-18
            (Key::Star, Short) => app.do_undo(),                          // MENU-22
            (Key::Exit | Key::Slot, Short) => app.back(),                 // as EDIT-6
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        let (label, p) = self.at();
        groups::turn_path(app, p, label, d);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let page = self.page;
        app.page(lcd, page);
        if app.dirty(R_TOP, Mix::new().h(&(page as u8)).get()) {
            w::field(lcd, 8, 224, 22, 50, 42, page.name().as_bytes(), const { &w::st(&UI_15B, TEXT, BG, Align::Left) });
            w::field(lcd, 120, 112, 26, 46, 42, b"Quick settings", const { &w::st(&UI_12, SUB, BG, Align::Right) });
        }
        for (i, (label, p)) in page.cells().iter().enumerate() {
            let (v, _, grey) = QGrid::value(app, cx.dev, p);
            let (x, y) = (7 + (i as i32 % 3) * 77, 56 + (i as i32 / 3) * 46);
            app.grid_cell(lcd, i, x, y, 72, 40, label.as_bytes(), short(v.as_bytes()).0, i as u8 == self.cell, grey);
        }
        // The help box: why the focused cell is greyed, its whole value when the cell had to cut it, or its menu
        // help line; with its path.
        let (path, help) = self.help(app);
        let (v, _, _) = QGrid::value(app, cx.dev, self.at().1);
        match help {
            groups::Help::Note(_) if short(v.as_bytes()).1 => app.help_box(lcd, 202, 44, v.as_bytes(), path.as_bytes()),
            _ => app.help_note(lcd, 202, 44, help, path.as_bytes()),
        }
    }

    fn hints(self, app: &App) -> Hints {
        if app.set_undo.is_some() || app.undo.is_some() {
            Hints::new(&[("Knob", "Change"), ("*", "Undo"), ("BACK", "Done")]) // MENU-22
        } else {
            Hints::new(&[(ARROWS_4, "Pick"), ("Knob", "Change"), ("BACK", "Done")])
        }
    }
}
