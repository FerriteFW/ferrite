//! Range presets and the skip list (MENU 0 1 4, 0 1 5): new presets until full, each editor row turned, the
//! warnings under a range, and the lists' knob.

use super::*;
use crate::data::Data;
use crate::screens::scanlists::{Page, ScanLists};
use ferrite_core::freq::mhz;

fn page(t: &T) -> Page {
    match t.screen() {
        Screen::ScanLists(s) => s.page,
        s => panic!("not the scan lists: {}", s.name()),
    }
}

/// MENU-23: # makes a new preset and opens it, until there are 16: then it says to delete one.
#[test]
fn new_presets_until_full() {
    let mut t = T::new();
    t.open(Dest::RangePresets);
    t.knob(1);
    t.knob(-1);
    for _ in 0..20 {
        t.tap(Key::Hash);
        if t.toast() == "16 presets / delete one first" {
            break;
        }
        assert!(matches!(page(&t), Page::Edit(0)), "a new one opens");
        t.tap(Key::Exit);
    }
    assert_eq!(t.toast(), "16 presets / delete one first");
}

/// SCAN-14, SCAN-24, VFO-20: the editor's rows: the end, the step, the mode (NFM narrow), the resume and its
/// names; a range across 64 MHz, or over two bands, says so.
#[test]
fn editor_rows() {
    let mut t = T::new();
    t.open(Dest::RangePresets);
    t.tap(Key::Menu);
    let p0 = t.s.preset(0).unwrap();
    t.tap(Key::Down);
    t.tap(Key::Down); // end
    t.knob(1);
    assert_eq!(t.s.preset(0).unwrap().end, p0.end + 10_000);
    t.tap(Key::Down); // step
    t.knob(1);
    assert_ne!(t.s.preset(0).unwrap().step, p0.step);
    t.tap(Key::Down); // mode
    for _ in 0..3 {
        t.knob(1);
        let p = t.s.preset(0).unwrap();
        assert_eq!(p.narrow, crate::slot::mode_narrow(p.mode)); // NFM and NAM (VFO-4.2)
    }
    t.tap(Key::Down); // resume
    for (n, name) in [(11, "Carrier drop"), (1, "Stop on hit"), (4, "After 3 s")] {
        t.knob(n);
        t.shows(name);
    }
    t.tap(Key::Down); // skips: the knob does nothing
    let p = t.s.preset(0).unwrap();
    t.knob(1);
    assert!(t.s.preset(0).unwrap() == p);
    // A range across 64 MHz, and one over two bands.
    let mut q = p;
    (q.start, q.end) = (mhz(50), mhz(70));
    t.s.request(crate::data::Op::Preset(0, q));
    t.draw();
    t.shows("Crosses 64 MHz: a scan of it is refused (SCAN-14)");
    (q.start, q.end) = (mhz(140), mhz(440));
    t.s.request(crate::data::Op::Preset(0, q));
    t.draw();
    t.shows("Spans more than one band");
}

/// SCAN-11: the skip list moves with the knob; a skipped frequency without a channel shows its band.
#[test]
fn skip_list() {
    let mut t = T::new();
    t.open(Dest::SkipList);
    assert!(t.s.skips() > 1);
    t.knob(1);
    assert!(matches!(t.screen(), Screen::ScanLists(ScanLists { page: Page::Skips, .. })));
    let s = t.texts();
    assert!(s.contains("this scan only"), "{s}");
    assert!(s.contains("North hill"), "a skip on a channel names it: {s}");
    // A store with one skip and no channels: the band instead.
    let st = OneSkip;
    let s = t.texts_in(&st);
    assert!(s.contains("1 skipped") && s.contains("UHF"), "{s}");
}

struct OneSkip;

impl Data for OneSkip {
    fn skips(&self) -> u16 {
        1
    }
    fn skip(&self, i: u16) -> Option<ferrite_core::freq::Freq10> {
        (i == 0).then_some(mhz(446) + 6_250)
    }
}

impl crate::chan::ChannelStore for OneSkip {
    fn zones(&self) -> crate::chan::ZoneId {
        0
    }
    fn zone_name(&self, _: crate::chan::ZoneId) -> Option<crate::chan::Name> {
        None
    }
    fn zone_len(&self, _: crate::chan::ZoneId) -> u16 {
        0
    }
    fn zone_at(&self, _: crate::chan::ZoneId, _: u16) -> Option<crate::chan::Channel> {
        None
    }
    fn count(&self) -> u16 {
        0
    }
    fn data(&self) -> &dyn Data {
        self
    }
}
