//! Home's APRS card (APRS-17, decisions 181 and 313): what slot C shows while it is the APRS slot, for the drawing
//! (`screens::home`) and the readback (`uistate`, TERM-37) alike.
//!
//! The header names the holder ("APRS Mode", "KISS", "APRS Beacon") and, like every card, the state at its
//! right; while C shares the VHF chip, who wins it beside the state ("APRS first", "A·B first": [`App::aprs_priority`]).
//!
//! The large card (M3, decision 313): the frequency at the compact cards' size with "3 heard · 11 /hr" at its right;
//! the two newest stations (call, age, distance and bearing); our last beacon's fate ("Last beacon digi'd by
//! VE7RHS"); the beacon row ("Beacon in 5m", "Beacon off") with the power at its right. KISS shows its frames and
//! port instead of stations.
//!
//! The small card keeps one fact right of the frequency ("14 heard", "Bluetooth", or with beacons alone "Beacon in"
//! over "5m") and the bottom line ("Last VE7ABC-9 · 2 min", "Sent 3 · Heard 14", "Every 10 min · GPS fix") with
//! the power, or with beacons beside APRS mode or KISS their countdown ("Beacon in 5m"), at its right.

use crate::aprs::Holder;
use crate::data::{Data, MsgState, Station};
use crate::screens::lists::{age_text, compass_name, dist_bearing, dist_short, Txt};
use crate::App;
use ferrite_ui::text::Buf;
use ufmt::uwrite;

/// One station row of the large card (APRS-17.2.2): call, age, and at the right the distance and bearing ("2.4 km
/// NE") or "via digi" or nothing.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Row {
    pub call: Buf<10>,
    pub age: Buf<12>,
    pub right: Buf<16>,
}

/// The card's texts.
pub struct AprsCard {
    pub holder: Holder,
    /// The small card's fact right of the frequency: "14 heard", "Bluetooth", the countdown "5m"
    /// ([`AprsCard::countdown`]) or a short reason ("no fix").
    pub right: Buf<16>,
    /// `right` is the beacon's countdown, drawn under a small "Beacon in".
    pub countdown: bool,
    /// `right` or `line` is a warning (amber): no fix, or the beacon waiting for the relay.
    pub right_warn: bool,
    pub line_warn: bool,
    /// The small card's bottom line, left part.
    pub line: Buf<48>,
    /// The beacon power, as the chips write it.
    pub power: &'static str,
    /// The beacon's countdown "m:ss" or why there is none ("no fix", "waiting", "by GPS", "soon"); empty while
    /// automatic beacons are off. The readback's `next=`.
    pub next: Buf<16>,
    /// `next` as the card writes it (APRS-17.2.1): "5m", "42s" (whole minutes rounded up, seconds in the last
    /// minute), or the reason.
    pub soon: Buf<16>,
    /// APRS-17.2.1: `next` is a timed countdown (m:ss) or "soon", not a reason it waits (`next_warn`).
    pub next_timed: bool,
    pub next_warn: bool,
    /// The two counts (APRS-17.2): stations heard since power-on and in the last hour.
    pub counts: Option<(u16, u16)>,
    /// The large card's station rows, newest first (APRS-17.2.2); `rows_n` of them hold a station.
    pub rows: [Row; 3],
    pub rows_n: u8,
    /// Unread messages and the newest one's sender (APRS-17.2.4): the second row says so while there are any.
    pub unread: u16,
    pub unread_from: Buf<10>,
    /// Our last beacon's fate (APRS-17.2.3): "Last beacon digi'd by VE7RHS", "Last beacon not heard back" (amber,
    /// `fate_warn`); empty before any beacon this power-on (the row then shows a third station).
    pub fate: Buf<40>,
    pub fate_warn: bool,
    /// The large card's beacon row ("Beacon in 5m", "Beacon off", "Beacon: no fix"); `bottom_warn` amber.
    pub bottom: Buf<28>,
    pub bottom_warn: bool,
    /// APRS-17.2.5: no position of our own: the rows show ages only, and the beacon row "no fix" (amber).
    pub nofix: bool,
    /// The beacon row adds "no fix" left of the power (APRS-17.2.5): no position, and the row doesn't say so already.
    pub fix_note: bool,
}

/// APRS-17.2 (decision 282, option C): the card's two counts. "14 heard": heard since this power-on, so the
/// stations restored from flash (APRS-4) don't count; the rows show only these.
fn since_boot(s: &Station, up_s: u32) -> bool {
    s.ago_s <= up_s
}

/// "3 /hr": heard in the last hour by real age (a row from before a restart counts once GPS time dates it;
/// `AGO_UNKNOWN` never does).
fn in_hour(s: &Station) -> bool {
    s.ago_s <= 3600
}

/// Kept for the tests of APRS-17.2.1: nothing on the card rotates any more.
pub const COUNT_FLIP_MS: u32 = 5000;

/// "4:32" from seconds.
fn mmss(s: u32, b: &mut Buf<16>) {
    b.clear();
    let _ = uwrite!(b, "{}:{}{}", s / 60, (s % 60) / 10, s % 10);
}

/// APRS-17.2.1: "5m" (whole minutes, rounded up), "42s" in the last minute.
pub fn soon_text<const N: usize>(s: u32, b: &mut Buf<N>) {
    let _ = if s < 60 { uwrite!(b, "{}s", s) } else { uwrite!(b, "{}m", s.div_ceil(60)) };
}

/// APRS-17.2.3: our last beacon's fate: "Last beacon digi'd by VE7RHS" ("Last " goes for a long call, so the call is
/// never clipped: 30 characters fit), "Last beacon 12 s ago" while just sent, then "Last beacon not heard back"
/// (true: amber).
pub fn fate_text(b: &crate::data::SentBeacon, fate: &mut Buf<40>) -> bool {
    fate.clear();
    let by = b.by[0].as_bytes();
    if !by.is_empty() {
        let _ = uwrite!(fate, "{}eacon digi'd by {}", if by.len() > 8 { "B" } else { "Last b" }, Txt(by));
    } else if b.ago_s < 30 {
        let mut a = Buf::<12>::new();
        age_text(b.ago_s, &mut a);
        let _ = uwrite!(fate, "Last beacon {} ago", Txt(a.as_bytes()));
    } else {
        fate.set(b"Last beacon not heard back");
        return true;
    }
    false
}

/// "2.4 km NE", "14 mi W", "far N" (0 6 4's units).
fn place(m: u32, deg: u16, units: u8, b: &mut Buf<16>) {
    let mut d = Buf::<12>::new();
    dist_short(m, units, &mut d);
    let db = d.as_bytes();
    let n = db.iter().position(|c| c.is_ascii_alphabetic()).unwrap_or(db.len());
    b.push(&db[..n]);
    if n > 0 {
        b.push(b" ");
    }
    b.push(&db[n..]);
    b.push(b" ");
    b.push(compass_name(deg).as_bytes());
}

impl App {
    /// APRS-17.2: the card's counts, since power-on and in the last hour ([`since_boot`], [`in_hour`]), and the
    /// newest station heard since power-on; the readback's `heard=` and `hour=` too.
    pub fn heard_counts(&self, data: &dyn Data) -> (u16, u16, Option<Station>) {
        let up_s = self.now / 1000;
        let (mut boot, mut hour) = (0u16, 0u16);
        // PWR-26.13: from RAM: the counts and the newest call and age need none of the stored details
        for s in (0..data.stations()).filter_map(|i| data.station_ram(i)) {
            boot += since_boot(&s, up_s) as u16;
            hour += in_hour(&s) as u16;
        }
        (boot, hour, data.station_ram(0).filter(|s| since_boot(s, up_s)))
    }

    /// APRS-21, WATCH-17.3: who wins the VHF chip C shares with A or B (0 5 8 7), for the header beside the state:
    /// "APRS first" (grey) or "A·B first" (amber, true); None while C has its chip to itself.
    pub fn aprs_priority(&self) -> Option<(&'static str, bool)> {
        self.aprs_shared()?;
        let ab = self.settings().aprs_share != 0;
        Some((crate::settings::APRS_SHARE[ab as usize], ab))
    }

    /// The APRS card's texts while slot C is the APRS slot (APRS-17); None otherwise.
    pub fn aprs_card(&self, data: &dyn Data) -> Option<AprsCard> {
        self.aprs_card_as(data, true)
    }

    /// As [`App::aprs_card`]; with `large` false the large card's own parts (the station rows, the unread messages,
    /// the beacon's fate) are left empty: the small card is drawn every frame, and those read the store (PWR-26.13).
    pub fn aprs_card_as(&self, data: &dyn Data, large: bool) -> Option<AprsCard> {
        let holder = self.aprs_holder()?;
        let st = data.beacon();
        let fix = data.fix();
        // The beacon's countdown or why there is none, while automatic beacons are on.
        let (mut next, mut soon) = (Buf::<16>::new(), Buf::<16>::new());
        let (mut next_warn, mut timed) = (false, false);
        if self.beacons_on() {
            if st.waiting || fix.is_none() {
                next.set(if st.waiting { b"waiting" } else { b"no fix" });
                next_warn = true;
            } else if let Some(s) = st.next_s {
                mmss(s, &mut next);
                soon_text(s, &mut soon);
                timed = true;
            } else if self.aprs_cfg().every_min().is_some() {
                next.set(b"soon");
            } else {
                next.set(b"by GPS");
            }
            if !timed {
                soon = next;
            }
        }
        let mut right = Buf::<16>::new();
        let (mut countdown, mut right_warn) = (false, false);
        let mut line = Buf::<48>::new();
        let (n, h, last) = self.heard_counts(data);
        match holder {
            Holder::Mode => {
                let _ = uwrite!(right, "{} heard", n);
                match last {
                    Some(s) => {
                        let mut a = Buf::<12>::new();
                        age_text(s.ago_s, &mut a);
                        let _ = uwrite!(line, "Last {} \u{b7} {}", Txt(s.call.as_bytes()), Txt(a.as_bytes()));
                    }
                    // APRS-17.10: stations restored from flash but none since power-on
                    None => line.set(self.nothing_heard(data).as_bytes()),
                }
            }
            Holder::Kiss => {
                right.set(if self.settings().kiss_on_cable() { b"Cable" } else { b"Bluetooth" });
                let (sent, heard) = data.kiss_counts();
                let _ = uwrite!(line, "Sent {} \u{b7} Heard {}", sent, heard);
            }
            Holder::Beacon => {
                right = soon;
                (countdown, right_warn) = (timed, next_warn);
                let mut m = Buf::<24>::new();
                self.beacon_mode_text(&mut m);
                let pos: &str = match fix {
                    Some(f) if f.age_s == crate::gpsdata::FIXED_AGE => "fixed pos",
                    Some(f) if f.phone => "phone pos (not sent)", // GPS-3.1: never beaconed
                    Some(_) => "GPS fix",
                    None => "no fix",
                };
                let _ = uwrite!(line, "{} \u{b7} {}", Txt(m.as_bytes()), pos);
            }
        }
        // APRS-20: a beacon waiting for the relay says so on the bottom line.
        let line_warn = st.waiting;
        let mut bottom = Buf::<28>::new();
        self.beacon_text(data, &mut bottom);
        if st.waiting {
            line = Buf::from(bottom.as_bytes());
        }
        // The large card's rows (APRS-17.2.2 – APRS-17.2.5).
        let nofix = fix.is_none();
        let up_s = self.now / 1000;
        let mut rows = [Row::default(); 3];
        let mut rows_n = 0u8;
        let rows_of = if large { data.stations().min(3) } else { 0 };
        for s in (0..rows_of).filter_map(|i| data.station(i)).take_while(|s| since_boot(s, up_s)) {
            let r = &mut rows[rows_n as usize];
            r.call = s.call;
            age_text(s.ago_s, &mut r.age);
            match (s.pos, fix) {
                (_, None) => {} // APRS-17.2.5: ages only
                (Some(p), Some(f)) => {
                    let (m, deg) = dist_bearing(f.pos, p);
                    place(m, deg, self.set.units, &mut r.right);
                }
                (None, Some(_)) if !s.via.is_empty() => r.right.set(b"via digi"),
                (None, Some(_)) => {}
            }
            rows_n += 1;
        }
        // APRS-17.2.4: unread messages, the newest one's sender
        let (mut unread, mut unread_from) = (0u16, Buf::<10>::new());
        let msgs_of = if large { data.messages().min(32) } else { 0 };
        for m in (0..msgs_of).filter_map(|i| data.message(i)).filter(|m| m.inbound && m.state == MsgState::Unread) {
            if unread == 0 {
                unread_from = m.peer;
            }
            unread += 1;
        }
        // APRS-17.2.3: our last beacon this power-on and who repeated it
        let mut fate = Buf::<40>::new();
        let fate_warn = large && data.beacon_sent(0).filter(|_| data.beacons_sent() > 0).is_some_and(|b| fate_text(&b, &mut fate));
        let mut bottom_warn = st.waiting;
        let mut fix_note = nofix;
        if holder == Holder::Kiss {
            bottom.set(b"The app sends"); // KISS-10: the radio's own beacons wait
            (bottom_warn, fix_note) = (false, false);
        } else if !st.waiting && self.beacons_on() && nofix {
            bottom.set(b"Beacon: no fix");
            (bottom_warn, fix_note) = (true, false);
        }
        let power = ["LOW", "MID", "HIGH"][self.aprs_cfg().power as usize % 3];
        let next_timed = timed || next.as_bytes() == b"soon";
        Some(AprsCard {
            holder,
            right,
            countdown,
            right_warn,
            line_warn,
            line,
            power,
            next,
            soon,
            next_timed,
            next_warn,
            counts: Some((n, h)),
            rows,
            rows_n,
            unread,
            unread_from,
            fate,
            fate_warn,
            bottom,
            bottom_warn,
            nofix,
            fix_note,
        })
    }

    /// APRS-17.10: "Nothing since power-on" while the heard list holds only stations from before it, else "Nothing
    /// heard yet".
    pub(crate) fn nothing_heard(&self, data: &dyn Data) -> &'static str {
        if data.stations() > 0 { "Nothing since power-on" } else { "Nothing heard yet" }
    }
}
