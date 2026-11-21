//! The store on the simulated flash: round trips, first boot and its range presets, a damaged or foreign header,
//! thousands of power cuts at random points of settings writes, channel, zone and preset changes, and of the
//! recovery after them, and reads while a write is under way. After every cut the store must mount, and every
//! setting, channel, zone and preset must read either as it was before the interrupted change or as it would be
//! after it, never anything else; the zone lists must agree with the channels.

use ferrite_app::chan::{channel, ChanWrite, Channel, ChannelStore, Name, MAX_CHANNELS, NO_ZONE};
use ferrite_app::data::{Data, NoData, Op, RangePreset, ZoneCfg};
use ferrite_app::screens::alert::AlertInfo;
use ferrite_app::settings::Settings;
use ferrite_app::slot::DEFAULT_CFG;
use ferrite_app::App;
use ferrite_store::sim::SimFlash;
use ferrite_store::store::Mounted;
use ferrite_store::{block, FlashStore};
use ferrite_ui::text::Buf;

type Store = FlashStore<SimFlash, NoData>;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

fn app() -> App {
    let set: &'static mut Settings = Box::leak(Box::new(Settings::DEFAULT));
    let alert: &'static mut AlertInfo = Box::leak(Box::new(AlertInfo::NONE));
    App::new(set, alert)
}

fn mount(f: SimFlash) -> Store {
    FlashStore::mount(f, NoData)
}

fn unmount(s: Store) -> SimFlash {
    s.into_parts().0
}

fn settings_of(s: &Store) -> Option<Settings> {
    let mut set = Settings::DEFAULT;
    s.load_settings(&mut set).then_some(set)
}

/// Everything the store holds that a change can touch.
#[derive(Clone, PartialEq)]
struct Snap {
    chans: Vec<Option<Channel>>,
    zones: Vec<Option<(Name, ZoneCfg)>>,
    presets: Vec<RangePreset>,
}

fn snap(s: &Store) -> Snap {
    let chans = (0..MAX_CHANNELS).map(|n| s.by_num(n).map(|x| x.0)).collect();
    let zones = (0..s.zones()).map(|z| s.zone_name(z).map(|n| (n, s.zone_cfg(z)))).collect();
    let presets = (0..s.presets()).filter_map(|i| s.preset(i)).collect();
    Snap { chans, zones, presets }
}

/// The zone lists agree with the records.
fn index_agrees(s: &impl ChannelStore) {
    let mut seen = 0u16;
    for z in (0..s.zones()).chain([NO_ZONE]) {
        let mut last: Option<u16> = None;
        for i in 0..s.zone_len(z) {
            let c = s.zone_at(z, i).expect("indexed channel exists");
            assert_eq!(c.zone, z, "channel {} indexed in zone {z}", c.num);
            assert!(last.is_none_or(|l| c.num > l), "zone {z} in number order");
            last = Some(c.num);
            assert_eq!(s.by_num(c.num).map(|x| x.1), Some(i), "position of {}", c.num);
            seen += 1;
        }
    }
    let real = (0..MAX_CHANNELS).filter(|&n| s.by_num(n).is_some()).count() as u16;
    assert_eq!(seen, real, "index covers every channel");
    assert_eq!(s.count(), real);
    for i in 0..real {
        assert!(s.by_number(i).is_some());
    }
    assert!(s.by_number(real).is_none());
}

fn cfg_with(tone: u16, off: i32) -> ferrite_app::slot::SlotCfg {
    let mut c = DEFAULT_CFG;
    c.tx_tone = tone;
    c.offset = off;
    c
}

/// A store with a few zones, channels and a preset, settings saved once.
fn seeded() -> SimFlash {
    let s = mount(SimFlash::new());
    assert_eq!(s.mounted(), Mounted::FirstBoot);
    let zc = ZoneCfg { scan_all: true, shown: true, ..ZoneCfg::default() };
    for n in [&b"Local"[..], b"Repeaters", b"Air"] {
        assert!(s.add_zone(n, &zc));
    }
    for (i, num) in [0u16, 1, 2, 9, 11, 14, 63, 64, 65, 130, 999].iter().enumerate() {
        let c = channel(*num, (i % 4) as u16 % 3, 14_400_000 + i as u32 * 2500, "Chan", cfg_with(1000, if i % 2 == 0 { 60_000 } else { 0 }));
        assert!(s.put_with_position(c, None));
    }
    assert!(s.add_preset(&RangePreset { name: Buf::from(b"Air"), start: 11_800_000, end: 13_700_000, step: 10, mode: 2, narrow: false, skips: 0, resume: 0 }));
    let a = app();
    assert!(s.save(&a));
    unmount(s)
}

#[test]
fn round_trip_and_first_boot() {
    let f = seeded();
    let s = mount(f);
    assert_eq!(s.mounted(), Mounted::Ok);
    assert_eq!(s.zones(), 3);
    assert_eq!(s.zone_name(1).unwrap().as_bytes(), b"Repeaters");
    assert_eq!(s.count(), 11);
    assert_eq!(s.presets(), 1);
    index_agrees(&s);
    // Settings and slots.
    let mut a = app();
    a.settings_mut().set_callsign(b"ve3abc");
    a.settings_mut().home_ch = Some(11);
    a.set_watch(2);
    a.set_channel(1, 1, 0, &s);
    assert!(s.save(&a));
    let s = mount(unmount(s));
    let mut set = Settings::DEFAULT;
    assert!(s.load_settings(&mut set));
    assert_eq!(set.callsign.as_bytes(), b"VE3ABC");
    assert_eq!(set.home_ch, Some(11));
    let mut b = app();
    assert!(s.restore(&mut b));
    assert_eq!(b.slot(1).ch.map(|c| c.num), a.slot(1).ch.map(|c| c.num));
    // The save timer: nothing to save right after restoring, a save 2 s after a change is seen; PWR-26.14: the
    // state is looked at every 500 ms at most, so a change at 100 is seen at 500.
    assert!(!s.autosave(0, &b));
    b.set_watch(0);
    assert!(!s.autosave(100, &b));
    assert!(!s.autosave(500, &b));
    assert!(!s.autosave(2400, &b));
    assert!(s.autosave(2500, &b));
    assert!(!s.flush(&b));
}

/// A layout header with the given version (and a good CRC) at `at`.
fn put_header(f: &mut SimFlash, at: u32, version: u16) {
    let mut h = [0xFFu8; 20];
    h[..4].copy_from_slice(&ferrite_store::store::HEADER_MAGIC.to_le_bytes());
    h[4..6].copy_from_slice(&version.to_le_bytes());
    let mut c = ferrite_store::Crc32::new();
    c.update(&h[..16]);
    h[16..20].copy_from_slice(&c.get().to_le_bytes());
    let a = at as usize;
    f.mem[a..a + 4096].fill(0xFF);
    f.mem[a..a + 20].copy_from_slice(&h);
}

/// UPD-16 (decision 333): a chip holding another layout's data is left alone: another version in the header at
/// 0x000000, or layout 2's header at its old place 0x02C000 with no layout 3 header. Nothing is read or written
/// (not even the scratch ring's recovery), nothing is stored this power-on, and the version found is told.
#[test]
fn another_layouts_data_is_left_alone() {
    use ferrite_core::layout::{LAYOUT_HEADER, V2_HEADER};
    let mut f = seeded();
    put_header(&mut f, LAYOUT_HEADER.start, 4);
    f.ops = 0;
    let before = f.mem.clone();
    let s = mount(f);
    assert_eq!((s.mounted(), s.layout_found()), (Mounted::Update(4), Some(4)));
    assert!(!s.first_boot() && s.count() == 0 && s.presets() == 0 && s.zones() == 0 && s.by_num(0).is_none());
    assert!(!s.apply(ChanWrite::Put { ch: channel(5, NO_ZONE, 14_600_000, "X", DEFAULT_CFG), from: None }), "writes refused");
    assert!(!s.save(&app()));
    let f = unmount(s);
    assert!(f.mem == before && f.ops == 0, "nothing written");
    // layout 2: its header at 0x02C000, nothing at 0x000000 (an update from a layout 2 firmware)
    let mut f = SimFlash::new();
    put_header(&mut f, V2_HEADER.start, 2);
    f.mem[0x2D000..0x2D100].fill(0x5A); // its settings
    let before = f.mem.clone();
    let s = mount(f);
    assert_eq!(s.mounted(), Mounted::Update(2));
    let f = unmount(s);
    assert!(f.mem == before && f.ops == 0, "layout 2's data untouched");
}

/// INST-6, BOOT-12: no Ferrite header and none of Ferrite's records (a blank chip, a chip of noise or of stock's
/// data): a first install, formatted. UPD-16 (decision 339): no header but Ferrite's records there (a damaged
/// header, a move cut where it shouldn't be): never formatted; the radio waits for Console, nothing written.
#[test]
fn no_header_formats_only_without_ferrites_records() {
    use ferrite_core::layout::LAYOUT_MOVING;
    let mut f = seeded();
    let h = ferrite_core::layout::LAYOUT_HEADER.start as usize;
    f.mem[h + 4] = 1; // its CRC no longer agrees
    let before = f.mem.clone();
    let s = mount(f);
    assert_eq!(s.mounted(), Mounted::Update(LAYOUT_MOVING), "the settings record is there: not formatted");
    let f = unmount(s);
    assert!(f.mem == before, "nothing written");
    // Only channels (no settings yet): the same.
    let mut f = f;
    f.mem[0x1000..0x3000].fill(0xFF);
    assert_eq!(mount(f.clone()).mounted(), Mounted::Update(LAYOUT_MOVING));
    // Layout 2's settings alone (its header gone): the same.
    let mut g = SimFlash::new();
    g.mem[0x2D000..0x2F000].copy_from_slice(&before[0x1000..0x3000]);
    assert_eq!(mount(g).mounted(), Mounted::Update(LAYOUT_MOVING));
    // A blank chip: formatted.
    assert_eq!(mount(SimFlash::new()).mounted(), Mounted::FirstBoot);
    // A chip full of noise (stock's data): formatted.
    let mut f = SimFlash::new();
    let mut r = Rng(7);
    for b in f.mem.iter_mut() {
        *b = r.next() as u8;
    }
    let s = mount(f);
    assert_eq!(s.mounted(), Mounted::FirstBoot);
    assert_eq!(s.count(), 0);
    assert_eq!(s.zones(), 0);
    index_agrees(&s);
}

/// Runs `op` on a copy to count its flash operations, then on `f` with the power cut at a random one of them,
/// then mounts again (sometimes cutting that too) and returns the store and what it was before and after.
fn cut_during(f: SimFlash, r: &mut Rng, op: &dyn Fn(&Store)) -> (Store, Snap, Snap) {
    let s = mount(f.clone());
    let before = snap(&s);
    let n0 = s.nor().ops;
    op(&s);
    let ops = s.nor().ops - n0;
    let after = snap(&s);
    let s = mount(f);
    if ops > 0 {
        let mut nor = s.nor();
        nor.cut_at = Some(nor.ops + 1 + r.below(ops));
        nor.seed(r.next());
    }
    op(&s);
    let mut f = unmount(s);
    f.power_on();
    if r.below(4) == 0 {
        // The power goes again while the store recovers.
        f.cut_at = Some(f.ops + 1 + r.below(20_000));
        f.seed(r.next());
        f = unmount(mount(f));
        f.power_on();
    }
    (mount(f), before, after)
}

#[test]
fn power_cuts_during_settings_writes() {
    let mut r = Rng(0x5EED_0001);
    let mut f = seeded();
    let mut a = app();
    let mut cur = settings_of(&mount(f.clone())).unwrap();
    let (mut old, mut new) = (0, 0);
    for i in 0..3000u32 {
        a.settings_mut().pin = i;
        a.settings_mut().fm_freq = (i % 2000) as u16;
        a.settings_mut().set_callsign(if i % 3 == 0 { b"VE3ABC" } else { b"VA3XYZ9" });
        let next = *a.settings();
        let (s, _, _) = cut_during(f, &mut r, &|s| {
            s.save(&a);
        });
        let got = settings_of(&s).expect("settings survive");
        assert!(got == cur || got == next, "iteration {i}: neither old nor new (pin {})", got.pin);
        assert_eq!(s.mounted(), Mounted::Ok);
        if got == next && got != cur {
            new += 1;
        } else {
            old += 1;
        }
        cur = got;
        // Half the time the next save gets through, so both copies take turns.
        if r.below(2) == 0 {
            assert!(s.save(&a));
            cur = next;
            assert!(settings_of(&s) == Some(cur));
        }
        f = unmount(s);
    }
    // A cut anywhere in the copy being written leaves it torn (the CRC is its last bytes), so nearly every cut
    // reads as before; a cut on the very last byte can still complete it.
    eprintln!("settings: {old} cuts left the old record, {new} the new one");
    assert_eq!(old + new, 3000);
}

#[test]
fn power_cuts_during_channel_zone_and_preset_changes() {
    let mut r = Rng(0xC0FF_EE01);
    let mut f = seeded();
    let (mut changed, mut old, mut new, mut ops_total) = (0, 0, 0, 0u64);
    for i in 0..2500u32 {
        let pick = r.below(9);
        let num = [0u16, 1, 2, 9, 11, 14, 63, 64, 65, 130, 999, 5, 300][r.below(13) as usize];
        let to = r.below(MAX_CHANNELS as u64) as u16;
        let z = r.below(4) as u16;
        let freq = 14_400_000 + r.below(400) as u32 * 1250;
        let op = move |s: &Store| {
            match pick {
                0 | 1 => {
                    let mut c = s.by_num(num).map(|x| x.0).unwrap_or_else(|| channel(num, NO_ZONE, freq, "New", DEFAULT_CFG));
                    c.rx = freq;
                    c.set_name(b"Edited");
                    s.apply(ChanWrite::Put { ch: c, from: None });
                }
                2 => {
                    s.apply(ChanWrite::Delete(num));
                }
                3 => {
                    s.request(Op::Renumber(num, to));
                }
                4 => {
                    s.request(Op::Move(num, if z == 3 { NO_ZONE } else { z }));
                }
                5 => {
                    s.request(Op::RenameZone(z.min(2), Name::from(b"Renamed")));
                }
                6 => {
                    if r_zone_delete(i) {
                        s.request(Op::DeleteZone(z.min(2)));
                    } else {
                        s.request(Op::NewZone);
                    }
                }
                7 => {
                    let p = RangePreset { name: Buf::from(b"P"), start: freq, end: freq + 100_000, step: 3, mode: 0, narrow: true, skips: 1, resume: 0 };
                    if s.presets() > 0 && i % 2 == 0 {
                        s.request(Op::Preset(0, p));
                    } else if s.presets() < 3 {
                        s.request(Op::NewPreset);
                    } else {
                        s.request(Op::DeletePreset(1));
                    }
                }
                _ => {
                    s.request(Op::Skip(num, i % 2 == 0));
                }
            }
        };
        let (s, before, after) = cut_during(f, &mut r, &op);
        assert_eq!(s.mounted(), Mounted::Ok, "iteration {i}");
        ops_total += s.nor().ops;
        let now = snap(&s);
        if before != after {
            changed += 1;
            if now == after {
                new += 1;
            } else if now == before {
                old += 1;
            }
        }
        for n in 0..MAX_CHANNELS as usize {
            assert!(
                now.chans[n] == before.chans[n] || now.chans[n] == after.chans[n],
                "iteration {i} (op {pick}): channel {n} is {:?}, was {:?}, would be {:?}",
                now.chans[n],
                before.chans[n],
                after.chans[n]
            );
        }
        for (zi, z) in now.zones.iter().enumerate() {
            let ok = before.zones.get(zi) == Some(z) || after.zones.get(zi) == Some(z);
            assert!(ok, "iteration {i}: zone {zi}");
        }
        assert!(now.presets == before.presets || now.presets == after.presets, "iteration {i}: presets");
        index_agrees(&s);
        f = unmount(s);
    }
    eprintln!("changes: {changed} of 2500 ops changed something; after the cut {old} read as before, {new} as after; {ops_total} flash ops");
    assert!(changed > 1000 && old > 300 && new > 300, "cuts landed on both sides");
}

fn r_zone_delete(i: u32) -> bool {
    i.is_multiple_of(3)
}

#[test]
fn settings_reset_erases_every_channel() {
    let s = mount(seeded());
    assert!(s.apply(ChanWrite::EraseAll));
    assert_eq!(s.count(), 0);
    let s = mount(unmount(s));
    assert_eq!(s.count(), 0);
    assert!(s.by_num(11).is_none());
    index_agrees(&s);
}

/// BOOT-14.3, BOOT-14.4, BOOT-14.5: the reset wipes every record area but the settings and calibration, and is a first
/// boot again: the next save after setup seeds as on a new radio. Outside the US and Canada no WX zone.
#[test]
fn reset_wipes_and_seeds_again() {
    let s = mount(seeded());
    assert!(s.count() > 0 && s.zones() > 0 && s.presets() > 0);
    let cal = s.nor().mem[0xF000..0x10000].to_vec();
    assert!(s.request(Op::Wipe));
    assert_eq!((s.count(), s.zones(), s.presets()), (0, 0, 0));
    assert!(s.first_boot());
    assert_eq!(s.nor().mem[0xF000..0x10000], cal[..], "calibration untouched");
    let mut a = app();
    a.settings_mut().set_region(Some(ferrite_core::txplan::Region::Uk));
    a.settings_mut().setup_done = true;
    assert!(s.save(&a));
    assert!(s.presets() > 0);
    assert_eq!(s.zones(), 0, "no WX zone in the UK");
    let s = mount(unmount(s));
    assert_eq!(s.count(), 0);
    index_agrees(&s);
}

#[test]
fn renumber_carries_the_position() {
    let s = mount(seeded());
    let c = s.by_num(11).unwrap().0;
    assert!(s.put_with_position(c, Some(ferrite_app::data::Pos { lat: 45, lon: -75 })));
    assert!(s.request(Op::Renumber(11, 500)));
    assert!(s.by_num(11).is_none());
    assert_eq!(s.position(500), Some(ferrite_app::data::Pos { lat: 45, lon: -75 }));
    let _ = s.version();
}

/// CHAN-2, CHAN-29: a channel written with its position (the terminal's `ch N pos=`), then without: the record
/// keeps the rest, `located` answers from the one record, and a plain write keeps the position.
#[test]
fn put_with_and_without_a_position() {
    use ferrite_app::data::Pos;
    let s = mount(seeded());
    let c = s.by_num(11).unwrap().0;
    let p = Pos { lat: 454_215_000, lon: -756_972_000 };
    assert!(s.apply(ChanWrite::PutPos { ch: c, pos: Some(p) }));
    assert_eq!(s.position(11), Some(p));
    assert_eq!(s.located(11), Some((c, p)));
    let renamed = { let mut r = c; r.set_name(b"Renamed"); r };
    assert!(s.apply(ChanWrite::Put { ch: renamed, from: None }));
    assert_eq!(s.position(11), Some(p), "a plain write keeps it");
    assert!(s.apply(ChanWrite::PutPos { ch: renamed, pos: None }));
    assert_eq!(s.position(11), None);
    assert_eq!(s.located(11), None);
    assert_eq!(s.by_num(11).unwrap().0.name(), b"Renamed");
    let s = mount(unmount(s));
    assert_eq!(s.position(11), None, "after a power cycle too");
    index_agrees(&s);
}

#[test]
fn first_boot_after_setup_seeds_the_regions_presets() {
    let s = mount(SimFlash::new());
    assert!(s.first_boot());
    let mut a = app();
    // A save during setup seeds nothing.
    assert!(s.save(&a));
    assert_eq!(s.presets(), 0);
    a.settings_mut().set_region(Some(ferrite_core::txplan::Region::Us));
    a.settings_mut().setup_done = true;
    assert!(s.save(&a));
    assert!(!s.first_boot());
    let names: Vec<_> = (0..s.presets()).map(|i| s.preset(i).unwrap().name).collect();
    let names: Vec<&[u8]> = names.iter().map(|n| n.as_bytes()).collect();
    assert_eq!(names, [&b"2 m"[..], b"70 cm", b"1.25 m", b"Airband", b"Marine", b"Weather", b"GMRS/FRS"]);
    // Deleting them all is kept: no new seeding on later saves or boots.
    for _ in 0..s.presets() {
        assert!(s.request(Op::DeletePreset(0)));
    }
    assert!(s.save(&a));
    let s = mount(unmount(s));
    assert_eq!(s.presets(), 0);
    // WX-1: the US and Canada get the weather channels as a "Weather" zone, receive only.
    assert_eq!(s.zones(), 1);
    assert_eq!(s.zone_name(0).unwrap().as_bytes(), b"Weather");
    assert_eq!(s.zone_len(0), 7);
    let wx: Vec<_> = (0..7).map(|i| s.zone_at(0, i).unwrap()).collect();
    assert_eq!(wx.iter().map(|c| c.rx).collect::<Vec<_>>(), (0..7).map(|k| 16_240_000 + k * 2_500).collect::<Vec<_>>());
    assert!(wx.iter().all(|c| c.flags & ferrite_app::chan::FLAG_RX_ONLY != 0 && c.cfg.mode == 0));
    assert_eq!(wx[0].name(), b"WX1");
    assert_eq!(wx.iter().map(|c| c.num).collect::<Vec<_>>(), (990..997).collect::<Vec<_>>(), "channels 991-997 as shown");
    // Europe: no weather or GMRS, PMR446 instead, 2 m to 146 MHz.
    let e: Vec<_> = ferrite_store::store::default_presets(ferrite_core::txplan::Region::Europe).collect();
    assert!(e.iter().any(|p| p.name.as_bytes() == b"PMR446") && !e.iter().any(|p| p.name.as_bytes() == b"Weather"));
    assert_eq!(e[0].end, 14_600_000);
}

/// Writes past the last channel number fail; a zone deleted through the channel queue (CHAN-12) leaves its
/// channels in no zone; an existing zone written again is renamed in place; 512 zones and 16 presets at most. The
/// Storage task checks again what the screens checked: a renumber onto a channel stored meanwhile, or an edit of
/// one deleted meanwhile, is refused and changes nothing.
#[test]
fn out_of_range_full_and_stale_requests() {
    use ferrite_store::block;
    use ferrite_store::store::Job;
    let s = mount(SimFlash::new());
    let c = |n: u16| channel(n, NO_ZONE, 14_652_000, "Ch", DEFAULT_CFG);
    assert!(!s.apply(ChanWrite::Put { ch: c(MAX_CHANNELS), from: None }));
    assert!(!s.apply(ChanWrite::PutPos { ch: c(MAX_CHANNELS), pos: None }));
    assert!(!s.apply(ChanWrite::Delete(MAX_CHANNELS)));
    assert!(s.add_zone(b"Local", &ZoneCfg::default()) && s.add_zone(b"Events", &ZoneCfg::default()));
    assert!(s.apply(ChanWrite::Put { ch: channel(1, 0, 14_652_000, "One", DEFAULT_CFG), from: None }));
    assert!(s.apply(ChanWrite::Put { ch: c(2), from: None }));
    assert!(s.apply(ChanWrite::DeleteZone(0)));
    assert_eq!((s.zones(), s.zone_of(1)), (2, Some(NO_ZONE)), "an earlier zone stays, empty; its channel in no zone");
    assert!(s.request(Op::PutZone(1, Buf::from(b"Renamed"), ZoneCfg::default())));
    assert_eq!((s.zones(), s.zone_name(1).unwrap().as_bytes()), (2, &b"Renamed"[..]));
    while s.zones() < ferrite_app::chan::MAX_ZONES {
        assert!(s.add_zone(b"Z", &ZoneCfg::default()));
    }
    assert!(!s.add_zone(b"Z", &ZoneCfg::default()) && !s.request(Op::NewZone));
    while s.presets() < 16 {
        assert!(s.request(Op::NewPreset));
    }
    let p = RangePreset { name: Buf::from(b"P"), start: 14_400_000, end: 14_800_000, step: 10, mode: 0, narrow: true, skips: 0, resume: 0 };
    assert!(!s.add_preset(&p) && !s.request(Op::NewPreset));
    let before = (s.get(1), s.get(2));
    assert!(!block(s.run(Job::Op(Op::Renumber(1, 2)))), "2 was stored meanwhile");
    assert!(!block(s.run(Job::Op(Op::Skip(7, true)))), "7 is gone");
    assert!((s.get(1), s.get(2)) == before);
}

/// CHAN-25: Recent lists the channels used, newest first, in RAM. The lookups the screens use: the next free
/// number from a given one (wrapping past the last), a number's zone, a channel with its position.
#[test]
fn channel_lookups() {
    let s = mount(SimFlash::new());
    for n in [5u16, 6, MAX_CHANNELS - 2, MAX_CHANNELS - 1] {
        assert!(s.queue_write(ChanWrite::Put { ch: channel(n, NO_ZONE, 14_652_000, "Ch", DEFAULT_CFG), from: None }));
    }
    assert!(s.add_zone(b"Local", &ZoneCfg::default()));
    assert!(s.request(Op::Move(6, 0)));
    assert_eq!((s.free_num(5), s.free_num(MAX_CHANNELS - 2), s.free_num(0)), (Some(7), Some(0), Some(0)));
    assert_eq!((s.zone_of(5), s.zone_of(6), s.zone_of(7), s.zone_of(MAX_CHANNELS)), (Some(NO_ZONE), Some(0), None, None));
    assert!(s.located(5).is_none() && s.located(7).is_none() && s.get(MAX_CHANNELS).is_none(), "no position, no channel");
    assert!(s.apply(ChanWrite::PutPos { ch: s.get(5).unwrap(), pos: Some(ferrite_app::data::Pos { lat: 45, lon: -75 }) }));
    assert!(s.located(5).is_some_and(|(c, p)| c.num == 5 && p.lat == 45 && p.lon == -75));
    s.used(5);
    s.used(998);
    s.tuned(6);
    assert_eq!((s.recent(0), s.recent(1), s.recent(2), s.recent(3)), (Some(6), Some(998), Some(5), None));
}

/// WX-1: a first boot that already has range presets (the simulator's sample radio) keeps its own after setup and
/// gets no seeded ones, and with channels of its own no weather zone either; its channels stay as they were.
#[test]
fn first_boot_keeps_presets_and_channels_already_there() {
    let s = mount(SimFlash::new());
    let own = RangePreset { name: Buf::from(b"Mine"), start: 14_400_000, end: 14_600_000, step: 10, mode: 0, narrow: true, skips: 0, resume: 0 };
    assert!(s.add_preset(&own));
    let mine = channel(992, NO_ZONE, 14_652_000, "Simplex", DEFAULT_CFG);
    assert!(s.apply(ChanWrite::Put { ch: mine, from: None }));
    let mut a = app();
    a.settings_mut().set_region(Some(ferrite_core::txplan::Region::Canada));
    a.settings_mut().setup_done = true;
    assert!(s.save(&a));
    assert_eq!(s.mounted(), Mounted::Ok);
    assert!(s.presets() == 1 && s.preset(0).unwrap().name.as_bytes() == b"Mine", "only its own preset");
    assert!(s.get(992).unwrap() == mine, "a number in use left alone");
    assert_eq!((s.zones(), s.count()), (0, 1), "no weather zone");
}

/// A flash that stays busy a few polls after each program and erase, and whose pause yields once: the store's
/// writes take several polls, as on the radio, and the test reads in between, as the UI does.
struct Slow {
    f: SimFlash,
    left: u32,
}

impl ferrite_store::Nor for Slow {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> ferrite_store::Result {
        self.f.read(addr, buf)
    }
    fn start_program(&mut self, addr: u32, data: &[u8]) -> ferrite_store::Result {
        self.left = 2;
        self.f.start_program(addr, data)
    }
    fn start_erase(&mut self, addr: u32) -> ferrite_store::Result {
        self.left = 3;
        self.f.start_erase(addr)
    }
    fn busy(&mut self) -> ferrite_store::Result<bool> {
        let b = self.left > 0;
        self.left = self.left.saturating_sub(1);
        Ok(b)
    }
    async fn pause() {
        let mut once = false;
        core::future::poll_fn(|cx| {
            if once {
                core::task::Poll::Ready(())
            } else {
                once = true;
                cx.waker().wake_by_ref();
                core::task::Poll::Pending
            }
        })
        .await
    }
}

#[test]
fn reads_during_a_queued_write_see_old_or_new() {
    use ferrite_store::Nor;
    use std::future::Future;
    use std::task::{Context, Poll, Waker};
    let mut r = Rng(0xD1E5_0001);
    let mut f = seeded();
    let mut polls = 0u32;
    for i in 0..300u32 {
        let s: FlashStore<Slow, NoData> = FlashStore::new(Slow { f, left: 0 }, NoData);
        ferrite_store::block(s.mount_async());
        s.set_deferred(true);
        let num = [0u16, 1, 2, 9, 11, 63, 64, 999, 7][r.below(9) as usize];
        let before = s.by_num(num).map(|x| x.0);
        let others: Vec<_> = (0..MAX_CHANNELS).filter(|&n| n != num).map(|n| s.by_num(n).map(|x| x.0)).collect();
        let mut c = before.unwrap_or_else(|| channel(num, NO_ZONE, 14_600_000, "New", DEFAULT_CFG));
        c.rx = 14_500_000 + i * 1250;
        c.set_name(b"Queued");
        let w = if i % 4 == 3 { ChanWrite::Delete(num) } else { ChanWrite::Put { ch: c, from: None } };
        let after = if i % 4 == 3 { None } else { Some(c) };
        assert!(s.apply(w));
        assert!(s.has_jobs() && !s.idle());
        let job = s.next_job().unwrap();
        {
        let mut fut = std::pin::pin!(s.run(job));
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            let done = matches!(fut.as_mut().poll(&mut cx), Poll::Ready(true));
            // The UI reads in between: the channel and its sector neighbours are whole, old or new.
            let now = s.by_num(num).map(|x| x.0);
            assert!(now == before || now == after, "iteration {i}: channel {num} read {now:?} mid-write");
            let now_others: Vec<_> = (0..MAX_CHANNELS).filter(|&n| n != num).map(|n| s.by_num(n).map(|x| x.0)).collect();
            assert!(now_others == others, "iteration {i}: another channel changed mid-write");
            if done {
                break;
            }
            polls += 1;
        }
        }
        assert!(s.idle());
        assert_eq!(s.by_num(num).map(|x| x.0), after);
        index_agrees(&s);
        let (mut slow, _) = s.into_parts();
        let mut probe = [0u8; 1];
        assert!(slow.read(0x02C000, &mut probe).is_ok());
        f = slow.f;
    }
    eprintln!("queued writes: {polls} polls with reads in between");
    assert!(polls > 1000);
}

/// BOOT-12, SAFE-1, INST-6: a first boot on a chip full of stock's (or any) data, with the power cut at points through
/// the format: the next boot formats again and comes up as a first boot, and nothing outside the areas this
/// firmware uses changes: not calibration, not the SSB patch (kept by a format), not the planned regions.
#[test]
fn power_cuts_during_the_first_format() {
    use ferrite_core::layout::*;
    let mut f = SimFlash::new();
    let mut r = Rng(11);
    for b in f.mem.iter_mut() {
        *b = r.next() as u8;
    }
    let before = f.mem.clone();
    // How many flash operations a whole first mount takes.
    let total = {
        let s = mount(f.clone());
        assert_eq!(s.mounted(), Mounted::FirstBoot);
        let n = s.nor().ops;
        n
    };
    assert!(total > 10);
    let used = [LAYOUT_HEADER, MIGRATE_MARK, SETTINGS, TRIMS, PRESETS, SATELLITES, CHANNELS, ZONES, APRS, CONTACTS, HEARD, MESSAGES, WAYPOINTS, EVENT_LOG, SCRATCH_RING];
    let addr = |i: usize| if i < ferrite_store::sim::LOW { i as u32 } else { ferrite_store::sim::HIGH.start + (i - ferrite_store::sim::LOW) as u32 };
    let kept = |i: usize| !used.iter().any(|r| r.contains(&addr(i)));
    assert!(kept(SSB_PATCH.start as usize) && kept(CALIBRATION.start as usize) && kept(MAILBOX.start as usize));
    for cut in [1, 2, total / 3, total / 2, total - 2, total - 1] {
        let mut g = f.clone();
        g.cut_at = Some(cut);
        let s = mount(g);
        let mut g = unmount(s);
        g.power_on();
        let s = mount(g);
        assert_eq!(s.mounted(), Mounted::FirstBoot, "cut at op {cut} of {total}: formats again");
        assert_eq!(s.count(), 0);
        index_agrees(&s);
        let g = unmount(s);
        for (a, (&x, &y)) in before.iter().zip(&g.mem).enumerate() {
            if kept(a) {
                assert_eq!(x, y, "cut at {cut}: byte {a:#x} outside the formatted areas changed");
            }
        }
        assert_eq!(&g.mem[CALIBRATION.start as usize..CALIBRATION.end as usize], &before[CALIBRATION.start as usize..CALIBRATION.end as usize]);
    }
}

fn fake_patch() -> (ferrite_core::ssb::Patch, Vec<u8>) {
    let p: Vec<u8> = (0..1979 * 8).map(|i| if i % 8 == 0 { if i % 80 == 0 { 0x15 } else { 0x16 } } else { (i * 13 + 5) as u8 }).collect();
    (ferrite_core::ssb::Patch { len: p.len() as u32, sha: ferrite_core::sha256::sha256(&p) }, p)
}

/// HF-18, HF-20, HF-21 (SAFE-9.1): the patch installs into SSB_PATCH and checks; the same patch again writes
/// nothing; a reformat (a foreign header) keeps it; a patch that doesn't match its header is refused before
/// anything is erased; nothing outside SSB_PATCH changes.
#[test]
fn ssb_patch_install_and_check() {
    use ferrite_core::layout::SSB_PATCH;
    let (p, patch) = fake_patch();
    let s = mount(SimFlash::new());
    assert_eq!(s.ssb_check(), Ok(None));
    let before = s.nor().mem.clone();
    assert_eq!(ferrite_store::block(s.ssb_install(p, &patch, ferrite_core::ssb::SOURCE_FLASHER)), Ok(true));
    assert_eq!(s.ssb_check(), Ok(Some(p)));
    for (a, (&x, &y)) in before.iter().zip(&s.nor().mem).enumerate() {
        if !SSB_PATCH.contains(&(a as u32)) {
            assert_eq!(x, y, "byte {a:#x} outside SSB_PATCH changed");
        }
    }
    let ops = s.nor().ops;
    assert_eq!(ferrite_store::block(s.ssb_install(p, &patch, ferrite_core::ssb::SOURCE_FLASHER)), Ok(false));
    assert!(s.nor().ops - ops <= 1, "the same patch again: read only");
    let mut wrong = patch.clone();
    wrong[100] ^= 1;
    let ops = s.nor().ops;
    assert!(ferrite_store::block(s.ssb_install(p, &wrong, ferrite_core::ssb::SOURCE_FLASHER)).is_err());
    assert_eq!(s.nor().ops, ops, "refused before any write");
    // A foreign layout header: the store formats, the patch stays.
    let mut f = unmount(s);
    f.mem[ferrite_core::layout::LAYOUT_HEADER.start as usize + 4] = 1;
    let s = mount(f);
    assert_eq!(s.mounted(), Mounted::FirstBoot);
    assert_eq!(s.ssb_check(), Ok(Some(p)), "a format keeps the patch");
}

/// HF-18: the power cut at any point of an install never leaves a patch that checks unless it's the whole one,
/// and the next install completes.
#[test]
fn ssb_patch_install_power_cuts() {
    let (p, patch) = fake_patch();
    let total = {
        let s = mount(SimFlash::new());
        let n0 = s.nor().ops;
        ferrite_store::block(s.ssb_install(p, &patch, 1)).unwrap();
        let n = s.nor().ops - n0;
        n
    };
    for cut in [1, 2, total / 4, total / 2, total - 3, total - 1] {
        let s = mount(SimFlash::new());
        let n0 = s.nor().ops;
        s.nor().cut_at = Some(n0 + cut);
        let _ = ferrite_store::block(s.ssb_install(p, &patch, 1));
        let mut f = unmount(s);
        f.power_on();
        let s = mount(f);
        let c = s.ssb_check().unwrap();
        assert!(c.is_none() || c == Some(p), "cut at {cut}: a half patch never checks");
        assert!(ferrite_store::block(s.ssb_install(p, &patch, 1)).is_ok());
        assert_eq!(s.ssb_check(), Ok(Some(p)), "cut at {cut}: the next install completes");
    }
}

/// CAL-7, TX-14 (TX audit F8): the calibration sector's CRC is recorded in the layout header on the first boot
/// that checks it, then compared at every boot; recording it leaves the header valid (nothing is formatted), and
/// a changed sector reads as changed. Calibration itself is only read.
#[test]
fn calibration_crc_recorded_then_checked() {
    use ferrite_core::txcal::CalCrc;
    let cal = ferrite_core::layout::CALIBRATION.start as usize;
    let mut f = seeded();
    for (i, b) in f.mem[cal..cal + 0x1000].iter_mut().enumerate() {
        *b = (i * 7) as u8;
    }
    let before = f.mem[cal..cal + 0x1000].to_vec();
    let s = mount(f);
    assert_eq!(s.mounted(), Mounted::Ok);
    let n = s.count();
    assert_eq!(ferrite_store::block(s.check_cal()), CalCrc::Recorded);
    assert_eq!(ferrite_store::block(s.check_cal()), CalCrc::Same);
    let mut f = unmount(s);
    assert_eq!(f.mem[cal..cal + 0x1000], before[..], "calibration never written");
    let s = mount(f.clone());
    assert_eq!((s.mounted(), s.count()), (Mounted::Ok, n), "the header still valid: nothing formatted");
    assert_eq!(ferrite_store::block(s.check_cal()), CalCrc::Same);
    f.mem[cal + 0x210] ^= 1;
    let s = mount(f);
    assert_eq!(ferrite_store::block(s.check_cal()), CalCrc::Changed);
}

/// Decision 227: channel writes queued for one sector go in one rewrite (one erase of the sector, one of a
/// scratch sector), in queue order; a write to another sector stays queued in its place, and a job of another
/// kind ends the run merged.
#[test]
fn queued_channel_writes_share_a_rewrite() {
    use ferrite_core::layout::CHANNELS;
    let s = mount(SimFlash::new());
    for n in 0..70u16 {
        assert!(s.apply(ChanWrite::Put { ch: channel(n, NO_ZONE, 14_600_000 + n as u32 * 1250, "Old", DEFAULT_CFG), from: None }));
    }
    let sector0 = (CHANNELS.start / 4096) as usize;
    let before = s.nor().erases.clone();
    s.set_deferred(true);
    let put = |n: u16, rx: u32| ChanWrite::Put { ch: channel(n, NO_ZONE, rx, "New", DEFAULT_CFG), from: None };
    assert!(s.apply(put(3, 14_500_000)));
    assert!(s.apply(put(64, 14_510_000))); // sector 1: stays queued
    assert!(s.apply(put(5, 14_520_000)));
    assert!(s.apply(ChanWrite::Delete(7)));
    assert!(s.apply(put(5, 14_530_000))); // the same channel again: the later one wins
    assert!(s.request(Op::NewZone)); // another kind: ends the run
    assert!(s.apply(put(9, 14_540_000)));
    let mut runs = 0;
    while let Some(j) = s.next_job() {
        ferrite_store::block(s.run(j));
        runs += 1;
    }
    let e = s.nor().erases.clone();
    assert_eq!(e[sector0] - before[sector0], 2, "3, 5, 7 and 5 again in one rewrite; 9 after the other job in a second");
    assert_eq!(e[sector0 + 1] - before[sector0 + 1], 1, "64 in its own");
    assert_eq!(runs, 4, "the first put (with 5, 7, 5), 64, the other job, 9");
    let rx = |n| s.by_num(n).map(|x| x.0.rx);
    assert_eq!((rx(3), rx(5), rx(7), rx(9), rx(64)), (Some(14_500_000), Some(14_530_000), None, Some(14_540_000), Some(14_510_000)));
    index_agrees(&s);
    let s = mount(unmount(s));
    assert_eq!(s.by_num(5).map(|x| x.0.rx), Some(14_530_000));
}

/// Decision 227: rewrites take the 32 scratch sectors in turn, so each takes a 32nd of the erases; the first
/// scratch sector isn't used any more.
#[test]
fn rewrites_spread_over_the_scratch_ring() {
    use ferrite_core::layout::SCRATCH_RING;
    let s = mount(SimFlash::new());
    assert!(s.apply(ChanWrite::Put { ch: channel(1, NO_ZONE, 14_600_000, "A", DEFAULT_CFG), from: None }));
    let before = s.nor().erases.clone();
    for i in 0..64u32 {
        assert!(s.apply(ChanWrite::Put { ch: channel(1, NO_ZONE, 14_600_000 + (i + 1) * 1250, "A", DEFAULT_CFG), from: None }));
    }
    let e = s.nor().erases.clone();
    let ring: Vec<u32> = (SCRATCH_RING.start / 4096..SCRATCH_RING.end / 4096).map(|k| e[k as usize] - before[k as usize]).collect();
    assert!(ring.iter().all(|&n| n == 2), "{ring:?}");
}


/// Decision 227: a merged rewrite is one rewrite: a power cut anywhere in it leaves all the queued channels as
/// they were, or all as written.
#[test]
fn power_cuts_during_a_merged_rewrite() {
    let mut r = Rng(0x2270_0001);
    let s = mount(SimFlash::new());
    for n in 0..20u16 {
        assert!(s.apply(ChanWrite::Put { ch: channel(n, NO_ZONE, 14_600_000 + n as u32 * 1250, "Old", DEFAULT_CFG), from: None }));
    }
    let mut f = unmount(s);
    for i in 0..300u32 {
        let (s, before, after) = cut_during(f, &mut r, &|s| {
            s.set_deferred(true);
            for n in [2u16, 7, 11, 19] {
                s.apply(ChanWrite::Put { ch: channel(n, NO_ZONE, 14_400_000 + i * 2500 + n as u32, "New", DEFAULT_CFG), from: None });
            }
            while let Some(j) = s.next_job() {
                ferrite_store::block(s.run(j));
            }
            s.set_deferred(false);
        });
        let got = snap(&s);
        assert!(got.chans == before.chans || got.chans == after.chans, "iteration {i}: neither before nor after");
        index_agrees(&s);
        f = unmount(s);
    }
}

/// LINK-61 (decision 333): the first raw write (Console's) stops the store's own: nothing more is read or written by
/// the store until a restart, which mounts what Console wrote; the radio says so as for another layout (UPD-16).
#[test]
fn a_raw_write_stops_the_stores_own() {
    use ferrite_app::data::DataWrite;
    use ferrite_core::layout::{CHANNELS, LAYOUT_MOVING};
    let s = mount(seeded());
    assert_eq!(s.mounted(), Mounted::Ok);
    assert!(s.get(0).is_some());
    assert!(s.data_write(DataWrite::Erase(CHANNELS.start)));
    assert_eq!((s.layout_found(), s.data().layout_found()), (Some(LAYOUT_MOVING), LAYOUT_MOVING));
    assert!(s.get(0).is_none() && !s.save(&app()), "the store reads and writes nothing more");
    let mut rec = [0u8; 64];
    rec[0] = 7;
    assert!(s.data_write(DataWrite::Program { addr: CHANNELS.start + 10 * 4096, len: 64, bytes: rec }));
    let mut b = [0u8; 64];
    assert!(s.data_read(CHANNELS.start + 10 * 4096, &mut b) && b == rec);
    assert_eq!(s.data_crc(CHANNELS.start, 64), Some({
        let mut c = ferrite_store::Crc32::new();
        c.update(&[0xFF; 64]);
        c.get()
    }));
    // Decision 339: a restart before the stamp waits for Console (the migration mark); the stamp, then a restart,
    // mounts what Console wrote.
    let s = mount(unmount(s));
    assert_eq!(s.mounted(), Mounted::Update(LAYOUT_MOVING), "not stamped: still waiting");
    assert!(s.data_write(DataWrite::Stamp));
    let s = mount(unmount(s));
    assert_eq!(s.mounted(), Mounted::Ok, "stamped: a restart mounts it");
    assert!(s.get(0).is_none() && s.get(1).is_none(), "channels 0-62 erased by Console");
}

// ---- Decision 339: a move (LINK-62) cut anywhere ----

/// The raw writes of a move as Console makes them from its copy (LINK-62's step 4, Appendix G): every sector of
/// each region erased, the target chip's bytes programmed 64 at a time (all-0xFF pieces skipped), the channel
/// sector that holds layout 2's header last; then the stamp.
fn move_plan(target: &SimFlash) -> Vec<ferrite_app::data::DataWrite> {
    use ferrite_app::data::DataWrite;
    use ferrite_core::layout::*;
    let hs = V2_HEADER.start;
    let mut regions = vec![SETTINGS, TRIMS, PRESETS, SSB_PATCH, CHANNELS.start..hs, V2_HEADER.end..CHANNELS.end, ZONES, APRS, CONTACTS, HEARD, MESSAGES, WAYPOINTS, EVENT_LOG];
    regions.push(V2_HEADER);
    let mut out = Vec::new();
    for r in regions {
        for s in r.clone().step_by(4096) {
            out.push(DataWrite::Erase(s));
        }
        for a in r.step_by(64) {
            let i = SimFlash::at(a);
            let piece = &target.mem[i..i + 64];
            if piece.iter().any(|&b| b != 0xFF) {
                let mut bytes = [0xFF; 64];
                bytes.copy_from_slice(piece);
                out.push(DataWrite::Program { addr: a, len: 64, bytes });
            }
        }
    }
    out.push(DataWrite::Stamp);
    out
}

/// Runs the queued jobs as the Storage task does (false once the flash has died).
fn drain(s: &Store) {
    while let Some(j) = s.next_job() {
        ferrite_store::block(s.run(j));
    }
}

/// The plan sent over the link to a radio (the store deferred, as on the radio): each write queued, `busy` waited
/// out; stops when the flash has died (a cut). The flash operation count after each write is done.
fn send(s: &Store, plan: &[ferrite_app::data::DataWrite]) -> Vec<u64> {
    s.set_deferred(true);
    let mut done = Vec::new();
    for &w in plan {
        let mut tries = 0;
        while !s.data_write(w) {
            drain(s);
            tries += 1;
            if tries > 3 || s.nor().dead {
                return done;
            }
        }
        drain(s);
        done.push(s.nor().ops);
    }
    done
}

/// What a mount after a cut may find: the move waiting for Console (the old layout's header, or the migration mark)
/// with nothing written by the mount, or (the stamp done) this layout with everything the target holds, or (a
/// layout 3 chip cut before its mark was in place, nothing written yet) all it held before.
fn check_after_cut(f: SimFlash, want: &Snap, want_set: &Option<Settings>, old: &(Snap, Option<Settings>), from: Mounted, what: &str) -> SimFlash {
    use ferrite_core::layout::LAYOUT_MOVING;
    let before = f.mem.clone();
    let s = mount(f);
    let m = s.mounted();
    assert_ne!(m, Mounted::FirstBoot, "{what}: formatted");
    if m == Mounted::Ok {
        let got = (snap(&s), settings_of(&s));
        assert!(got.0 == *want && got.1 == *want_set || from == Mounted::Ok && got == *old, "{what}: mounted Ok without all the data");
        return unmount(s);
    }
    assert!(m == Mounted::Update(LAYOUT_MOVING) || m == from, "{what}: {m:?}");
    let f = unmount(s);
    assert!(f.mem == before, "{what}: the mount wrote");
    f
}

/// Moves `source` to `target`'s data with the power cut at every flash operation of it (each erase, each programmed
/// byte: every write boundary, inside every write, the begin's mark and header, the stamp), the operation cut left
/// half done the worst way NOR leaves it; after each cut the mount waits for Console or (stamped) holds all the data,
/// never formats; and Console starting again from its copy finishes the move.
fn cut_everywhere(source: SimFlash, target: SimFlash, from: Mounted) -> usize {
    let want = {
        let s = mount(target.clone());
        (snap(&s), settings_of(&s))
    };
    let old = {
        let s = mount(source.clone());
        (snap(&s), settings_of(&s))
    };
    let plan = move_plan(&target);
    // A clean run: where each write ends.
    let s = mount(source.clone());
    assert_eq!(s.mounted(), from);
    let n0 = s.nor().ops;
    let ends: Vec<u64> = send(&s, &plan).into_iter().map(|e| e - n0).collect();
    assert_eq!(ends.len(), plan.len());
    let f = check_after_cut(unmount(s), &want.0, &want.1, &old, from, "a clean move");
    assert_eq!(mount(f).mounted(), Mounted::Ok, "a clean move mounts");
    let total = *ends.last().unwrap();
    let mut r = Rng(0x3390_0001);
    for c in 1..=total {
        let s = mount(source.clone());
        {
            let mut nor = s.nor();
            nor.cut_at = Some(nor.ops + c);
            nor.seed(r.next());
        }
        send(&s, &plan);
        let mut f = unmount(s);
        f.power_on();
        let f = check_after_cut(f, &want.0, &want.1, &old, from, &format!("cut at op {c} of {total}"));
        // Console starts again from its copy (every cut in the begin and the stamp, at each write's first
        // operation, and every 16th elsewhere).
        if c <= ends[0] || c > ends[ends.len() - 2] || ends.contains(&(c - 1)) || c % 16 == 0 {
            let s = mount(f);
            send(&s, &plan);
            let s = mount(unmount(s));
            assert_eq!(s.mounted(), Mounted::Ok, "cut at op {c}: the move started again finishes");
            assert!(snap(&s) == want.0 && settings_of(&s) == want.1, "cut at op {c}: the move started again holds the data");
        }
    }
    total as usize
}

/// Decision 339, LINK-62, UPD-16: a layout 2 chip (its header at 0x02C000, its settings, stock's old data at
/// 0x000000 and where layout 3's channels go) moved to layout 3, cut everywhere: never formatted, always waiting
/// for Console or done, and finished from Console's copy.
#[test]
fn a_move_from_layout_2_cut_anywhere_loses_nothing() {
    let target = seeded();
    let mut source = SimFlash::new();
    let mut r = Rng(0x0002_0333);
    for a in (0..0x1000).chain(0x1_0000..0x1_8000) {
        source.mem[a] = r.next() as u8; // stock's data
    }
    let mut h = [0xFFu8; 20];
    h[..4].copy_from_slice(&ferrite_store::store::HEADER_MAGIC.to_le_bytes());
    h[4..6].copy_from_slice(&2u16.to_le_bytes());
    let mut c = ferrite_store::Crc32::new();
    c.update(&h[..16]);
    h[16..20].copy_from_slice(&c.get().to_le_bytes());
    source.mem[0x2C000..0x2C014].copy_from_slice(&h);
    source.mem[0x2D000..0x2F000].copy_from_slice(&target.mem[0x1000..0x3000]); // its settings
    let n = cut_everywhere(source, target, Mounted::Update(2));
    assert!(n > 1500, "{n} cuts");
}

/// Decision 339: Console writing a layout 3 chip raw (a restore from its copy onto a radio already on layout 3),
/// cut everywhere: the same.
#[test]
fn a_raw_rewrite_of_a_layout_3_chip_cut_anywhere_loses_nothing() {
    let target = seeded();
    let s = mount(SimFlash::new());
    assert!(s.add_zone(b"Other", &ZoneCfg::default()));
    for n in [3u16, 70, 500] {
        assert!(s.put_with_position(channel(n, 0, 43_000_000 + n as u32 * 1250, "Old", DEFAULT_CFG), None));
    }
    assert!(s.save(&app()));
    let n = cut_everywhere(unmount(s), target, Mounted::Ok);
    assert!(n > 1500, "{n} cuts");
}

/// LINK-62, decision 339: the stamp is taken only once the store has nothing queued or running (`busy` before);
/// a restart right after its answer, before the Storage task has run it, leaves the mark: the radio waits for
/// Console, and the stamp sent again finishes it.
#[test]
fn a_reset_right_after_the_stamps_answer_waits_for_console() {
    use ferrite_app::data::DataWrite;
    use ferrite_core::layout::{CHANNELS, LAYOUT_MOVING};
    let s = mount(seeded());
    s.set_deferred(true);
    assert!(s.data_write(DataWrite::Erase(CHANNELS.start + 4096)));
    assert!(!s.data_write(DataWrite::Stamp), "busy: the begin and the erase are queued");
    drain(&s);
    assert!(s.data_write(DataWrite::Stamp), "taken once the store is idle");
    let s = mount(unmount(s)); // the reset: the queued stamp never ran
    assert_eq!(s.mounted(), Mounted::Update(LAYOUT_MOVING));
    assert!(s.data_write(DataWrite::Stamp));
    let s = mount(unmount(s));
    assert_eq!(s.mounted(), Mounted::Ok);
    assert_eq!(s.count(), 11 - 3, "channels 63, 64 and 65 erased by the raw write, the rest kept");
}

/// LINK-59, decision 339: a raw write the Storage task has taken but not finished still counts as busy, so
/// `DATA_READ` never reads (or CRCs) a half-done write; once done, the CRC is the written bytes'.
#[test]
fn a_raw_write_being_done_keeps_reads_waiting() {
    use ferrite_app::data::DataWrite;
    use ferrite_core::layout::CHANNELS;
    let s = mount(seeded());
    s.set_deferred(true);
    let at = CHANNELS.start + 9 * 4096;
    assert!(s.data_write(DataWrite::Program { addr: at, len: 64, bytes: [0x42; 64] }));
    assert!(s.data().busy());
    let begin = s.next_job().unwrap();
    assert!(s.data().busy(), "the begin taken, the program queued");
    ferrite_store::block(s.run(begin));
    let j = s.next_job().unwrap();
    assert!(s.data().busy(), "taken by the Storage task, not done: still busy");
    ferrite_store::block(s.run(j));
    assert!(!s.data().busy() && s.idle());
    let mut c = ferrite_store::Crc32::new();
    c.update(&[0x42; 64]);
    assert_eq!(s.data_crc(at, 64), Some(c.get()));
}

/// PWR-26.13: home's headers ask for a zone's name every frame: it is read from the flash once, then kept until the
/// store writes anything (a rename shows at once); three zones (one a slot) are kept at a time.
#[test]
fn zone_names_read_once_until_a_write() {
    let s = mount(SimFlash::new());
    for n in [&b"Local"[..], b"Events", b"Air", b"Marine"] {
        assert!(s.add_zone(n, &ZoneCfg::default()));
    }
    assert_eq!(s.zone_name(1).unwrap().as_bytes(), b"Events");
    let r0 = s.nor().reads;
    for _ in 0..10 {
        for z in [0, 1, 2] {
            assert!(s.zone_name(z).is_some());
        }
    }
    assert!(s.nor().reads - r0 <= 2, "zones 0 and 2 read once each, then kept: {} reads", s.nor().reads - r0);
    assert!(s.request(Op::PutZone(1, Buf::from(b"Renamed"), ZoneCfg::default())));
    assert_eq!(s.zone_name(1).unwrap().as_bytes(), b"Renamed");
    assert_eq!(s.zone_name(3).unwrap().as_bytes(), b"Marine");
    assert_eq!(s.zone_name(9), None);
}

/// LINK-71: the lists hash is made from what is stored: the same after a restart with nothing written; one channel
/// written changes the channel list's hash and exactly one sector hash (its own, 63 channels a sector), and writing
/// it back as it was puts both back; a zone renamed changes the zones' hash alone; `sector_hash` covers 70 sectors.
#[test]
fn lists_hashes_follow_the_contents() {
    let f = seeded();
    let s = mount(f);
    let lists = s.lists_versions();
    let sectors: Vec<u32> = (0..70).map(|i| s.sector_hash(i).unwrap()).collect();
    assert_eq!(s.sector_hash(70), None);
    let s = mount(unmount(s));
    assert_eq!(s.lists_versions(), lists, "a restart with nothing written: the same hashes");
    assert_eq!((0..70).map(|i| s.sector_hash(i).unwrap()).collect::<Vec<_>>(), sectors);
    let old = s.get(130).expect("the seeded radio has channel 130");
    let mut c = old;
    c.rx = c.rx.wrapping_add(2500);
    assert!(s.apply(ChanWrite::Put { ch: c, from: None }));
    let now: Vec<u32> = (0..70).map(|i| s.sector_hash(i).unwrap()).collect();
    let moved: Vec<usize> = (0..70).filter(|&i| now[i] != sectors[i]).collect();
    assert_eq!(moved, vec![130 / 63], "only channel 130's sector");
    let l2 = s.lists_versions();
    assert!(l2[0] != lists[0] && l2[1] == lists[1] && l2[2] == lists[2], "the channels' hash alone");
    assert!(s.apply(ChanWrite::Put { ch: old, from: None }));
    assert_eq!(s.lists_versions(), lists, "written back as it was: the hash as it was");
    assert!(s.request(Op::RenameZone(1, Name::from(b"Renamed".as_slice()))));
    let l3 = s.lists_versions();
    assert!(l3[0] == lists[0] && l3[1] != lists[1] && l3[2] == lists[2], "a zone renamed: the zones' hash alone");
    let s = mount(unmount(s));
    assert_eq!(s.lists_versions(), l3, "and the same after a restart");
}

/// CAL-20.3: the power cut at every step of accepting the calibration: the store mounts with its records, and the
/// check is either still Changed (accept again) or Same; never a format.
#[test]
fn accepting_the_calibration_through_a_power_cut() {
    use ferrite_core::layout::CALIBRATION;
    use ferrite_core::txcal::{testing::sector, CalCrc};
    let mut f = seeded();
    let at = SimFlash::at(CALIBRATION.start);
    let good = sector([0x81, 0x87, 0x8E, 0x93, 0x98, 0xA0, 0xBB]);
    f.mem[at..at + good.len()].copy_from_slice(&good);
    let s = mount(f);
    assert_eq!(block(s.check_cal()), CalCrc::Recorded);
    let mut f = unmount(s);
    f.mem[at + 0x10] ^= 1;
    let s = mount(f.clone());
    let (count, n0) = (s.count(), s.nor().ops);
    assert!(block(s.accept_cal()));
    let ops = s.nor().ops - n0;
    assert!(ops > 0);
    for k in 0..=ops {
        let s = mount(f.clone());
        let at = s.nor().ops + k;
        s.nor().cut_at = Some(at);
        let _ = block(s.accept_cal());
        let mut g = unmount(s);
        (g.cut_at, g.dead) = (None, false);
        let s = mount(g);
        assert_eq!((s.mounted(), s.count()), (Mounted::Ok, count), "cut at op {k}");
        let c = block(s.check_cal());
        assert!(matches!(c, CalCrc::Changed | CalCrc::Same), "cut at op {k}: {c:?}");
        if c == CalCrc::Changed {
            assert!(block(s.accept_cal()), "accepted again after the cut at op {k}");
            assert_eq!(block(s.check_cal()), CalCrc::Same);
        }
    }
}

/// PWR-17.1: a flush that a save already under way refused (the settings changed in the last seconds before the
/// switch went off) is made again once that save is done, and the newer settings are what the next start finds.
/// Before, power-off made one attempt and waited only for the store to go idle (a review of Nov 20, 2026).
#[test]
fn a_flush_refused_by_a_save_under_way_is_made_again() {
    let s = mount(SimFlash::new());
    s.set_deferred(true); // as on the radio: the Storage task runs the save later
    let mut a = app();
    a.set_watch(1);
    assert!(s.save(&a), "a save under way");
    a.set_watch(2); // changed just before power-off
    assert!(!s.flush(&a), "refused: a save is under way");
    assert!(s.unsaved(&a));
    drain(&s); // the earlier save done
    assert!(s.unsaved(&a), "it held the older state");
    assert!(s.flush(&a), "made again");
    drain(&s);
    assert!(!s.unsaved(&a));
    s.set_deferred(false);
    let s = mount(unmount(s));
    let mut b = app();
    assert!(s.restore(&mut b));
    assert_eq!(b.watch(), 2, "the newer settings kept");
}
