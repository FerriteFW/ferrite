//! A screen's own options (MENU-25; D4): hold MENU on the event log, APRS heard, the station, messages, the GPS
//! screens or the band scope. Each list of options is a table of rows: a label and what OK does (a request to
//! the record's owner, the same after a confirm (MENU-23), or a screen). One style for every options page (FM
//! broadcast and HF mode draw theirs the same way): numbered rows, digits jump, "0 Full menu" opens advanced
//! settings, the help box names the page. BACK returns. Screens without options answer hold MENU with a toast
//! ([`crate::screens::View::options`]).

use super::nav::Dest;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use super::contacts::Sel;
use crate::data::Op;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets::Row;

/// Whose options.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Of {
    Log,
    Heard,
    Messages,
    Gps,
    Waypoints,
    /// The band scope (SCOPE): its settings list.
    Scope,
    /// The channel browser: show hidden channels (decision 63).
    Channels,
    /// Home's APRS card (APRS-17).
    AprsCard,
    /// A station's page (APRS-41) and the Contacts screen (APRS-40).
    Station,
    Contacts,
    /// One of our messages selected (APRS-13.1, APRS-13.2): being tried, or ended without an ack.
    MsgSending,
    MsgEnded,
    /// Any other message selected (one to us, or ours acked or rejected): Delete first (APRS-46.1).
    Msg,
}

impl Of {
    pub fn screen(self) -> Screen {
        Screen::Options(Options { of: self, row: 0, confirm: false })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Simple {
    ReadAll,
    ClearLog,
    MessagesReadAll,
    ClearMessages,
    MarkHere,
    BeaconNow,
    ClearHeard,
}

impl Simple {
    fn op(self) -> Op {
        match self {
            Simple::ReadAll => Op::ReadAll,
            Simple::ClearLog => Op::ClearLog,
            Simple::MessagesReadAll => Op::MessagesReadAll,
            Simple::ClearMessages => Op::ClearMessages,
            Simple::MarkHere => Op::MarkHere,
            Simple::BeaconNow => Op::BeaconNow,
            Simple::ClearHeard => Op::ClearHeard,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Act {
    /// A small kind, not the `Op` itself: `Op` is about 96 bytes (it carries drafts and buffers), and each one
    /// in a table was that size in flash (the flash audit's #10, Nov 1).
    Do(Simple),
    /// Asks first (MENU-23).
    Confirm(Simple),
    Open(Dest),
    /// Show hidden channels in the lists, or not (a setting, kept).
    ShowHidden,
    /// A waypoint typed as a grid square (GPS-21): text entry.
    Grid,
    /// The arrival alarm's next radius (GPS-12, 0 6 7): off, 50 m ... 1 km.
    Arrival,
    /// APRS-13.6: message retries, the next of 0 (off) to 4.
    Retries,
    /// On the station, contact or waypoint under the page; true asks first (MENU-23).
    Sel(super::contacts::Sel, bool),
    /// On the message selected under the page: stop sending it (APRS-13.1), or send it again (APRS-13.2).
    MsgStop,
    MsgResend,
    /// On the message selected under the page: delete it, after a confirm (APRS-46.1, MENU-23).
    MsgDelete,
}

pub(crate) type Opt = (&'static str, Act, crate::text::Txt);

/// The rows: label, action, help line.
const LOG: [Opt; 3] = [
    ("Mark all read", Act::Do(Simple::ReadAll), txt!("Clears the log's new count")),
    ("Clear the log", Act::Confirm(Simple::ClearLog), txt!("All events, unread ones too")),
    ("Decoder settings", Act::Open(Dest::Group(4)), txt!("Pagers, DTMF, popups, weather")),
];
const MESSAGES: [Opt; 7] = [
    ("Write a message", Act::Open(Dest::Compose), txt!("Recipient, then the text")),
    ("Quick messages", Act::Open(Dest::QuickMessages), txt!("The nine shown on an empty message")),
    ("Contacts", Act::Open(Dest::Contacts), txt!("Saved stations and their names")),
    ("Mark all read", Act::Do(Simple::MessagesReadAll), txt!("Clears the unread count")), // APRS-46
    ("Delete all messages", Act::Confirm(Simple::ClearMessages), txt!("Inbox and sent, unread too")), // APRS-46
    ("APRS settings", Act::Open(Dest::Group(5)), txt!("Ringer, path, beacons")),
    // APRS-13.6: last, so the message pages' rows (MESSAGES[0..=5]) keep their digits
    ("Message retries", Act::Retries, txt!("Tries after the first, until an ack")),
];
/// APRS-46.1: deletes the message under the page.
const MSG_DELETE: Opt = ("Delete message", Act::MsgDelete, txt!("This one only, either way"));
/// APRS-13.1, APRS-13.2, APRS-46.1: the messages' rows under one of ours that is being tried, or ended without an
/// ack, or under any other message.
const MSG_SENDING: [Opt; 8] = [
    ("Stop sending", Act::MsgStop, txt!("No more tries; it stays, cancelled")),
    MSG_DELETE,
    MESSAGES[0],
    MESSAGES[1],
    MESSAGES[2],
    MESSAGES[3],
    MESSAGES[4],
    MESSAGES[5],
];
const MSG_ENDED: [Opt; 8] = [
    ("Send again", Act::MsgResend, txt!("As a new message, tried afresh")),
    MSG_DELETE,
    MESSAGES[0],
    MESSAGES[1],
    MESSAGES[2],
    MESSAGES[3],
    MESSAGES[4],
    MESSAGES[5],
];
const MSG: [Opt; 7] = [MSG_DELETE, MESSAGES[0], MESSAGES[1], MESSAGES[2], MESSAGES[3], MESSAGES[4], MESSAGES[5]];
const GPS: [Opt; 3] = [
    ("Mark here", Act::Do(Simple::MarkHere), txt!("A waypoint at this fix")),
    ("Waypoints", Act::Open(Dest::Waypoints), txt!("Distance and bearing to each")),
    ("GPS settings", Act::Open(Dest::Group(6)), txt!("On or off, format, units, time zone")),
];
const SCOPE: [Opt; 1] = [("Scope settings", Act::Open(Dest::Sub(3, 1)), txt!("Bars, step, speed, trigger, waterfall"))];
/// APRS-17.7 (decision 257): the one APRS list, the same from the APRS slot (OK or hold MENU on home) and from the
/// heard list (hold MENU). OK on the APRS slot opens it on its "Heard" row (APRS mode, KISS) or its "Beacon now"
/// row (beacons alone hold C); a digit opens a row that opens a screen (MENU-25), so OK 1 reads the messages.
pub(crate) const APRS_ROW_HEARD: u8 = 1;
pub(crate) const APRS_ROW_BEACON_NOW: u8 = 3;
const APRS: [Opt; 7] = [
    ("Messages", Act::Open(Dest::AprsMessages), txt!("Inbox, sent and compose")),
    ("Heard", Act::Open(Dest::AprsHeard), txt!("Stations heard, newest first")),
    ("Sent", Act::Open(Dest::AprsSent), txt!("Your beacons and who repeated them")),
    ("Beacon now", Act::Do(Simple::BeaconNow), txt!("Sends your position once (needs a callsign)")),
    ("Event log", Act::Open(Dest::EventLog), txt!("Everything decoded and logged")),
    ("Clear the heard list", Act::Confirm(Simple::ClearHeard), txt!("Stations return when heard")),
    ("APRS settings", Act::Open(Dest::Group(5)), txt!("Beacon, path, symbol, frequency")),
];
const CHANNELS: [Opt; 1] = [("Show hidden channels", Act::ShowHidden, txt!("Hidden channels stay reachable by number"))];
const WAYPOINTS: [Opt; 5] = [
    ("Mark here", Act::Do(Simple::MarkHere), txt!("A waypoint at this fix")),
    ("Add a grid square", Act::Grid, txt!("A waypoint at a square's centre: FN31pr")),
    ("Arrival alarm", Act::Arrival, txt!("Off, 50 m to 1 km from the one you go to")),
    ("Use as fixed position", Act::Sel(Sel::Fixed, false), txt!("This waypoint, used when there's no fix")),
    ("GPS position", Act::Open(Dest::GpsPosition), txt!("Fix, satellites, grid square")),
];
const STATION: [Opt; 3] = [
    ("Message", Act::Sel(Sel::Message, false), txt!("Write this station a message")),
    ("Save contact", Act::Sel(Sel::Name, false), txt!("Its callsign and a name; renames a saved one")),
    ("Save waypoint", Act::Sel(Sel::Waypoint, false), txt!("Its last position, named with its callsign")),
];
const CONTACTS: [Opt; 3] = [
    ("Edit name", Act::Sel(Sel::Name, false), txt!("Up to 12 characters")),
    ("Delete", Act::Sel(Sel::Delete, true), txt!("Contact goes; messages stay")),
    ("Station", Act::Sel(Sel::Station, false), txt!("Its page, when it is in the heard list")),
];

pub(crate) fn rows(of: Of) -> &'static [Opt] {
    match of {
        Of::Log => &LOG,
        Of::Heard | Of::AprsCard => &APRS,
        Of::Messages => &MESSAGES,
        Of::MsgSending => &MSG_SENDING,
        Of::MsgEnded => &MSG_ENDED,
        Of::Msg => &MSG,
        Of::Gps => &GPS,
        Of::Waypoints => &WAYPOINTS,
        Of::Scope => &SCOPE,
        Of::Channels => &CHANNELS,
        Of::Station => &STATION,
        Of::Contacts => &CONTACTS,
    }
}

/// The page's name, for the help box ("Event log MENU").
pub(crate) fn title(of: Of) -> &'static [u8] {
    match of {
        Of::Log => b"Event log",
        Of::Heard => b"APRS heard",
        Of::Messages => b"APRS messages",
        Of::MsgSending | Of::MsgEnded | Of::Msg => b"APRS message",
        Of::Gps => b"GPS position",
        Of::Waypoints => b"Waypoints",
        Of::Scope => b"Scope",
        Of::Channels => b"Channels",
        Of::AprsCard => b"APRS slot",
        Of::Station => b"APRS station",
        Of::Contacts => b"Contacts",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Options {
    pub of: Of,
    pub row: u8,
    pub confirm: bool,
}

impl View for Options {
    fn name(self) -> &'static str {
        "Options"
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let all = rows(self.of);
        let (label, act, _) = all[self.row as usize % all.len()];
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) if !self.confirm => self.knob(app, if e.key == Key::Up { -1 } else { 1 }, store),
            // MENU-25: 0 the full menu; as in the menus (MENU-5) a digit picks its row and opens a row that opens a
            // screen; a row that does something waits for OK.
            (Key::Digit(0), Short) if !self.confirm => app.open(Dest::Advanced),
            (Key::Digit(d), Short) if !self.confirm && (d as usize) <= all.len() => {
                let o = Options { row: d - 1, ..self };
                app.screen = Screen::Options(o);
                if matches!(all[d as usize - 1].1, Act::Open(_)) {
                    o.key(app, KeyEvent { key: Key::Menu, press: Short }, store);
                }
            }
            (Key::Menu, Short) => match act {
                Act::Confirm(_) | Act::Sel(_, true) | Act::MsgDelete if !self.confirm => app.screen = Screen::Options(Options { confirm: true, ..self }),
                Act::Sel(s, _) => {
                    app.back();
                    super::contacts::sel(app, s, store);
                }
                Act::Do(Simple::BeaconNow) => {
                    app.back();
                    beacon_now(app, store);
                }
                Act::MsgStop | Act::MsgResend => {
                    app.back();
                    super::msgs::stop_or_resend(app, store, act == Act::MsgStop);
                }
                Act::MsgDelete => {
                    app.back();
                    super::msgs::delete(app, store);
                }
                Act::Do(k) | Act::Confirm(k) => {
                    let op = k.op();
                    if matches!(op, Op::BeaconNow | Op::Send) && app.remote_tx_refused() {
                        return; // LINK-19: a beacon or message now is a transmission
                    }
                    if let Some(why) = app.kiss_holds_aprs().filter(|_| matches!(op, Op::BeaconNow | Op::Send)) {
                        app.back();
                        return app.show_toast(b"Not sent", why.as_bytes(), false); // KISS-10
                    }
                    app.back();
                    if store.data().request(op) {
                        match op {
                            // the log's: the mail badge counts messages (INBOX-3.1); an emptied log has none new (INBOX-3.2)
                            Op::ReadAll | Op::ClearLog => app.log_unread = 0,
                            // INBOX-3.2: each message read takes its log entry with it (INBOX-7.1), as one read does
                            Op::MessagesReadAll => (app.log_unread, app.inbox) = (app.log_unread.saturating_sub(app.inbox), 0),
                            Op::ClearMessages => app.inbox = 0, // APRS-46
                            _ => {}
                        }
                        app.show_toast(label.as_bytes(), b"done", false);
                    } else {
                        let why: &[u8] = match op {
                            Op::BeaconNow if !app.set.callsign.as_bytes().is_empty() => b"callsign over 6", // SETUP-13.1
                            Op::BeaconNow => b"set a callsign first", // APRS-8
                            Op::MarkHere => b"needs a GPS fix",
                            _ => b"not possible now",
                        };
                        app.show_toast(label.as_bytes(), why, false);
                    }
                }
                Act::Open(d) => {
                    app.back();
                    // the heard list's own row from the heard list: back to it, not a second one over it
                    if !(d == Dest::AprsHeard && matches!(app.screen, Screen::Aprs(a) if !a.station)) {
                        app.open(d);
                    }
                }
                Act::Grid => {
                    app.back();
                    super::text::open(app, super::text::TextFor::GridWaypoint, store);
                }
                Act::Arrival => {
                    let n = crate::settings::ARRIVAL_M.len() as u8;
                    app.set.arrival = (app.set.arrival + 1) % n;
                    app.back();
                    let m = crate::settings::ARRIVAL_M[app.set.arrival as usize];
                    let mut v = Buf::<12>::new();
                    match m {
                        0 => v.set(b"off"),
                        1000 => v.set(b"1 km"),
                        _ => {
                            let _ = ufmt::uwrite!(v, "{} m", m);
                        }
                    }
                    app.show_toast(b"Arrival alarm", v.as_bytes(), false);
                }
                Act::Retries => {
                    let c = app.aprs_cfg();
                    let n = (c.retries.min(crate::aprs::RETRIES_DEFAULT) + 1) % (crate::aprs::RETRIES_DEFAULT + 1);
                    app.set_aprs(crate::aprs::AprsCfg { retries: n, ..c });
                    app.back();
                    app.show_toast(b"Message retries", retries_text(n).as_bytes(), false);
                }
                Act::ShowHidden => {
                    app.set.show_hidden = !app.set.show_hidden;
                    app.back();
                    let on: &[u8] = if app.set.show_hidden { b"shown" } else { b"left out" };
                    app.show_toast(b"Hidden channels", on, false);
                }
            },
            (Key::Exit, Short) if self.confirm => app.screen = Screen::Options(Options { confirm: false, ..self }),
            (Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        if !self.confirm {
            let n = rows(self.of).len() as i32;
            app.screen = Screen::Options(Options { row: (self.row as i32 + d).rem_euclid(n) as u8, ..self });
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, _: &Cx<impl ChannelStore>) {
        let all = rows(self.of);
        app.page(lcd, (self.of, self.confirm));
        let retries = retries_text(app.aprs_cfg().retries); // APRS-13.6: the row shows its value
        let right = |a: &Act| -> &[u8] {
            match a {
                Act::Open(_) => b">",
                Act::Retries => retries.as_bytes(),
                _ => b"",
            }
        };
        options_page(app, lcd, all.iter().map(|(l, a, _)| (l.as_bytes(), right(a))), self.row);
        let (label, _, help) = all[self.row as usize % all.len()];
        if self.confirm {
            let mut q = Buf::<32>::new();
            q.push(label.as_bytes());
            q.push(b"?");
            app.confirm_box_t(lcd, q.as_bytes(), help);
        } else {
            let mut p = Buf::<24>::new();
            p.set(title(self.of));
            p.push(b" MENU");
            // under the rows and "0 Full menu": six or seven rows push it down
            let y = opts_help_y(all.len());
            app.help_txt(lcd, y, 46, help, p.as_bytes());
        }
    }

    fn hints(self, _: &App) -> Hints {
        if self.confirm {
            Hints::new(&[("OK", "Confirm"), ("BACK", "Keep")])
        } else {
            Hints::new(&[("OK", "Do it"), ("0", "Full menu"), ("BACK", "Back")])
        }
    }
}

/// APRS-13.6: message retries as shown: "off", or the count.
fn retries_text(n: u8) -> Buf<4> {
    let mut b = Buf::<4>::new();
    if n == 0 {
        b.set(b"off");
    } else {
        let _ = ufmt::uwrite!(b, "{}", n.min(crate::aprs::RETRIES_DEFAULT));
    }
    b
}

/// Where an options page's help box goes (under five rows and "0 Full menu"; lower under more).
pub(crate) const OPTS_HELP_Y: i32 = 140;
/// An options page's rows (decision 372: the menus' 14 px text in 25 px rows).
pub(crate) const OPTS_ROW_H: i32 = 25;

/// The help box's place under `rows` rows and "0 Full menu".
pub(crate) fn opts_help_y(rows: usize) -> i32 {
    OPTS_HELP_Y.max(23 + OPTS_ROW_H * (rows as i32 + 1) + 6)
}

/// An options page's rows, the one style (MENU-25): numbered 1-n with their values, the selected one
/// highlighted, then "0 Full menu".
pub(crate) fn options_page<'a>(app: &mut App, lcd: &mut impl Panel, rows: impl Iterator<Item = (&'a [u8], &'a [u8])>, sel: u8) {
    let mut n = 0;
    for (i, (name, value)) in rows.enumerate() {
        let r = Row { num: Some(i as u8 + 1), name, value, dot: false, selected: i as u8 == sel, grey: false };
        app.menu_row(lcd, i, 23 + OPTS_ROW_H * i as i32, OPTS_ROW_H, &r);
        n = i + 1;
    }
    let r = Row { num: Some(0), name: b"Full menu", value: b">", dot: false, selected: false, grey: false };
    app.menu_row(lcd, n, 23 + OPTS_ROW_H * n as i32, OPTS_ROW_H, &r);
}

/// Beacon now (APRS-17.7, APRS-17.8): from the APRS list, hold V/M on the APRS card and V/M in compose. Refused while the link
/// may not transmit (LINK-19) or KISS holds C (KISS-10); a refusal says why.
pub(crate) fn beacon_now(app: &mut App, store: &impl ChannelStore) {
    if app.remote_tx_refused() {
        return;
    }
    if let Err((head, why)) = send_beacon(app, store) {
        app.show_toast(head.as_bytes(), why.as_bytes(), false);
    }
}

/// A beacon now (APRS-17.8), from the radio's keys or the link's `BEACON` (LINK-69): asked of the APRS task and
/// "Beacon now / done" shown, or refused with the toast's two lines and nothing asked.
pub(crate) fn send_beacon(app: &mut App, store: &impl ChannelStore) -> Result<(), (&'static str, &'static str)> {
    if let Some(why) = app.kiss_holds_aprs() {
        return Err(("Not sent", why));
    }
    // APRS-17.9: with neither APRS mode nor beacons holding C nothing may send, so the beacon isn't asked for
    // (asked for, it would wait and go the moment either came on).
    if app.aprs_holder().is_none() {
        return Err(("Not sent", "APRS and beacons off"));
    }
    if !store.data().request(Op::BeaconNow) {
        return Err(("Beacon now", if app.set.callsign.as_bytes().is_empty() { "set a callsign first" } else { "callsign over 6" }));
    }
    app.show_toast(b"Beacon now", b"done", false);
    Ok(())
}
