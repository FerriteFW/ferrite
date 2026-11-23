//! The scan lists (manual chapter 14): range presets (MENU 0 1 4, up to 16; C12), one preset's editor (A11:
//! name, start, end, step, mode, bandwidth, skips; SCAN-4, SCAN-10) and this scan's nuisance skips (MENU 0 1
//! 8; SCAN-11; C13), which live in the Radio task's RAM for this scan only.

use super::lists::*;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::data::{Op, RangePreset};
use crate::keys::{KeyEvent, Press};
use crate::slot::{band_of, MODES, STEPS};
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::keys::Key;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Page {
    /// The presets (deleting one asks with the shared confirm).
    Presets,
    /// One preset's editor, its row.
    Edit(u8),
    Skips,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScanLists {
    pub page: Page,
    pub cur: Cursor,
}

impl ScanLists {
    pub fn new(page: Page) -> Self {
        ScanLists { page, cur: Cursor::default() }
    }
}

const ALL: fn(u16) -> bool = |_| true;
pub const PRESETS_MAX: u8 = 16;

/// The editor's rows and their help.
const ROWS: [(&str, crate::text::Txt); 7] = [
    ("Name", txt!("Up to 12 characters")),
    ("Start", txt!("Knob 100 kHz, left and right 1 MHz")),
    ("End", txt!("Knob 100 kHz, left and right 1 MHz")),
    ("Step", txt!("The scan's step through the range")),
    ("Mode", txt!("FM wide, NFM narrow or AM (VFO-20)")),
    ("Resume", txt!("Global follows MENU 0 1 2; or this range's own")),
    ("Skips", txt!("Never stopped on (up to 48); * on a hit adds")),
];

fn range_text(p: &RangePreset, b: &mut Buf<24>) {
    let mut s = Buf::<16>::new();
    let mut e = Buf::<16>::new();
    mhz_text(p.start, &mut s);
    mhz_text(p.end, &mut e);
    b.clear();
    let _ = uwrite!(b, "{}-{}", Txt(trim_mhz(s.as_bytes())), Txt(trim_mhz(e.as_bytes())));
}

/// "144.000" as "144", "162.025" as is.
fn trim_mhz(s: &[u8]) -> &[u8] {
    match s.iter().position(|&c| c == b'.') {
        Some(p) if s[p + 1..].iter().all(|&c| c == b'0') => &s[..p],
        _ => s,
    }
}

/// A range crossing 64 MHz needs the relay switched mid-scan: SCAN-14 refuses it when the scan starts.
fn crosses_relay(p: &RangePreset) -> bool {
    p.start < mhz(64) && p.end > mhz(64)
}

impl View for ScanLists {
    /// KEY-19: ◀▶ step a range edge by 1 MHz; otherwise they are the knob.
    fn own_arrows(self) -> bool {
        true
    }

    fn name(self) -> &'static str {
        match self.page {
            Page::Presets => "Range presets",
            Page::Edit(_) => "Range preset",
            Page::Skips => "Skip list",
        }
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(match self.page {
            Page::Presets => b"Ranges".as_slice(),
            Page::Edit(_) => b"Range",
            Page::Skips => b"Skips",
        });
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let d = store.data();
        let mut s = self;
        let n = match self.page {
            Page::Skips => d.skips(),
            _ => d.presets() as u16,
        };
        match (e.key, e.press, self.page) {
            (Key::Up | Key::Down, Short | Repeat, Page::Edit(r)) => {
                s.page = Page::Edit((r as i32 + if e.key == Key::Up { -1 } else { 1 }).rem_euclid(ROWS.len() as i32) as u8);
            }
            (Key::Up | Key::Down, Short | Repeat, _) => s.cur = self.cur.step(if e.key == Key::Up { -1 } else { 1 }, n, ROWS2, &ALL),
            (Key::Left | Key::Right, Short | Repeat, page) => {
                let big = matches!(page, Page::Edit(1 | 2));
                return self.knob(app, if e.key == Key::Right { 1 } else { -1 } * if big { 10 } else { 1 }, store);
            }
            (Key::Menu, Short, Page::Presets) if n > 0 => s.page = Page::Edit(0),
            // SCAN-27 (ferrite#83): SIDE2 scans the selected preset on the active slot
            (Key::Side(2), Short, Page::Presets) if n > 0 => return super::scan::preset(app, self.cur.sel as u8, store),
            (Key::Hash, Short, Page::Presets) => {
                if d.presets() < PRESETS_MAX && d.request(Op::NewPreset) {
                    let n = d.presets() as u16;
                    s.cur = Cursor { sel: n - 1, top: self.cur.top }.shown(n, ROWS2, &ALL);
                    s.page = Page::Edit(0);
                } else {
                    app.show_toast(b"16 presets", b"delete one first", false);
                }
            }
            // MENU-23: the shared confirm asks.
            (Key::Star, Short, Page::Presets) if n > 0 => return super::confirm::ask(app, super::confirm::Ask::DeletePreset(self.cur.sel as u8)),
            (Key::Menu, Short, Page::Edit(0)) => return super::text::open(app, super::text::TextFor::Preset(self.cur.sel as u8), store),
            // MENU-31: OK on an edge types it in the frequency entry box; the knob and ◀▶ still step it.
            (Key::Menu, Short, Page::Edit(r @ (1 | 2))) => {
                let i = self.cur.sel as u8;
                let fs = if r == 1 { super::entry::FreqSet::RangeStart(i) } else { super::entry::FreqSet::RangeEnd(i) };
                return super::entry::open_setting(app, fs);
            }
            // C13: OK lets one back in; # clears them all, asked first as the lists beside it ask (SCAN-17.1).
            (Key::Menu, Short, Page::Skips) if n > 0 => {
                d.request(Op::Unskip(self.cur.sel));
                s.cur = Cursor { sel: self.cur.sel.min(n.saturating_sub(2)), ..self.cur }.shown(n - 1, ROWS2, &ALL);
            }
            (Key::Hash, Short, Page::Skips) if n > 0 => return super::confirm::ask(app, super::confirm::Ask::ClearSkips),
            (Key::Exit, Short, Page::Edit(_)) => s.page = Page::Presets,
            (Key::Exit, Short, _) => return app.back(),
            _ => return,
        }
        app.screen = Screen::ScanLists(s);
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        let data = store.data();
        match self.page {
            Page::Edit(r) => {
                let i = self.cur.sel as u8;
                let Some(mut p) = data.preset(i) else { return };
                let khz100: i32 = 10_000; // 100 kHz in 10 Hz units
                match r {
                    1 => p.start = (p.start as i32 + d * khz100).clamp(mhz(18) as i32, p.end as i32 - khz100) as Freq10,
                    2 => p.end = (p.end as i32 + d * khz100).clamp(p.start as i32 + khz100, mhz(1300) as i32) as Freq10,
                    3 => p.step = (p.step as i32 + d).rem_euclid(STEPS.len() as i32) as u8,
                    4 => {
                        p.mode = (p.mode as i32 + d).rem_euclid(3) as u8; // FM, NFM, AM (CHAN-23)
                        p.narrow = p.mode == 1; // VFO-20
                    }
                    5 => p.resume = (p.resume as i32 + d).rem_euclid(13) as u8, // SCAN-24
                    _ => return,
                }
                // SCAN-14: a span across the band relay's edges (18 and 64 MHz) can't be scanned; refused here, when
                // the edge is set, not at scan time (decided Oct 7, 2026). The editor stays open.
                if let Some(&e) = [mhz(18), mhz(64)].iter().find(|&&e| p.start < e && p.end > e) {
                    let head: &[u8] = if e == mhz(18) { b"Can't span 18 MHz" } else { b"Can't span 64 MHz" };
                    return app.show_toast(head, b"one side only", false);
                }
                data.request(Op::Preset(i, p));
            }
            Page::Presets => {
                let cur = self.cur.step(d, data.presets() as u16, ROWS2, &ALL);
                app.screen = Screen::ScanLists(ScanLists { cur, ..self });
            }
            Page::Skips => {
                let cur = self.cur.step(d, data.skips(), ROWS2, &ALL);
                app.screen = Screen::ScanLists(ScanLists { cur, ..self });
            }
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let store = cx.store;
        let data = store.data();
        app.page(lcd, (matches!(self.page, Page::Edit(_)), self.page == Page::Skips));
        if !app.gate((self, data.version())) {
            return;
        }
        match self.page {
            Page::Presets => {
                let n = data.presets() as u16;
                let mut l = Buf::<24>::new();
                let _ = uwrite!(l, "{} of {} presets", n, PRESETS_MAX);
                app.list_head(lcd, l.as_bytes(), b"MENU 0 1 4");
                let rows = self.cur.rows(n, ROWS2, &ALL, |row, i| {
                    if let Some(p) = data.preset(i as u8) {
                        let (mut r, mut sub) = (Buf::<24>::new(), Buf::<40>::new());
                        range_text(&p, &mut r);
                        r.push(b" MHz");
                        let _ = uwrite!(sub, "{} \u{b7} {}{}", STEPS[p.step as usize % STEPS.len()].1, MODES[p.mode as usize % MODES.len()], if p.narrow { "" } else { " wide" });
                        let mut sk = Buf::<12>::new();
                        if p.skips > 0 {
                            let _ = uwrite!(sk, "{} skips", p.skips);
                        }
                        let r2 = Row2 { title: p.name.as_bytes(), right: r.as_bytes(), sub: sub.as_bytes(), sub_right: sk.as_bytes(), selected: i == self.cur.sel, ..Row2::default() };
                        app.row2(lcd, row, &r2);
                    }
                });
                app.rows_end(lcd, rows, ROWS2, ROW2_H);
                app.list_cue(lcd, self.cur, n, ROWS2, ROW2_H, rows, &ALL);
                if n == 0 {
                    app.empty(lcd, b"No range presets", b"# makes one");
                }
            }
            Page::Edit(row) => {
                let Some(p) = data.preset(self.cur.sel as u8) else {
                    return app.empty(lcd, b"No such preset", b"MENU 0 1 4 lists them");
                };
                let mut r = Buf::<24>::new();
                range_text(&p, &mut r);
                r.push(b" MHz");
                app.list_head(lcd, p.name.as_bytes(), r.as_bytes());
                for (i, (label, _)) in ROWS.iter().enumerate() {
                    let mut v = Buf::<24>::new();
                    let mut f = Buf::<16>::new();
                    match i {
                        0 => v.set(p.name.as_bytes()),
                        1 | 2 => {
                            mhz_text(if i == 1 { p.start } else { p.end }, &mut f);
                            let _ = uwrite!(v, "{} MHz", Txt(f.as_bytes()));
                        }
                        3 => v.set(STEPS[p.step as usize % STEPS.len()].1.as_bytes()),
                        4 => v.set(MODES[p.mode as usize % MODES.len()].as_bytes()),
                        5 => {
                            let _ = match p.resume {
                                0 => uwrite!(v, "Global"),
                                11 => uwrite!(v, "Carrier drop"),
                                12 => uwrite!(v, "Stop on hit"),
                                s => uwrite!(v, "After {} s", s),
                            };
                        }
                        _ => {
                            let _ = uwrite!(v, "{} of 48", p.skips);
                        }
                    }
                    app.row1(lcd, i, label.as_bytes(), v.as_bytes(), false, i as u8 == row, i == 6);
                }
                // The help, or why this range won't scan (SCAN-14).
                let text = if crosses_relay(&p) {
                    txt!("Crosses 64 MHz: a scan of it is refused (SCAN-14)")
                } else if band_of(p.start) != band_of(p.end.saturating_sub(1)) {
                    txt!("Spans more than one band")
                } else {
                    ROWS[row as usize].1
                };
                app.help_txt(lcd, 300 - 38, 34, text, b"MENU 0 1 4");
            }
            Page::Skips => {
                let n = data.skips();
                let mut l = Buf::<24>::new();
                let _ = uwrite!(l, "{} skipped", n);
                app.list_head(lcd, l.as_bytes(), b"this scan only");
                let rows = self.cur.rows(n, ROWS2, &ALL, |row, i| {
                    if let Some(f) = data.skip(i) {
                        let mut t = Buf::<16>::new();
                        mhz_text(f, &mut t);
                        // the channel on it, if one is stored
                        let ch = (0..store.count()).filter_map(|k| store.by_number(k)).find(|c| c.rx == f);
                        let mut sub = Buf::<40>::new();
                        match ch {
                            Some(c) => {
                                let mut l = Buf::<32>::new();
                                crate::slot::chan_label(c.num, &mut l);
                                let _ = uwrite!(sub, "{} \u{b7} ", Txt(l.as_bytes()));
                                sub.push(c.name());
                            }
                            None => sub.set(crate::slot::band_name(f).as_bytes()),
                        }
                        let r2 = Row2 { title: t.as_bytes(), right: b"MHz", sub: sub.as_bytes(), selected: i == self.cur.sel, ..Row2::default() };
                        app.row2(lcd, row, &r2);
                    }
                });
                app.rows_end(lcd, rows, ROWS2, ROW2_H);
                app.list_cue(lcd, self.cur, n, ROWS2, ROW2_H, rows, &ALL);
                if n == 0 {
                    app.empty(lcd, b"Nothing skipped", b"* on a hit skips it this scan");
                }
            }
        }
    }

    fn hints(self, _: &App) -> Hints {
        match self.page {
            Page::Presets => Hints::new(&[("OK", "Edit"), ("SIDE2", "Scan"), ("#", "New")]),
            Page::Edit(1 | 2) => Hints::new(&[("OK", "Type"), ("Knob", "Change"), ("BACK", "Done")]), // MENU-31
            Page::Edit(_) => Hints::new(&[(ARROWS_UD, "Move"), ("Knob", "Change"), ("BACK", "Done")]),
            Page::Skips => Hints::new(&[("OK", "Unskip"), ("#", "Clear all"), ("BACK", "Back")]),
        }
    }
}
