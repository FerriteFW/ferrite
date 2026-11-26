//! APRS heard (MENU 0 5 1; manual chapter 25, APRS-3 to APRS-5; AprsHeard mock-up) and one station
//! (OK on a station; AprsStation mock-up): C3, C4. The heard list lives in the APRS task's RAM (APRS-4) and
//! is read a row at a time. Distances and bearings are from the GPS fix; a station without a position shows
//! "no position" and a dash (APRS-5).
//!
//! On the station: OK writes it a message, # saves it as a waypoint, ▲▼ step through the list.
//!
//! Sent (the APRS list's 3; APRS-53): our beacons since power-on, newest first, each with its age and the
//! digipeaters heard repeating it (C3a). Traffic (the APRS list's 8; APRS-59): every packet heard lately, a row
//! each, newest first (C3b).

use super::lists::*;
use super::nav::Dest;
use super::opts::Of;
use super::shared::*;
use super::table::{Table, Val};
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::data::{Data, Draft, Op, SentBeacon, Station};
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_core::msg::{Device, LiveAll};
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// GPS-20: station `i`'s last position as a waypoint named with its callsign.
pub(crate) fn save_waypoint(app: &mut App, d: &dyn Data, i: u16) {
    match d.station(i) {
        Some(s) if s.pos.is_some() && d.request(Op::SaveStation(i)) => app.show_toast(s.call.as_bytes(), b"saved as a waypoint", false),
        Some(s) if s.pos.is_none() => app.show_toast(s.call.as_bytes(), b"has no position", false),
        Some(s) => app.show_toast(s.call.as_bytes(), b"not saved", false),
        None => {}
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Aprs {
    pub cur: Cursor,
    /// The selected station's page (C4).
    pub station: bool,
}

const ALL: fn(u16) -> bool = |_| true;

impl View for Aprs {
    fn options(self, app: &mut App) -> bool {
        app.open_screen(if self.station { Of::Station } else { Of::Heard }.screen());
        true
    }

    fn name(self) -> &'static str {
        if self.station { "APRS station" } else { "APRS heard" }
    }

    fn mockup(self) -> Option<&'static str> {
        Some(if self.station { "AprsStation.png" } else { "AprsHeard.png" })
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(if self.station { b"APRS" } else { b"Heard" });
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let d = store.data();
        let st = d.station(self.cur.sel);
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => self.knob(app, if e.key == Key::Up { -1 } else { 1 }, store),
            (Key::Menu, Short) if !self.station && st.is_some() => app.screen = Screen::Aprs(Aprs { station: true, ..self }),
            // OK on the station: a message to it (A16).
            (Key::Menu, Short) => {
                if let Some(s) = st {
                    let mut to = Buf::<9>::new();
                    to.set(s.call.as_bytes());
                    d.request(Op::Draft(Draft { to, ..d.draft() }));
                    app.open(Dest::Compose);
                }
            }
            (Key::Hash, Short) if self.station => save_waypoint(app, d, self.cur.sel),
            (Key::Exit, Short) if self.station => app.screen = Screen::Aprs(Aprs { station: false, ..self }),
            (Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        let cur = self.cur.step(d, store.data().stations(), ROWS2, &ALL);
        app.screen = Screen::Aprs(Aprs { cur, ..self });
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let data = cx.store.data();
        app.page(lcd, self.station);
        if !app.gate((self, data.version(), cx.dev.clock_min)) {
            return;
        }
        if self.station {
            match data.station(self.cur.sel) {
                Some(mut s) => {
                    last_message(&mut s, data, cx.dev.clock_min);
                    station(app, lcd, &s, data, cx.live, cx.dev)
                }
                None => app.empty(lcd, b"No such station", b"The heard list changed"),
            }
            return;
        }
        let n = data.stations();
        let fix = data.fix();
        let mut l = Buf::<16>::new();
        let _ = uwrite!(l, "{} heard", n);
        let mut r = Buf::<32>::new();
        let mut f = Buf::<16>::new();
        mhz_text(app.aprs_freq(), &mut f);
        let _ = uwrite!(r, "newest first \u{b7} {}", Txt(f.as_bytes()));
        app.list_head(lcd, l.as_bytes(), r.as_bytes());
        let rows = self.cur.rows(n, ROWS2, &ALL, |row, i| {
            if let Some(s) = data.station(i) {
                let (mut dist, mut sub, mut age) = (Buf::<12>::new(), Buf::<40>::new(), Buf::<12>::new());
                age_text(s.ago_s, &mut age);
                // APRS-43: a saved contact's name leads the line
                if let Some(c) = data.contact_of(s.call.as_bytes()) {
                    let _ = uwrite!(sub, "{} \u{b7} ", Txt(c.name.as_bytes()));
                }
                let _ = uwrite!(sub, "{} \u{b7} ", Txt(age.as_bytes()));
                let arrow = match (s.pos, fix) {
                    (Some(p), Some(f)) => {
                        let (m, deg) = dist_bearing(f.pos, p);
                        dist_text(m, &mut dist);
                        Some(Some(deg))
                    }
                    (Some(_), None) => None,
                    (None, _) => Some(None), // APRS-5
                };
                if s.pos.is_none() {
                    sub.push(b"no position");
                } else if s.via.is_empty() {
                    sub.push(b"direct");
                } else {
                    // the first digipeater, without its used mark
                    let hop = s.via.as_bytes().split(|&c| c == b',').next().unwrap_or(b"");
                    sub.push(b"via ");
                    sub.push(hop.strip_suffix(b"*").unwrap_or(hop));
                }
                let r = Row2 {
                    tag: s.sym.name().as_bytes(),
                    title: s.call.as_bytes(),
                    big: true,
                    right: dist.as_bytes(),
                    arrow,
                    sub: sub.as_bytes(),
                    selected: i == self.cur.sel,
                    ..Row2::default()
                };
                app.row2(lcd, row, &r);
            }
        });
        app.rows_end(lcd, rows, ROWS2, ROW2_H);
        app.list_cue(lcd, self.cur, n, ROWS2, ROW2_H, rows, &ALL);
        if n == 0 {
            app.empty(lcd, b"No stations heard yet", b"APRS listens on the APRS frequency");
        }
    }

    fn hints(self, _: &App) -> Hints {
        if self.station {
            Hints::new(&[("OK", "Msg"), ("#", "Wpt"), ("Hold MENU", "Contact")]) // APRS-41: hold MENU: Save contact
        } else {
            Hints::new(&[(ARROWS_UD, "Move"), ("OK", "Open"), ("Hold MENU", "More")])
        }
    }
}

/// C3a: our beacons (APRS-53).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Sent {
    pub cur: Cursor,
}

/// APRS-53: a Sent row's text: the age ("4 min ago"), who repeated it ("by VE3RSB-1, VE3OTW-10", or "not heard
/// back"), and whether anyone did. The screen and the readback both say it this way.
pub(crate) fn sent_row(e: &SentBeacon) -> (Buf<16>, Buf<32>, bool) {
    let (mut age, mut ago, mut by) = (Buf::<12>::new(), Buf::<16>::new(), Buf::<32>::new());
    age_text(e.ago_s, &mut age);
    let _ = uwrite!(ago, "{} ago", Txt(age.as_bytes()));
    for (k, c) in e.by.iter().filter(|c| !c.is_empty()).enumerate() {
        by.push(if k == 0 { b"by ".as_slice() } else { b", " });
        by.push(c.as_bytes());
    }
    let heard = !by.is_empty();
    if !heard {
        by.set(b"not heard back");
    }
    (ago, by, heard)
}

impl View for Sent {
    fn name(self) -> &'static str {
        "APRS sent"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Sent");
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => self.knob(app, if e.key == Key::Up { -1 } else { 1 }, store),
            (Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        let cur = self.cur.step(d, store.data().beacons_sent(), ROWS2, &ALL);
        app.screen = Screen::AprsSent(Sent { cur });
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let data = cx.store.data();
        app.page(lcd, ());
        if !app.gate((self, data.version(), cx.dev.clock_min)) {
            return;
        }
        let n = data.beacons_sent();
        let mut l = Buf::<16>::new();
        let _ = uwrite!(l, "{} sent", n);
        app.list_head(lcd, l.as_bytes(), b"newest first");
        let rows = self.cur.rows(n, ROWS2, &ALL, |row, i| {
            if let Some(e) = data.beacon_sent(i) {
                let (ago, by, heard) = sent_row(&e);
                // APRS-53.2: one not heard back in amber
                let r = Row2 { title: ago.as_bytes(), sub: by.as_bytes(), sub_c: if heard { 0 } else { AMBER }, selected: i == self.cur.sel, ..Row2::default() };
                app.row2(lcd, row, &r);
            }
        });
        app.rows_end(lcd, rows, ROWS2, ROW2_H);
        app.list_cue(lcd, self.cur, n, ROWS2, ROW2_H, rows, &ALL);
        if n == 0 {
            app.empty(lcd, b"No beacons sent since power-on", b"Beacons show here as they go");
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[(ARROWS_UD, "Move"), ("BACK", "Back")])
    }
}

/// C3b: the packets heard lately, every kind (APRS-59).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Traffic {
    pub cur: Cursor,
}

/// The heard list's index of `call` (RAM only, PWR-26.13: the heard list's own fields).
fn heard_index(d: &dyn Data, call: &[u8]) -> Option<(u16, Station)> {
    (0..d.stations()).find_map(|i| d.station_ram(i).filter(|s| s.call.as_bytes() == call).map(|s| (i, s)))
}

/// APRS-59.6: a Traffic row, as the screen and the readback say it: line one the sender (`VE7GPL-1`, a message's
/// `VE7GPL-1 > TEST`) with the age at the right; line two what it was: a message's text in quotes, `ack 12`,
/// `position · 2.1 km NE` (the heard list's position from the GPS fix), `object CLUBNET`, then the times heard in
/// a row (`· x3`, `· x8+`).
#[inline(never)]
pub(crate) fn traffic_row(r: &crate::aprs_traffic::Rec, d: &dyn Data) -> (Buf<24>, Buf<48>, Buf<12>) {
    use crate::aprs_traffic::{Kind, COUNT_MAX};
    let (mut title, mut sub, mut age) = (Buf::<24>::new(), Buf::<48>::new(), Buf::<12>::new());
    title.push(r.src.as_bytes());
    let addressed = matches!(r.kind, Kind::Message | Kind::Ack | Kind::Rej | Kind::Query) && !r.to.is_empty();
    if addressed {
        let _ = uwrite!(title, " > {}", Txt(r.to.as_bytes()));
    }
    match r.kind {
        Kind::Message => {
            let _ = uwrite!(sub, "\"{}\"", Txt(r.text.as_bytes()));
        }
        Kind::Ack | Kind::Rej | Kind::Object | Kind::Item | Kind::ThirdParty | Kind::Query => {
            sub.push(r.kind.name().as_bytes());
            let what = if r.kind == Kind::ThirdParty { r.to.as_bytes() } else { r.text.as_bytes() };
            if !what.is_empty() {
                let _ = uwrite!(sub, " {}", Txt(what));
            }
        }
        Kind::Position => {
            sub.push(b"position");
            if let (Some((_, s)), Some(f)) = (heard_index(d, r.src.as_bytes()), d.fix()) {
                if let Some(p) = s.pos {
                    let (m, deg) = dist_bearing(f.pos, p);
                    let mut t = Buf::<12>::new();
                    dist_text(m, &mut t);
                    let _ = uwrite!(sub, " \u{b7} {} {}", Txt(t.as_bytes()), compass_name(deg));
                }
            }
        }
        k => sub.push(k.name().as_bytes()),
    }
    if r.count > 1 {
        let _ = uwrite!(sub, " \u{b7} x{}{}", r.count, if r.count == COUNT_MAX { "+" } else { "" });
    }
    age_text(r.ago_s, &mut age);
    (title, sub, age)
}

impl View for Traffic {
    fn name(self) -> &'static str {
        "APRS traffic"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Traffic");
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let d = store.data();
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => self.knob(app, if e.key == Key::Up { -1 } else { 1 }, store),
            // APRS-59.7: OK opens the station when it is in the heard list; else a message shows whole in a toast
            (Key::Menu, Short) => {
                let Some(r) = d.traffic().and_then(|t| t.get(self.cur.sel)) else { return };
                if let Some((i, _)) = heard_index(d, r.src.as_bytes()) {
                    app.open_screen(Screen::Aprs(Aprs { cur: Cursor { sel: i, top: i }, station: true }));
                } else if !r.text.is_empty() {
                    let mut b = Buf::<48>::new();
                    let _ = uwrite!(b, "to {}: {}", Txt(r.to.as_bytes()), Txt(r.text.as_bytes()));
                    app.show_toast(r.src.as_bytes(), b.as_bytes(), false);
                } else {
                    app.show_toast(r.src.as_bytes(), b"is no longer in the heard list", false);
                }
            }
            (Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        let n = store.data().traffic().map_or(0, |t| t.len());
        let cur = self.cur.step(d, n, ROWS2, &ALL);
        app.screen = Screen::AprsTraffic(Traffic { cur });
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let data = cx.store.data();
        app.page(lcd, ());
        if !app.gate((self, data.version(), cx.dev.clock_min)) {
            return;
        }
        let t = data.traffic();
        let n = t.map_or(0, |t| t.len());
        let mut l = Buf::<16>::new();
        let _ = uwrite!(l, "{} heard", n);
        app.list_head(lcd, l.as_bytes(), b"newest first");
        let rows = self.cur.rows(n, ROWS2, &ALL, |row, i| {
            if let Some(r) = t.and_then(|t| t.get(i)) {
                let (title, sub, age) = traffic_row(&r, data);
                let r = Row2 { title: title.as_bytes(), big: true, right: age.as_bytes(), sub: sub.as_bytes(), selected: i == self.cur.sel, ..Row2::default() };
                app.row2(lcd, row, &r);
            }
        });
        app.rows_end(lcd, rows, ROWS2, ROW2_H);
        app.list_cue(lcd, self.cur, n, ROWS2, ROW2_H, rows, &ALL);
        if n == 0 {
            app.empty(lcd, b"Nothing heard yet", b"Every packet shows here, RAM only");
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[(ARROWS_UD, "Move"), ("OK", "Open"), ("BACK", "Back")])
    }
}

fn text(b: &mut Buf<40>, s: &[u8]) -> u16 {
    b.set(s);
    TEXT
}

/// C4's rows.
const ROWS: [(&str, Val<Station>); 8] = [
    ("Position", |c, b| match c.x.pos {
        Some(p) => {
            let mut pb = Buf::<32>::new();
            pos_text(p, c.app.set.coord_fmt, &mut pb);
            text(b, pb.as_bytes())
        }
        None => text(b, b"none"),
    }),
    ("Course", |c, b| match (c.x.course, crate::aprs::wx(c.x.comment.as_bytes())) {
        // APRS-23: a weather station's course and speed are its wind
        (_, Some(w)) => {
            b.push(b"wind ");
            match w.dir {
                Some(d) => {
                    let _ = uwrite!(b, "{}\u{b0}", d);
                }
                None => b.push(b"-"),
            }
            if let Some(v) = w.wind {
                b.push(" \u{b7} ".as_bytes());
                speed(c.app.set.units, v, b);
            }
            if let Some(g) = w.gust {
                b.push(b", gust ");
                speed(c.app.set.units, g, b);
            }
            TEXT
        }
        (Some((d, v)), None) => {
            let _ = uwrite!(b, "{}\u{b0} \u{b7} {} km/h", d, v);
            TEXT
        }
        (None, None) => text(b, b"-"),
    }),
    ("Weather", |c, b| match crate::aprs::wx(c.x.comment.as_bytes()) {
        Some(w) => {
            weather(c.app.set.units, &w, b);
            TEXT
        }
        None => text(b, b"-"),
    }),
    ("Altitude", |c, b| match c.x.alt_m {
        Some(a) => {
            let _ = uwrite!(b, "{} m", a);
            TEXT
        }
        None => text(b, b"-"),
    }),
    ("Path", |c, b| {
        let s = c.x;
        if s.via.is_empty() {
            b.push(b"direct");
        }
        for (k, hop) in s.via.as_bytes().split(|&ch| ch == b',').filter(|h| !h.is_empty()).enumerate() {
            if k > 0 {
                b.push(b", ");
            }
            b.push(hop);
        }
        // APRS-44: heard last as FX.25
        if let Some(n) = s.fx25 {
            let _ = uwrite!(b, " \u{b7} FX.25, {} fixed", n);
        }
        TEXT
    }),
    ("Heard", |c, b| {
        let s = c.x;
        let mut age = Buf::<12>::new();
        age_text(s.ago_s, &mut age);
        let _ = uwrite!(b, "{} ago \u{b7} {} {}", Txt(age.as_bytes()), s.times, if s.times == 1 { "time" } else { "times" });
        TEXT
    }),
    ("Comment", |c, b| {
        // a weather station's comment is what follows its weather fields
        let t = c.x.comment.as_bytes();
        let t = crate::aprs::wx(t).map_or(t, |w| t[w.rest..].trim_ascii());
        b.set(t);
        // APRS-5.2: the radio its Mic-E codes named, after the comment
        if let Some((_, model)) = c.x.device.and_then(|d| uaprs::parse::MIC_E_DEVICES.get(d as usize)) {
            if !b.is_empty() {
                b.push(" \u{b7} ".as_bytes());
            }
            b.push(model.as_bytes());
        }
        if b.is_empty() {
            b.set(b"-");
        }
        TEXT
    }),
    ("Message", |c, b| {
        let s = c.x;
        if s.msg_to.is_empty() {
            return text(b, b"-");
        }
        let mut clock = Buf::<12>::new();
        if let Some(m) = s.msg_min {
            clock_text(Some(m), 0, &mut clock);
        }
        // APRS-28.1: who it was addressed to, first
        if !s.msg_to.is_empty() {
            let _ = uwrite!(b, "to {}: ", Txt(s.msg_to.as_bytes()));
        }
        let _ = uwrite!(b, "\"{}\" {}", Txt(s.msg.as_bytes()), Txt(clock.as_bytes()));
        TEXT
    }),
];

/// The station screen's Message line (APRS-28): the newest message the station was heard sending, to anyone, that
/// Traffic still holds (APRS-59), its addressee and the clock time it came; none ("-") once it has left Traffic.
pub fn last_message(s: &mut crate::data::Station, data: &dyn crate::data::Data, clock_min: Option<u16>) {
    let Some(m) = data.traffic().and_then(|t| t.last_message(s.call.as_bytes())) else {
        (s.msg, s.msg_to, s.msg_min) = (Buf::new(), Buf::new(), None);
        return;
    };
    (s.msg, s.msg_to) = (m.text, m.to);
    s.msg_min = clock_min.map(|c| (c as i32 - (m.ago_s / 60) as i32).rem_euclid(1440) as u16);
}

/// A speed sent in mph, in the units of 0 6 4 (metric km/h, imperial mph, nautical knots).
fn speed(units: u8, mph: u16, b: &mut Buf<40>) {
    let mph = mph as u32;
    let _ = match units {
        1 => uwrite!(b, "{} mph", mph),
        2 => uwrite!(b, "{} kn", (mph * 869 + 500) / 1000),
        _ => uwrite!(b, "{} km/h", (mph * 1609 + 500) / 1000),
    };
}

/// APRS-23: temperature, humidity, pressure and the last hour's rain, as they are sent, in 0 6 4's units.
fn weather(units: u8, w: &crate::aprs::Wx, b: &mut Buf<40>) {
    let sep = |b: &mut Buf<40>| {
        if !b.is_empty() {
            b.push(" \u{b7} ".as_bytes());
        }
    };
    if let Some(f) = w.temp_f {
        sep(b);
        let _ = if units == 1 { uwrite!(b, "{}\u{b0}F", f) } else { uwrite!(b, "{}\u{b0}C", ((f as i32 - 32) * 5 - if f < 32 { 4 } else { -4 }) / 9) };
    }
    if let Some(h) = w.hum {
        sep(b);
        let _ = uwrite!(b, "{}%", h);
    }
    if let Some(p) = w.baro {
        sep(b);
        let _ = uwrite!(b, "{} hPa", (p + 5) / 10);
    }
    if let Some(r) = w.rain.filter(|&r| r > 0) {
        sep(b);
        let _ = if units == 1 { uwrite!(b, "rain {}.{}\"", r / 100, ferrite_ui::text::Zero(r as u32 % 100, 2)) } else { uwrite!(b, "rain {} mm", (r as u32 * 254 + 500) / 1000) };
    }
    if b.is_empty() {
        b.set(b"-");
    }
}

const STATION: Table<Station> = Table { rows: &ROWS, go: &[], y: 122, h: 0, style: w::KV_DETAIL };

/// C4: one station.
fn station(app: &mut App, lcd: &mut impl Panel, s: &Station, data: &dyn Data, live: &LiveAll, dev: &Device) {
    let fix = data.fix();
    let where_ = match (s.pos, fix) {
        (Some(p), Some(f)) => Some(dist_bearing(f.pos, p)),
        _ => None,
    };
    let moving = s.course.is_some_and(|c| c.1 > 0);
    if app.dirty(R_TOP, Mix::new().h(&s.call).any(&s.sym).h(&moving).h(&where_).get()) {
        ferrite_ui::canvas::banded(lcd, 0, 20, 156, 102, BG, |lcd| {
            lcd.text4(8, 42, s.call.as_bytes(), &UI_15B, TEXT, BG);
            let mut t = Buf::<24>::new();
            t.push(s.sym.name().as_bytes());
            t.push(" \u{b7} ".as_bytes());
            t.push(if moving { b"moving".as_slice() } else if s.course.is_some() { b"stopped" } else { b"fixed" });
            lcd.text4(8, 61, t.as_bytes(), &UI_12, SUB, BG);
            match where_ {
                Some((m, deg)) => {
                    // the distance big: "4.2" in the digit font, the unit beside it
                    let mut d = Buf::<12>::new();
                    dist_text(m, &mut d);
                    let (num, unit) = d.as_bytes().split_at(d.as_bytes().iter().position(|&c| c == b' ').unwrap_or(d.len()));
                    let x = 8 + lcd.text4(8, 98, num, &DIG_28, TEXT, BG);
                    lcd.text4(x + 4, 98, unit.trim_ascii_start(), &UI_12B, TEXT, BG);
                    let mut b = Buf::<12>::new();
                    let _ = uwrite!(b, "{} {}\u{b0}", compass_name(deg), deg);
                    lcd.text4(8, 117, b.as_bytes(), &UI_12B, TEXT, BG);
                }
                None => {
                    lcd.text4(8, 98, b"-", &DIG_28, SUB, BG);
                    lcd.text4(8, 117, if s.pos.is_none() { b"No position".as_slice() } else { b"No GPS fix here" }, &UI_12, SUB, BG);
                }
            }
            // DISP-31, DISP-25: the line under the heading, in its band (y 121), not on the table's first row (122):
            // there each row redraw erased it, and drawing it again after was flicker
            lcd.fill(0, 121, 156, 1, LINE);
        });
    }
    if app.dirty(R_NOTE, Mix::new().h(&where_.map(|w| w.1)).get()) {
        compass(lcd, 194, 71, 33, where_.map(|w| w.1));
    }
    app.table(lcd, &STATION, live, dev, s, None);
    if app.dirty(R_HELP, 1) {
        lcd.fill(156, 121, 84, 1, LINE); // the line's right part, beside the compass (nothing else paints y 121 there)
    }
}
