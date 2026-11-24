//! Decision 64: the mode default by region, # in tone search saves, channel-only mode and hold #, the reset's
//! flags in the settings record.

use super::*;
use crate::chan::ChanWrite;
use ferrite_core::msg::ModeLive;
use ferrite_core::txplan::Region;

/// SETUP-7: a VFO in FM follows the region (NFM in the UK, Europe, Japan and Other); a band change too.
#[test]
fn mode_follows_the_region() {
    let mut t = T::new();
    assert_eq!(t.app.slot(0).cfg().mode, 0, "wide before a region");
    t.app.set_region(Some(Region::Uk), false);
    assert_eq!(t.app.slot(0).cfg().mode, 1);
    assert!(t.app.slot(0).cfg().narrow);
    t.tap(Key::Right); // the next band: Marine, FM there too
    assert_eq!(t.app.slot(0).cfg().mode, 1);
    t.app.set_region(Some(Region::Australia), false);
    assert_eq!(t.app.slot(0).cfg().mode, 0);
    assert_eq!(crate::slot::band_mode(2, Some(Region::Uk)), crate::slot::NAM, "airband is NAM (VFO-4.2)");
    for (r, m) in [(Region::Us, 0), (Region::Canada, 0), (Region::NewZealand, 0), (Region::Europe, 1), (Region::Japan, 1), (Region::Other, 1)] {
        assert_eq!(crate::slot::band_mode(3, Some(r)), m, "{r:?}");
    }
}

/// TSRCH-9: on a channel # saves the found tone into the record.
#[test]
fn tone_search_hash_saves() {
    let mut t = T::new();
    t.tap(Key::VfoMem);
    while t.app.pop_write().is_some() {}
    t.live.mode = ModeLive::Probe { freq: 0, rssi: -80, tone: 1000 };
    t.open(crate::screens::nav::Dest::ToneSearch);
    t.wait(100);
    t.tap(Key::Hash);
    assert_eq!(t.app.screen(), Screen::Home);
    match t.app.pop_write() {
        Some(ChanWrite::Put { ch, .. }) => assert_eq!(ch.cfg.rx_tone, 1000),
        w => panic!("{w:?}"),
    }
}

/// PIN-5, KEY-23: channel-only mode refuses V/M, hold 9 (copy) and MENU 0, while hold V/M (the zone list) still
/// opens; hold # leaves it (the PIN first when one is set), never the
/// key lock.
#[test]
fn channel_only_mode() {
    let mut t = T::new();
    t.tap(Key::VfoMem);
    t.tap(Key::VfoMem);
    assert!(!t.app.slot(0).on_channel());
    assert!(crate::screens::groups::change(&mut t.app, &[9, 3, 2], 1));
    assert!(t.app.set.chan_only);
    assert!(t.app.slot(0).on_channel(), "slots with a channel go to it");
    t.tap(Key::VfoMem);
    assert!(t.app.slot(0).on_channel(), "no V/M");
    t.hold(Key::Digit(9));
    assert_eq!(t.app.screen(), Screen::Home, "no copy to the VFO");
    t.hold(Key::VfoMem);
    assert!(matches!(t.app.screen(), Screen::Zones(_)), "zone browsing works: {:?}", t.app.screen());
    t.tap(Key::Exit);
    t.hold(Key::Menu);
    assert_eq!(t.app.screen(), Screen::Home, "no MENU 0");
    t.tap(Key::Menu);
    assert_eq!(t.app.screen(), Screen::Home, "no quick settings");
    t.hold(Key::Hash);
    assert!(!t.app.set.chan_only && !t.app.locked(), "hold # leaves, and doesn't lock");
    // With a PIN: asked first.
    (t.app.set.pin, t.app.set.pin_len) = (1234, 4);
    crate::screens::groups::change(&mut t.app, &[9, 3, 2], 1);
    t.hold(Key::Hash);
    assert!(matches!(t.app.screen(), Screen::Pin(_)));
    for d in [1, 2, 3, 4] {
        t.tap(Key::Digit(d));
    }
    t.tap(Key::Menu);
    assert!(!t.app.set.chan_only);
    assert!(!t.app.locked());
}

/// The settings record keeps show hidden and channel-only in one byte; an older record's byte (show hidden
/// alone) still reads.
#[test]
fn packed_flags_round_trip() {
    use crate::persist::{Persist, Sink, Source};
    struct V(Vec<u8>);
    impl Sink for V {
        fn put(&mut self, b: &[u8]) {
            self.0.extend_from_slice(b);
        }
    }
    struct R<'a>(&'a [u8]);
    impl Source for R<'_> {
        fn take(&mut self, b: &mut [u8]) -> bool {
            if self.0.len() < b.len() {
                return false;
            }
            b.copy_from_slice(&self.0[..b.len()]);
            self.0 = &self.0[b.len()..];
            true
        }
    }
    let mut a = crate::settings::Settings::DEFAULT;
    (a.show_hidden, a.chan_only) = (false, true);
    let mut v = V(Vec::new());
    a.save(&mut v);
    let mut b = crate::settings::Settings::DEFAULT;
    b.load(&mut R(&v.0));
    assert_eq!((b.show_hidden, b.chan_only), (false, true));
    v.0.truncate(v.0.len() - super::watch_heard::then_len()); // a record from before every field after the flags ...
    *v.0.last_mut().unwrap() = 1; // ... and before the flags: the show-hidden bool
    let mut c = crate::settings::Settings::DEFAULT;
    c.load(&mut R(&v.0));
    assert_eq!((c.show_hidden, c.chan_only), (true, false));
}
