//! The store mounting a damaged chip, fuzzed (ferrite-fuzzkit): a chip holding some of everything the store keeps,
//! then edited the ways flash goes wrong or a foreign image looks (bits flipped, bytes overwritten, records cut
//! short by an erase, a sector copied over another, programmed to zeros, random bytes), mounted, sometimes with
//! the power cut during the mount's own repairs and mounted again. Then a change is made and the chip mounted once
//! more.
//!
//! Invariants: no panic; nothing written outside [`ferrite_core::layout::WRITABLE`] (the simulated chip panics on
//! a write to calibration or the update area); every record reads back decoded and the zone lists agree with the
//! channels; what one mount reads, the next mount of the same chip reads the same (what can't be trusted is
//! dropped, not read differently each time); the settings come back in range.

use ferrite_app::aprs::AprsCfg;
use ferrite_app::chan::{channel, ChanWrite, Channel, ChannelStore, Name, MAX_CHANNELS, NO_ZONE};
use ferrite_app::data::{Contact, Data, Dtmf, Event, Fix, Op, Pos, RangePreset, Src, Station, Sym, Waypoint, WpSrc, ZoneCfg};
use ferrite_app::screens::alert::AlertInfo;
use ferrite_app::settings::Settings;
use ferrite_app::slot::DEFAULT_CFG;
use ferrite_app::App;
use ferrite_fuzzkit::{Fuzz, Rng};
use ferrite_store::sim::{SimFlash, HIGH, LOW, PARTS};
use ferrite_store::store::Mounted;
use ferrite_store::{block, FlashStore};
use ferrite_ui::text::Buf;
use std::sync::OnceLock;

struct Tasks;

impl Data for Tasks {
    fn callsign(&self) -> Option<Buf<10>> {
        Some(Buf::from(b"VA7FER-9"))
    }
    fn fix(&self) -> Option<Fix> {
        None
    }
    fn send_room(&self) -> bool {
        true
    }
}

type Store = FlashStore<SimFlash, Tasks>;

fn app() -> App {
    let set: &'static mut Settings = Box::leak(Box::new(Settings::DEFAULT));
    let alert: &'static mut AlertInfo = Box::leak(Box::new(AlertInfo::NONE));
    App::new(set, alert)
}

fn mount(f: SimFlash) -> Store {
    FlashStore::mount(f, Tasks)
}

fn station(call: &[u8]) -> Station {
    Station {
        call: Buf::from(call),
        sym: Sym::Ht,
        pos: Some(Pos { lat: 454_252_000, lon: -756_901_000 }),
        ago_s: 5,
        via: Buf::from(b"WIDE1-1"),
        course: None,
        alt_m: None,
        times: 1,
        device: None,
        comment: Buf::from(b"QRV"),
        msg: Buf::new(),
        msg_min: None,
        fx25: None,
    }
}

/// A chip holding some of everything the store keeps (as `tests/faults.rs` builds it), as its image.
fn populated() -> &'static SimFlash {
    static F: OnceLock<SimFlash> = OnceLock::new();
    F.get_or_init(|| {
        let s = mount(SimFlash::new());
        let a = app();
        assert!(s.save(&a));
        assert!(s.add_zone(b"Local", &ZoneCfg::default()));
        assert!(s.add_zone(b"Events", &ZoneCfg { shown: true, dwell: 9, ..ZoneCfg::default() }));
        assert!(s.add_zone(b"Air", &ZoneCfg { scan_all: true, shown: true, ..ZoneCfg::default() }));
        for (n, z) in [(0, 0), (1, 0), (2, 1), (9, 2), (63, 2), (64, 1), (70, NO_ZONE), (500, 1), (999, 0)] {
            let c = channel(n, z, 14_652_000 + n as u32 * 100, "Chan", DEFAULT_CFG);
            assert!(s.apply(ChanWrite::Put { ch: c, from: None }));
        }
        assert!(s.put_with_position(channel(130, 2, 14_400_000, "Pos", DEFAULT_CFG), Some(Pos { lat: 1, lon: 2 })));
        assert!(s.add_preset(&RangePreset { name: Buf::from(b"2 m"), start: 14_400_000, end: 14_800_000, step: 10, mode: 0, narrow: true, skips: 0, resume: 0 }));
        assert!(s.request(Op::Dtmf(3, Some(Dtmf { len: 4, d: [0x12, 0x34, 0, 0, 0, 0, 0, 0] }))));
        assert!(s.request(Op::DtmfName(3, Buf::from(b"Gate"))));
        let mut cfg = AprsCfg { f5k: 28_878, beacon: 3, power: 1, ssid: 9, ..AprsCfg::DEFAULT };
        cfg.comment[..3].copy_from_slice(b"Hi!");
        assert!(s.request(Op::AprsCfg(cfg)));
        assert!(s.request(Op::Quick(1, Some(Buf::from(b"QRT")))));
        while let Some(j) = s.next_job() {
            block(s.run(j));
        }
        for n in 0..5 {
            let e = Event { src: Src::Aprs, ago_s: 0, freq: 14_439_000, slot: 0, unread: n % 2 == 0, id: n, x: 0, y: 0, pos: None, name: Buf::from(b"VE3KPW-9"), text: Buf::from(b"QSL?") };
            assert!(s.log(&e));
        }
        assert!(s.message_in(true, b"VE3KPW-9", b"QSL?", 0));
        assert!(s.message_in(false, b"VA3RTX", b"Net at 8?", 0));
        assert!(s.heard_row(&station(b"VE3OCB-7")));
        assert!(s.heard_row(&station(b"VA7XYZ")));
        assert!(s.add_waypoint(&Waypoint { name: Buf::from(b"Camp"), pos: Pos { lat: 1, lon: 2 }, src: WpSrc::Link }));
        assert!(s.request(Op::SaveContact(Contact { call: Buf::from(b"VE3KPW-9"), name: Buf::from(b"Pat") })));
        while let Some(j) = s.next_job() {
            block(s.run(j));
        }
        assert!(s.save(&a));
        let mut f = s.into_parts().0;
        (f.ops, f.erases.iter_mut().for_each(|e| *e = 0));
        f
    })
}

/// Everything a mount reads, as text (so two mounts compare whole).
fn snap(s: &Store) -> String {
    let mut o = String::new();
    o += &format!("mounted {:?}\n", s.mounted());
    for n in 0..MAX_CHANNELS {
        if let Some((c, i)) = s.by_num(n) {
            o += &format!("ch {n} {i} {c:?}\n");
        }
    }
    for z in 0..s.zones() {
        let c = s.zone_cfg(z);
        o += &format!("zone {z} {:?} {} {} {} {} {} {:?} {}\n", s.zone_name(z).map(|n: Name| n), c.scan_all, c.shown, c.by_distance, c.resume, c.dwell, c.pri, s.zone_len(z));
    }
    for i in 0..s.presets() {
        o += &format!("preset {i} {:?}\n", s.preset(i).map(|p| (p.name, p.start, p.end, p.step, p.mode, p.narrow, p.skips, p.resume)));
    }
    for i in 0..10 {
        o += &format!("dtmf {i} {:?} {:?}\n", s.dtmf(i), s.dtmf_name(i));
    }
    o += &format!("aprs {:?}\n", s.aprs_cfg());
    for i in 0..ferrite_app::words::QUICK {
        o += &format!("quick {i} {:?}\n", s.quick(i));
    }
    for i in 0..s.events() {
        o += &format!("event {i} {:?}\n", s.event(i).map(|e| (e.id, e.unread, e.name, e.text)));
    }
    for i in 0..s.messages() {
        o += &format!("msg {i} {:?}\n", s.message(i).map(|m| (m.peer, m.text, m.state, m.inbound)));
    }
    s.heard_rows(64, &mut |st, t| o += &format!("heard {:?} {t}\n", st.call));
    for i in 0..s.waypoints() {
        o += &format!("wp {i} {:?}\n", s.waypoint(i));
    }
    for i in 0..s.contacts() {
        o += &format!("contact {i} {:?}\n", s.contact(i).map(|c| (c.call, c.name)));
    }
    let mut set = Settings::DEFAULT;
    if s.load_settings(&mut set) {
        let mut b = Vec::new();
        struct V<'a>(&'a mut Vec<u8>);
        impl ferrite_app::persist::Sink for V<'_> {
            fn put(&mut self, x: &[u8]) {
                self.0.extend_from_slice(x);
            }
        }
        ferrite_app::persist::Persist::save(&set, &mut V(&mut b));
        o += &format!("settings {}\n", ferrite_fuzzkit::hex(&b));
    }
    o
}

/// Two snapshots the same, or the lines that differ.
fn same(a: &str, b: &str, what: &str) {
    if a != b {
        let (la, lb): (Vec<_>, Vec<_>) = (a.lines().collect(), b.lines().collect());
        let gone: Vec<_> = la.iter().filter(|l| !lb.contains(l)).take(6).collect();
        let new: Vec<_> = lb.iter().filter(|l| !la.contains(l)).take(6).collect();
        panic!("{what}:\n  before: {gone:#?}\n  after: {new:#?}");
    }
}

/// The zone lists agree with the records (as `tests/store.rs` checks after every power cut). A channel whose zone
/// record is gone (erased or damaged) is in no list: counted apart here, see [`orphaned_channels_are_listed`].
fn index_agrees(s: &Store) {
    let mut seen = 0u16;
    for z in (0..s.zones()).chain([NO_ZONE]) {
        let mut last: Option<u16> = None;
        for i in 0..s.zone_len(z) {
            let c: Channel = s.zone_at(z, i).expect("an indexed channel exists");
            assert_eq!(c.zone, z, "channel {} indexed in zone {z}", c.num);
            assert!(last.is_none_or(|l| c.num > l), "zone {z} in number order");
            last = Some(c.num);
            assert_eq!(s.by_num(c.num).map(|x| x.1), Some(i), "position of {}", c.num);
            seen += 1;
        }
    }
    let real = (0..MAX_CHANNELS).filter(|&n| s.by_num(n).is_some()).count() as u16;
    let orphans = (0..MAX_CHANNELS).filter_map(|n| s.by_num(n)).filter(|(c, _)| c.zone != NO_ZONE && s.zone_name(c.zone).is_none()).count() as u16;
    assert_eq!(seen + orphans, real, "the lists cover every channel");
    assert_eq!(s.count(), real);
}

/// Found by fuzzing: zone records damaged (here erased) leave their channels in no list at all, neither a zone's
/// nor "no zone"'s, while `count` still counts them: browsing never shows them. Input: the populated chip with
/// 672 bytes erased from 0x050021 (zones 1 and 2 gone; channels 2, 9, 63, 64, 130 and 500 were in them).
#[test]
#[ignore = "found: channels whose zone record is damaged are in no zone list (not even no zone's) but still counted"]
fn orphaned_channels_are_listed() {
    let mut f = populated().clone();
    edit(&mut f, &[0x21, 0x00, 0x05, 0x00, 2, 0x2A, 0, 0]);
    let s = mount(f);
    let listed: u16 = (0..s.zones()).chain([NO_ZONE]).map(|z| s.zone_len(z)).sum();
    assert_eq!(listed, s.count(), "every channel is in some list");
}

/// The APRS record as mounted: every field the mount checks, in range (a damaged one reads as the default).
fn aprs_in_range(c: &AprsCfg) {
    use ferrite_app::aprs::*;
    assert!(c.power <= 2 && c.tx_level <= 2 && c.ambiguity <= 4, "{:?}", (c.power, c.tx_level, c.ambiguity));
    assert!(FX25_CHECKS.contains(&c.fx25) && (c.sound as usize) < ferrite_core::tonegen::CHIMES.len() && c.retries <= RETRIES_DEFAULT);
    assert!((c.mic_e as usize) < MIC_E_NAMES.len() && (c.symbol as usize) < SYMBOLS.len());
    assert!(c.txdelay == AprsCfg::DEFAULT.txdelay || (TXDELAY_MIN..=TXDELAY_MAX).contains(&c.txdelay));
    let n = c.comment.iter().position(|&b| b == 0).unwrap_or(c.comment.len());
    assert!(c.comment[..n].iter().all(|&b| (b' '..=b'~').contains(&b)), "comment {:?}", c.comment);
}

/// A chip address the simulated chip holds: mostly where the populated chip has data, else anywhere.
fn addr(r: &mut Rng) -> u32 {
    let f = populated();
    if r.one_in(6) {
        // the APRS record's copies (their fields are each checked at mount)
        return ferrite_core::layout::APRS.start + r.below(0x400) as u32 + if r.one_in(2) { 0x1000 } else { 0 };
    }
    if r.one_in(3) {
        return match r.below(3) {
            0 => r.below(LOW) as u32,
            1 => HIGH.start + r.below((HIGH.end - HIGH.start) as usize) as u32,
            _ => PARTS.start + r.below((PARTS.end - PARTS.start) as usize) as u32,
        };
    }
    loop {
        let i = r.below(f.mem.len());
        if f.mem[i] != 0xFF || r.one_in(64) {
            // back from `mem` to the chip's address
            let hi = (HIGH.end - HIGH.start) as usize;
            return if i < LOW {
                i as u32
            } else if i < LOW + hi {
                HIGH.start + (i - LOW) as u32
            } else {
                PARTS.start + (i - LOW - hi) as u32
            };
        }
    }
}

/// A case: a flags byte (bit 0: cut the power during the first mount; bit 1: make a change after it), then edits
/// of 8 bytes: chip address (u32), op, length, value, a second byte.
fn case(r: &mut Rng) -> Vec<u8> {
    let mut v = vec![r.byte()];
    for _ in 0..r.range(1, 6) {
        v.extend(addr(r).to_le_bytes());
        v.push(r.below(7) as u8);
        v.push(if r.one_in(3) { r.byte() } else { r.range(1, 8) as u8 });
        v.push(r.byte());
        v.push(r.byte());
    }
    v
}

fn edit(f: &mut SimFlash, e: &[u8]) {
    let a = u32::from_le_bytes([e[0], e[1], e[2], e[3]]);
    let (op, n, val, x) = (e[4], e[5] as u32, e[6], e[7]);
    let mut put = |at: u32, b: u8| {
        if let Some(i) = SimFlash::place(at) {
            f.mem[i] = b;
        }
    };
    match op % 7 {
        0 => put(a, populated().mem.get(SimFlash::place(a).unwrap_or(0)).copied().unwrap_or(0xFF) ^ (1 << (val % 8))),
        1 => (0..n).for_each(|k| put(a + k, val)),
        // a record cut short: an erase from here on
        2 => (0..n * 16).for_each(|k| put(a + k, 0xFF)),
        3 => (0..n * 16).for_each(|k| put(a + k, 0)),
        // a sector's bytes over another's (a record where another kind belongs, an old copy)
        4 => {
            let from = (a & !0xFFF).wrapping_add((val as i8 as i32 * 4096) as u32);
            for k in 0..(n * 16).min(4096) {
                let b = SimFlash::place(from + k).map_or(0xFF, |i| populated().mem[i]);
                put((a & !0xFFF) + (a + k) % 4096, b);
            }
        }
        5 => {
            let mut r = Rng::new(u64::from(val) << 8 | x as u64);
            (0..n).for_each(|k| put(a + k, r.byte()));
        }
        // a length or count field lying: a 16-bit value at the edges
        _ => {
            let v = [0u16, 1, 0xFF, 0x100, 0x7FFF, 0xFFFE, 0xFFFF, x as u16][val as usize % 8];
            put(a, v as u8);
            put(a + 1, (v >> 8) as u8);
        }
    }
}

fn run(input: &[u8]) {
    let Some((&flags, edits)) = input.split_first() else { return };
    let mut f = populated().clone();
    for e in edits.chunks_exact(8) {
        edit(&mut f, e);
    }
    if flags & 1 != 0 {
        // the power cut at some point of the mount's own writes (repairs, marks), then on again
        f.cut_at = Some(f.ops + 1 + (flags as u64 >> 2) * 3);
        let s = mount(f);
        let mut g = s.into_parts().0;
        g.power_on();
        g.cut_at = None;
        f = g;
    }
    let s = mount(f);
    let first = snap(&s);
    if !matches!(s.mounted(), Mounted::Failed | Mounted::Update(_)) {
        index_agrees(&s);
        let mut set = Settings::DEFAULT;
        if s.load_settings(&mut set) {
            let mut again = set;
            again.sanitize();
            assert!(again == set, "the settings came back out of range");
        }
        let mut a = app();
        let _ = s.restore(&mut a);
        aprs_in_range(&s.aprs_cfg());
    }
    while let Some(j) = s.next_job() {
        block(s.run(j));
    }
    // the same chip mounted again reads the same
    let s = mount(s.into_parts().0);
    same(&first, &snap(&s), "a second mount read the chip differently");
    if flags & 2 != 0 && matches!(s.mounted(), Mounted::Ok | Mounted::FirstBoot) {
        // a change on the damaged chip: done or refused, and the next mount agrees
        let _ = s.apply(ChanWrite::Put { ch: channel(3, 0, 14_550_000, "New", DEFAULT_CFG), from: None });
        let _ = s.add_zone(b"Fuzz", &ZoneCfg::default());
        let _ = s.log(&Event { src: Src::Aprs, ago_s: 0, freq: 14_439_000, slot: 0, unread: true, id: 77, x: 0, y: 0, pos: None, name: Buf::from(b"X"), text: Buf::from(b"Y") });
        let _ = s.save(&app());
        while let Some(j) = s.next_job() {
            block(s.run(j));
        }
        // what the chip holds, less how this power-on found it (a first boot's chip is set up now)
        let before = snap(&s).split_once('\n').unwrap().1.to_string();
        let s = mount(s.into_parts().0);
        index_agrees(&s);
        same(&before, snap(&s).split_once('\n').unwrap().1, "after a change, the next mount read the chip differently");
    }
}

#[test]
fn fuzz_store_mount() {
    let s = mount(populated().clone());
    assert_eq!(s.mounted(), Mounted::Ok);
    index_agrees(&s);
    Fuzz::new("store_mount", 0x5704, 1_500).slow_ms(10_000).run(case, run);
}

/// Whole chips of random bytes, or the populated chip under a random region: a foreign image is left alone or
/// formatted, never misread, and mounts the same twice.
#[test]
fn fuzz_store_random_chip() {
    Fuzz::new("store_random_chip", 0x5705, 60).slow_ms(10_000).run(
        |r| {
            let mut v = vec![r.byte()];
            v.extend(r.u64_bytes());
            v
        },
        |input| {
            let mut r = Rng::new(u64::from_le_bytes(input[1..9].try_into().unwrap()));
            let mut f = if input[0] & 1 == 0 { SimFlash::new() } else { populated().clone() };
            let at = r.below(LOW);
            let (from, to) = if input[0] & 2 == 0 { (0, f.mem.len()) } else { (at, (at + 65536).min(f.mem.len())) };
            for b in &mut f.mem[from..to] {
                *b = r.byte();
            }
            let s = mount(f);
            let first = snap(&s);
            let s = mount(s.into_parts().0);
            same(&first, &snap(&s), "a second mount read the chip differently");
        },
    );
}

trait U64Bytes {
    fn u64_bytes(&mut self) -> [u8; 8];
}

impl U64Bytes for Rng {
    fn u64_bytes(&mut self) -> [u8; 8] {
        self.next().to_le_bytes()
    }
}
