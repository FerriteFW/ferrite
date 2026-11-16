//! The SSTV screen (chapter 27a): the picture scaled to 240 wide at the top (240 x 192 for 256 lines), drawn from
//! the SPI flash a few dozen rows a pass, and as a received picture's lines come in; what it is below. ◀▶ step
//! through the pictures, OK opens Send (the knob picks the mode, OK sends), # starts a mode by hand.

use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::sstv::{headers, mode_secs, views, SEND_MODES, FORCE_MODES, VIEW_CARD};
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_core::sstv::{self, Header, HEADER_LEN, LINES, ROW_LEN, SLOTS, TEST_CARD, WIDTH};
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::{Buf, Bytes};
use ferrite_ui::widgets as w;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Sstv {
    /// The Send page: the mode picked (an index into [`SEND_MODES`]).
    pub send: Option<u8>,
}

/// The picture area: its top and its height (256 lines at three quarters).
const TOP: i32 = 22;
const H: i32 = 192;
/// A picture coming in: the source rows drawn again each time more arrive (more than come between two passes).
const RECENT: i32 = 16;

pub fn open(app: &mut App) -> Screen {
    app.sstv_enter();
    Screen::Sstv(Sstv { send: None })
}

/// The header of slot `s` (None: empty, or the test card).
fn header(st: &impl ChannelStore, s: u8) -> Option<Header> {
    if s >= SLOTS {
        return None;
    }
    let mut b = [0xFFu8; HEADER_LEN];
    st.data().data_read(sstv::slot_addr(s), &mut b).then(|| Header::decode(&b)).flatten()
}

impl Sstv {
    fn step_view(app: &mut App, st: &impl ChannelStore, d: i32) {
        let hs = headers(st);
        let mut v = [0u8; SLOTS as usize + 1];
        let n = views(&hs, &mut v);
        let at = v[..n].iter().position(|&x| x == app.sstv.view).unwrap_or(0) as i32;
        app.sstv.view = v[(at + d).rem_euclid(n as i32) as usize];
    }

    /// The picture's lines that can be drawn now: a received one's so far (the last a row behind, for the store
    /// to write it), a stored one's, the test card's 256.
    fn avail(app: &App, st: &impl ChannelStore) -> (u16, u16) {
        let v = app.sstv.view;
        if v == VIEW_CARD {
            return (LINES as u16, LINES as u16);
        }
        if let Some(rx) = app.sstv.rx.filter(|r| r.slot == v) {
            let total = ferrite_core::usstv::Mode::by_vis(rx.vis).map_or(LINES as u16, |m| m.lines);
            return (rx.lines.saturating_sub(1), total);
        }
        match header(st, v) {
            Some(h) => {
                let l = h.lines.unwrap_or(0);
                (l, l)
            }
            None => (0, 0),
        }
    }

    /// Rows `from..to` of the picture area: source row `d * 4 / 3`, source column `x * 4 / 3`.
    fn draw_rows(app: &App, lcd: &mut impl Panel, st: &impl ChannelStore, from: i32, to: i32) {
        let v = app.sstv.view;
        let call = app.set.callsign.as_bytes();
        let mut src = [0u8; ROW_LEN];
        let mut px = [0u16; 240];
        // only the rows of the band being drawn (`banded`: each band calls this again)
        let (r0, r1) = lcd.rows();
        for d in from.max(r0 - TOP)..to.min(r1 - TOP + 1) {
            let sy = (d * 4 / 3) as u16;
            if v == VIEW_CARD {
                for (x, p) in px.iter_mut().enumerate() {
                    let (r, g, b) = sstv::test_card(x * 4 / 3, sy as usize, call);
                    *p = ferrite_core::usstv::color::rgb565(r, g, b);
                }
            } else {
                if !st.data().data_read(sstv::row_addr(v, sy), &mut src) {
                    return;
                }
                for (x, p) in px.iter_mut().enumerate() {
                    let sx = (x * 4 / 3).min(WIDTH - 1);
                    *p = u16::from_le_bytes([src[2 * sx], src[2 * sx + 1]]);
                }
            }
            lcd.window(0, TOP + d, 239, TOP + d);
            lcd.push_buf(&px);
        }
    }

    fn info(app: &App, st: &impl ChannelStore, send: Option<u8>, l1: &mut Buf<40>, l2: &mut Buf<40>, l3: &mut Buf<40>) {
        let v = app.sstv.view;
        if let Some(t) = app.sstv.tx {
            let _ = ufmt::uwrite!(l1, "Sending {}", sstv::mode_name(t.vis));
            let frames = ferrite_core::usstv::Mode::by_vis(t.vis).map_or(1, |m| m.frames);
            let left = mode_secs(t.vis).saturating_sub(app.now.wrapping_sub(t.at_ms) / 1000);
            let _ = ufmt::uwrite!(l2, "Line {} of {}, {}:{}{} left", app.sstv.frame + 1, frames, left / 60, if left % 60 < 10 { "0" } else { "" }, left % 60);
            l3.set(b"BACK stops");
            return;
        }
        if app.sstv.send.is_some() {
            l1.set(b"Keying...");
            return;
        }
        if let Some(i) = send {
            let vis = SEND_MODES[i as usize % SEND_MODES.len()];
            let _ = ufmt::uwrite!(l1, "Send in {}", sstv::mode_name(vis));
            let s = mode_secs(vis);
            let f = w::freq3(app.tuned_freq(app.active));
            let _ = ufmt::uwrite!(l2, "{}:{}{} on {}, {}", s / 60, if s % 60 < 10 { "0" } else { "" }, s % 60, ["A", "B", "C"][app.active as usize % 3], Bytes(f.as_bytes()));
            l3.set(b"OK sends, BACK goes back");
            return;
        }
        if let Some(rx) = app.sstv.rx.filter(|r| r.slot == v) {
            let _ = ufmt::uwrite!(l1, "{}, receiving", sstv::mode_name(rx.vis));
            let total = ferrite_core::usstv::Mode::by_vis(rx.vis).map_or(LINES as u16, |m| m.lines);
            let _ = ufmt::uwrite!(l2, "Line {} of {}", rx.lines, total);
        } else if v == VIEW_CARD {
            l1.set(b"Test card");
            l2.set(b"Drawn with your callsign");
        } else {
            match header(st, v) {
                Some(h) if h.kind == sstv::KIND_SEND => {
                    let _ = ufmt::uwrite!(l1, "To send {}", v + 1);
                    let n = h.name.iter().position(|&c| c == 0).unwrap_or(16);
                    l2.push(&h.name[..n]);
                }
                Some(h) => {
                    l1.push(sstv::mode_name(h.vis).as_bytes());
                    l1.push(b", received");
                    if h.freq != 0 {
                        l2.push(w::freq3(h.freq).as_bytes());
                    }
                    if !h.complete {
                        l2.push(if l2.is_empty() { b"incomplete" } else { b", incomplete" });
                    }
                }
                None if v < sstv::FIRST_RX => {
                    let _ = ufmt::uwrite!(l1, "To send {}: empty", v + 1);
                    l2.set(b"Console writes pictures here");
                }
                None => l1.set(b"Nothing received yet"),
            }
        }
        match app.sstv.forced {
            0 => {
                let f = w::freq3(app.tuned_freq(app.active));
                let _ = ufmt::uwrite!(l3, "Listening on {}, {}", ["A", "B", "C"][app.active as usize % 3], Bytes(f.as_bytes()));
            }
            k => {
                let _ = ufmt::uwrite!(l3, "Waiting for a {} sync", sstv::mode_name(FORCE_MODES[k as usize - 1]));
            }
        }
    }
}

impl View for Sstv {
    fn name(self) -> &'static str {
        "SSTV"
    }

    fn key(self, app: &mut App, e: KeyEvent, st: &impl ChannelStore) {
        use Press::*;
        // SSTV-5.4: BACK stops a picture going out
        if app.sstv.tx.is_some() || app.sstv.send.is_some() {
            if (e.key, e.press) == (Key::Exit, Short) {
                app.sstv.stop = true;
            }
            return;
        }
        if let Some(i) = self.send {
            match (e.key, e.press) {
                (Key::Left | Key::Up, Short | Repeat) => return self.knob(app, -1, st),
                (Key::Right | Key::Down, Short | Repeat) => return self.knob(app, 1, st),
                (Key::Menu, Short) => {
                    app.sstv_send(SEND_MODES[i as usize % SEND_MODES.len()], if app.sstv.view == VIEW_CARD { TEST_CARD } else { app.sstv.view });
                    app.screen = Screen::Sstv(Sstv { send: None });
                }
                (Key::Exit, Short) => app.screen = Screen::Sstv(Sstv { send: None }),
                _ => {}
            }
            return;
        }
        match (e.key, e.press) {
            (Key::Left, Short | Repeat) => Self::step_view(app, st, -1),
            (Key::Right, Short | Repeat) => Self::step_view(app, st, 1),
            (Key::Menu, Short) => {
                let v = app.sstv.view;
                if v != VIEW_CARD && header(st, v).is_none_or(|h| h.lines.is_none()) {
                    return app.show_toast(b"Nothing to send", b"", false);
                }
                app.screen = Screen::Sstv(Sstv { send: Some(0) });
            }
            (Key::Hash, Short) => app.sstv_force_next(),
            (Key::Exit, Short) => {
                app.sstv_close();
                app.back();
            }
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, st: &impl ChannelStore) {
        match self.send {
            Some(i) => app.screen = Screen::Sstv(Sstv { send: Some((i as i32 + d).rem_euclid(SEND_MODES.len() as i32) as u8) }),
            None if app.sstv.tx.is_none() => Self::step_view(app, st, d.signum()),
            None => {}
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let st = cx.store;
        // the picture: all of it when the view or its picture changed (about 30 ms), else a received picture's
        // newest rows as they come
        let key = Mix::new().h(&app.sstv.view).h(&app.sstv.ver).get();
        let (avail, total) = Self::avail(app, st);
        let height = (total as i32 * 3 / 4).min(H);
        let target = (avail as i32 * 3 / 4).min(height);
        if app.dirty(R_FRAME, key) {
            ferrite_ui::canvas::banded(lcd, 0, TOP, 240, target, BG, |b| Self::draw_rows(app, b, st, 0, target));
            // what a shorter picture (or none) leaves, cleared
            if height < H || target < height {
                let top = if total == 0 { 0 } else { target };
                ferrite_ui::canvas::banded(lcd, 0, TOP + top, 240, H - top, BG, |lcd| {
                    if total == 0 {
                        w::field(lcd, 8, 224, TOP + 80, TOP + 104, TOP + 97, b"No picture", const { &w::st(&UI_12, DIM, BG, Align::Center) });
                    }
                });
            }
            app.dirty(R_HEAD, Mix::new().h(&key).h(&avail).get());
        } else if app.dirty(R_HEAD, Mix::new().h(&key).h(&avail).get()) {
            let from = ((avail as i32 - RECENT).max(0) * 3 / 4).min(target);
            ferrite_ui::canvas::banded(lcd, 0, TOP + from, 240, target - from, BG, |b| Self::draw_rows(app, b, st, from, target));
        }
        let (mut l1, mut l2, mut l3) = (Buf::<40>::new(), Buf::<40>::new(), Buf::<40>::new());
        Self::info(app, st, self.send, &mut l1, &mut l2, &mut l3);
        if app.dirty(R_INFO, Mix::new().h(&l1).h(&self.send.is_some()).get()) {
            ferrite_ui::canvas::banded(lcd, 0, 218, 240, 24, BG, |lcd| {
                w::field(lcd, 8, 224, 220, 240, 235, l1.as_bytes(), const { &w::st(&UI_12B, TEXT, BG, Align::Left) });
            });
        }
        if app.dirty(R_SUB, Mix::new().h(&l2).get()) {
            ferrite_ui::canvas::banded(lcd, 0, 242, 240, 22, BG, |lcd| {
                w::field(lcd, 8, 224, 244, 262, 257, l2.as_bytes(), const { &w::st(&UI_12, SUB, BG, Align::Left) });
            });
        }
        if app.dirty(R_NOTE, Mix::new().h(&l3).get()) {
            ferrite_ui::canvas::banded(lcd, 0, 266, 240, 22, BG, |lcd| {
                w::field(lcd, 8, 224, 268, 286, 281, l3.as_bytes(), const { &w::st(&UI_12, DIM, BG, Align::Left) });
            });
        }
    }

    fn hints(self, app: &App) -> Hints {
        if app.sstv.tx.is_some() || app.sstv.send.is_some() {
            Hints::new(&[("BACK", "Stop")])
        } else if self.send.is_some() {
            Hints::new(&[("Knob", "Mode"), ("OK", "Send"), ("BACK", "Back")])
        } else {
            Hints::new(&[(ARROWS_LR, "Picture"), ("OK", "Send"), ("#", "Mode")])
        }
    }
}
