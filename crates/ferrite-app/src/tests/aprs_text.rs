//! The APRS card's two text lines (APRS-17): the beacon line and the last heard line, from what the store says.

use super::T;
use crate::data::{Beacon, Data, Station, Sym};
use ferrite_ui::text::Buf;
use std::string::String;

/// A store's APRS side: one beacon state and at most one station.
#[derive(Default)]
struct D {
    beacon: Beacon,
    station: Option<Station>,
}

impl Data for D {
    fn beacon(&self) -> Beacon {
        self.beacon
    }
    fn stations(&self) -> u16 {
        self.station.is_some() as u16
    }
    fn station(&self, i: u16) -> Option<Station> {
        self.station.filter(|_| i == 0)
    }
}

fn station(call: &[u8], ago_s: u32) -> Station {
    Station { call: Buf::from(call), sym: Sym::Car, pos: None, ago_s, via: Buf::new(), course: None, alt_m: None, times: 1, device: None, comment: Buf::new(), msg: Buf::new(), msg_min: None, msg_to: Default::default(), msg_ago_s: None, fx25: None }
}

fn beacon_line(t: &T, d: &D) -> String {
    let mut b = Buf::<40>::new();
    t.app.beacon_text(d, &mut b);
    String::from_utf8_lossy(b.as_bytes()).into_owned()
}

/// APRS-17, APRS-20, APRS-24: the beacon line: waiting (for the relay: HF), off, manual, smart, the countdown in
/// seconds then minutes, and soon when no time is known.
#[test]
fn beacon_line_by_mode() {
    let mut t = T::new();
    let mut d = D::default();
    d.beacon.waiting = true;
    assert_eq!(beacon_line(&t, &d), "Beacon waiting");
    t.app.set_vfo(t.app.active(), 710_000); // 7.1 MHz: the relay's other state
    assert_eq!(beacon_line(&t, &d), "Beacon waiting: HF");
    let mut t = T::new();
    d.beacon.waiting = false;
    t.app.aprs.beacon = 0;
    assert_eq!(beacon_line(&t, &d), "Beacon off");
    t.app.aprs.beacon = 1;
    assert_eq!(beacon_line(&t, &d), "Beacon manual");
    t.app.aprs.beacon = crate::aprs::SMART as u8;
    assert_eq!(beacon_line(&t, &d), "Smart beacon");
    t.app.aprs.beacon = 2;
    d.beacon.next_s = Some(42);
    assert_eq!(beacon_line(&t, &d), "Beacon in 42s");
    d.beacon.next_s = Some(181);
    assert_eq!(beacon_line(&t, &d), "Beacon in 4m");
    d.beacon.next_s = None;
    assert_eq!(beacon_line(&t, &d), "Beacon soon");
}

/// APRS-17: the last line: the newest station with its age, or nothing heard yet.
#[test]
fn last_heard_line() {
    let mut b = Buf::<40>::new();
    crate::aprs::last_text(&D::default(), &mut b);
    assert_eq!(b.as_bytes(), b"Nothing heard yet");
    let d = D { station: Some(station(b"VE3KPW-9", 300)), ..D::default() };
    crate::aprs::last_text(&d, &mut b);
    let s = String::from_utf8_lossy(b.as_bytes()).into_owned();
    assert!(s.starts_with("VE3KPW-9 \u{b7} ") && s.ends_with(" ago"), "{s}");
}
