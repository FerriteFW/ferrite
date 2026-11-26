//! The APRS address book (manual chapter 25, APRS-37 – APRS-41; decision 226): the contacts the store keeps in
//! callsign order, the list compose shows on an empty recipient ([`Book`]: saved, recent, heard and the
//! services), the Contacts screen (C5a, from the Messages options), and the options rows that act on the
//! station, contact or waypoint under them ([`Sel`]).

use super::lists::*;
use super::nav::Dest;
use super::opts::Of;
use super::text::TextFor;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::data::{Contact, Data, Draft, Op};
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ufmt::uwrite;

/// APRS-39: the services, with what each does and the format of a message to it.
pub(crate) const SERVICES: [(&str, &str, &str); 6] = [
    ("SMSGTE", "Text a phone", "@number message"),
    ("EMAIL-2", "Send an email", "address message"),
    ("WXBOT", "Weather for a place", "place"),
    ("ANSRVR", "Group messages", "CQ group message"),
    ("WHO-IS", "Look up a callsign", "callsign"),
    // suggested by Daria, an alpha tester (Nov 9, 2026)
    ("GWALL", "Graffiti wall", "message (? for help)"), // ? answers with its own format
];

/// The service a recipient is, if it is one (APRS-39: picked or typed).
pub(crate) fn service(to: &[u8]) -> Option<usize> {
    SERVICES.iter().position(|s| s.0.as_bytes() == to)
}

/// The groups, in order, as the rows tag them.
pub(crate) const GROUPS: [&str; 4] = ["Saved", "Recent", "Heard", "Services"];
const RECENT: usize = 8;
const HEARD: usize = 20;
/// APRS-38: the newest stored messages looked through for Recent.
const RECENT_FROM: u16 = 32;

/// APRS-38: compose's address book, built when drawn or keyed: the contacts are read in place; Recent and
/// Heard are the callsigns not already above (a callsign shows once, in its first group).
pub(crate) struct Book {
    saved: u16,
    recent: [Buf<10>; RECENT],
    nr: u8,
    /// Heard stations by their index in the heard list.
    heard: [u16; HEARD],
    nh: u8,
}

impl Book {
    pub(crate) fn new(d: &dyn Data) -> Self {
        let mut b = Book { saved: d.contacts(), recent: [Buf::new(); RECENT], nr: 0, heard: [0; HEARD], nh: 0 };
        let saved = |c: &[u8]| d.contact_of(c).is_some();
        for i in 0..d.messages().min(RECENT_FROM) {
            if b.nr as usize == RECENT {
                break;
            }
            if let Some(m) = d.message(i) {
                let p = m.peer.as_bytes();
                if !p.is_empty() && !b.recent[..b.nr as usize].iter().any(|r| r.as_bytes() == p) && !saved(p) {
                    b.recent[b.nr as usize] = m.peer;
                    b.nr += 1;
                }
            }
        }
        for i in 0..d.stations() {
            if b.nh as usize == HEARD {
                break;
            }
            if let Some(s) = d.station(i) {
                let c = s.call.as_bytes();
                if !b.recent[..b.nr as usize].iter().any(|r| r.as_bytes() == c) && !saved(c) {
                    b.heard[b.nh as usize] = i;
                    b.nh += 1;
                }
            }
        }
        b
    }

    fn sizes(&self) -> [u16; 4] {
        [self.saved, self.nr as u16, self.nh as u16, SERVICES.len() as u16]
    }

    pub(crate) fn len(&self) -> u16 {
        self.sizes().iter().sum()
    }

    /// Row `i`'s group, its first row, and how many the group has.
    pub(crate) fn group(&self, i: u16) -> (usize, u16, u16) {
        let mut from = 0;
        for (g, &n) in self.sizes().iter().enumerate() {
            if i < from + n {
                return (g, from, n);
            }
            from += n;
        }
        (3, from - SERVICES.len() as u16, SERVICES.len() as u16)
    }

    /// The row after `i` that starts the next (`d` > 0) or previous group with rows, wrapping (◀▶).
    pub(crate) fn jump(&self, i: u16, d: i32) -> u16 {
        let sizes = self.sizes();
        let (g, _, _) = self.group(i);
        let mut k = g;
        loop {
            k = (k as i32 + d.signum()).rem_euclid(4) as usize;
            if sizes[k] > 0 || k == g {
                return sizes[..k].iter().sum();
            }
        }
    }

    /// Row `i`: its group, the callsign, and what follows it (a name, an age, what a service does).
    pub(crate) fn row(&self, d: &dyn Data, i: u16) -> Option<(usize, Buf<10>, Buf<20>)> {
        let (g, from, _) = self.group(i);
        let k = i - from;
        let mut sub = Buf::<20>::new();
        let call = match g {
            0 => {
                let c = d.contact(k)?;
                sub.push(c.name.as_bytes());
                c.call
            }
            1 => self.recent[k as usize],
            2 => {
                let s = d.station(self.heard[k as usize])?;
                let mut a = Buf::<12>::new();
                age_text(s.ago_s, &mut a);
                sub.push(a.as_bytes());
                s.call
            }
            _ => {
                let (call, what, _) = SERVICES[k as usize];
                sub.push(what.as_bytes());
                Buf::from(call.as_bytes())
            }
        };
        Some((g, call, sub))
    }
}

// ---- the Contacts screen (APRS-40) ----

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Contacts {
    pub cur: Cursor,
}

const ALL: fn(u16) -> bool = |_| true;

/// A message to `call`: compose with it as the recipient.
pub(crate) fn write_to(app: &mut App, d: &dyn Data, call: &[u8]) {
    let mut to = Buf::<9>::new();
    to.set(call);
    d.request(Op::Draft(Draft { to, ..d.draft() }));
    app.open(Dest::Compose);
}

/// The heard list's index of `call`, if heard.
fn heard_at(d: &dyn Data, call: &[u8]) -> Option<u16> {
    (0..d.stations()).find(|&i| d.station(i).is_some_and(|s| s.call.as_bytes() == call))
}

impl View for Contacts {
    fn options(self, app: &mut App) -> bool {
        app.open_screen(Of::Contacts.screen());
        true
    }

    fn name(self) -> &'static str {
        "Contacts"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Contacts");
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let d = store.data();
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => self.knob(app, if e.key == Key::Up { -1 } else { 1 }, store),
            (Key::Menu, Short) => {
                if let Some(c) = d.contact(self.cur.sel) {
                    write_to(app, d, c.call.as_bytes());
                }
            }
            (Key::Hash, Short) => {
                app.contact_call.clear();
                super::text::open(app, TextFor::ContactCall, store);
            }
            (Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        let cur = self.cur.step(d, store.data().contacts(), ROWS2, &ALL);
        app.screen = Screen::Contacts(Contacts { cur });
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let data = cx.store.data();
        app.page(lcd, 0u8);
        if !app.gate((self, data.version())) {
            return;
        }
        let n = data.contacts();
        if n == 0 {
            return app.empty(lcd, b"No contacts", b"# adds one");
        }
        let mut l = Buf::<24>::new();
        let _ = uwrite!(l, "{} saved \u{b7} {} max", n, CONTACT_MAX);
        app.list_head(lcd, l.as_bytes(), b"");
        let fix = data.fix();
        let rows = self.cur.rows(n, ROWS2, &ALL, |r, i| {
            if let Some(c) = data.contact(i) {
                let st = heard_at(data, c.call.as_bytes()).and_then(|k| data.station(k));
                let mut dist = Buf::<12>::new();
                match (st.and_then(|s| s.pos), fix) {
                    (Some(p), Some(f)) => dist_text(dist_bearing(f.pos, p).0, &mut dist),
                    _ => dist.set(b"-"),
                }
                let r2 = Row2 {
                    tag: st.map_or(b"-".as_slice(), |s| s.sym.name().as_bytes()),
                    title: c.call.as_bytes(),
                    big: true,
                    right: dist.as_bytes(),
                    sub: c.name.as_bytes(),
                    selected: i == self.cur.sel,
                    ..Row2::default()
                };
                app.row2(lcd, r, &r2);
            }
        });
        app.rows_end(lcd, rows, ROWS2, ROW2_H);
        app.list_cue(lcd, self.cur, n, ROWS2, ROW2_H, rows, &ALL);
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[("OK", "Message"), ("#", "New"), ("Hold MENU", "More")])
    }
}

/// APRS-37: the book's size (the store's `logs::CONTACT_MAX`).
pub const CONTACT_MAX: u16 = 1016;

// ---- options rows on the thing under them (APRS-40, APRS-41, GPS-36) ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sel {
    /// A message to the station or contact.
    Message,
    /// Its name typed: a new contact for a station, or the name of a contact.
    Name,
    /// The station's last position as a waypoint (GPS-20).
    Waypoint,
    Delete,
    /// The contact's station page, when it is in the heard list.
    Station,
    /// The waypoint as the fixed position (GPS-36).
    Fixed,
}

/// Runs `s` on the screen the options page came from (already back on it).
pub(crate) fn sel(app: &mut App, s: Sel, store: &impl ChannelStore) {
    let d = store.data();
    let call = match app.screen {
        Screen::Aprs(a) if a.station => d.station(a.cur.sel).map(|st| st.call),
        Screen::Contacts(c) => d.contact(c.cur.sel).map(|c| c.call),
        _ => None,
    };
    match (s, app.screen) {
        (Sel::Fixed, Screen::Gps(g)) => match d.waypoint(g.cur.sel) {
            Some(w) => {
                (app.set.fixed, app.set.fixed_on, app.set.fixed_alt) = (w.pos, true, 0);
                let note: &[u8] = if app.set.aprs_fixed { w.name.as_bytes() } else { b"used when there's no fix" };
                app.show_toast(b"Fixed position set", note, false);
            }
            None => app.show_toast(b"Waypoint", b"none selected", false),
        },
        (Sel::Waypoint, Screen::Aprs(a)) => super::aprs::save_waypoint(app, d, a.cur.sel),
        // GPS-10.2: the selected waypoint deleted (the list or the one being gone to); back on the list.
        (Sel::Delete, Screen::Gps(g)) => {
            let i = g.cur.sel;
            let (name, done) = match d.waypoint(i) {
                Some(w) => (w.name, d.request(Op::DeleteWaypoint(i))),
                None => (Buf::from(&b"Waypoint"[..]), false),
            };
            app.screen = Screen::Gps(super::gps::Gps { page: super::gps::Page::Waypoints, ..g });
            app.show_toast(name.as_bytes(), if done { b"deleted" } else { b"not deleted" }, false);
        }
        (_, _) if call.is_none() => app.show_toast(b"Contacts", b"nothing selected", false),
        (Sel::Message, _) => write_to(app, d, call.unwrap_or_default().as_bytes()),
        (Sel::Name, _) => {
            app.contact_call.set(call.unwrap_or_default().as_bytes());
            super::text::open(app, TextFor::ContactName, store);
        }
        (Sel::Delete, _) => {
            let c = call.unwrap_or_default();
            let done: &[u8] = if d.request(Op::DeleteContact(c)) { b"deleted" } else { b"not deleted" };
            app.show_toast(c.as_bytes(), done, false);
        }
        (Sel::Station, _) => match heard_at(d, call.unwrap_or_default().as_bytes()) {
            Some(i) => app.open_screen(Screen::Aprs(super::aprs::Aprs { cur: Cursor { sel: i, top: i }, station: true })),
            None => app.show_toast(call.unwrap_or_default().as_bytes(), b"not in the heard list", false),
        },
        _ => {}
    }
}

/// Text entry's end for a contact (APRS-37): the callsign checked, then its name typed; or the name saved.
pub(crate) fn typed(app: &mut App, to: TextFor, t: &[u8], store: &impl ChannelStore) {
    if to == TextFor::ContactCall {
        let ok = t.len() >= 3 && t.iter().all(|&c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'-');
        if !ok {
            return app.show_toast(b"Callsign", b"like VE3KPW-9", false);
        }
        app.contact_call.set(t);
        app.back();
        return super::text::open(app, TextFor::ContactName, store);
    }
    let c = Contact { call: app.contact_call, name: Buf::from(t) };
    app.back();
    if store.data().request(Op::SaveContact(c)) {
        app.show_toast(b"Contact saved", c.call.as_bytes(), false);
    } else {
        app.show_toast(b"1016 contacts", b"delete one first", false);
    }
}
