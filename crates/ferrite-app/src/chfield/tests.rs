//! The channel field model on its own (CHAN-32, decision 223): each field's step, typed value, default and refusal.

use super::*;
use crate::chan::channel;
use crate::slot::{DEFAULT_CFG, SQ_GLOBAL};

const CX: Ctx<'static> = Ctx { zones: 3, region: None, room: &crate::chfield::any_room };

fn ch(rx: Freq10) -> Channel {
    channel(7, 0, rx, "Test", DEFAULT_CFG)
}

/// Steps `f` by `d` on `c` (no home channel), the result or the refusal.
fn step(f: ChField, mut c: Channel, d: i32) -> Result<Channel, Refusal> {
    f.change(&mut c, &mut None, Op::Step(d), CX).map(|()| c)
}

/// CHAN-32, CHAN-10, EDIT-5: the zone steps through every zone, then none, and wraps both ways; # leaves it.
#[test]
fn zone_wraps_through_none() {
    let mut c = ch(mhz(146) + 52_000);
    let seen: std::vec::Vec<crate::chan::ZoneId> = (0..5)
        .map(|_| {
            c = step(ChField::Zone, c, 1).unwrap();
            c.zone
        })
        .collect();
    assert_eq!(seen, [1, 2, NO_ZONE, 0, 1]);
    assert_eq!(step(ChField::Zone, ch(mhz(146)), -1).unwrap().zone, NO_ZONE);
    let mut r = c;
    ChField::Zone.change(&mut r, &mut None, Op::Reset, CX).unwrap();
    assert_eq!(r, c, "a zone has no default");
}

/// CHAN-32, VFO-18, CHAN-22: RX steps by its band's step in the region, stays on its side of 64 MHz and steps over
/// 630-756 MHz, which no chip receives; a simplex channel goes up to 1300 MHz (receive only above 1000, VFO-21).
#[test]
fn rx_steps_by_band_and_range() {
    assert_eq!(step(ChField::Rx, ch(mhz(146) + 52_000), 1).unwrap().rx, mhz(146) + 53_250, "12.5 kHz with no region");
    let us = Ctx { region: Some(Region::Us), ..CX };
    let mut c = ch(mhz(146) + 52_000);
    ChField::Rx.change(&mut c, &mut None, Op::Step(1), us).unwrap();
    assert_eq!(c.rx, mhz(146) + 52_500, "5 kHz in the US");
    assert_eq!(step(ChField::Rx, ch(mhz(64)), -1).unwrap().rx, mhz(64), "never onto HF");
    assert_eq!(step(ChField::Rx, ch(mhz(630)), 1).unwrap().rx, mhz(756), "over the gap");
    assert_eq!(step(ChField::Rx, ch(mhz(756)), -1).unwrap().rx, mhz(630), "and back");
    assert_eq!(step(ChField::Rx, ch(mhz(1300)), 1).unwrap().rx, mhz(1300), "the top");
    assert_eq!(step(ChField::Rx, ch(mhz(999) + 99_000), 1).unwrap().rx, mhz(1000) + 250, "simplex above 1000 MHz");
    let mut hf = ch(mhz(28) + 40_000);
    hf.cfg.mode = 2;
    assert_eq!(step(ChField::Rx, hf, 10_000).unwrap().rx, crate::chan::HF_TOP - 1, "an HF channel stays HF");
}

/// CHAN-32, CHAN-22: a TX apart from RX stays within 64-1000 MHz, by the knob or typed.
#[test]
fn tx_range_refused() {
    let mut c = ch(mhz(999) + 90_000);
    c.cfg.offset = 500_000;
    assert_eq!(check_edit(&c), Err(Refusal::TxRange));
    c.cfg.offset = 60_000;
    let near = Channel { rx: mhz(999) + 30_000, ..c };
    assert_eq!(step(ChField::Rx, near, 10), Err(Refusal::TxRange), "+600k past 1000 MHz");
    let mut t = ch(mhz(446));
    assert_eq!(ChField::Tx.change(&mut t, &mut None, Op::Typed(b"1200"), CX), Err(Refusal::TxRange));
    assert_eq!(ChField::Tx.change(&mut t, &mut None, Op::Typed(b"abc"), CX), Err(Refusal::TxRange));
    assert!(!t.cfg.split, "unchanged");
}

/// RX typed (CHAN-22): MHz, refused outside what a channel holds; the refusal names the field.
#[test]
fn rx_typed() {
    let mut c = ch(mhz(146));
    assert_eq!(ChField::Rx.change(&mut c, &mut None, Op::Typed(b"700"), CX), Err(Refusal::RxRange));
    assert_eq!(ChField::Rx.change(&mut c, &mut None, Op::Typed(b"x"), CX), Err(Refusal::RxRange));
    ChField::Rx.change(&mut c, &mut None, Op::Typed(b"446.0125"), CX).unwrap();
    assert_eq!(c.rx, mhz(446) + 1_250);
    // HF from the terminal: HF mode's rules (CHAN-26), NFM isn't one.
    assert_eq!(ChField::Rx.change(&mut c, &mut None, Op::Typed(b"7.1"), CX), Err(Refusal::HfMode));
    let mut b = Buf::<64>::new();
    Refusal::RxRange.line(&mut b);
    assert_eq!(b.as_bytes(), b"RX: 0.15-64 MHz (HF), 64-630 or 756-1300 MHz");
}

/// CHAN-32, VFO-16, TX-37: Offset's 4th choice is Split on today's TX; typed TX turns split on; a split that needs the
/// band relay in the other state from RX is refused.
#[test]
fn offset_and_split() {
    let c = ch(mhz(146) + 52_000);
    let plus = step(ChField::It(Item::Offset), c, 1).unwrap();
    assert_eq!(plus.cfg.offset, crate::slot::BANDS[band_near(c.rx)].shift, "the band's shift");
    let split = step(ChField::It(Item::Offset), plus, 2).unwrap();
    assert!(split.cfg.split);
    assert_eq!(split.cfg.offset as Freq10, c.rx + 60_000, "today's TX");
    assert!(!step(ChField::It(Item::Offset), split, 1).unwrap().cfg.split, "and on to simplex");
    let mut t = c;
    ChField::Tx.change(&mut t, &mut None, Op::Typed(b"446.0"), CX).unwrap();
    assert!(t.cfg.split && t.cfg.offset == mhz(446) as i32);
    assert_eq!(step(ChField::Tx, t, 1).unwrap().cfg.offset, (mhz(446) + 1_250) as i32, "TX steps while split");
    assert_eq!(step(ChField::Tx, c, 1).unwrap(), c, "typed only while not split");
    let mut r = c;
    assert_eq!(ChField::Tx.change(&mut r, &mut None, Op::Typed(b"50.125"), CX), Err(Refusal::Relay));
    assert_eq!(Refusal::Relay.field(), None, "a sentence of its own");
}

/// CHAN-32, TX-9 (decision 224): on 1.25 m power steps and shows like any band, and a whole channel may say High.
#[test]
fn power_free_on_125m() {
    let c = ch(mhz(223) + 50_000);
    assert_eq!(step(ChField::It(Item::Power), c, 1).unwrap().cfg.power, c.cfg.power + 1);
    let mut h = c;
    h.cfg.power = 2;
    assert_eq!(check(&h), Ok(()));
}

/// CHAN-32, CHAN-4, EDIT-7: a receive-only channel's power doesn't change; the flag itself toggles.
#[test]
fn rx_only_power_refused() {
    let c = step(ChField::Flag(FLAG_RX_ONLY), ch(mhz(146)), 1).unwrap();
    assert_ne!(c.flags & FLAG_RX_ONLY, 0);
    assert_eq!(step(ChField::It(Item::Power), c, 1), Err(Refusal::RxOnly));
    let mut b = Buf::<64>::new();
    Refusal::RxOnly.line(&mut b);
    assert_eq!(b.as_bytes(), b"Power: this channel is RX only");
    assert_eq!(step(ChField::Flag(FLAG_RX_ONLY), c, 1).unwrap().flags & FLAG_RX_ONLY, 0);
}

/// CHAN-32, CHAN-4, CHAN-24, decision 63: flags toggle and reset off; Scan is in scans, Skip or Priority, never both.
#[test]
fn flags_and_scan() {
    for bit in [FLAG_APRS, FLAG_HIDDEN] {
        let on = step(ChField::Flag(bit), ch(mhz(146)), 1).unwrap();
        assert_ne!(on.flags & bit, 0);
        let mut off = on;
        ChField::Flag(bit).change(&mut off, &mut None, Op::Reset, CX).unwrap();
        assert_eq!(off.flags & bit, 0);
    }
    let mut c = ch(mhz(146));
    let mut seen = std::vec::Vec::new();
    for _ in 0..3 {
        c = step(ChField::Scan, c, 1).unwrap();
        let mut b = Buf::new();
        ChField::Scan.text(&c, None, false, &mut b);
        seen.push(std::string::String::from_utf8(b.as_bytes().to_vec()).unwrap());
    }
    assert_eq!(seen, ["Skip", "Priority", "In scans"]);
    c.flags |= FLAG_SKIP | FLAG_PRIORITY;
    assert_eq!(c.scan(), 2, "both (an old record) reads as Priority");
    c.set_scan(1);
    assert_eq!(c.flags & (FLAG_SKIP | FLAG_PRIORITY), FLAG_SKIP);
    ChField::Scan.change(&mut c, &mut None, Op::Reset, CX).unwrap();
    assert_eq!(c.scan(), 0);
}

/// CHAN-32, CHAN-17, CHAN-18: Home toggles the one home channel; # clears it only when it is this one.
#[test]
fn home_toggles() {
    let mut c = ch(mhz(146));
    let mut home = Some(3);
    ChField::Home.change(&mut c, &mut home, Op::Step(1), CX).unwrap();
    assert_eq!(home, Some(7), "this one now, the old one off");
    ChField::Home.change(&mut c, &mut home, Op::Step(-1), CX).unwrap();
    assert_eq!(home, None);
    home = Some(3);
    ChField::Home.change(&mut c, &mut home, Op::Reset, CX).unwrap();
    assert_eq!(home, Some(3), "another channel's stays");
    let mut b = Buf::new();
    ChField::Home.text(&c, Some(7), false, &mut b);
    assert_eq!(b.as_bytes(), b"Yes");
}

/// CHAN-32, MENU-18, SQL-6: # puts an item back to its default; a channel's squelch default is Global.
#[test]
fn item_defaults() {
    let mut c = ch(mhz(146));
    c.cfg.squelch = 7;
    c.cfg.mode = 2;
    c.cfg.offset = 60_000;
    for f in [ChField::It(Item::Squelch), ChField::It(Item::Mode), ChField::It(Item::Offset)] {
        f.change(&mut c, &mut None, Op::Reset, CX).unwrap();
    }
    assert_eq!((c.cfg.squelch, c.cfg.mode, c.cfg.offset), (SQ_GLOBAL, DEFAULT_CFG.mode, 0));
    let mut n = c;
    ChField::Rx.change(&mut n, &mut None, Op::Reset, CX).unwrap();
    assert_eq!(n, c, "a frequency has no default");
}

/// CHAN-32, CHAN-5, CHAN-31: a typed name is printable ASCII, 16 at most; outside the name set maps to '-'.
#[test]
fn name_typed() {
    let mut c = ch(mhz(146));
    assert_eq!(ChField::Name.change(&mut c, &mut None, Op::Typed(b"Tab\there"), CX), Err(Refusal::NameChars));
    assert_eq!(ChField::Name.change(&mut c, &mut None, Op::Typed(b"seventeen chars!!"), CX), Err(Refusal::NameLong));
    ChField::Name.change(&mut c, &mut None, Op::Typed(b"Club {2m}"), CX).unwrap();
    assert_eq!(c.name(), b"Club -2m-");
    assert_eq!(step(ChField::Name, c, 1).unwrap(), c, "the knob leaves a name");
}

/// CHAN-32, CHAN-26, CHAN-23: an HF channel is HF mode's mode and simplex; USB, LSB and CW only below 64 MHz.
#[test]
fn hf_rules() {
    let mut c = ch(mhz(7) + 10_000);
    assert_eq!(check_edit(&c), Err(Refusal::HfMode), "NFM isn't HF mode's");
    c.cfg.mode = 3;
    assert_eq!(check_edit(&c), Ok(()));
    c.cfg.offset = 60_000;
    assert_eq!(check_edit(&c), Err(Refusal::HfSimplex));
    let mut v = ch(mhz(146));
    v.cfg.mode = 3;
    assert_eq!(check_edit(&v), Err(Refusal::NotHf));
}

/// CHAN-26, CHAN-26.1, CHAN-26.2, VFO-14 (ferrite#84): an 18-64 MHz channel (CB, chip 1 through the relay) takes
/// FM, NFM and AM, and the Mode step goes on past the modes it doesn't have (NAM, WFM), so the knob reaches each
/// of the three both ways; below 18 MHz NFM stays refused.
#[test]
fn cb_channel_modes() {
    let cb = mhz(27) + 18_500;
    let mut c = ch(cb);
    c.cfg.mode = 0;
    assert_eq!(check_edit(&c), Ok(()));
    let mut seen = std::vec::Vec::new();
    for _ in 0..4 {
        c = step(ChField::It(Item::Mode), c, 1).unwrap();
        seen.push(c.cfg.mode);
    }
    assert_eq!(seen, [1, 2, 0, 1], "FM, NFM, AM round");
    assert!(c.cfg.narrow, "NFM is the narrow filter (VFO-20)");
    let back: std::vec::Vec<u8> = (0..3)
        .map(|_| {
            c = step(ChField::It(Item::Mode), c, -1).unwrap();
            c.cfg.mode
        })
        .collect();
    assert_eq!(back, [0, 2, 1], "and back the other way, over WFM and NAM");
    let mut low = ch(mhz(7) + 10_000);
    low.cfg.mode = 2;
    assert_eq!(step(ChField::It(Item::Mode), low, 1), Err(Refusal::HfMode), "below 18 MHz: HF mode's AM, no NFM");
}
