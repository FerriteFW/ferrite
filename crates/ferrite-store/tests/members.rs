//! Decision 333: zones without a zone map. The slots' member lists (CHAN-36) answer exactly as paging through flash
//! does, through every kind of channel write; each zone's count reaches its record once writes are quiet and is put
//! right at mount; a zone takes 256 channels and refuses the 257th (CHAN-35); and what building a list costs.

use ferrite_app::chan::{channel, ChanWrite, Channel, ChannelStore, ZoneId, MAX_CHANNELS, NO_ZONE, ZONE_CAP};
use ferrite_app::data::{Data, NoData, Op, ZoneCfg};
use ferrite_app::screens::alert::AlertInfo;
use ferrite_app::settings::Settings;
use ferrite_app::slot::DEFAULT_CFG;
use ferrite_app::App;
use ferrite_store::sim::SimFlash;
use ferrite_store::FlashStore;

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

fn ch(num: u16, zone: ZoneId) -> Channel {
    channel(num, zone, 14_400_000 + num as u32 * 125, "C", DEFAULT_CFG)
}

fn zones(s: &Store, n: u16) {
    for _ in 0..n {
        assert!(s.add_zone(b"Z", &ZoneCfg { shown: true, ..ZoneCfg::default() }));
    }
}

/// What a zone holds, by reading every record (no list, no count).
fn truth(s: &Store, z: ZoneId) -> Vec<u16> {
    (0..MAX_CHANNELS).filter(|&n| s.get(n).is_some_and(|c| c.zone == z)).collect()
}

/// Every zone (and no zone) answers as the records say: its length, each channel by position, each channel's
/// position (CHAN-10); the channels in all.
fn agrees(s: &Store, what: &str) {
    for z in (0..s.zones()).chain([NO_ZONE]) {
        let want = truth(s, z);
        assert_eq!(s.zone_len(z), want.len() as u16, "{what}: zone {z}'s length");
        for (i, &n) in want.iter().enumerate() {
            assert_eq!(s.zone_at(z, i as u16).map(|c| c.num), Some(n), "{what}: zone {z} at {i}");
            assert_eq!(s.by_num(n).map(|x| x.1), Some(i as u16), "{what}: channel {n}'s position");
        }
        assert!(s.zone_at(z, want.len() as u16).is_none());
        // and backwards, as ▲ steps (the cursor walks down)
        for (i, &n) in want.iter().enumerate().rev() {
            assert_eq!(s.zone_at(z, i as u16).map(|c| c.num), Some(n), "{what}: zone {z} at {i}, going down");
        }
    }
    let all: Vec<u16> = (0..MAX_CHANNELS).filter(|&n| s.get(n).is_some()).collect();
    assert_eq!(s.count(), all.len() as u16, "{what}: count");
    for (i, &n) in all.iter().enumerate() {
        assert_eq!(s.by_number(i as u16).map(|c| c.num), Some(n), "{what}: all at {i}");
    }
}

/// CHAN-36: with the slots on two zones and on no zone, the lists follow every write (a put, a move between zones,
/// a renumber, a delete, a zone deleted, all erased) and the slots moving to other zones; every answer matches the
/// records. The counts reach the zone records once writes are quiet, and a mount finds nothing to put right.
#[test]
fn member_lists_agree_with_flash() {
    let s = Store::mount(SimFlash::new(), NoData);
    zones(&s, 6);
    let mut r = Rng(0x5EED_0333);
    for n in 0..300u16 {
        let z = r.below(7) as u16;
        assert!(s.apply(ChanWrite::Put { ch: ch(r.below(MAX_CHANNELS as u64) as u16, if z == 6 { NO_ZONE } else { z }), from: None }));
        let _ = n;
    }
    let mut slots = [0, 1, NO_ZONE];
    s.slot_zones(slots);
    agrees(&s, "start");
    let a = app();
    let mut now = 0u32;
    for i in 0..600u32 {
        let num = r.below(MAX_CHANNELS as u64) as u16;
        let z = match r.below(8) {
            7 => NO_ZONE,
            k => (k as u16).min(s.zones().saturating_sub(1)),
        };
        match r.below(10) {
            0..=3 => {
                let c = s.get(num).map_or(ch(num, z), |c| Channel { zone: z, ..c });
                s.apply(ChanWrite::Put { ch: c, from: None });
            }
            4 => {
                s.apply(ChanWrite::Delete(num));
            }
            5 => {
                s.request(Op::Move(num, z));
            }
            6 => {
                let to = r.below(MAX_CHANNELS as u64) as u16;
                s.request(Op::Renumber(num, to));
            }
            7 => {
                slots[r.below(3) as usize] = z;
                s.slot_zones(slots);
            }
            8 if i % 50 == 8 => {
                s.request(Op::DeleteZone(z.min(s.zones().saturating_sub(1))));
            }
            _ => {
                now += 3000; // quiet: the counts are written
                s.autosave(now, &a);
            }
        }
        if i % 20 == 0 {
            agrees(&s, &format!("op {i}"));
        }
    }
    agrees(&s, "end");
    now += 3000;
    s.autosave(now, &a);
    assert!(!s.counts_pending(), "every count written");
    let lens: Vec<u16> = (0..s.zones()).map(|z| s.zone_len(z)).collect();
    let s = Store::mount(s.into_parts().0, NoData);
    assert!(!s.counts_pending(), "the mount finds every record's count right");
    assert_eq!((0..s.zones()).map(|z| s.zone_len(z)).collect::<Vec<_>>(), lens, "from the records alone");
    agrees(&s, "after a mount, no lists");
    // all erased: every zone's count is 0, written a sector at a time
    assert!(s.apply(ChanWrite::EraseAll));
    assert!(!s.counts_pending());
    let s = Store::mount(s.into_parts().0, NoData);
    assert!(!s.counts_pending() && (0..s.zones()).all(|z| s.zone_len(z) == 0));
}

/// CHAN-36: a count a power cut left wrong (here written wrong on purpose) is put right: the mount counts every zone
/// and marks the wrong one; it is written once things are quiet.
#[test]
fn a_wrong_count_is_put_right_at_mount() {
    let s = Store::mount(SimFlash::new(), NoData);
    zones(&s, 2);
    for n in 0..10 {
        assert!(s.apply(ChanWrite::Put { ch: ch(n, 1), from: None }));
    }
    let a = app();
    s.autosave(5000, &a);
    assert!(!s.counts_pending());
    // zone 1's count changed behind the store's back (7 instead of 10), its CRC kept good
    let mut f = s.into_parts().0;
    let at = ferrite_store::records::zone_addr(1) as usize;
    let mut rec = [0u8; 32];
    rec.copy_from_slice(&f.mem[at..at + 32]);
    ferrite_store::records::set_zone_count(&mut rec, 7);
    f.mem[at..at + 32].copy_from_slice(&rec);
    let s = Store::mount(f, NoData);
    assert!(s.counts_pending(), "marked at mount");
    assert_eq!(s.zone_len(1), 10, "the count found, not the record's");
    s.autosave(5000, &a);
    assert!(!s.counts_pending());
    let s = Store::mount(s.into_parts().0, NoData);
    assert!(!s.counts_pending() && s.zone_len(1) == 10);
}

/// CHAN-35: a zone takes 256 channels; the 257th is refused whichever way it comes (a write, a move), and nothing
/// changes; a channel already in the full zone can still be edited or renumbered within it; no zone has no limit.
#[test]
fn a_zone_takes_256_channels() {
    let s = Store::mount(SimFlash::new(), NoData);
    zones(&s, 2);
    for n in 0..ZONE_CAP {
        assert!(s.apply(ChanWrite::Put { ch: ch(n, 0), from: None }), "channel {n}");
    }
    assert_eq!(s.zone_len(0), ZONE_CAP);
    assert!(!s.zone_room(0, 300) && s.zone_room(0, 5) && s.zone_room(1, 300) && s.zone_room(NO_ZONE, 300));
    assert!(!s.apply(ChanWrite::Put { ch: ch(300, 0), from: None }), "a 257th refused");
    assert!(s.get(300).is_none());
    assert!(s.apply(ChanWrite::Put { ch: ch(301, 1), from: None }));
    assert!(!s.request(Op::Move(301, 0)), "a move into the full zone refused");
    assert_eq!(s.get(301).map(|c| c.zone), Some(1));
    assert!(s.apply(ChanWrite::Put { ch: Channel { rx: 14_600_000, ..ch(5, 0) }, from: None }), "an edit of one inside");
    assert!(s.request(Op::Renumber(5, 900)), "a renumber within the zone");
    assert_eq!((s.zone_len(0), s.get(900).map(|c| c.zone)), (ZONE_CAP, Some(0)));
    assert!(s.request(Op::Move(900, 1)) && s.request(Op::Move(301, 0)), "one out, then one in");
    for n in 400..700 {
        assert!(s.apply(ChanWrite::Put { ch: ch(n, NO_ZONE), from: None }));
    }
    s.slot_zones([NO_ZONE, 0, 1]);
    assert_eq!(s.zone_len(NO_ZONE), 300, "no zone: more than a list holds, paged from flash");
    assert_eq!(s.zone_at(NO_ZONE, 299).map(|c| c.num), Some(699));
    assert_eq!(s.by_num(450).map(|x| x.1), Some(50));
}

/// CHAN-36: what building a slot's list costs on the radio's bus: one look (2 bytes) at every record and one whole
/// read of each record that may be in the zone. Printed for the build notes; held under 150 ms at 15 MHz.
#[test]
fn building_a_list_costs() {
    let s = Store::mount(SimFlash::new(), NoData);
    zones(&s, 18);
    // every number used: a full zone of 256 among them, the rest spread over 17 others (under 256 each) and no zone
    for n in 0..MAX_CHANNELS {
        let z = if n % 4 == 0 && n / 4 < ZONE_CAP { 0 } else { [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, NO_ZONE][n as usize % 18] };
        assert!(s.apply(ChanWrite::Put { ch: ch(n, z), from: None }), "channel {n}");
    }
    assert_eq!(s.zone_len(0), ZONE_CAP.min(MAX_CHANNELS.div_ceil(4)));
    let r0 = s.nor().reads;
    let b0 = s.nor().read_bytes;
    s.slot_zones([0, 0, 0]);
    let reads = s.nor().reads - r0;
    let bytes = s.nor().read_bytes - b0;
    // a read: command and 3 address bytes, then the data, 8 bits each at 15 MHz
    let ms = (bytes + 4 * reads) as f64 * 8.0 / 15_000.0;
    eprintln!("list of {} from {} records: {reads} reads, {bytes} bytes, about {ms:.0} ms on the radio's bus", s.zone_len(0), MAX_CHANNELS);
    assert!(ms < 150.0, "{ms} ms");
}

/// CHAN-36: an empty record below the zone count (a zone deleted over the link, or one the layout 3 update carried
/// over blank) holds no count; the mount doesn't mark it, since its rewrite skips empty records and a mark would rewrite
/// the zone sector at every power-on.
#[test]
fn an_empty_middle_zone_record_needs_no_count() {
    let s = Store::mount(SimFlash::new(), NoData);
    zones(&s, 3);
    for n in 0..6 {
        assert!(s.apply(ChanWrite::Put { ch: ch(n, if n % 2 == 0 { 0 } else { 2 }), from: None }));
    }
    let a = app();
    s.autosave(5000, &a);
    assert!(!s.counts_pending());
    // zone 1's record erased behind the store's back (the zone count stays 3)
    let mut f = s.into_parts().0;
    let at = ferrite_store::records::zone_addr(1) as usize;
    f.mem[at..at + 32].fill(0xFF);
    let s = Store::mount(f, NoData);
    assert!(!s.counts_pending(), "no count marked for the empty record");
}

/// CHAN-35.1: queued channel writes count against a zone's 256: a bulk write into a nearly full zone is refused at
/// the room check (the link's, which tells Console) once the queued ones fill it, and writes that reach the queue
/// anyway never merge past 256 (a review of Nov 20, 2026: 258 in a zone, or writes dropped after Console was told
/// OK).
#[test]
fn queued_writes_count_against_a_zone() {
    let s = Store::mount(SimFlash::new(), NoData);
    zones(&s, 1);
    for n in 0..ZONE_CAP - 1 {
        assert!(s.apply(ChanWrite::Put { ch: ch(n, 0), from: None }));
    }
    assert_eq!(s.zone_len(0), ZONE_CAP - 1);
    s.set_deferred(true); // as on the radio: queued, not yet written
    assert!(s.zone_room(0, 300));
    assert!(s.apply(ChanWrite::Put { ch: ch(300, 0), from: None }));
    assert!(!s.zone_room(0, 301), "the queued one fills it");
    assert!(s.zone_room(0, 300), "its own write is no extra");
    assert!(s.apply(ChanWrite::Delete(5)));
    assert!(s.zone_room(0, 301), "a queued delete makes room");
    // written past the check (in one sector, so they'd merge): the zone stops at 256
    assert!(s.apply(ChanWrite::Put { ch: ch(301, 0), from: None }));
    assert!(s.apply(ChanWrite::Put { ch: ch(302, 0), from: None }));
    while let Some(j) = s.next_job() {
        ferrite_store::block(s.run(j));
    }
    s.set_deferred(false);
    assert_eq!(truth(&s, 0).len(), ZONE_CAP as usize, "256, never more");
    assert_eq!(s.zone_len(0), ZONE_CAP);
}
