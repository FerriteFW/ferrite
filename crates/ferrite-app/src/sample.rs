//! A radio's worth of sample records: channels in zones (some with positions), an event log, APRS stations
//! and messages, a GPS fix with satellites, waypoints, range presets and HF recent, all
//! around Ottawa. The simulator and the host tests read them through the same traits the firmware's stores
//! implement ([`ChannelStore`], [`Data`]); requests ([`Op`]) change them, so deleting, skipping or marking
//! shows on screen. Host memory only: nothing here is linked into the firmware (it doesn't use it).

use crate::chan::{channel, ChanWrite, Channel, ChannelStore, Name, FLAG_SKIP, NO_ZONE, ZoneId};
use crate::data::*;
use crate::settings::Settings;
use crate::slot::{SlotCfg, DCS_FLAG, DEFAULT_CFG};
use core::cell::Cell;

mod aprs;
pub use aprs::AprsNews;
use ferrite_core::freq::{mhz, Freq10};
use ferrite_ui::text::Buf;

const fn cfg(tone: u16, offset: i32) -> SlotCfg {
    let mut c = DEFAULT_CFG;
    c.tx_tone = tone;
    c.offset = offset;
    c
}

const fn mode(m: u8, narrow: bool) -> SlotCfg {
    let mut c = DEFAULT_CFG;
    c.mode = m;
    c.narrow = narrow;
    c
}

const AIR: SlotCfg = mode(2, false);
const WIDE: SlotCfg = mode(0, false);

const ZONES: [&[u8]; 8] = [b"Simplex", b"Repeaters", b"Empty", b"Air", b"Marine", b"Weather", b"Zone 7", b"Zone 8"];
/// Zones 0-5 exist at first; "new zone" brings the next one in.
const ZONES_AT_START: usize = 6;

/// The channels: the first eight are the look review's sample (home's mock-ups).
const CHANNELS: [Channel; 21] = [
    channel(0, 0, mhz(146) + 52_000, "Calling 2 m", DEFAULT_CFG),
    channel(1, 0, mhz(446), "Calling 70 cm", DEFAULT_CFG),
    channel(2, 0, mhz(147) + 55_500, "Simplex B", DEFAULT_CFG),
    channel(9, 1, mhz(146) + 94_000, "North hill", cfg(1000, -60_000)),
    channel(11, 1, mhz(443) + 47_500, "Club repeater", cfg(1000, 500_000)),
    channel(14, 1, mhz(444) + 20_000, "Downtown DCS", cfg(DCS_FLAG | 0o023, 500_000)),
    channel(30, 3, mhz(121) + 50_000, "Guard", AIR),
    channel(31, 3, mhz(119) + 10_000, "Tower", AIR),
    channel(16, 1, mhz(147) + 15_000, "Camp Fortune", cfg(1000, 60_000)),
    channel(17, 1, mhz(145) + 29_000, "Far east", cfg(1109, -60_000)),
    channel(32, 3, mhz(132) + 95_000, "ATIS", AIR),
    channel(40, 4, mhz(156) + 80_000, "Marine 16", WIDE),
    channel(41, 4, mhz(156) + 42_500, "Marine 68", WIDE),
    channel(42, 4, mhz(156) + 62_500, "Marine 72", WIDE),
    channel(50, 5, mhz(162) + 55_000, "WX1", WIDE),
    channel(51, 5, mhz(162) + 40_000, "WX2", WIDE),
    channel(52, 5, mhz(162) + 47_500, "WX3", WIDE),
    channel(60, NO_ZONE, mhz(145) + 80_000, "ISS downlink", DEFAULT_CFG),
    channel(61, NO_ZONE, mhz(438) + 80_000, "Hotspot", DEFAULT_CFG),
    channel(62, NO_ZONE, mhz(462) + 56_250, "FRS 1", DEFAULT_CFG),
    channel(3, 0, mhz(146) + 58_000, "Simplex C", DEFAULT_CFG),
];

/// Channels with a position (CHAN-2), by number.
const CHAN_POS: [(u16, Pos); 5] = [
    (9, Pos { lat: 455_454_922, lon: -757_283_492 }),
    (11, Pos { lat: 454_075_605, lon: -756_628_014 }),
    (14, Pos { lat: 454_178_090, lon: -757_116_483 }),
    (16, Pos { lat: 451_800_612, lon: -758_958_003 }),
    (17, Pos { lat: 453_838_771, lon: -750_845_200 }),
];

/// Where the sample radio is: Parliament Hill, Ottawa.
pub const HERE: Pos = Pos { lat: 454_215_000, lon: -756_972_000 };

const fn at(lat: i32, lon: i32) -> Option<Pos> {
    Some(Pos { lat, lon })
}

// call, symbol, position, age s, via, course and speed, altitude, times heard, comment, last message, its minute
type StationRow = (&'static str, Sym, Option<Pos>, u32, &'static str, Option<(u16, u16)>, Option<i32>, u16, &'static str, &'static str, Option<u16>);
const STATIONS: [StationRow; 11] = [
    ("VE3KPW-9", Sym::Car, at(454_467_741, -756_572_082), 120, "", Some((92, 54)), Some(87), 6, "Mobile, on 146.520", "On my way", Some(14 * 60 + 10)),
    ("VA3RTX", Sym::Home, at(454_633_076, -758_249_370), 240, "VE3RSB-1*", None, Some(102), 14, "Ron, Kanata. QRV 2 m/70 cm", "Net at 8 tonight?", Some(13 * 60 + 12)),
    ("VE3RSB-1", Sym::Digi, at(456_158_696, -755_964_076), 300, "", None, Some(212), 31, "Fill-in digi, Gatineau", "", None),
    ("VE3OBW", Sym::Wx, at(452_778_355, -757_716_987), 540, "VE3RSB-1*", None, Some(95), 9, "270/007g012t072r004h65b10123 Ottawa WX", "", None),
    ("VA3ZZB-7", Sym::Ht, at(454_243_110, -756_744_871), 720, "", Some((10, 4)), Some(71), 3, "On foot", "", None),
    ("VE2GPA-10", Sym::IGate, at(455_383_826, -752_396_720), 1260, "WIDE2*", None, None, 22, "RX-only iGate", "", None),
    ("VE3JLH", Sym::Home, at(454_662_798, -756_436_661), 2040, "", None, Some(78), 5, "", "", None),
    ("VE3XK-9", Sym::Car, None, 3120, "", None, None, 1, "Status only", "", None),
    ("VA3MAR", Sym::Boat, at(453_671_612, -756_049_365), 3600, "VE3RSB-1*,WIDE2-1", Some((245, 13)), Some(58), 4, "Rideau canal", "", None),
    ("VE3TOS-5", Sym::Truck, at(452_277_209, -764_557_333), 4500, "VE3RSB-1,WIDE2*", Some((70, 96)), Some(120), 2, "Hwy 417 east", "", None),
    ("VA3TCJ", Sym::Home, at(453_969_925, -756_844_914), 6000, "", None, None, 8, "Tim, Glebe", "", None),
];

// source, age s, frequency, slot, unread, id, x, y, position, name, text
type EventRow = (Src, u32, Freq10, u8, bool, u32, u16, u16, Option<Pos>, &'static str, &'static str);
const EVENTS: [EventRow; 6] = [
    (Src::Aprs, 120, mhz(144) + 39_000, 2, true, 0, 0, 0, None, "VE3KPW-9", "On my way"),
    (Src::Pocsag, 380, mhz(439) + 98_750, 2, true, 1_234_567, 1200, 3, None, "", "STN 4 RESPOND MVC HWY 417 EB AT KM 120"),
    (Src::Same, 840, mhz(162) + 55_000, 2, true, 0, 0, 0, None, "", "ZCZC-WXR-SVR-035001-035003+0045-2761410-KOTT/NWS-"),
    (Src::Pocsag, 2820, mhz(439) + 98_750, 2, false, 1_234_571, 1200, 0, None, "", "MEETING MOVED TO 1930"),
    (Src::Aprs, 4800, mhz(144) + 39_000, 2, false, 0, 0, 0, None, "VA3RTX", "Net at 8 tonight?"),
    (Src::Pocsag, 9000, mhz(439) + 98_750, 2, false, 1_234_570, 512, 0, None, "", "TEST PAGE 0900"),
];

// inbound, peer, text, age s, state
type MsgRow = (bool, &'static str, &'static str, u32, MsgState);
const MESSAGES: [MsgRow; 5] = [
    (true, "VE3KPW-9", "On my way", 120, MsgState::Unread),
    (false, "VE3KPW-9", "Where are you?", 260, MsgState::Acked),
    (false, "VA3RTX", "Yes, 146.940 at 20:00", 4200, MsgState::Retrying(2)),
    (true, "VA3RTX", "Net at 8 tonight?", 4800, MsgState::Read),
    (false, "VE3JLH", "73, thanks for the QSO", 10_800, MsgState::Failed),
];

/// The sample contacts, in callsign order.
const CONTACTS: [(&str, &str); 4] = [("VA3RTX", "Ron"), ("VA3ZZB-7", "Club net"), ("VE2GPA", "Gilles"), ("VE3KPW-9", "Mike")];

const WAYPOINTS: [(&str, Pos, WpSrc); 5] = [
    ("Home", Pos { lat: 454_164_295, lon: -756_998_294 }, WpSrc::Marked),
    ("Club shack", Pos { lat: 454_089_577, lon: -756_588_789 }, WpSrc::Link),
    ("Trailhead P2", Pos { lat: 455_076_150, lon: -758_001_499 }, WpSrc::Marked),
    ("VE3RSB-1", Pos { lat: 456_158_696, lon: -755_964_076 }, WpSrc::Aprs),
    ("Pink Lake", Pos { lat: 455_015_396, lon: -758_947_145 }, WpSrc::Marked),
];

// prn, snr, used, BeiDou
/// PRN, signal (dB-Hz), used, BeiDou, elevation, azimuth.
const SATS: [(u8, u8, bool, bool, u8, u16); 12] = [
    (2, 41, true, false, 62, 48),
    (5, 38, true, false, 41, 300),
    (6, 22, false, false, 9, 205),
    (12, 44, true, false, 78, 140),
    (13, 35, true, false, 33, 92),
    (15, 18, false, false, 6, 330),
    (19, 40, true, false, 55, 230),
    (24, 29, true, false, 21, 12),
    (25, 33, true, false, 27, 165),
    (8, 36, true, true, 47, 260),
    (11, 27, true, true, 18, 120),
    (14, 12, false, true, 4, 60),
];

const PRESETS: [(&str, Freq10, Freq10, u8, u8, bool, u8); 4] = [
    ("2 m", mhz(144), mhz(148), 10, 0, true, 3),
    ("70 cm", mhz(430), mhz(450), 10, 0, true, 0),
    ("Airband", mhz(118), mhz(137), 11, 2, false, 5),
    ("Marine", mhz(156), mhz(162) + 2_500, 11, 0, false, 1),
];

const SKIPS: [Freq10; 3] = [mhz(146) + 94_000, mhz(147) + 15_000, mhz(145) + 29_000];

/// HF recent, newest first: frequency and mode (as `Param::Mode`: 2 AM, 3 USB, 4 LSB, 5 CW).
const HF: [(Freq10, u8); 6] = [(707_400, 3), (1_407_400, 3), (376_000, 4), (1_013_600, 3), (703_000, 5), (607_000, 2)];

/// The sample radio's settings: the look review's callsign, Canada (as the mock-ups have it), HF recent, and GPS on
/// (off by default since decision 235, GPS-1.4; the sample shows the position screens with a fix).
pub fn settings() -> Settings {
    let mut s = Settings::DEFAULT;
    s.gps = crate::settings::GPS_ON;
    s.set_callsign(b"VA7FER");
    s.set_region(Some(ferrite_core::txplan::Region::Canada));
    s.setup_done = true;
    for (r, &(f, m)) in s.hf.recent.iter_mut().zip(&HF) {
        *r = f | (m as u32) << 28;
    }
    s
}

/// The sample owner card (SETUP-11), shown at boot (0 7 3 3): the sim's boot-owner shot and tests.
pub fn owner_card(s: &mut Settings) {
    s.owner[0].set(b"Sam Fernandez");
    s.owner[1].set(b"+1 604 555 0142");
    s.owner[2].set(b"va7fer@example.org");
    s.boot_card = true;
}

/// The sample radio's records. Cells hold what requests change.
pub struct Sample {
    /// The sample channels, then room for the ones the app saves.
    chans: [Cell<Option<Channel>>; CHANNELS.len() + 16],
    /// Recent (CHAN-25).
    recent: crate::chan::RecentList,
    zones: Cell<ZoneId>,
    zone_cfg: [Cell<ZoneCfg>; 8],
    zone_names: [Cell<Name>; 8],
    presets: [Cell<Option<RangePreset>>; 16],
    /// Each preset place's skipped frequencies (SCAN-10, LINK-58): they stay with the preset in its place.
    preset_skips: [Cell<[Freq10; 48]>; 16],
    /// LINK-71: the channels', zones' and presets' write counts, as the radio's store keeps them.
    lists: Cell<[u32; 3]>,
    dtmf: [Cell<Option<Dtmf>>; DTMF_MEMS as usize],
    dtmf_names: [Cell<Buf<10>>; DTMF_MEMS as usize],
    events_read: Cell<u32>,
    /// Events logged since (DTMF heard, [`Sample::log_dtmf`]), newest first, ahead of the sample's.
    logged: Cell<u16>,
    events_cleared: Cell<bool>,
    msgs_read: Cell<u32>,
    /// APRS-6.4: messages to everyone, put here by tests (none in the simulators' sample data).
    notes: Cell<[Option<crate::data::Notice>; crate::aprs_live::NOTICES]>,
    /// APRS-46.1: sample messages deleted one by one, a bit each (by their place in `MESSAGES`).
    msgs_del: Cell<u32>,
    /// The reset (Op::Wipe) took the sample messages.
    msgs_gone: Cell<bool>,
    /// The APRS task's live side: heard, messages with their tries, beacons.
    aprs: aprs::AprsSim,
    draft: Cell<Draft>,
    quick: Cell<[Option<Buf<67>>; crate::words::QUICK]>,
    wp_gone: Cell<u32>,
    wp_marked: Cell<u8>,
    wp_saved: Cell<Option<u16>>,
    /// A waypoint typed as a grid square (GPS-21), or added from the link (LINK-55): one, first in the list.
    wp_typed: Cell<Option<Waypoint>>,
    /// Waypoints replaced from the link (LINK-55), by their place in the whole list (deleted ones counted).
    wp_over: Cell<[Option<(u16, Waypoint)>; 4]>,
    /// Contacts in callsign order (APRS-37); a few to start with, room for more.
    contacts: Cell<[Option<Contact>; 12]>,
    skips_gone: Cell<u32>,
    /// KISS frames sent and heard (APRS-17's KISS card): the simulators' KISS port counts them.
    kiss: Cell<(u16, u16)>,
    heard_cleared: Cell<bool>,
    version: Cell<u32>,
    /// None: no fix (GPS searching).
    pub fix: Cell<Option<Fix>>,
    /// Positions set or cleared since (`ChanWrite::PutPos`), over [`CHAN_POS`]; `no_positions` hides them all.
    pos_set: [Cell<Option<PosSet>>; 8],
    no_positions: Cell<bool>,
    pub call: Cell<Option<Buf<10>>>,
    /// ACARS (chapter 27b): the sample traffic, heard just before power-on (ACARS-8).
    pub acars: crate::acars::AcarsLive,
}

/// A channel's position as set since: its number and the position (None: cleared).
type PosSet = (u16, Option<Pos>);

fn txt<const N: usize>(s: &str) -> Buf<N> {
    Buf::from(s.as_bytes())
}

impl Sample {
    /// APRS-6.4, tests: a message to everyone heard, at the front.
    pub fn add_notice(&self, n: crate::data::Notice) {
        let mut a = self.notes.get();
        a.rotate_right(1);
        a[0] = Some(n);
        self.notes.set(a);
    }

    /// A beacon asked for (tests: APRS-17.8).
    #[cfg(test)]
    pub(crate) fn beacon_asked(&self) -> bool {
        self.aprs.beacon_asked()
    }

    pub fn new() -> Self {
        let s = Self::blank();
        crate::acars::sample::feed(&s.acars, usize::MAX, 0, 0);
        s
    }

    fn blank() -> Self {
        let zc = |z: usize| ZoneCfg { scan_all: z == 1, shown: z != 5, resume: if z == 3 { 11 } else { 0 }, dwell: 255, pri: [if z == 1 { Some(11) } else { None }, None], ..ZoneCfg::default() };
        Sample {
            chans: core::array::from_fn(|i| Cell::new(CHANNELS.get(i).copied())),
            recent: crate::chan::RecentList::new(),
            lists: Cell::new([0; 3]),
            zones: Cell::new(ZONES_AT_START as ZoneId),
            zone_cfg: core::array::from_fn(|z| Cell::new(zc(z))),
            zone_names: core::array::from_fn(|z| Cell::new(Name::from(ZONES[z]))),
            preset_skips: core::array::from_fn(|i| Cell::new(core::array::from_fn(|j| PRESETS.get(i).map_or(0, |p| p.1 + (j as u32 + 1) * 2_500)))),
            presets: core::array::from_fn(|i| {
                Cell::new(PRESETS.get(i).map(|&(n, start, end, step, mode, narrow, skips)| RangePreset { name: txt(n), start, end, step, mode, narrow, skips,
                    // Airband resumes when the carrier drops (SCAN-24); the rest follow 0 1 5.
                    resume: if n == "Airband" { 11 } else { 0 } }))
            }),
            // Two DTMF memories (chapter 17): a repeater's autopatch and a link code.
            dtmf: core::array::from_fn(|i| Cell::new([Dtmf::parse(b"*79"), Dtmf::parse(b"12345#")].get(i).copied().flatten())),
            dtmf_names: core::array::from_fn(|i| Cell::new(Buf::from(if i == 0 { &b"Link on"[..] } else { b"" }))),
            events_read: Cell::new(0),
            logged: Cell::new(0),
            events_cleared: Cell::new(false),
            msgs_read: Cell::new(0),
            notes: Cell::new([None; crate::aprs_live::NOTICES]),
            msgs_del: Cell::new(0),
            msgs_gone: Cell::new(false),
            aprs: aprs::AprsSim::new(),
            draft: Cell::new(Draft::default()),
            quick: Cell::new([None; crate::words::QUICK]),
            wp_gone: Cell::new(0),
            wp_marked: Cell::new(0),
            wp_saved: Cell::new(None),
            wp_typed: Cell::new(None),
            wp_over: Cell::new([None; 4]),
            contacts: Cell::new({
                let mut c = [None; 12];
                for (k, &(call, name)) in CONTACTS.iter().enumerate() {
                    c[k] = Some(Contact { call: txt(call), name: txt(name) });
                }
                c
            }),
            skips_gone: Cell::new(0),
            kiss: Cell::new((0, 0)),
            heard_cleared: Cell::new(false),
            version: Cell::new(1),
            fix: Cell::new(Some(Fix { pos: HERE, three_d: true, alt_m: 74, speed_kmh: 0, heading: 0, hdop_x10: 9, used: 9, age_s: 1, utc_s: 18 * 3600 + 32 * 60 + 5, phone: false })),
            call: Cell::new(Some(txt("VA7FER-9"))),
            pos_set: core::array::from_fn(|_| Cell::new(None)),
            no_positions: Cell::new(false),
            acars: crate::acars::AcarsLive::new(),
        }
    }

    /// The simulators' KISS port: a frame from the client sent (`sent`) or one heard passed to it (APRS-17's
    /// KISS card counts them).
    pub fn kiss_frame(&self, sent: bool) {
        let (s, h) = self.kiss.get();
        self.kiss.set(if sent { (s.saturating_add(1), h) } else { (s, h.saturating_add(1)) });
        self.bump();
    }

    /// Every channel without a position (true), or the sample's again: the zones page's plain list (CHAN-29).
    pub fn hide_positions(&self, on: bool) {
        self.no_positions.set(on);
        self.bump();
    }

    /// The sample's fix (Parliament Hill), as it starts.
    pub fn sample_fix() -> Option<Fix> {
        Sample::new().fix.get()
    }

    fn live(&self) -> impl Iterator<Item = Channel> + '_ {
        self.chans.iter().filter_map(|c| c.get())
    }

    /// The i-th smallest-numbered channel among those that pass `keep`.
    fn nth(&self, i: u16, keep: impl Fn(&Channel) -> bool) -> Option<Channel> {
        let mut last: Option<u16> = None;
        let mut found = None;
        for _ in 0..=i {
            found = self.live().filter(|c| keep(c) && last.is_none_or(|l| c.num > l)).min_by_key(|c| c.num);
            last = Some(found?.num);
        }
        found
    }

    /// LINK-71: the lists a write changed, counted as the radio's store counts them.
    fn lists_moved(&self, which: u8) {
        let mut l = self.lists.get();
        for (i, v) in l.iter_mut().enumerate() {
            if which & 1 << i != 0 {
                *v = v.wrapping_add(1);
            }
        }
        self.lists.set(l);
    }

    /// A channel write from the app ([`crate::App::pop_write`]), as the Storage task does it in SPI flash.
    pub fn apply(&self, w: ChanWrite) {
        self.lists_moved(w.lists());
        match w {
            ChanWrite::Put { ch, from } => {
                for c in &self.chans {
                    if c.get().is_some_and(|c| c.num == ch.num || Some(c.num) == from) {
                        c.set(None);
                    }
                }
                if let Some(c) = self.chans.iter().find(|c| c.get().is_none()) {
                    c.set(Some(ch));
                }
            }
            ChanWrite::PutPos { ch, pos } => {
                self.apply(ChanWrite::Put { ch, from: None });
                self.set_pos(ch.num, pos);
            }
            ChanWrite::Pos { num, pos } => {
                if self.slot_of(num).is_some() {
                    self.set_pos(num, pos);
                }
            }
            ChanWrite::Delete(n) => {
                if let Some(c) = self.slot_of(n) {
                    c.set(None);
                }
            }
            ChanWrite::DeleteZone(z) => {
                for c in &self.chans {
                    if let Some(mut ch) = c.get().filter(|ch| ch.zone == z) {
                        ch.zone = NO_ZONE;
                        c.set(Some(ch));
                    }
                }
            }
            ChanWrite::EraseAll => self.chans.iter().for_each(|c| c.set(None)),
        }
        self.bump();
    }

    fn set_pos(&self, num: u16, pos: Option<Pos>) {
        let at = self.pos_set.iter().find(|c| c.get().is_some_and(|(n, _)| n == num)).or_else(|| self.pos_set.iter().find(|c| c.get().is_none()));
        if let Some(c) = at {
            c.set(Some((num, pos)));
        }
    }

    fn slot_of(&self, num: u16) -> Option<&Cell<Option<Channel>>> {
        self.chans.iter().find(|c| c.get().is_some_and(|c| c.num == num))
    }

    fn bump(&self) -> bool {
        self.version.set(self.version.get().wrapping_add(1));
        true
    }

    /// A waypoint went in first: the deleted bits and the replaced ones move along with the stored ones.
    fn wp_added(&self) {
        self.wp_gone.set(self.wp_gone.get() << 1);
        self.wp_over.set(self.wp_over.get().map(|o| o.map(|(k, w)| (k + 1, w))));
    }

    /// An event logged now (a DTMF sequence heard, "*7N#"): the link's event log subscription sees it (LINK-57).
    pub fn log_dtmf(&self) {
        self.logged.set(self.logged.get() + 1);
        self.bump();
    }

    fn marked(&self) -> u16 {
        self.wp_marked.get() as u16 + self.wp_saved.get().is_some() as u16 + self.wp_typed.get().is_some() as u16
    }

    fn sent_n(&self) -> u16 {
        self.aprs.live_n()
    }

    /// The sample message shown at place `v` after the live ones: its place in `MESSAGES` (deleted ones skipped).
    fn sample_msg(&self, v: u16) -> Option<usize> {
        if self.msgs_gone.get() {
            return None;
        }
        (0..MESSAGES.len()).filter(|&k| self.msgs_del.get() & 1 << k == 0).nth(v as usize)
    }

    /// A second of the simulated APRS task ([`aprs`]): APRS mode on or off, and whether C can transmit with the
    /// band relay where the active slot holds it. The caller notifies the arrivals and shows the transmissions.
    pub fn aprs_tick(&self, now_s: u32, on: bool, relay_ok: bool) -> AprsNews {
        let known = |c: &[u8]| STATIONS.iter().any(|s| s.0.as_bytes() == c) || self.aprs.heard(c);
        let n = self.aprs.tick(now_s, on, relay_ok, &known, self.fix.get());
        if n.changed {
            self.bump();
        }
        n
    }
}

impl ChannelStore for Sample {
    fn lists_versions(&self) -> [u32; 3] {
        self.lists.get()
    }

    /// The simulator's lists aren't in sectors: every sector of a list says that list's count, so a phone reading
    /// against the simulator reads the whole list again after a change.
    fn sector_hash(&self, i: u16) -> Option<u32> {
        let l = self.lists.get();
        match i {
            0..64 => Some(l[0]),
            64..69 => Some(l[1]),
            69 => Some(l[2]),
            _ => None,
        }
    }

    fn queue_write(&self, w: ChanWrite) -> bool {
        self.apply(w); // at once, as its writes always are
        true
    }
    fn recent(&self, r: u16) -> Option<u16> {
        self.recent.get(r)
    }
    fn used(&self, num: u16) {
        self.recent.touch(num);
    }
    fn tuned(&self, num: u16) {
        self.recent.tuned(num);
    }
    fn zones(&self) -> ZoneId {
        self.zones.get()
    }
    fn zone_name(&self, zone: ZoneId) -> Option<Name> {
        (zone < self.zones.get()).then(|| self.zone_names[zone as usize].get())
    }
    fn zone_len(&self, zone: ZoneId) -> u16 {
        self.live().filter(|c| c.zone == zone).count() as u16
    }
    fn zone_at(&self, zone: ZoneId, i: u16) -> Option<Channel> {
        self.nth(i, |c| c.zone == zone)
    }
    fn count(&self) -> u16 {
        self.live().count() as u16
    }
    fn by_number(&self, i: u16) -> Option<Channel> {
        self.nth(i, |_| true)
    }
    fn position(&self, num: u16) -> Option<Pos> {
        if self.no_positions.get() {
            return None;
        }
        if let Some((_, p)) = self.pos_set.iter().filter_map(Cell::get).find(|(n, _)| *n == num) {
            return p;
        }
        CHAN_POS.iter().find(|p| p.0 == num).map(|p| p.1)
    }
    fn data(&self) -> &dyn Data {
        self
    }
}

impl Data for Sample {
    fn version(&self) -> u32 {
        self.version.get()
    }

    fn acars(&self) -> Option<&crate::acars::AcarsLive> {
        Some(&self.acars)
    }

    fn events(&self) -> u16 {
        self.logged.get() + if self.events_cleared.get() { 0 } else { EVENTS.len() as u16 }
    }
    fn event(&self, i: u16) -> Option<Event> {
        if i >= self.events() {
            return None;
        }
        let n = self.logged.get();
        if i < n {
            let mut text = Buf::<64>::from(b"*7");
            text.push(&[b'0' + ((n - 1 - i) % 10) as u8, b'#']);
            return Some(Event { src: Src::Dtmf, ago_s: 0, freq: mhz(146) + 52_000, slot: 0, unread: true, id: 0, x: 0, y: 0, pos: None, name: Buf::new(), text });
        }
        let i = i - n;
        let (src, ago_s, freq, slot, unread, id, x, y, pos, name, text) = EVENTS[i as usize];
        let unread = unread && self.events_read.get() & 1 << i == 0;
        Some(Event { src, ago_s, freq, slot, unread, id, x, y, pos, name: txt(name), text: txt(text) })
    }

    fn stations(&self) -> u16 {
        let n = self.aprs.stations() + if self.heard_cleared.get() { 0 } else { STATIONS.len() as u16 };
        n.min(crate::aprs::HEARD_MAX)
    }
    fn station(&self, i: u16) -> Option<Station> {
        if i >= self.stations() {
            return None;
        }
        let Some(k) = i.checked_sub(self.aprs.stations()) else { return self.aprs.station(i) };
        let (call, sym, pos, ago_s, via, course, alt_m, times, comment, msg, msg_min) = STATIONS[k as usize];
        let ago_s = ago_s + self.aprs.now_s();
        Some(Station { call: txt(call), sym, pos, ago_s, via: txt(via), course, alt_m, times, comment: txt(comment), msg: txt(msg), msg_min, msg_to: Default::default(), fx25: None, device: None })
    }
    fn callsign(&self) -> Option<Buf<10>> {
        self.call.get()
    }

    fn messages(&self) -> u16 {
        let kept = if self.msgs_gone.get() { 0 } else { MESSAGES.len() as u16 - self.msgs_del.get().count_ones() as u16 };
        kept + self.sent_n()
    }
    fn message(&self, i: u16) -> Option<Message> {
        if i < self.sent_n() {
            return self.aprs.message(i);
        }
        let k = self.sample_msg(i - self.sent_n())?;
        let &(inbound, peer, text, ago_s, state) = MESSAGES.get(k)?;
        let ago_s = ago_s + self.aprs.now_s();
        let state = if state == MsgState::Unread && self.msgs_read.get() & 1 << k != 0 { MsgState::Read } else { state };
        Some(Message { inbound, peer: txt(peer), text: txt(text), ago_s, state, to: Buf::new() })
    }
    fn notices(&self) -> u16 {
        self.notes.get().iter().flatten().count() as u16
    }
    fn notice(&self, i: u16) -> Option<crate::data::Notice> {
        self.notes.get().iter().flatten().nth(i as usize).copied()
    }
    fn notice_read(&self, i: u16) -> bool {
        let mut n = self.notes.get();
        let Some(m) = n.iter_mut().flatten().nth(i as usize) else { return false };
        let was = core::mem::replace(&mut m.unread, false);
        self.notes.set(n);
        was
    }
    fn draft(&self) -> Draft {
        self.draft.get()
    }
    fn quick(&self, i: usize) -> Option<Buf<67>> {
        self.quick.get().get(i).copied().flatten()
    }
    fn aprs_cfg(&self) -> AprsCfg {
        self.aprs.cfg.get()
    }
    fn beacon(&self) -> Beacon {
        self.aprs.beacon()
    }
    fn beacons_sent(&self) -> u16 {
        self.aprs.beacons_sent()
    }
    fn beacon_sent(&self, i: u16) -> Option<crate::data::SentBeacon> {
        self.aprs.sent_beacon(i)
    }
    fn kiss_counts(&self) -> (u16, u16) {
        self.kiss.get()
    }

    fn contacts(&self) -> u16 {
        self.contacts.get().iter().flatten().count() as u16
    }
    fn contact(&self, i: u16) -> Option<Contact> {
        self.contacts.get().iter().flatten().nth(i as usize).copied()
    }

    fn waypoints(&self) -> u16 {
        (0..WAYPOINTS.len() as u16 + self.marked()).filter(|&i| self.wp_gone.get() & 1 << i == 0).count() as u16
    }
    fn waypoint(&self, i: u16) -> Option<Waypoint> {
        // Marked ones first (newest first), then the stored ones; deleted ones skipped.
        let k = (0..WAYPOINTS.len() as u16 + self.marked()).filter(|&k| self.wp_gone.get() & 1 << k == 0).nth(i as usize)?;
        if let Some((_, w)) = self.wp_over.get().into_iter().flatten().find(|o| o.0 == k) {
            return Some(w);
        }
        let fix = self.fix.get().map_or(HERE, |f| f.pos);
        let mut m = self.marked();
        let mut k = k;
        if let Some(w) = self.wp_typed.get() {
            if k == 0 {
                return Some(w);
            }
            (k, m) = (k - 1, m - 1);
        }
        if k < m {
            if let (0, Some(s)) = (k, self.wp_saved.get()) {
                let st = self.station(s)?;
                return Some(Waypoint { name: Buf::from(st.call.as_bytes()), pos: st.pos?, src: WpSrc::Aprs });
            }
            let mut name = Buf::<16>::new();
            let _ = ufmt::uwrite!(name, "Marked {}", m - k);
            return Some(Waypoint { name, pos: fix, src: WpSrc::Marked });
        }
        let (n, pos, src) = WAYPOINTS[(k - m) as usize];
        Some(Waypoint { name: txt(n), pos, src })
    }

    fn fix(&self) -> Option<Fix> {
        self.fix.get()
    }
    fn sats(&self) -> u8 {
        SATS.len() as u8
    }
    fn sat(&self, i: u8) -> Option<Sat> {
        let &(prn, snr, used, beidou, elev, azim) = SATS.get(i as usize)?;
        Some(Sat { prn, snr, used, beidou, elev, azim })
    }

    fn presets(&self) -> u8 {
        self.presets.iter().filter(|p| p.get().is_some()).count() as u8
    }
    fn preset(&self, i: u8) -> Option<RangePreset> {
        self.presets.iter().filter_map(|p| p.get()).nth(i as usize)
    }
    fn preset_skip(&self, i: u8, j: u8) -> Option<Freq10> {
        let k = (0..16).filter(|&k| self.presets[k].get().is_some()).nth(i as usize)?;
        (j < self.presets[k].get()?.skips && j < 48).then(|| self.preset_skips[k].get()[j as usize])
    }
    fn skips(&self) -> u16 {
        (0..SKIPS.len()).filter(|&i| self.skips_gone.get() & 1 << i == 0).count() as u16
    }
    fn skip(&self, i: u16) -> Option<Freq10> {
        (0..SKIPS.len()).filter(|&k| self.skips_gone.get() & 1 << k == 0).nth(i as usize).map(|k| SKIPS[k])
    }
    fn zone_cfg(&self, zone: ZoneId) -> ZoneCfg {
        self.zone_cfg.get(zone as usize).map_or_else(ZoneCfg::default, |c| c.get())
    }

    fn dtmf(&self, i: u8) -> Option<Dtmf> {
        self.dtmf.get(i as usize)?.get()
    }
    fn dtmf_name(&self, i: u8) -> Buf<10> {
        self.dtmf_names.get(i as usize).map_or(Buf::new(), |c| c.get())
    }

    fn request(&self, op: Op) -> bool {
        self.lists_moved(op.lists());
        let edit = |num: u16, f: &dyn Fn(&mut Channel)| match self.slot_of(num) {
            Some(cell) => {
                let mut c = cell.get();
                if let Some(c) = &mut c {
                    f(c);
                }
                cell.set(c);
                true
            }
            None => false,
        };
        let ok = match op {
            Op::Read(i) => {
                self.events_read.set(self.events_read.get() | 1 << i);
                true
            }
            Op::ReadAll => {
                self.events_read.set(u32::MAX);
                true
            }
            Op::ClearLog => {
                self.events_cleared.set(true);
                true
            }
            Op::MessagesReadAll => {
                self.msgs_read.set(u32::MAX);
                // DEC-22: the pages shown among the messages are read with them
                let pages = EVENTS.iter().enumerate().filter(|(_, e)| e.0 == Src::Pocsag).fold(0u32, |m, (i, _)| m | 1 << i);
                self.events_read.set(self.events_read.get() | pages);
                (0..self.sent_n()).for_each(|i| self.aprs.read(i));
                true
            }
            Op::DeleteMessage(i) => match i.checked_sub(self.sent_n()) {
                Some(v) => self.sample_msg(v).map(|k| self.msgs_del.set(self.msgs_del.get() | 1 << k)).is_some(),
                None => self.aprs.delete(i),
            },
            Op::ClearMessages => {
                self.msgs_gone.set(true);
                self.aprs.clear_messages();
                true
            }
            Op::DeleteChannel(n) => self.slot_of(n).map(|c| c.set(None)).is_some(),
            Op::Skip(n, on) => edit(n, &|c| c.flags = if on { c.flags | FLAG_SKIP } else { c.flags & !FLAG_SKIP }),
            Op::Hide(n, on) => edit(n, &|c| c.flags = if on { c.flags | crate::chan::FLAG_HIDDEN } else { c.flags & !crate::chan::FLAG_HIDDEN }),
            Op::Move(n, z) => edit(n, &|c| c.zone = z),
            Op::Renumber(n, to) => self.slot_of(to).is_none() && edit(n, &|c| c.num = to),
            Op::NewZone => {
                let z = self.zones.get();
                (z < ZONES.len() as ZoneId).then(|| self.zones.set(z + 1)).is_some()
            }
            Op::DeleteZone(z) => {
                for c in &self.chans {
                    if let Some(mut ch) = c.get().filter(|ch| ch.zone == z) {
                        ch.zone = NO_ZONE;
                        c.set(Some(ch));
                    }
                }
                if z + 1 == self.zones.get() {
                    self.zones.set(z);
                }
                true
            }
            Op::Zone(z, cfg) => self.zone_cfg.get(z as usize).map(|c| c.set(cfg)).is_some(),
            Op::RenameZone(z, name) => (z < self.zones.get()).then(|| self.zone_names[z as usize].set(name)).is_some(),
            Op::Preset(i, p) => match self.presets.iter().filter(|p| p.get().is_some()).nth(i as usize) {
                Some(c) => {
                    c.set(Some(p));
                    true
                }
                None => false,
            },
            Op::Dtmf(i, d) => {
                if d.is_none() {
                    if let Some(c) = self.dtmf_names.get(i as usize) {
                        c.set(Buf::new());
                    }
                }
                self.dtmf.get(i as usize).map(|c| c.set(d)).is_some()
            }
            Op::DtmfName(i, n) => self.dtmf_names.get(i as usize).map(|c| c.set(n)).is_some(),
            Op::NewPreset => match self.presets.iter().find(|p| p.get().is_none()) {
                Some(c) => {
                    c.set(Some(RangePreset { name: txt("New range"), start: mhz(144), end: mhz(148), step: 10, mode: 0, narrow: true, skips: 0, resume: 0 }));
                    true
                }
                None => false,
            },
            Op::DeletePreset(i) => self.presets.iter().filter(|p| p.get().is_some()).nth(i as usize).map(|c| c.set(None)).is_some(),
            Op::PutZone(z, name, cfg) => {
                let n = self.zones.get();
                if z > n || z as usize >= ZONES.len() {
                    return false;
                }
                if z == n {
                    self.zones.set(n + 1);
                }
                self.zone_names[z as usize].set(name);
                self.zone_cfg[z as usize].set(cfg);
                true
            }
            Op::PutPreset(i, p) => {
                let used = self.presets.iter().filter(|p| p.get().is_some()).count();
                let cell = match self.presets.iter().filter(|p| p.get().is_some()).nth(i as usize) {
                    Some(c) => Some(c),
                    None if i as usize == used => self.presets.iter().find(|p| p.get().is_none()),
                    None => None,
                };
                match cell {
                    Some(c) => {
                        let skips = c.get().map_or(0, |o| o.skips);
                        c.set(Some(RangePreset { skips, ..p }));
                        true
                    }
                    None => false,
                }
            }
            Op::PresetSkips { preset, at, count, f } => match (0..16).filter(|&k| self.presets[k].get().is_some()).nth(preset as usize) {
                Some(k) => {
                    let mut l = self.preset_skips[k].get();
                    for (j, v) in f.iter().enumerate().map(|(j, v)| (at as usize + j, *v)).filter(|(j, _)| *j < (count as usize).min(48)) {
                        l[j] = v;
                    }
                    self.preset_skips[k].set(l);
                    self.presets[k].set(self.presets[k].get().map(|p| RangePreset { skips: count, ..p }));
                    true
                }
                None => false,
            },
            Op::ClearSkips => {
                self.skips_gone.set(u32::MAX);
                true
            }
            Op::Unskip(i) => match (0..SKIPS.len()).filter(|&k| self.skips_gone.get() & 1 << k == 0).nth(i as usize) {
                Some(k) => {
                    self.skips_gone.set(self.skips_gone.get() | 1 << k);
                    true
                }
                None => false,
            },
            Op::MarkHere => {
                // Shifts the stored ones' deleted bits along with them.
                let ok = self.fix.get().is_some() && self.wp_saved.get().is_none() && self.wp_typed.get().is_none() && self.wp_marked.get() < 9;
                if ok {
                    self.wp_marked.set(self.wp_marked.get() + 1);
                    self.wp_added();
                }
                ok
            }
            Op::AddWaypoint(pos, name) => {
                let ok = self.wp_typed.get().is_none() && self.marked() == 0;
                if ok {
                    self.wp_typed.set(Some(Waypoint { name, pos, src: WpSrc::Grid }));
                    self.wp_added();
                }
                ok
            }
            // LINK-55: one replaced in its place (four at most here), or one added (as a typed one)
            Op::PutWaypoint(i, w) => match (0..WAYPOINTS.len() as u16 + self.marked()).filter(|&k| self.wp_gone.get() & 1 << k == 0).nth(i as usize) {
                Some(k) => {
                    let mut o = self.wp_over.get();
                    let ok = match o.iter().position(|x| x.is_some_and(|x| x.0 == k)).or_else(|| o.iter().position(Option::is_none)) {
                        Some(j) => {
                            o[j] = Some((k, w));
                            true
                        }
                        None => false,
                    };
                    self.wp_over.set(o);
                    ok
                }
                None => {
                    let ok = self.wp_typed.get().is_none() && self.marked() == 0;
                    if ok {
                        self.wp_typed.set(Some(w));
                        self.wp_added();
                    }
                    ok
                }
            },
            Op::SaveStation(i) => {
                let ok = self.station(i).is_some_and(|s| s.pos.is_some()) && self.marked() == 0;
                if ok {
                    self.wp_saved.set(Some(i));
                    self.wp_added();
                }
                ok
            }
            Op::DeleteWaypoint(i) => {
                match (0..WAYPOINTS.len() as u16 + self.marked()).filter(|&k| self.wp_gone.get() & 1 << k == 0).nth(i as usize) {
                    Some(k) => {
                        self.wp_gone.set(self.wp_gone.get() | 1 << k);
                        true
                    }
                    None => false,
                }
            }
            Op::SaveContact(c) => {
                let mut all = self.contacts.get();
                let mut v: [Option<Contact>; 13] = [None; 13];
                let mut n = 0;
                for x in all.iter().flatten().filter(|x| x.call != c.call) {
                    v[n] = Some(*x);
                    n += 1;
                }
                if n == all.len() {
                    return false;
                }
                v[n] = Some(c);
                v[..=n].sort_unstable_by(|a, b| a.as_ref().map(|x| x.call.as_bytes()).cmp(&b.as_ref().map(|x| x.call.as_bytes())));
                all.copy_from_slice(&v[..12]);
                self.contacts.set(all);
                true
            }
            Op::DeleteContact(call) => {
                let mut all = self.contacts.get();
                let Some(k) = all.iter().position(|x| x.is_some_and(|x| x.call == call)) else { return false };
                all[k..].rotate_left(1);
                all[11] = None;
                self.contacts.set(all);
                true
            }
            // satellites live in the flash store only (SAT-2)
            Op::PutSat(_) | Op::DeleteSat(_) | Op::ClearSats | Op::SatInfo(_) | Op::SatPrefs(..) => false,
            Op::Draft(d) => {
                self.draft.set(d);
                true
            }
            Op::Quick(i, t) => {
                let mut q = self.quick.get();
                match q.get_mut(i as usize) {
                    Some(s) => {
                        *s = t;
                        self.quick.set(q);
                        true
                    }
                    None => false,
                }
            }
            Op::Send => {
                let d = self.draft.get();
                let ok = self.call.get().is_some() && !d.to.is_empty() && !d.text.is_empty(); // APRS-8
                if ok {
                    self.aprs.send(d.to.as_bytes(), d.text);
                    self.draft.set(Draft::default());
                }
                ok
            }
            Op::StopMessage(i) => i < self.sent_n() && self.aprs.stop(i),
            Op::ResendMessage(i) => i < self.sent_n() && self.aprs.resend(i),
            Op::Queue(..) | Op::Unqueue(..) => false, // the store's, to the radio's APRS task
            Op::MessageRead(i) => match i.checked_sub(self.sent_n()).and_then(|v| self.sample_msg(v)) {
                Some(k) => {
                    self.msgs_read.set(self.msgs_read.get() | 1 << k);
                    true
                }
                None => {
                    self.aprs.read(i);
                    true
                }
            },
            Op::BeaconNow | Op::AfterVoice => {
                let ok = self.call.get().is_some(); // APRS-8
                if ok {
                    self.aprs.beacon_now();
                }
                ok
            }
            Op::ClearHeard => {
                self.heard_cleared.set(true);
                self.aprs.clear_heard();
                true
            }
            Op::AcceptCal => true, // the sample has no calibration lock
            Op::Wipe => {
                self.chans.iter().for_each(|c| c.set(None));
                self.zones.set(0);
                self.presets.iter().for_each(|p| p.set(None));
                self.dtmf.iter().for_each(|d| d.set(None));
                self.dtmf_names.iter().for_each(|d| d.set(Buf::new()));
                self.events_cleared.set(true);
                self.heard_cleared.set(true);
                self.msgs_gone.set(true);
                self.wp_gone.set(u32::MAX);
                self.wp_over.set([None; 4]);
                self.quick.set([None; crate::words::QUICK]);
                self.draft.set(Draft::default());
                self.aprs.clear();
                true
            }
            Op::AprsCfg(c) => {
                self.aprs.cfg.set(c);
                true
            }
        };
        ok && self.bump()
    }
}

