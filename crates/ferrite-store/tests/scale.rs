//! CHAN-36.1: the store at its limits stays quick. 500 zones made and 4,000 channels written into them (8 each),
//! counting the flash reads: a pass over the channel slots (4,000 reads) per new zone and per zone count made a
//! full bulk write take minutes on the radio (black, Nov 19, 2026: the link silent meanwhile, as the store worked).
use ferrite_app::chan::{channel, ChanWrite};
use ferrite_app::data::{NoData, ZoneCfg};
use ferrite_app::slot::DEFAULT_CFG;
use ferrite_app::settings::Settings;
use ferrite_app::screens::alert::AlertInfo;
use ferrite_app::App;
use ferrite_store::sim::SimFlash;
use ferrite_store::FlashStore;

type Store = FlashStore<SimFlash, NoData>;

fn rd(s: &Store) -> (u64, u64) {
    let n = s.nor();
    (n.reads, n.read_bytes)
}

fn drain(s: &Store) {
    while let Some(j) = s.next_job() {
        ferrite_store::block(s.run(j));
    }
}

/// CHAN-36.1: reads within budget: making 500 zones (no pass each: a new zone holds none), each channel write (a
/// handful, however full), and the zone counts after it all (one pass for up to 8 zones).
#[test]
fn the_store_at_its_limits_stays_quick() {
    let s: Store = FlashStore::mount(SimFlash::new(), NoData);
    s.set_deferred(true);
    for z in 0..500u16 {
        let _ = z;
        assert!(s.add_zone(b"Z", &ZoneCfg { shown: true, ..ZoneCfg::default() }));
        drain(&s);
    }
    let (r0, b0) = rd(&s);
    println!("500 zones: {r0} reads, {b0} B");
    assert!(r0 < 50_000, "500 new zones: {r0} reads (a pass each would be 2 million)");
    let mut last = rd(&s);
    for n in 0..4000u16 {
        while !s.apply(ChanWrite::Put { ch: channel(n, n / 8, 43_000_000 + n as u32 * 250, "LIM", DEFAULT_CFG), from: None }) {
            drain(&s);
        }
        drain(&s);
        if n % 500 == 499 {
            let now = rd(&s);
            assert!(now.0 - last.0 <= 500 * 8, "channels to {}: {} reads for 500", n + 1, now.0 - last.0);
            last = now;
        }
    }
    let before = rd(&s);
    let set: &'static mut Settings = Box::leak(Box::new(Settings::DEFAULT));
    let alert: &'static mut AlertInfo = Box::leak(Box::new(AlertInfo::NONE));
    let app = App::new(set, alert);
    for t in [1_000_000u32, 2_000_000, 3_000_000] {
        s.autosave(t, &app);
        drain(&s);
    }
    let after = rd(&s);
    println!("counts: {} reads, {} B", after.0 - before.0, after.1 - before.1);
    assert!(after.0 - before.0 < 400_000, "500 zone counts: {} reads (a pass each would be 2 million)", after.0 - before.0);
}
