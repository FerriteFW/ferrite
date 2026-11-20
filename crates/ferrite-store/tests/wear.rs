//! Flash wear (decision 227, Alex: "keep an eye on how we abuse the flash with writes"): a heavy day of use and a
//! Console bulk write in the simulated flash, counting the erases each sector takes against a budget. A change that
//! makes the store erase more fails here, and the table shows where the erases go.
//!
//! NOR flash like this radio's is usually rated for 100,000 erases a sector; at the budgets below the most-worn
//! sector lasts the years the test prints.

use ferrite_app::chan::{channel, ChanWrite, ChannelStore, NO_ZONE};
use ferrite_app::data::{Data, Event, NoData, Op, Pos, Src, Waypoint, WpSrc};
use ferrite_app::screens::alert::AlertInfo;
use ferrite_app::settings::Settings;
use ferrite_app::slot::DEFAULT_CFG;
use ferrite_app::App;
use ferrite_core::layout::{CHANNELS, EVENT_LOG, SCRATCH_RING, SETTINGS};
use ferrite_store::sim::SimFlash;
use ferrite_store::FlashStore;
use ferrite_ui::text::Buf;

type Store = FlashStore<SimFlash, NoData>;

const RATED: u32 = 100_000;

fn app() -> App {
    let set: &'static mut Settings = Box::leak(Box::new(Settings::DEFAULT));
    let alert: &'static mut AlertInfo = Box::leak(Box::new(AlertInfo::NONE));
    App::new(set, alert)
}

fn event(n: u32) -> Event {
    let mut text = Buf::<64>::new();
    text.set(b"wear");
    Event { src: Src::Pocsag, ago_s: 0, freq: 15_000_000, slot: 0, unread: true, id: n, x: 0, y: 0, pos: None, name: Buf::new(), text }
}

/// Erases per sector (sector index, count), most first, between two snapshots.
fn worst(before: &[u32], after: &[u32]) -> Vec<(usize, u32)> {
    let mut v: Vec<(usize, u32)> = after.iter().zip(before).enumerate().map(|(i, (a, b))| (i, a - b)).filter(|x| x.1 > 0).collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1));
    v
}

fn region(sector: usize) -> &'static str {
    let a = sector as u32 * 4096;
    match a {
        a if SETTINGS.contains(&a) => "settings",
        a if CHANNELS.contains(&a) => "channels",
        a if EVENT_LOG.contains(&a) => "event log",
        a if SCRATCH_RING.contains(&a) => "scratch ring",
        _ => "other",
    }
}

/// A heavy day on the keypad: 300 settings saves (tuning and settings changes, each 2 s after the last change),
/// 50 channel edits, 200 logged events, 30 messages each way, 5 waypoints. Budget: the most-worn sector takes at
/// most 60 erases a day (a channel sector, the 50 edits; 160 before decision 377, the settings' A/B pair then);
/// each settings sector at most 30 (six saves to a sector before an erase: 25, about 11 years of such days at the
/// rated 100,000); a normal day is a fraction of this.
#[test]
fn a_heavy_day_on_the_keypad() {
    let s: Store = FlashStore::mount(SimFlash::new(), NoData);
    for n in 0..60u16 {
        assert!(s.apply(ChanWrite::Put { ch: channel(n, NO_ZONE, 14_600_000 + n as u32 * 1250, "Ch", DEFAULT_CFG), from: None }));
    }
    let before = s.nor().erases.clone();
    let mut a = app();
    let mut now = 0u32;
    for i in 0..300u32 {
        a.set_watch((i % 2) as u8);
        s.autosave(now, &a);
        now += 2_500;
        s.autosave(now, &a);
    }
    for i in 0..50u16 {
        assert!(s.apply(ChanWrite::Put { ch: channel(i, NO_ZONE, 14_700_000 + i as u32 * 1250, "Edited", DEFAULT_CFG), from: None }));
    }
    for n in 0..200 {
        s.log(&event(n));
    }
    for n in 0..30 {
        s.message_in(true, b"VE3KPW-9", format!("hello {n}").as_bytes(), 0);
        s.message_in(false, b"VE3KPW-9", format!("reply {n}").as_bytes(), 0);
    }
    for n in 0..5 {
        s.add_waypoint(&Waypoint { name: Buf::from(b"Here"), pos: Pos { lat: n, lon: n }, src: WpSrc::Marked });
    }
    s.request(Op::ReadAll);
    let w = worst(&before, &s.nor().erases);
    println!("a heavy day: the most-worn sectors");
    for (sec, n) in w.iter().take(6) {
        println!("  0x{:06X} {:13} {n:4} erases ({:.1} years of such days)", sec * 4096, region(*sec), RATED as f64 / *n as f64 / 365.0);
    }
    let (sec, n) = w[0];
    assert!(n <= 60, "0x{:06X} ({}) took {n} erases in a day", sec * 4096, region(sec));
    let set: u32 = w.iter().filter(|(s, _)| region(*s) == "settings").map(|x| x.1).max().unwrap_or(0);
    assert!(set <= 30, "a settings sector took {set} erases (decision 377)");
    let scratch: u32 = w.iter().filter(|(s, _)| region(*s) == "scratch ring").map(|x| x.1).max().unwrap_or(0);
    assert!(scratch <= 3, "a scratch sector took {scratch} erases");
}

/// A Console bulk write of 1,000 changed channels through the Storage task's queue (8 deep, merged per sector):
/// each channel sector is erased about 63 / 8 times, each scratch sector a few times.
#[test]
fn a_console_bulk_write() {
    let s: Store = FlashStore::mount(SimFlash::new(), NoData);
    for n in 0..1000u16 {
        assert!(s.apply(ChanWrite::Put { ch: channel(n, NO_ZONE, 14_600_000 + n as u32 * 125, "Old", DEFAULT_CFG), from: None }));
    }
    let before = s.nor().erases.clone();
    s.set_deferred(true);
    // The link's WRITE_MANY fills the queue; the Storage task takes a job, which merges what is queued behind it.
    let mut n = 0u16;
    while n < 1000 || s.has_jobs() {
        while n < 1000 && s.apply(ChanWrite::Put { ch: channel(n, NO_ZONE, 14_400_000 + n as u32 * 125, "New", DEFAULT_CFG), from: None }) {
            n += 1;
        }
        if let Some(j) = s.next_job() {
            ferrite_store::block(s.run(j));
        }
    }
    s.set_deferred(false);
    assert_eq!(s.by_num(999).map(|c| c.0.rx), Some(14_400_000 + 999 * 125));
    let w = worst(&before, &s.nor().erases);
    let ch: u32 = w.iter().filter(|(s, _)| region(*s) == "channels").map(|x| x.1).max().unwrap_or(0);
    let sc: u32 = w.iter().filter(|(s, _)| region(*s) == "scratch ring").map(|x| x.1).max().unwrap_or(0);
    println!("a bulk write of 1,000 channels: a channel sector {ch} erases, a scratch sector {sc}");
    assert!(ch <= 10, "a channel sector took {ch} erases");
    assert!(sc <= 6, "a scratch sector took {sc} erases");
}

/// LINK-53 (decision 325): Console's positions for 1,000 channels after the channels themselves. Positions are kept
/// in the channel records, so a position write is a channel record write: queued behind the channel writes and
/// merged with the ones for the same sector (decision 227), it costs what a bulk channel write costs, and a position
/// that is already stored costs nothing.
#[test]
fn a_console_position_import() {
    let s: Store = FlashStore::mount(SimFlash::new(), NoData);
    let at = |n: u16| Pos { lat: 450_000_000 + n as i32 * 1_000, lon: -750_000_000 - n as i32 * 1_000 };
    s.set_deferred(true);
    let before = s.nor().erases.clone();
    // the channels and then their positions, as the link queues them: each position lands on its channel as the
    // writes before it leave it, whether or not those are on flash yet
    let mut jobs = (0..1000u16).map(|n| ChanWrite::Put { ch: channel(n, NO_ZONE, 14_400_000 + n as u32 * 125, "New", DEFAULT_CFG), from: None }).chain((0..1000u16).map(|n| ChanWrite::Pos { num: n, pos: Some(at(n)) })).peekable();
    while jobs.peek().is_some() || s.has_jobs() {
        while let Some(&j) = jobs.peek() {
            if !s.apply(j) {
                break;
            }
            jobs.next();
        }
        if let Some(j) = s.next_job() {
            ferrite_store::block(s.run(j));
        }
    }
    let w = worst(&before, &s.nor().erases);
    let ch: u32 = w.iter().filter(|(s, _)| region(*s) == "channels").map(|x| x.1).max().unwrap_or(0);
    println!("1,000 channels then their positions: a channel sector {ch} erases");
    assert!(ch <= 10, "a channel sector took {ch} erases");
    for n in [0, 62, 63, 500, 999] {
        assert_eq!(s.position(n), Some(at(n)), "channel {n}");
        assert_eq!(s.get(n).map(|c| c.name().to_vec()), Some(b"New".to_vec()), "channel {n} kept");
    }
    // the same positions again: nothing is written; one cleared and a number with no channel: one record
    let before = s.nor().erases.clone();
    for n in 0..1000u16 {
        assert!(s.apply(ChanWrite::Pos { num: n, pos: Some(at(n)) }));
        while let Some(j) = s.next_job() {
            ferrite_store::block(s.run(j));
        }
    }
    assert_eq!(s.nor().erases, before, "positions as stored: no erase");
    s.set_deferred(false);
    assert!(s.apply(ChanWrite::Pos { num: 5, pos: None }));
    assert_eq!(s.position(5), None);
    assert!(s.get(5).is_some(), "the channel stays");
    assert!(s.apply(ChanWrite::Delete(6)));
    assert!(s.apply(ChanWrite::Pos { num: 6, pos: Some(at(6)) }));
    assert!(s.get(6).is_none() && s.position(6).is_none(), "no channel at 6: nothing made");
}

/// LINK-54 – LINK-56 (decision 327): Console writing the lists. 256 waypoints (of the 2,032 the store holds) added and 16 DTMF memories with names
/// set on a blank store are programmed only (no erase); one waypoint replaced, one deleted, one DTMF memory renamed
/// each rewrite one sector once (through the scratch ring); a quick message saves the APRS record (one erase of one
/// of its two copies), as the radio's own editors do. The link writes nothing for a record as stored.
#[test]
fn console_writes_the_lists() {
    use ferrite_app::data::Dtmf;
    let s: Store = FlashStore::mount(SimFlash::new(), NoData);
    let wp = |n: u32| Waypoint { name: Buf::from(format!("WP {n}").as_bytes()), pos: Pos { lat: n as i32 * 1000, lon: -(n as i32) }, src: WpSrc::Link };
    let before = s.nor().erases.clone();
    for n in 0..256 {
        assert!(s.request(Op::PutWaypoint(ferrite_core::link::ADD, wp(n))));
    }
    for i in 0..16u8 {
        assert!(s.request(Op::Dtmf(i, Dtmf::parse(b"*79#"))));
        assert!(s.request(Op::DtmfName(i, Buf::from(b"Link on"))));
    }
    assert_eq!(s.nor().erases, before, "added to blank flash: programmed only");
    assert_eq!((s.waypoints(), s.waypoint(0), s.waypoint(255)), (256, Some(wp(255)), Some(wp(0))));
    let before = s.nor().erases.clone();
    assert!(s.request(Op::PutWaypoint(10, wp(1000))));
    assert_eq!((s.waypoint(10), s.waypoint(9), s.waypoint(11), s.waypoints()), (Some(wp(1000)), Some(wp(246)), Some(wp(244)), 256), "in its place");
    assert!(s.request(Op::PutWaypoint(10, wp(1000))), "as stored: nothing to do");
    assert!(s.request(Op::DtmfName(3, Buf::from(b"IRLP 3500"))));
    let w = worst(&before, &s.nor().erases);
    let most = w.iter().filter(|(s, _)| region(*s) == "other").map(|x| x.1).max().unwrap_or(0);
    println!("a waypoint replaced and a DTMF name changed: {w:?}");
    assert!(most <= 1, "a list sector took {most} erases: {w:?}");
    assert_eq!(s.dtmf_name(3).as_bytes(), b"IRLP 3500");
    let before = s.nor().erases.clone();
    assert!(s.request(Op::Quick(0, Some(Buf::from(b"At the club")))));
    let n: u32 = worst(&before, &s.nor().erases).iter().filter(|(s, _)| region(*s) != "scratch ring").map(|x| x.1).sum();
    assert_eq!(n, 1, "a quick message: one erase of one APRS record copy");
}

/// LINK-58 (decision 329): a range preset's 48 skipped frequencies written from the link go as four store writes,
/// queued together, and the four go in one rewrite of the presets' sector (LINK-58.2, decision 227's merge, Nov 19, 2026: four
/// rewrites before, 128 erases for a Console write of 16 lists); the list lives in the preset's own record, so
/// deleting the preset before it moves its index, never its list.
#[test]
fn console_writes_preset_skips() {
    use ferrite_app::data::{RangePreset, SKIP_CHUNK};
    let s: Store = FlashStore::mount(SimFlash::new(), NoData);
    let pr = |n: &[u8], start: u32| RangePreset { name: Buf::from(n), start, end: start + 400_000, step: 10, mode: 0, narrow: true, skips: 0, resume: 0 };
    assert!(s.add_preset(&pr(b"2 m", 14_400_000)) && s.add_preset(&pr(b"70 cm", 43_000_000)));
    let list: Vec<u32> = (0..48).map(|j| 43_000_000 + 2_500 * (j + 1)).collect();
    let before = s.nor().erases.clone();
    s.set_deferred(true); // as the link queues them for the Storage task
    for at in (0..48).step_by(SKIP_CHUNK) {
        let mut f = [0; SKIP_CHUNK];
        f.copy_from_slice(&list[at..at + SKIP_CHUNK]);
        assert!(s.request(Op::PresetSkips { preset: 1, at: at as u8, count: 48, f }));
    }
    let mut runs = 0;
    while let Some(j) = s.next_job() {
        assert!(ferrite_store::block(s.run(j)));
        runs += 1;
    }
    s.set_deferred(false);
    assert_eq!(runs, 1, "the other three merged into the first's rewrite");
    let w = worst(&before, &s.nor().erases);
    println!("48 skipped frequencies: {w:?}");
    let most = w.iter().filter(|(s, _)| region(*s) == "other").map(|x| x.1).max().unwrap_or(0);
    assert!(most <= 1, "the presets' sector took {most} erases: {w:?}");
    assert_eq!(s.preset(1).map(|p| (p.name, p.start, p.skips)), Some((Buf::from(b"70 cm"), 43_000_000, 48)), "the header as it was, its count new");
    assert_eq!((0..48).map(|j| s.preset_skip(1, j).unwrap()).collect::<Vec<_>>(), list);
    assert!(s.request(Op::DeletePreset(0)));
    assert_eq!((s.preset(0).map(|p| p.skips), s.preset_skip(0, 47)), (Some(48), Some(list[47])), "with its preset");
}
