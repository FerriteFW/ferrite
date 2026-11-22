//! Satellite mode (MENU 0 3 6; manual chapter 28a, SAT-1 to SAT-20; mock-ups
//! https://claude.ai/artifact/ScfAxNJLH8UHkU5tHd18zh): the pass list (B22), the pass page before the rise and
//! overhead, keyed up (B23), and the pointing page (B24). While it is open (or under the screen up) the
//! satellite mode runs on the Radio task ([`super::activity::Mode::Sat`]): both chips, the downlink retuned each
//! second ([`App::sat_retune`]). The passes come from the source the platform gives ([`App::sat_attach`]).
//!
//! Keys. List: ▲▼ or the knob a pass, OK the pass page, # the alarm picker (SAT-12), ◀▶ one satellite or all.
//! Pass page: ◀▶ (and the knob) trim the downlink 100 Hz (SAT-8), 1-5 the transponder, * full or half duplex
//! (SAT-11), # the alarm picker, 0 the pointing page (0 again: the plot, SAT-17), PTT the uplink, BACK the list.

use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::sat::{LEADS, SAT_SLOT};
use crate::{App, Screen};
use ferrite_core::freq::Freq10;
use ferrite_core::keys::Key;
use ferrite_core::msg::{LiveAll, ModeLive};
use ferrite_core::sat::data::State;
use ferrite_core::sat::fmath::{rad, sin_cos};
use ferrite_core::sat::look::{self, Look};
use ferrite_core::sat::pass::Pass;
use ferrite_ui::assets::*;
use ferrite_ui::font4::Font4;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

const LIST: u8 = 0;
const PASS: u8 = 1;
const POINT: u8 = 2;
/// `page`'s bit while keyed (the keyed layout, the red bar for the status bar).
const TX: u8 = 0x80;
/// No satellite filter.
const ALL: u8 = 0xFF;

/// The screen's state (12 bytes): the page, the satellite filter, the alarm picker (0 closed, else 1 + the lead
/// highlighted), the options page (SAT-22: 0 closed, else 1 + the row selected), and the pass selected or open, by
/// identity (satellite and rise), so the selection stays on it while prediction adds rows (SAT-6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SatScr {
    pub page: u8,
    pub filter: u8,
    pub picker: u8,
    pub opt: u8,
    pub sat: u8,
    pub aos: u32,
}

impl SatScr {
    pub const LIST: SatScr = SatScr { page: LIST, filter: ALL, picker: 0, opt: 0, sat: 0, aos: 0 };

    /// The pass page on a pass.
    pub fn pass_of(p: &Pass) -> SatScr {
        SatScr { page: PASS, sat: p.sat, aos: p.aos, ..Self::LIST }
    }

    fn page(self) -> u8 {
        self.page & !TX
    }
}

/// MENU 0 3 6 (SAT-1): the pass list; refused while transmitting or repeating, as HF mode.
pub fn open(app: &mut App) -> Screen {
    if app.radio.keyed.is_some() || app.radio.mode == super::activity::Mode::Xb {
        app.show_toast(b"Satellites", if app.radio.keyed.is_some() { b"not keyed" } else { b"stop repeater first" }, false);
        return app.screen;
    }
    Screen::Sat(SatScr::LIST)
}

// ---- the list's rows ----

/// The passes shown: the filter's satellite, or all; none under the minimum elevation (SAT-22; the predictor
/// leaves them out already, Console's tracks may not).
fn shown(app: &App, filter: u8) -> impl Iterator<Item = Pass> + '_ {
    let d = app.sat_data();
    let min = app.min_el();
    (0..d.passes()).filter_map(move |i| d.pass(i)).filter(move |p| (filter == ALL || p.sat == filter) && p.max_el >= min)
}

/// The row of the selected pass (by identity; the first when it is gone).
fn sel_row(app: &App, s: SatScr) -> usize {
    shown(app, s.filter).position(|p| p.sat == s.sat && p.aos.abs_diff(s.aos) < 120).unwrap_or(0)
}

fn nth(app: &App, s: SatScr, i: usize) -> Option<Pass> {
    shown(app, s.filter).nth(i)
}

/// The pass the list or the page acts on.
fn target(app: &App, s: SatScr) -> Option<Pass> {
    if s.page() == LIST { nth(app, s, sel_row(app, s)) } else { app.sat.cur }
}

const ROWS: usize = 11;
const ROW_Y: i32 = 58;
const ROW_H: i32 = 22;

/// Local time of UTC seconds: "hh:mm", or "2:32p" with the 12 h clock (MENU 0 6 6, SBAR-2.1).
fn hhmm(app: &App, utc: u32, b: &mut Buf<8>) {
    let (m, z) = crate::gpsdata::local_clock(utc, app.settings().tz);
    let _ = uwrite!(b, "{}", ferrite_ui::text::Clock(m, app.settings().clock == 1 && !z));
}

fn name_of(app: &App, sat: u8) -> Buf<16> {
    let mut b = Buf::new();
    if let Some(s) = app.sat_data().sat(sat) {
        b.set(s.name());
    }
    b
}

/// "436.805" (3 decimals) or "437.8042" (4).
fn mhz(f: Freq10, four: bool, b: &mut Buf<12>) {
    let (m, r) = (f / 100_000, f % 100_000);
    if four {
        let r = r / 10;
        let _ = uwrite!(b, "{}.{}{}{}{}", m, r / 1000, r / 100 % 10, r / 10 % 10, r % 10);
    } else {
        let r = (r + 50) / 100;
        let (m, r) = if r >= 1000 { (m + 1, 0) } else { (m, r) };
        let _ = uwrite!(b, "{}.{}{}{}", m, r / 100, r / 10 % 10, r % 10);
    }
}

/// A shift in 10 Hz as "+4.2k".
fn khz(d: i32, b: &mut Buf<12>) {
    let t = (d.abs() + 5) / 10;
    let _ = uwrite!(b, "{}{}.{}k", if d < 0 { "-" } else { "+" }, t / 10, t % 10);
}

/// A tone in 0.1 Hz as "67.0".
/// The downlink's S-meter (0-9, 6 dB a unit from -127 dBm) and whether its squelch is open, from the snapshot.
fn s_level(live: &LiveAll) -> (i16, bool) {
    let (rssi, open) = match live.mode {
        ModeLive::Sat { rssi, open } => (rssi, open),
        _ => (-127, false),
    };
    (((rssi + 127) / 6).clamp(0, 9), open)
}

/// `secs` as m:ss after what `b` holds.
fn mmss_after<const N: usize>(b: &mut Buf<N>, secs: u32) {
    let mut m = Buf::<8>::new();
    if secs >= 3600 {
        // SAT-17.1: an hour or more as hours and minutes ("1h03"): "62:45" was cut off in the pointing page's field
        let mins = secs / 60;
        let _ = uwrite!(m, "{}h{}{}", mins / 60, (mins % 60) / 10, mins % 10);
    } else {
        crate::screens::activity::mmss(secs, &mut m);
    }
    b.push(m.as_bytes());
}

fn tone(t: u16, b: &mut Buf<12>) {
    let _ = uwrite!(b, "{}.{}", t / 10, t % 10);
}

impl App {
    /// The satellite screen's pass, kept current: the selected pass from the list, refreshed from the source
    /// (its rise refined), and the same satellite's next pass once it has set.
    fn sat_follow(&mut self, s: SatScr) {
        let now = self.sat_now().unwrap_or(0);
        let d = self.sat_data();
        let mut best = (0..d.passes()).filter_map(|i| d.pass(i)).find(|p| p.sat == s.sat && p.aos.abs_diff(s.aos) < 120);
        if best.is_none_or(|p| p.los() + 30 < now) {
            if let Some(n) = (0..d.passes()).filter_map(|i| d.pass(i)).find(|p| p.sat == s.sat && p.los() > now) {
                best = Some(n);
            }
        }
        if let Some(p) = best {
            self.sat.cur = Some(p);
            if p.aos != s.aos {
                if let Screen::Sat(m) = &mut self.screen {
                    m.aos = p.aos;
                }
            }
        }
    }
}

impl View for SatScr {
    fn name(self) -> &'static str {
        "Satellites"
    }

    fn title(self, app: &App, b: &mut Buf<10>) {
        if self.page() == LIST {
            b.set(b"Passes");
        } else {
            let n = name_of(app, self.sat);
            b.set(&n.as_bytes()[..n.len().min(8)]);
        }
    }

    fn bar(self) -> bool {
        self.page & TX == 0
    }

    fn own_arrows(self) -> bool {
        true
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        // SAT-22.1: hold 2 on the pass or pointing page cycles the satellite's uplink power, as hold 2 cycles TX
        // power on home (KEY-11); the same value as the options page's, used from the next key-up.
        if e.press == Press::Long && e.key == Key::Digit(2) && self.opt == 0 && self.picker == 0 && self.page() != LIST {
            let mut pr = app.sat_data().prefs(self.sat);
            pr.power = (pr.power + 1) % 3;
            set_prefs(app, self.sat, pr);
            let name: &[u8] = [&b"Low"[..], b"Mid", b"High"][pr.power as usize];
            let body: &[u8] = if self.page & TX != 0 { b"from the next key-up" } else { name };
            let head: &[u8] = if self.page & TX != 0 { [&b"Uplink power: Low"[..], b"Uplink power: Mid", b"Uplink power: High"][pr.power as usize] } else { b"Uplink power" };
            app.show_popup(head, body);
            return;
        }
        if !matches!(e.press, Press::Short | Press::Repeat) {
            return;
        }
        let mut s = self;
        if s.opt != 0 {
            return opts_key(app, s, e.key);
        }
        if s.picker != 0 {
            match e.key {
                Key::Up => s.picker = (s.picker - 1).max(1),
                Key::Down => s.picker = (s.picker + 1).min(LEADS.len() as u8),
                Key::Digit(d @ 1..=5) => s.picker = d,
                Key::Menu => {
                    if let Some(p) = target(app, s) {
                        app.sat_set_alarm(p, s.picker - 1);
                    }
                    s.picker = 0;
                }
                Key::Exit | Key::Hash => s.picker = 0,
                _ => {}
            }
            app.screen = Screen::Sat(s);
            return;
        }
        if e.key == Key::Hash {
            // SAT-12: the picker on the pass's own lead when it has the alarm, else the last choice
            let on = target(app, s).filter(|p| app.sat_data().alarm().is_some_and(|a| a.sat == p.sat && a.aos.abs_diff(p.aos) < 120));
            let lead = if on.is_some() { LEADS.iter().position(|&m| m == app.sat.alarm_lead).unwrap_or(2) as u8 } else { app.sat.lead };
            if target(app, s).is_some() {
                s.picker = lead + 1;
            }
            app.screen = Screen::Sat(s);
            return;
        }
        if s.page() == LIST {
            let n = shown(app, s.filter).count();
            let row = sel_row(app, s);
            let pick = |app: &App, s: &mut SatScr, i: usize| {
                if let Some(p) = nth(app, *s, i) {
                    (s.sat, s.aos) = (p.sat, p.aos);
                }
            };
            match e.key {
                Key::Up if row > 0 => pick(app, &mut s, row - 1),
                Key::Down if row + 1 < n => pick(app, &mut s, row + 1),
                Key::Left | Key::Right => {
                    // ◀▶: from all, the selected pass's satellite (its passes, the selection kept on that pass);
                    // then one satellite at a time, and past either end all again (Alex, Nov 3: it always showed
                    // the first satellite)
                    let k = app.sat_data().sats();
                    let f = if s.filter == ALL { None } else { Some(s.filter) };
                    let next = match (e.key, f) {
                        (_, None) => nth(app, s, row).map(|p| p.sat),
                        (Key::Right, Some(f)) => (f + 1 < k).then_some(f + 1),
                        (_, Some(f)) => f.checked_sub(1),
                    };
                    let keep = (s.sat, s.aos);
                    s.filter = next.filter(|_| k > 0).unwrap_or(ALL);
                    // the same pass if the new list has it, else its first
                    if !shown(app, s.filter).any(|p| p.sat == keep.0 && p.aos.abs_diff(keep.1) < 120) {
                        pick(app, &mut s, 0);
                    }
                }
                Key::Menu => {
                    if let Some(p) = nth(app, s, row) {
                        app.sat.cur = Some(p);
                        app.screen = Screen::Sat(SatScr { page: PASS, sat: p.sat, aos: p.aos, ..s });
                        return;
                    }
                }
                // BACK on one satellite's passes: all of them again, the selection kept (Alex, Nov 3); then home
                Key::Exit if s.filter != ALL => s.filter = ALL,
                Key::Exit => return app.back(),
                _ => {}
            }
            app.screen = Screen::Sat(s);
            return;
        }
        // the pass page and the pointing page
        let tx = s.page & TX;
        let mut pr = app.sat_data().prefs(s.sat);
        let ntp = app.sat_data().sat(s.sat).map_or(0, |x| x.ntp);
        match e.key {
            Key::Left | Key::Right => pr.trim = (pr.trim + if e.key == Key::Right { 10 } else { -10 }).clamp(-500, 500),
            Key::Digit(d @ 1..=5) if d <= ntp => pr.tp = d - 1,
            Key::Star => pr.hd = !pr.hd, // SAT-11: even keyed
            Key::Digit(0) => s.page = if s.page() == POINT { PASS } else { POINT } | tx,
            Key::Exit if tx == 0 => s.page = LIST,
            _ => {}
        }
        set_prefs(app, s.sat, pr);
        app.screen = Screen::Sat(s);
    }

    /// SAT-22: hold MENU on the pass list, a pass page, the pointing page or the alarm picker (closing it) opens
    /// the satellite's options; not while keyed (a toast says so).
    fn options(self, app: &mut App) -> bool {
        if self.page & TX != 0 {
            app.show_toast(b"Options", b"after you release PTT", false);
            return true;
        }
        let Some(p) = target(app, self) else { return false };
        app.screen = Screen::Sat(SatScr { opt: 1, picker: 0, sat: p.sat, aos: if self.page() == LIST { p.aos } else { self.aos }, ..self });
        true
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        // the list and the options: a row; the pages: the trim (as ◀▶)
        let key = match (self.page() == LIST || self.opt != 0, d > 0) {
            (true, true) => Key::Down,
            (true, false) => Key::Up,
            (false, true) => Key::Right,
            (false, false) => Key::Left,
        };
        for _ in 0..d.unsigned_abs().min(5) {
            if let Screen::Sat(s) = app.screen {
                s.key(app, KeyEvent { key, press: Press::Short }, store);
            }
        }
    }

    fn ptt(self, app: &mut App, down: bool) {
        if !down || self.page() == LIST {
            if down {
                super::nav::transmit(app); // KEY-7 from the list: home, as any screen
            }
            return;
        }
        app.sat_ptt();
        if app.radio.keyed == Some(SAT_SLOT) {
            if let Screen::Sat(s) = &mut app.screen {
                s.page |= TX;
            }
        }
    }

    fn tick(self, app: &mut App, live: &LiveAll) {
        if self.page() != LIST {
            app.sat_follow(self);
        } else if self.opt == 0 {
            // SAT-6: the selection is a pass, not a row: once there is one, rows arriving above it don't move it
            let row = shown(app, self.filter).position(|p| p.sat == self.sat && p.aos.abs_diff(self.aos) < 120);
            if let (None, Some(p)) = (row, nth(app, self, 0)) {
                if let Screen::Sat(m) = &mut app.screen {
                    (m.sat, m.aos) = (p.sat, p.aos);
                }
            }
        }
        let keyed = app.radio.keyed == Some(SAT_SLOT);
        if keyed {
            // the Radio task refused (calibration, read-back, battery under load)
            // TX-50: a refusal after this key-up; unkeyed all the same, in case it keyed
            if let Some(r) = live.tx.refused.filter(|_| live.tx.slot == SAT_SLOT && live.tx.refusals != app.radio.refusals_at_key) {
                app.out.push(ferrite_core::msg::RadioCmd::Transmit { slot: SAT_SLOT, on: false });
                app.radio.keyed = None;
                super::transmit::refuse(app, SAT_SLOT, r);
                return;
            }
            // TX-30: unkey at the timeout; PTT must come up before the next transmission
            if app.now.wrapping_sub(app.radio.t0) >= app.settings().tx_timeout_s() * 1000 {
                app.ptt_released();
                app.show_toast(b"TX timeout", b"release PTT", false);
                app.timeout_beep = app.now.wrapping_add(400).max(1);
            }
        }
        if let Screen::Sat(s) = &mut app.screen {
            if (s.page & TX != 0) != keyed {
                s.page = if keyed { s.page | TX } else { s.page & !TX };
            }
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let fresh = app.page(lcd, (self.page, self.picker != 0, self.filter, self.opt != 0));
        // SAT-12.1: while the alarm picker is up the page under it waits (drawn once as it opens, whole again as it
        // closes): its countdown and live values redrew every second over the box and tore it (Nov 18, 2026)
        if self.picker != 0 && !fresh {
            return picker(app, lcd, self.picker - 1);
        }
        if self.opt != 0 {
            return opts_page(app, lcd, self);
        }
        match self.page() {
            LIST => list(app, lcd, self),
            POINT => point(app, lcd, self, cx.live),
            _ => pass_page(app, lcd, self, cx.live),
        }
        if self.picker != 0 {
            picker(app, lcd, self.picker - 1);
        }
    }

    fn hints(self, app: &App) -> Hints {
        let keyed = self.page & TX != 0;
        if self.opt != 0 {
            Hints::new(&[("OK", if self.opt == OPT_TRIM { "Reset" } else { "Change" }), ("0", "Full menu"), ("BACK", "Back")])
        } else if self.picker != 0 {
            Hints::new(&[("OK", "Set"), (ARROWS_UD, "Choose"), ("BACK", "Cancel")])
        } else if self.page() == LIST {
            if self.filter != ALL {
                Hints::new(&[("OK", "Open"), (ARROWS_LR, "Satellite"), ("BACK", "All")])
            } else {
                Hints::new(&[("OK", "Open"), ("#", "Alarm"), (ARROWS_LR, "Satellite")])
            }
        } else if keyed {
            Hints::new(&[("PTT", "Release"), ("*", if app.sat_data().prefs(self.sat).hd { "HD" } else { "FD" }), (ARROWS_LR, "Trim")])
        } else if self.page() == POINT {
            Hints::new(&[("PTT", "Talk"), ("0", "Plot"), ("BACK", "Back")])
        } else if app.sat.cur.zip(app.sat_now()).is_some_and(|(p, n)| p.up(n)) {
            Hints::new(&[("PTT", "Talk"), (ARROWS_LR, "Trim"), ("1-5", "Mode")])
        } else {
            let on = app.sat_data().alarm().zip(app.sat.cur).is_some_and(|(a, p)| a.sat == p.sat && a.aos.abs_diff(p.aos) < 120);
            Hints::new(&[("1-5", "Mode"), ("#", if on { "Alarm on" } else { "Alarm" }), ("BACK", "Back")])
        }
    }
}

// ---- the satellite's options (SAT-22) ----

/// The rows: Doppler, the uplink's power, duplex, the transponder, the trim; 1 + the row in `SatScr::opt`.
pub(crate) const OPT_ROWS: [(&str, crate::text::Txt); 6] = [
    ("Doppler", txt!("Both sides, or the 70 cm side only")),
    ("Uplink power", txt!("Low to start: a Yagi needs little")),
    ("Duplex", txt!("Half mutes the downlink while you talk")),
    ("Transponder", txt!("Which mode PTT and the page use")),
    ("Trim", txt!("Downlink fine tune; OK sets it to 0")),
    ("Lowest pass", txt!("Every satellite: passes peaking lower aren't listed")),
];
const OPT_TRIM: u8 = 5;

/// A satellite's choices changed: kept (the store writes them, SAT-2) and the radio retuned.
fn set_prefs(app: &mut App, sat: u8, pr: ferrite_core::sat::data::Prefs) {
    if pr == app.sat_data().prefs(sat) {
        return;
    }
    if let Some(d) = app.sat_data_mut() {
        d.set_prefs(sat, pr);
    }
    if let Some(now) = app.sat_now() {
        app.sat_retune(now, true);
    }
}

/// Row `i`'s value now.
pub(crate) fn opt_value(app: &App, sat: u8, i: u8, b: &mut Buf<16>) {
    let pr = app.sat_data().prefs(sat);
    match i {
        1 => b.set(if pr.uhf_only { b"70 cm only" } else { b"Both sides" }),
        2 => b.set([&b"Low"[..], b"Mid", b"High"][pr.power.min(2) as usize]),
        3 => b.set(if pr.hd { b"Half" } else { b"Full" }),
        4 => {
            let kind = app.sat_data().sat(sat).and_then(|s| s.tp(pr.tp).map(|t| t.kind.name())).unwrap_or("none");
            let _ = uwrite!(b, "{} {}", pr.tp + 1, kind);
        }
        5 => {
            let mut k = Buf::<12>::new();
            khz(pr.trim as i32, &mut k);
            b.set(k.as_bytes());
            b.push(b"Hz");
        }
        _ => {
            let _ = uwrite!(b, "{}\u{b0}", app.min_el());
        }
    }
}

fn opts_key(app: &mut App, mut s: SatScr, key: Key) {
    let n = OPT_ROWS.len() as u8;
    let mut pr = app.sat_data().prefs(s.sat);
    let ntp = app.sat_data().sat(s.sat).map_or(0, |x| x.ntp);
    let change = |app: &mut App, pr: &mut ferrite_core::sat::data::Prefs, row: u8| match row {
        1 => pr.uhf_only = !pr.uhf_only,
        2 => pr.power = (pr.power + 1) % 3,
        3 => pr.hd = !pr.hd,
        4 => pr.tp = if pr.tp + 1 < ntp { pr.tp + 1 } else { 0 },
        5 => pr.trim = 0,
        // SAT-22 (decision 341): the minimum elevation, round 0, 5, 10, 15, 20 degrees; the list predicts again
        _ => app.set.sat_min_el = (app.set.sat_min_el + 1) % crate::settings::SAT_MIN_EL.len() as u8,
    };
    match key {
        Key::Up => s.opt = if s.opt > 1 { s.opt - 1 } else { n },
        Key::Down => s.opt = if s.opt < n { s.opt + 1 } else { 1 },
        Key::Digit(0) => {
            app.screen = Screen::Sat(SatScr { opt: 0, ..s });
            return app.open(super::nav::Dest::Advanced);
        }
        // MENU-25: a digit picks its row; OK changes it (a press each, round the values)
        Key::Digit(d) if d <= n => s.opt = d,
        Key::Menu => change(app, &mut pr, s.opt),
        Key::Exit => s.opt = 0,
        _ => {}
    }
    set_prefs(app, s.sat, pr);
    app.screen = Screen::Sat(s);
}

fn opts_page(app: &mut App, lcd: &mut impl Panel, s: SatScr) {
    let mut vals = [Buf::<16>::new(); OPT_ROWS.len()];
    for (i, v) in vals.iter_mut().enumerate() {
        opt_value(app, s.sat, i as u8 + 1, v);
    }
    super::opts::options_page(app, lcd, OPT_ROWS.iter().zip(vals.iter()).map(|((l, _), v)| (l.as_bytes(), v.as_bytes())), s.opt - 1);
    let mut p = Buf::<24>::new();
    p.set(name_of(app, s.sat).as_bytes());
    p.push(b" MENU");
    app.help_txt(lcd, super::opts::opts_help_y(OPT_ROWS.len()), 46, OPT_ROWS[(s.opt - 1) as usize % OPT_ROWS.len()].1, p.as_bytes());
}

// ---- drawing parts ----

/// A small bell (the marked pass, SAT-12): 9 x 10.
fn bell(lcd: &mut impl Panel, x: i32, y: i32, c: u16) {
    lcd.fill_r(x + 1, y, 7, 7, 3, c);
    lcd.fill(x + 1, y + 4, 7, 3, c);
    lcd.fill(x, y + 7, 9, 1, c);
    lcd.fill(x + 3, y + 8, 3, 2, c);
}

/// ↑ or ↓ (12 px tall), from a triangle and a stem.
fn arrow(lcd: &mut impl Panel, x: i32, y: i32, up: bool, c: u16) {
    if up {
        lcd.tri(x, y, 11, 0, c);
        lcd.fill(x + 4, y + 6, 3, 7, c);
    } else {
        lcd.fill(x + 4, y, 3, 7, c);
        lcd.tri(x, y + 3, 11, 1, c);
    }
}

/// `n` bars of a meter from x to x + width, `lit` of them in `c`.
fn bars(lcd: &mut impl Panel, x: i32, y: i32, width: i32, h: i32, n: i32, lit: i32, c: u16) {
    let bw = width / n;
    for i in 0..n {
        lcd.fill_r(x + i * bw, y, bw - 3, h, 1, if i < lit { c } else { LINE });
    }
}

/// "m:ss" (or "h:mm:ss") in a digits-only font, its colons drawn as two squares; returns the width.
fn big_time(lcd: &mut impl Panel, x: i32, baseline: i32, secs: u32, f: &'static Font4, c: u16) -> i32 {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    let mut t = Buf::<12>::new();
    if h > 0 {
        let _ = uwrite!(t, "{}:{}{}:{}{}", h, m / 10, m % 10, s / 10, s % 10);
    } else {
        let _ = uwrite!(t, "{}:{}{}", m, s / 10, s % 10);
    }
    let dot = (f.line_h as i32 / 11).max(2);
    let a = f.ascent as i32;
    let mut cx = x;
    for (i, part) in t.as_bytes().split(|&b| b == b':').enumerate() {
        if i > 0 {
            lcd.fill(cx + dot / 2, baseline - a * 2 / 3, dot, dot, c);
            lcd.fill(cx + dot / 2, baseline - a / 5 - dot, dot, dot, c);
            cx += dot * 2;
        }
        cx += lcd.text4(cx, baseline, part, f, c, BG);
    }
    cx - x
}

/// The degree sign after a big number: a small ring.
fn deg_ring(lcd: &mut impl Panel, x: i32, y: i32, r: i32, c: u16) {
    super::lists::ring(lcd, x + r, y + r, r, (r / 3).max(1), c);
}

// ---- the pass list (B22) ----

fn list(app: &mut App, lcd: &mut impl Panel, s: SatScr) {
    let (state, nsats) = (app.sat_data().state(), app.sat_data().sats());
    let now = app.sat_now().unwrap_or(0);
    let n = shown(app, s.filter).count();
    // the head line: what is predicted (SAT-6) and the elements' age (SAT-9: the oldest)
    let mut l = Buf::<32>::new();
    let (done, all) = predict_progress(app);
    if state == State::Busy {
        let _ = uwrite!(l, "Predicting {} of {}...", done.min(all) + 1, all.max(1));
    } else if s.filter != ALL {
        let nm = name_of(app, s.filter);
        let _ = uwrite!(l, "{} \u{b7} {} passes", crate::screens::lists::Txt(nm.as_bytes()), n);
    } else {
        let _ = uwrite!(l, "{} passes in 24 h", n);
    }
    // SAT-9: the oldest elements' age, read from every satellite only when the line is drawn (each a record read
    // from the SPI flash on the radio): its key is the source's version and the hour
    let aged = nsats > 0 && now != 0 && state != State::Busy;
    if app.dirty(1, Mix::new().b(l.as_bytes()).n(app.sat_data().version()).n(if aged { now / 3600 + 1 } else { 0 }).get()) {
        let d = app.sat_data();
        let (age, lvl) = (0..d.sats()).filter_map(|i| d.sat(i)).map(|x| (x.el.age_days(now), x.age_level(now))).max().unwrap_or((0, 0));
        let mut r = Buf::<16>::new();
        if aged {
            let _ = uwrite!(r, "Elements {} d", age);
        }
        let c = [GREEN, AMBER, RED][lvl as usize % 3];
        let rw = w::field(lcd, 120, 112, 22, 40, 34, r.as_bytes(), &w::st(&UI_12, c, BG, Align::Right));
        // one satellite's name beside the elements' age didn't fit ("LILACSAT-2 · 4 passes", 147 px in 134): the
        // count alone then, as each row names it
        if s.filter != ALL && state != State::Busy && text4_width(l.as_bytes(), &UI_12) > 224 - rw - 6 {
            l.clear();
            let _ = uwrite!(l, "{} passes", n);
        }
        w::field(lcd, 8, 224 - rw - 6, 22, 40, 34, l.as_bytes(), const { &w::st(&UI_12, SUB, BG, Align::Left) });
    }
    let msg: Option<(&[u8], &[u8])> = match state {
        State::NoSats => Some((b"No satellites", b"Load them from Ferrite Console")),
        State::NoClock => Some((b"No clock yet", b"GPS time, or set it at 0 6 6")),
        State::NoPosition => Some((b"No position", b"A GPS fix, or the fixed one (0 6 9)")),
        _ if n == 0 && state == State::Busy => Some((b"Predicting\x2e\x2e\x2e", b"")),
        _ if n == 0 => Some((b"No passes in 24 h", b"")),
        _ => None,
    };
    if let Some((a, b)) = msg {
        if app.dirty(15, Mix::new().b(a).b(b).get()) {
            app.forget(2, 12);
            lcd.fill(0, 40, 240, 260, BG);
            app.empty(lcd, a, b);
        }
        return;
    }
    if app.dirty(2, 1) {
        lcd.fill(0, 40, 240, 18, BG);
        for (t, x) in [(&b"Satellite"[..], 8), (b"Rises", 124), (b"Max", 172), (b"Min", 208)] {
            lcd.text4_rows(x, 51, t, &UI_12, DIM, BG, 40, 56);
        }
        lcd.fill(0, 56, 240, 1, LINE);
        app.drawn.hash[15] = 0;
    }
    let row = sel_row(app, s);
    let top = row.saturating_sub(ROWS - 1);
    let alarm = app.sat_data().alarm();
    for k in 0..ROWS {
        let p = nth(app, s, top + k);
        let y = ROW_Y + k as i32 * ROW_H;
        let sel = top + k == row;
        let up = p.is_some_and(|p| p.up(now));
        let marked = p.zip(alarm).is_some_and(|(p, a)| p.sat == a.sat && p.aos.abs_diff(a.aos) < 120);
        // the time column: the rise, or UP; the length, or the time to set while up
        let mut t = Buf::<8>::new();
        let mut m = Buf::<8>::new();
        let mut el = Buf::<8>::new();
        if let Some(p) = p {
            if up {
                t.set(b"UP");
                crate::screens::activity::mmss(p.los() - now, &mut m);
            } else {
                hhmm(app, p.aos, &mut t);
                let _ = uwrite!(m, "{}", (p.dur as u32 + 30) / 60);
            }
            let _ = uwrite!(el, "{}\u{b0}", p.max_el);
        }
        let h = Mix::new().n(p.map_or(0, |p| 1 + p.sat as u32 * 256 + p.max_el as u32)).b(t.as_bytes()).b(m.as_bytes()).n(sel as u32 | (marked as u32) << 1 | (up as u32) << 2 | (app.sat.alarm_lead as u32) << 3).get();
        if !app.dirty(3 + k, h) {
            continue;
        }
        // the name only for a row drawn (a record read from the SPI flash on the radio)
        let nm = p.map_or(Buf::new(), |p| name_of(app, p.sat));
        let low = p.is_some_and(|p| p.max_el < 10);
        let bg = if sel { FOCUS } else if up { rgb565(0x12, 0x32, 0x1D) } else { BG };
        let fg = if low { DIM } else { TEXT };
        // the rows stop short of the scroll cue's column (DISP-24), which draws itself
        let rw = w::CUE_X - 2;
        ferrite_ui::canvas::banded(lcd, 0, y, rw, ROW_H, BG, |lcd| {
            lcd.fill(0, y, rw, ROW_H - 1, bg);
            lcd.fill(0, y + ROW_H - 1, rw, 1, rgb565(0x1B, 0x1F, 0x25));
            if p.is_none() {
                return;
            }
            if sel || up {
                lcd.fill(0, y, 3, ROW_H - 1, if up { GREEN } else { BLUE });
            }
            let b = y + 15;
            w::field(lcd, 8, 88, y, y + ROW_H - 1, b, nm.as_bytes(), &w::st(&UI_12B, fg, bg, Align::Left));
            if marked {
                bell(lcd, 100, y + 5, AMBER);
                let mut lt = Buf::<4>::new();
                let _ = uwrite!(lt, "{}", app.sat.alarm_lead);
                lcd.text4(110, b, lt.as_bytes(), &UI_12, AMBER, bg);
            }
            lcd.text4(124, b, t.as_bytes(), &UI_12B, if up { GREEN } else { fg }, bg);
            lcd.text4(172, b, el.as_bytes(), &UI_12B, fg, bg);
            // while up, the time to set ("10:30") is wider than the length's column: right-aligned from the Max
            // column's end (its "90°" ends by 193)
            let (mx, al) = if up { (198, Align::Right) } else { (208, Align::Left) };
            w::field(lcd, mx, rw - mx, y, y + ROW_H - 1, b, m.as_bytes(), &w::st(&UI_12B, fg, bg, al));
        });
    }
    app.scroll_cue(lcd, (2, ROWS), ROW_Y, ROWS as i32 * ROW_H, n > ROWS, top as u32, ROWS as u32, n as u32);
}

/// SAT-6's progress: satellites done and in all (the source says nothing but busy: the list's own count then).
fn predict_progress(app: &App) -> (u8, u8) {
    let d = app.sat_data();
    let all = d.sats();
    let mut seen = [false; 32];
    for p in (0..d.passes()).filter_map(|i| d.pass(i)) {
        seen[p.sat as usize % 32] = true;
    }
    (d.progress().unwrap_or((seen.iter().filter(|&&x| x).count() as u8, all)).0, all)
}

// ---- the alarm picker (SAT-12) ----

fn picker(app: &mut App, lcd: &mut impl Panel, sel: u8) {
    if !app.dirty(14, Mix::new().n(sel as u32).get()) {
        return;
    }
    let (x, y, wd) = (40, 84, 160);
    ferrite_ui::canvas::banded(lcd, x, y, wd, 150, BG, |lcd| {
        lcd.fill_r(x, y, wd, 150, 6, SURFACE);
        lcd.frame_r(x, y, wd, 150, 6, LINE);
        lcd.text4_rows(x + 12, y + 20, b"Alarm before it rises", &UI_12B, TEXT, SURFACE, y + 4, y + 24);
        for (i, &m) in LEADS.iter().enumerate() {
            let ry = y + 28 + i as i32 * 23;
            let on = i as u8 == sel;
            let bg = if on { FOCUS } else { SURFACE };
            lcd.fill(x + 6, ry, wd - 12, 21, bg);
            if on {
                lcd.fill(x + 6, ry, 3, 21, BLUE);
            }
            let mut t = Buf::<12>::new();
            if m == 0 {
                t.set(b"Off");
            } else {
                let _ = uwrite!(t, "{} min", m);
            }
            lcd.text4(x + 16, ry + 15, t.as_bytes(), &UI_12B, if on { TEXT } else { SUB }, bg);
        }
    });
}

// ---- the pass page (B23) ----

/// The polar plot's centre and horizon radius; the keyed layout's smaller one.
const PLOT: (i32, i32, i32) = (67, 86, 54);
const PLOT_TX: (i32, i32, i32) = (50, 70, 40);

fn xy(l: &Look, (cx, cy, r): (i32, i32, i32)) -> (i32, i32) {
    let d = r as f32 * (90.0 - l.el.clamp(0.0, 90.0)) / 90.0;
    let (s, c) = sin_cos(rad(l.az));
    (cx + (d * s) as i32, cy - (d * c) as i32)
}

/// The sky plot (SAT-7): rings at 0, 30 and 60°, north up, the predicted track dashed, the part flown solid,
/// the satellite now; before the rise its rise point and direction.
#[inline(never)]
fn plot(app: &mut App, lcd: &mut impl Panel, p: &Pass, now: u32, g: (i32, i32, i32), r_dot: i32) {
    let (cx, cy, r) = g;
    let up = p.up(now);
    let h = Mix::new().n(p.aos).n(p.sat as u32).n(g.2 as u32).n(if up { now } else { 0 }).get();
    if !app.dirty(1, h) {
        return;
    }
    let d = app.sat_data();
    ferrite_ui::canvas::banded(lcd, cx - r - 2, cy - r - 2, 2 * r + 5, 2 * r + 5, BG, |lcd| {
        super::lists::ring(lcd, cx, cy, r, 1, LINE);
        super::lists::ring(lcd, cx, cy, r * 2 / 3, 1, rgb565(0x1F, 0x24, 0x2B));
        super::lists::ring(lcd, cx, cy, r / 3, 1, rgb565(0x1F, 0x24, 0x2B));
        lcd.fill(cx - r, cy, 2 * r + 1, 1, rgb565(0x1F, 0x24, 0x2B));
        lcd.fill(cx, cy - r, 1, 2 * r + 1, rgb565(0x1F, 0x24, 0x2B));
        lcd.text4(cx - 3, cy - r + 13, b"N", &UI_12, SUB, BG);
        const N: u32 = 24;
        let mut prev: Option<(i32, i32, u32)> = None;
        for k in 0..=N {
            let t = p.aos + p.dur as u32 * k / N;
            let Some(l) = d.look(p.sat, t) else { continue };
            let (x, y) = xy(&l, g);
            if let Some((px, py, pt)) = prev {
                if pt < now && up {
                    lcd.line(px, py, x, y, GREEN);
                    lcd.line(px + 1, py, x + 1, y, GREEN);
                } else if k % 2 == 1 {
                    lcd.line(px, py, x, y, SUB);
                }
            }
            prev = Some((x, y, t));
        }
        if up {
            if let Some(l) = d.look(p.sat, now) {
                let (x, y) = xy(&l, g);
                lcd.fill_r(x - r_dot, y - r_dot, 2 * r_dot + 1, 2 * r_dot + 1, r_dot, GREEN);
            }
        } else if let Some(l) = d.look(p.sat, p.aos) {
            let (x, y) = xy(&l, g);
            lcd.fill_r(x - r_dot, y - r_dot, 2 * r_dot + 1, 2 * r_dot + 1, r_dot, AMBER);
        }
    });
}

#[inline(never)]
fn pass_page(app: &mut App, lcd: &mut impl Panel, s: SatScr, live: &LiveAll) {
    let Some((p, sat, t, pr)) = app.sat_cur() else {
        if app.dirty(15, 1) {
            app.empty(lcd, b"Pass gone", b"BACK for the list");
        }
        return;
    };
    let now = app.sat_now().unwrap_or(0);
    let l = app.sat_data().look(p.sat, now).unwrap_or_default();
    let keyed = s.page & TX != 0;
    let up = p.up(now);
    // SAT-8: the frequencies as corrected now, or at the rise before it
    let at = if up || now > p.los() { l } else { app.sat_data().look(p.sat, p.aos).unwrap_or_default() };
    let (down, upl) = crate::sat::corrected(&t, &at, &pr);
    if keyed {
        return keyed_page(app, lcd, &p, &sat, &t, &pr, now, &l, (down, upl), live);
    }
    plot(app, lcd, &p, now, PLOT, 5);
    // the right column: the countdown and where it is
    let mut lines: [Buf<24>; 4] = [Buf::new(), Buf::new(), Buf::new(), Buf::new()];
    let mut tm = Buf::<8>::new();
    hhmm(app, p.aos + p.tmax as u32, &mut tm);
    let (head, secs, c): (&[u8], u32, u16) = if up {
        let _ = uwrite!(lines[0], "Az {}\u{b0}", (l.az + 0.5) as u32 % 360);
        let _ = uwrite!(lines[1], "El {}\u{b0}", l.el.max(0.0) as u32);
        let top = p.aos + p.tmax as u32;
        if now < top {
            let _ = uwrite!(lines[2], "Max {}\u{b0} in ", p.max_el);
            mmss_after(&mut lines[2], top - now);
        } else {
            let _ = uwrite!(lines[2], "Max {}\u{b0} passed", p.max_el);
        }
        (b"Sets in", p.los() - now, GREEN)
    } else if now < p.aos {
        let _ = uwrite!(lines[0], "Max {}\u{b0} {}", p.max_el, crate::screens::lists::Txt(tm.as_bytes()));
        let _ = uwrite!(lines[1], "{} min pass", (p.dur as u32 + 30) / 60);
        let _ = uwrite!(lines[2], "rises {}", look::point8(p.aos_az as f32 * 2.0));
        if p.max_el < 10 {
            lines[3].set(b"Low: hard to hear");
        }
        (b"Rises in", p.aos - now, AMBER)
    } else {
        lines[0].set(b"Pass over");
        (b"Set", 0, SUB)
    };
    if !app.sat.by_gps {
        lines[3].set(b"time \xc2\xb11 min"); // SAT-5
    }
    if app.dirty(2, Mix::new().b(head).n(secs).b(lines[0].as_bytes()).b(lines[1].as_bytes()).b(lines[2].as_bytes()).b(lines[3].as_bytes()).get()) {
        ferrite_ui::canvas::banded(lcd, 128, 24, 112, 122, BG, |lcd| {
            lcd.text4(132, 38, head, &UI_12, SUB, BG);
            if secs > 0 {
                big_time(lcd, 132, 70, secs, &DIG_26, c);
            }
            for (i, b) in lines.iter().enumerate() {
                let col = if i == 3 { AMBER } else if up && i < 2 { TEXT } else { SUB };
                lcd.text4(132, 92 + i as i32 * 17, b.as_bytes(), &UI_12, col, BG);
            }
        });
    }
    lcd_rule(app, lcd, 146);
    // ↓ the downlink (large) and its shift; the meter while up; ↑ the uplink, its shift and tone
    let mut f = Buf::<12>::new();
    mhz(down, up, &mut f);
    let mut sh = Buf::<12>::new();
    khz(down as i32 - t.down as i32 - pr.trim as i32, &mut sh);
    if app.dirty(3, Mix::new().b(f.as_bytes()).b(sh.as_bytes()).get()) {
        ferrite_ui::canvas::banded(lcd, 0, 149, 240, 34, BG, |lcd| {
            arrow(lcd, 8, 162, false, GREEN);
            let wd = lcd.text4(26, 178, f.as_bytes(), &DIG_26, FREQ, BG);
            lcd.text4(32 + wd, 176, sh.as_bytes(), &UI_12, SUB, BG);
        });
    }
    let (su, open) = s_level(live);
    if app.dirty(4, Mix::new().n(up as u32 | (open as u32) << 1).n(su as u32).get()) {
        ferrite_ui::canvas::banded(lcd, 0, 183, 240, 10, BG, |lcd| {
            if up {
                bars(lcd, 26, 184, 168, 7, 7, (su as i32 * 7 + 8) / 9, if open { GREEN } else { DIM });
                let mut sb = Buf::<4>::new();
                let _ = uwrite!(sb, "S{}", su);
                lcd.text4_rows(200, 193, sb.as_bytes(), &UI_12B, TEXT, BG, 183, 193);
            }
        });
    }
    let mut uf = Buf::<12>::new();
    let mut us = Buf::<24>::new();
    if t.up != 0 {
        mhz(upl, up, &mut uf);
        let mut k = Buf::<12>::new();
        khz(upl as i32 - t.up as i32, &mut k);
        us.set(k.as_bytes());
        if t.tone != 0 {
            let mut tt = Buf::<12>::new();
            tone(t.tone, &mut tt);
            us.push(b" \xc2\xb7 ");
            us.push(tt.as_bytes());
        }
    }
    if app.dirty(5, Mix::new().b(uf.as_bytes()).b(us.as_bytes()).get()) {
        ferrite_ui::canvas::banded(lcd, 0, 194, 240, 24, BG, |lcd| {
            if t.up == 0 {
                lcd.text4(26, 211, b"Receive only", &UI_12, SUB, BG);
                return;
            }
            arrow(lcd, 8, 198, true, BLUE);
            let wd = lcd.text4(26, 213, uf.as_bytes(), &DIG_17, TEXT, BG);
            lcd.text4(32 + wd, 212, us.as_bytes(), &UI_12, SUB, BG);
        });
    }
    // the transponder, arming tone, trim, duplex and the elements' age (SAT-9, SAT-10, SAT-11)
    let mut info = Buf::<48>::new();
    info.set(t.kind.name().as_bytes());
    if t.arm != 0 && !up {
        let mut a = Buf::<12>::new();
        tone(t.arm, &mut a);
        let _ = uwrite!(info, " \u{b7} arms with {} first", crate::screens::lists::Txt(a.as_bytes()));
    } else if pr.trim != 0 {
        let mut k = Buf::<12>::new();
        khz(pr.trim as i32, &mut k);
        let _ = uwrite!(info, " \u{b7} trim {}", crate::screens::lists::Txt(k.as_bytes()));
    }
    let age = sat.el.age_days(now);
    let lvl = sat.age_level(now);
    if app.dirty(6, Mix::new().b(info.as_bytes()).n(pr.hd as u32 | (lvl as u32) << 1 | (pr.power as u32) << 3).n(age).get()) {
        ferrite_ui::canvas::banded(lcd, 0, 219, 240, 18, BG, |lcd| {
            let wd = lcd.text4(8, 232, info.as_bytes(), &UI_12, SUB, BG);
            let mut x = lcd.text4(14 + wd, 232, if pr.hd { b"* HD" } else { b"* FD" }, &UI_12B, TEXT, BG) + 14 + wd;
            let mut a = Buf::<8>::new();
            let _ = uwrite!(a, "{} d", age);
            // SAT-22.1: hold 2's hint, the uplink power, when it fits before the age (a long mode line leaves it off)
            let pw: &[u8] = [&b"Low"[..], b"Mid", b"High"][pr.power as usize % 3];
            let need = ferrite_ui::gfx::text4_width(b"2", &UI_12B) + 4 + ferrite_ui::gfx::text4_width(pw, &UI_12);
            if x + 10 + need + 8 + ferrite_ui::gfx::text4_width(a.as_bytes(), &UI_12) <= 232 {
                let k = lcd.text4(x + 10, 232, b"2", &UI_12B, TEXT, BG);
                x += 10 + k + 4 + lcd.text4(x + 10 + k + 4, 232, pw, &UI_12, SUB, BG);
            }
            w::field(lcd, x + 6, 226 - x, 219, 237, 232, a.as_bytes(), &w::st(&UI_12, [DIM, AMBER, RED][lvl as usize % 3], BG, Align::Right));
        });
    }
    timeline(app, lcd, &p, now, 244);
}

fn lcd_rule(app: &mut App, lcd: &mut impl Panel, y: i32) {
    if app.dirty(11, Mix::new().n(y as u32).get()) {
        lcd.fill(0, y, 240, 2, LINE);
    }
}

/// SAT-16: the pass as a timeline: rise, top (time and elevation), set, a marker at now (amber before the
/// rise, green while up); under it the same satellite's next pass.
fn timeline(app: &mut App, lcd: &mut impl Panel, p: &Pass, now: u32, y: i32) {
    let frac = if now <= p.aos { 0 } else { ((now - p.aos) * 1000 / (p.dur as u32).max(1)).min(1000) as i32 };
    let up = p.up(now);
    let mut a = Buf::<8>::new();
    let mut m = Buf::<8>::new();
    let mut z = Buf::<8>::new();
    hhmm(app, p.aos, &mut a);
    hhmm(app, p.aos + p.tmax as u32, &mut m);
    hhmm(app, p.los(), &mut z);
    let mut mid = Buf::<16>::new();
    let _ = uwrite!(mid, "{} {}\u{b0}", crate::screens::lists::Txt(m.as_bytes()), p.max_el);
    let d = app.sat_data();
    let next = (0..d.passes()).filter_map(|i| d.pass(i)).find(|q| q.sat == p.sat && q.aos > p.aos);
    let mut nl = Buf::<40>::new();
    if let Some(q) = next {
        let mut t = Buf::<8>::new();
        hhmm(app, q.aos, &mut t);
        let nm = name_of(app, q.sat);
        let _ = nm;
        let _ = uwrite!(nl, "Next pass {}, {}\u{b0} high", crate::screens::lists::Txt(t.as_bytes()), q.max_el);
    }
    if !app.dirty(7, Mix::new().n(frac as u32).n(up as u32).b(a.as_bytes()).b(mid.as_bytes()).b(z.as_bytes()).b(nl.as_bytes()).get()) {
        return;
    }
    let c = if up { GREEN } else { AMBER };
    ferrite_ui::canvas::banded(lcd, 0, y, 240, 54, BG, |lcd| {
        let (x0, wd) = (12, 216);
        lcd.fill_r(x0, y + 7, wd, 3, 1, LINE);
        let fx = x0 + wd * frac / 1000;
        lcd.fill_r(x0, y + 7, fx - x0, 3, 1, c);
        let mx = x0 + wd * p.tmax as i32 / (p.dur as i32).max(1);
        lcd.fill(mx, y + 2, 2, 13, SUB);
        lcd.fill_r(fx - 6, y + 2, 13, 13, 6, c);
        lcd.text4(8, y + 30, a.as_bytes(), &UI_12, SUB, BG);
        w::field(lcd, 70, 100, y + 18, y + 34, y + 30, mid.as_bytes(), const { &w::st(&UI_12, SUB, BG, Align::Center) });
        w::field(lcd, 170, 62, y + 18, y + 34, y + 30, z.as_bytes(), const { &w::st(&UI_12, SUB, BG, Align::Right) });
        lcd.text4(8, y + 48, nl.as_bytes(), &UI_12, c, BG);
    });
}

/// Keyed up (B23, SAT-11): the red frame and bar, the uplink large, the downlink live with the "you" meter in
/// full duplex, or muted in half duplex.
#[inline(never)]
fn keyed_page(app: &mut App, lcd: &mut impl Panel, p: &Pass, sat: &ferrite_core::sat::Sat, t: &ferrite_core::sat::Transponder, pr: &ferrite_core::sat::data::Prefs, now: u32, l: &Look, (down, upl): (Freq10, Freq10), live: &LiveAll) {
    let secs = app.now.wrapping_sub(app.radio.t0) / 1000;
    let mut tt = Buf::<8>::new();
    crate::screens::activity::mmss(secs, &mut tt);
    if app.dirty(9, Mix::new().b(tt.as_bytes()).n(pr.power as u32).get()) {
        // the status bar's place: the red bar (the screen draws its own while keyed)
        ferrite_ui::canvas::banded(lcd, 0, 0, 240, w::SBAR_H, RED, |lcd| {
            let mut n = Buf::<20>::new();
            n.set(b"TX \xc2\xb7 ");
            n.push(sat.name());
            w::field(lcd, 8, 100, 0, 20, 15, n.as_bytes(), const { &w::st(&UI_12B, BLACK, RED, Align::Left) });
            w::field(lcd, 108, 44, 0, 20, 15, tt.as_bytes(), const { &w::st(&UI_12B, BLACK, RED, Align::Center) });
            w::field(lcd, 152, 80, 0, 20, 15, crate::sat::POWER_NAMES[pr.power.min(2) as usize].as_bytes(), const { &w::st(&UI_12B, BLACK, RED, Align::Right) });
        });
        lcd.fill(0, w::SBAR_H, 2, w::HINT_Y - w::SBAR_H, RED);
        lcd.fill(238, w::SBAR_H, 2, w::HINT_Y - w::SBAR_H, RED);
    }
    plot(app, lcd, p, now, PLOT_TX, 4);
    let mut a = Buf::<24>::new();
    let _ = uwrite!(a, "Az {}\u{b0} \u{b7} El {}\u{b0}", (l.az + 0.5) as u32 % 360, l.el.max(0.0) as u32);
    let left = p.los().saturating_sub(now);
    if app.dirty(2, Mix::new().b(a.as_bytes()).n(left).get()) {
        ferrite_ui::canvas::banded(lcd, 96, 24, 140, 86, BG, |lcd| {
            lcd.text4(100, 38, b"Sets in", &UI_12, SUB, BG);
            big_time(lcd, 100, 62, left, &DIG_17, GREEN);
            lcd.text4(100, 84, a.as_bytes(), &UI_12, TEXT, BG);
        });
    }
    lcd_rule(app, lcd, 113);
    let mut f = Buf::<12>::new();
    mhz(upl, true, &mut f);
    let mut sh = Buf::<24>::new();
    let mut k = Buf::<12>::new();
    khz(upl as i32 - t.up as i32, &mut k);
    sh.set(k.as_bytes());
    if t.tone != 0 {
        let mut tn = Buf::<12>::new();
        tone(if app.now.wrapping_sub(app.sat.arm_until) > u32::MAX / 2 { t.arm } else { t.tone }, &mut tn);
        sh.push(b" \xc2\xb7 ");
        sh.push(tn.as_bytes());
    }
    let pw = crate::sat::POWER_BARS[pr.power.min(2) as usize];
    if app.dirty(3, Mix::new().b(f.as_bytes()).b(sh.as_bytes()).n(pw as u32).get()) {
        let bg = rgb565(0x2A, 0x0F, 0x0F);
        ferrite_ui::canvas::banded(lcd, 4, 117, 232, 54, BG, |lcd| {
            lcd.fill_r(6, 118, 228, 34, 4, bg);
            arrow(lcd, 12, 128, true, RED);
            let wd = lcd.text4(30, 146, f.as_bytes(), &DIG_26, TEXT, bg);
            lcd.text4(36 + wd, 145, sh.as_bytes(), &UI_12, rgb565(0xFF, 0xB3, 0xB3), bg);
            lcd.text4(30, 166, b"power", &UI_12, SUB, BG);
            bars(lcd, 76, 158, 150, 6, 5, pw, RED);
        });
    }
    let (su, open) = s_level(live);
    let mut df = Buf::<12>::new();
    mhz(down, true, &mut df);
    let mut ds = Buf::<12>::new();
    khz(down as i32 - t.down as i32 - pr.trim as i32, &mut ds);
    if app.dirty(5, Mix::new().b(df.as_bytes()).b(ds.as_bytes()).n(su as u32 | (open as u32) << 8 | (pr.hd as u32) << 9).get()) {
        ferrite_ui::canvas::banded(lcd, 4, 172, 232, 64, BG, |lcd| {
            arrow(lcd, 12, 178, false, if pr.hd { DIM } else { GREEN });
            let wd = lcd.text4(30, 192, df.as_bytes(), &DIG_17, if pr.hd { DIM } else { TEXT }, BG);
            lcd.text4(36 + wd, 191, ds.as_bytes(), &UI_12, SUB, BG);
            if pr.hd {
                lcd.text4(30, 214, b"Downlink muted while you talk (HD)", &UI_12, SUB, BG);
                return;
            }
            lcd.text4(30, 212, b"you", &UI_12, GREEN, BG);
            bars(lcd, 64, 204, 132, 6, 6, (su as i32 * 6 + 8) / 9, GREEN);
            let mut sb = Buf::<4>::new();
            let _ = uwrite!(sb, "S{}", su);
            lcd.text4(200, 212, sb.as_bytes(), &UI_12B, TEXT, BG);
            let say: &[u8] = if open { b"Hearing yourself through it" } else { b"Listening for yourself" };
            lcd.text4(30, 230, say, &UI_12, GREEN, BG);
        });
    }
    let to = app.settings().tx_timeout_s().saturating_sub(secs);
    let mut b = Buf::<48>::new();
    b.set("Doppler on \u{b7} timeout ".as_bytes());
    mmss_after(&mut b, to);
    if app.dirty(10, Mix::new().b(b.as_bytes()).get()) {
        w::field(lcd, 8, 224, 278, 296, 291, b.as_bytes(), const { &w::st(&UI_12, SUB, BG, Align::Left) });
    }
}

// ---- the pointing page (B24, SAT-17 to SAT-19) ----

/// One of the pointing page's two blocks from `y` (azimuth at 24, 86 high; elevation at 112, 84 high): its label,
/// the angle large with its degree ring and `side` after it, and the line under it in green.
#[inline(never)]
fn big_angle(lcd: &mut impl Panel, y: i32, label: &[u8], deg: u32, side: &[u8], under: &[u8]) {
    let h = if y == 24 { 86 } else { 84 };
    ferrite_ui::canvas::banded(lcd, 0, y, 240, h, BG, |lcd| {
        lcd.text4(16, y + 16, label, &UI_12, SUB, BG);
        let mut b = Buf::<4>::new();
        let _ = uwrite!(b, "{}", deg);
        let wd = lcd.text4(14, y + 66, b.as_bytes(), &DIG_46, TEXT, BG);
        deg_ring(lcd, 15 + wd, y + 23, 5, TEXT);
        if !side.is_empty() {
            lcd.text4(34 + wd, y + 64, side, &NAME_22, SUB, BG);
        }
        lcd.text4(16, y + 82, under, &UI_12, GREEN, BG);
    });
}

#[inline(never)]
fn point(app: &mut App, lcd: &mut impl Panel, s: SatScr, live: &LiveAll) {
    if s.page & TX != 0 {
        return pass_page(app, lcd, s, live);
    }
    let Some((p, _, t, pr)) = app.sat_cur() else {
        if app.dirty(15, 1) {
            app.empty(lcd, b"Pass gone", b"BACK for the list");
        }
        return;
    };
    let now = app.sat_now().unwrap_or(0);
    // before the rise: where it rises
    let at = if now < p.aos { p.aos } else { now };
    let l = app.sat_data().look(p.sat, at).unwrap_or_default();
    let l1 = app.sat_data().look(p.sat, at + 1).unwrap_or(l);
    let mut daz = l1.az - l.az;
    if daz > 180.0 {
        daz -= 360.0;
    } else if daz < -180.0 {
        daz += 360.0;
    }
    let az = (l.az + 0.5) as u32 % 360;
    let mut mv = Buf::<32>::new();
    let rate = (daz.abs() * 10.0 + 0.5) as u32;
    if now < p.aos {
        mv.set(b"where it rises");
    } else if rate >= 5 {
        let _ = uwrite!(mv, "moving {}, {}.{}\u{b0} a second", if daz > 0.0 { "right" } else { "left" }, rate / 10, rate % 10);
    } else {
        let _ = uwrite!(mv, "moving {}", if daz > 0.0 { "right" } else { "left" });
    }
    if app.dirty(1, Mix::new().n(az).b(mv.as_bytes()).get()) {
        big_angle(lcd, 24, b"Azimuth \xc2\xb7 point to", az, look::point16(l.az).as_bytes(), mv.as_bytes());
    }
    let el = l.el.max(0.0) as u32;
    let mut em = Buf::<32>::new();
    let top = p.aos + p.tmax as u32;
    if now < top {
        let _ = uwrite!(em, "rising \u{b7} max {}\u{b0} in ", p.max_el);
        mmss_after(&mut em, top - at);
    } else {
        em.set(b"falling");
    }
    if app.dirty(2, Mix::new().n(el).b(em.as_bytes()).get()) {
        big_angle(lcd, 112, b"Elevation \xc2\xb7 tilt up", el, b"", em.as_bytes());
    }
    // SAT-18: the compass bearing; SAT-19: from the sun while it is over 10°
    let decl = app.sat_data().declination();
    let mut c1 = Buf::<12>::new();
    let mut c2 = Buf::<24>::new();
    match decl {
        Some(dc) => {
            let _ = uwrite!(c1, "{}\u{b0}", look::magnetic(l.az, dc));
            let a = dc.unsigned_abs();
            let _ = uwrite!(c2, "magnetic {}\u{b0}{}", (a + 5) / 10, if dc < 0 { "W" } else { "E" });
        }
        None => {
            let _ = uwrite!(c1, "{}\u{b0}", az);
            c2.set(b"true bearing");
        }
    }
    let sun = app.sat.here.map(|(a, o, h)| look::Observer::new(a, o, h).sun(at));
    let mut s1 = Buf::<12>::new();
    let mut s2 = Buf::<24>::new();
    match sun.filter(|s| s.1 > 10.0) {
        Some(sp) => {
            let (across, upd) = look::from_sun((l.az, l.el), sp);
            let _ = uwrite!(s1, "{}\u{b0} {}", across.unsigned_abs(), if across < 0 { "left" } else { "right" });
            let _ = uwrite!(s2, "{}\u{b0} {} it", upd.unsigned_abs(), if upd < 0 { "below" } else { "above" });
        }
        None => {
            s1.set(b"-");
            s2.set(b"sun too low");
        }
    }
    if app.dirty(3, Mix::new().b(c1.as_bytes()).b(c2.as_bytes()).b(s1.as_bytes()).b(s2.as_bytes()).get()) {
        ferrite_ui::canvas::banded(lcd, 0, 200, 240, 62, BG, |lcd| {
            for (i, (h, a, b, c)) in [(&b"On a compass"[..], &c1, &c2, TEXT), (b"From the sun", &s1, &s2, AMBER)].into_iter().enumerate() {
                let x = 8 + i as i32 * 114;
                lcd.frame_r(x, 202, 110, 56, 4, LINE);
                lcd.text4(x + 6, 217, h, &UI_12, SUB, BG);
                lcd.text4(x + 6, 237, a.as_bytes(), &UI_15B, c, BG);
                w::field(lcd, x + 6, 100, 241, 255, 252, b.as_bytes(), const { &w::st(&UI_12, SUB, BG, Align::Left) });
            }
        });
    }
    let (down, _) = crate::sat::corrected(&t, &l, &pr);
    let mut f = Buf::<16>::new();
    let mut fm = Buf::<12>::new();
    mhz(down, true, &mut fm);
    f.set(fm.as_bytes());
    let mut r = Buf::<16>::new();
    if p.up(now) {
        r.set(b"sets ");
        mmss_after(&mut r, p.los() - now);
    } else if now < p.aos {
        r.set(b"rises ");
        mmss_after(&mut r, p.aos - now);
    }
    if app.dirty(4, Mix::new().b(f.as_bytes()).b(r.as_bytes()).get()) {
        ferrite_ui::canvas::banded(lcd, 0, 266, 240, 32, BG, |lcd| {
            lcd.fill(0, 266, 240, 2, LINE);
            arrow(lcd, 14, 276, false, GREEN);
            lcd.text4(30, 289, f.as_bytes(), &UI_15B, TEXT, BG);
            w::field(lcd, 128, 100, 272, 296, 289, r.as_bytes(), &w::st(&UI_15B, if p.up(now) { GREEN } else { AMBER }, BG, Align::Right));
        });
    }
}

/// The readback (LINK-19): what the page shows, as values. `page`, the source's state (SAT-6) and progress, the
/// passes and the selected row; on a pass, its corrected frequencies, tones and choices.
pub(crate) struct Shown {
    pub state: State,
    pub done: u8,
    pub all: u8,
    pub rows: u8,
    pub sel: u8,
    pub top: u8,
}

pub(crate) fn shown_list(app: &App, s: &SatScr) -> Shown {
    let (done, all) = predict_progress(app);
    let rows = shown(app, s.filter).count();
    let sel = sel_row(app, *s);
    Shown { state: app.sat_data().state(), done, all, rows: rows as u8, sel: sel as u8, top: sel.saturating_sub(ROWS - 1) as u8 }
}

/// A list row as the readback gives it: the name, and "rises max min" ("UP" and the time to set while up), the
/// alarm's bell and lead.
pub(crate) fn row_text(app: &App, s: &SatScr, i: usize, name: &mut Buf<16>, value: &mut Buf<32>) -> bool {
    let Some(p) = nth(app, *s, i) else { return false };
    let now = app.sat_now().unwrap_or(0);
    *name = name_of(app, p.sat);
    let mut t = Buf::<8>::new();
    if p.up(now) {
        crate::screens::activity::mmss(p.los() - now, &mut t);
        let _ = uwrite!(value, "UP {}\u{b0} {}", p.max_el, crate::screens::lists::Txt(t.as_bytes()));
    } else {
        hhmm(app, p.aos, &mut t);
        let _ = uwrite!(value, "{} {}\u{b0} {}", crate::screens::lists::Txt(t.as_bytes()), p.max_el, (p.dur as u32 + 30) / 60);
    }
    if app.sat_data().alarm().is_some_and(|a| a.sat == p.sat && a.aos.abs_diff(p.aos) < 120) {
        let _ = uwrite!(value, " bell={}", app.sat.alarm_lead);
    }
    true
}

/// The pass page's frequencies as shown (SAT-8): the downlink and uplink corrected now (at the rise before it).
pub(crate) fn pass_freqs(app: &App) -> Option<(Freq10, Freq10)> {
    let (p, _, t, pr) = app.sat_cur()?;
    let now = app.sat_now()?;
    let at = if p.up(now) || now > p.los() { now } else { p.aos };
    let l = app.sat_data().look(p.sat, at)?;
    Some(crate::sat::corrected(&t, &l, &pr))
}

/// The readback's screen id (LINK-19): the page open, keyed apart.
pub(crate) fn page_id(s: &SatScr) -> &'static str {
    match s.page() {
        _ if s.opt != 0 => "SatOptions",
        _ if s.picker != 0 => "SatAlarm",
        LIST => "SatList",
        _ if s.page & TX != 0 => "SatKeyed",
        POINT => "SatPoint",
        _ => "SatPass",
    }
}
