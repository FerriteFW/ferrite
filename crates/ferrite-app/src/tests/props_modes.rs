//! Property tests for the exclusive modes (MODE-1, MODE-2, PWR-24.4, SQL-14.1, TX-35) and SCAN-6: random streams
//! of screens, BACK, PTT, slot changes, monitors, layouts, Low Power Mode, the APRS holder (0 5 3), broadcast slots
//! and data key-ups, the App driven through the tests' harness and the Radio task modelled from the commands it is
//! sent. A failure prints the seed and the shrunk event sequence. `FERRITE_FUZZ_SCALE=N` runs N times as many.

use super::*;
use crate::aprs::APRS_SLOT;
use crate::screens::activity::Mode;
use crate::screens::nav::Dest;
use ferrite_core::msg::BroadcastCmd;
use ferrite_core::txplan::Region;
use std::format;
use std::string::String;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

/// What the sequences reached: each mode run (by `Mode as usize`), data key-ups sent, Low Power Mode, monitors.
static REACHED: [AtomicU32; 16] = [const { AtomicU32::new(0) }; 16];

/// SplitMix64: the same sequences every run.
struct Rng(u64);
impl Rng {
    fn below(&mut self, n: u32) -> u32 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) % n.max(1) as u64) as u32
    }
}

fn scale() -> u32 {
    std::env::var("FERRITE_FUZZ_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(1).max(1)
}

/// Drop chunks of a failing sequence while it still fails.
fn shrink<E: Clone>(mut evs: Vec<E>, fails: impl Fn(&[E]) -> bool) -> Vec<E> {
    let mut chunk = evs.len() / 2;
    while chunk > 0 {
        let mut i = 0;
        while i < evs.len() {
            let mut t = evs.clone();
            t.drain(i..(i + chunk).min(evs.len()));
            if fails(&t) {
                evs = t;
            } else {
                i += chunk;
            }
        }
        chunk /= 2;
    }
    evs
}

fn check<E: Clone + core::fmt::Debug>(name: &str, base: u64, runs: u32, gen: impl Fn(&mut Rng) -> Vec<E>, run: impl Fn(&[E]) -> Result<(), String>) {
    for i in 0..runs * scale() {
        let seed = base.wrapping_mul(1_000_003).wrapping_add(i as u64);
        let evs = gen(&mut Rng(seed));
        if let Err(e) = run(&evs) {
            let small = shrink(evs, |t| run(t).is_err());
            let why = run(&small).err().unwrap_or(e);
            let list: Vec<String> = small.iter().map(|e| format!("  {e:?}")).collect();
            panic!("{name}: seed {seed}: {why}\n{} events after shrinking:\n{}", small.len(), list.join("\n"));
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Ev {
    Open(u8),
    Back,
    HoldBack,
    Ptt(bool),
    SlotKey,
    /// SIDE1 tapped (a latched monitor, SQL-14) or held (while held).
    Side1(bool),
    /// \# on the crossband screen starts the repeater (XB-1).
    Hash,
    Layout(u8),
    Lpm(bool),
    /// 0 5 3: Off, Tracker, On, KISS.
    Aprs(u8),
    /// The active slot into the broadcast band (BCR-22) or back to 2 m.
    Bcast(bool),
    /// The APRS task's frame: a data key-up on C (`data_key`), and its end.
    DataKey(u8),
    DataUnkey,
    Wait(u16),
}

const DESTS: [Dest; 7] = [Dest::BandScope, Dest::Counter, Dest::ToneSearch, Dest::HfMode, Dest::Satellites, Dest::Crossband, Dest::Group(7)];
const WHAT: [&[u8]; 3] = [b"Beacon", b"KISS", b"Digipeat"];

fn gen(r: &mut Rng) -> Vec<Ev> {
    (0..20 + r.below(60))
        .map(|_| match r.below(17) {
            0..=2 => Ev::Open(r.below(DESTS.len() as u32) as u8),
            3 => Ev::Back,
            4 => Ev::HoldBack,
            5 => Ev::Ptt(r.below(2) == 0),
            6 => Ev::SlotKey,
            7 => Ev::Side1(r.below(3) == 0),
            8 => Ev::Hash,
            9 => Ev::Layout(r.below(3) as u8),
            10 => Ev::Lpm(r.below(3) == 0),
            11 => Ev::Aprs(r.below(4) as u8),
            12 => Ev::Bcast(r.below(2) == 0),
            13 => Ev::DataKey(r.below(WHAT.len() as u32) as u8),
            14 => Ev::DataUnkey,
            _ => Ev::Wait(50 + r.below(2_000) as u16),
        })
        .collect()
}

/// The Radio task as the commands leave it.
#[derive(Default, Debug)]
struct Radio {
    sweep: bool,
    tone: bool,
    counter: bool,
    hf: bool,
    sat: bool,
    xb: bool,
    fm: bool,
    /// Latched or held monitors (SQL-14).
    monitor: u8,
    keyed: bool,
    data: bool,
}

impl Radio {
    fn take(&mut self, c: &RadioCmd) {
        match *c {
            RadioCmd::Sweep { on, .. } => self.sweep = on,
            RadioCmd::ToneSearch { on, .. } => self.tone = on,
            RadioCmd::Counter(on) => self.counter = on,
            RadioCmd::Hf { on, .. } => self.hf = on,
            RadioCmd::SatRx(f) => self.sat = f != 0,
            RadioCmd::Crossband(x) => self.xb = x.is_some(),
            RadioCmd::Broadcast(BroadcastCmd::Off) => self.fm = false,
            RadioCmd::Broadcast(BroadcastCmd::Fm(_)) => self.fm = true,
            RadioCmd::Monitor { slot, on } => self.monitor = self.monitor & !(1 << slot) | (on as u8) << slot,
            RadioCmd::Transmit { on, .. } => self.keyed = on,
            RadioCmd::TxCut { .. } => self.keyed = false,
            RadioCmd::TransmitData { on, .. } => self.data = on,
            _ => {}
        }
    }

    /// The modes running, as a mode (None: more than one, or not one the App knows).
    fn mode(&self) -> Option<Mode> {
        let on = [self.sweep, self.tone, self.counter, self.hf, self.sat, self.xb];
        match on.iter().filter(|&&x| x).count() {
            0 => Some(Mode::Off),
            1 => [Mode::Scope, Mode::Tone, Mode::Counter, Mode::Hf, Mode::Sat, Mode::Xb].into_iter().zip(on).find(|x| x.1).map(|x| x.0),
            _ => None,
        }
    }
}

fn app() -> T {
    let mut t = T::new();
    t.app.radio.tx_built = true;
    t.app.set_region(Some(Region::Us), false);
    t.app.set.lpm = 0; // Low Power Mode by hand only (the default is Auto)
    t.app.set_callsign(b"VA7FER");
    if t.app.slot(0).on_channel() {
        t.tap(Key::VfoMem);
    }
    t.app.set_vfo(0, 14_652_000);
    t.wait(200);
    t
}

/// `auto_tx_paused`: also check ferrite#131 (no automatic transmission during HF, the scope, the counter, tone search
/// or satellite mode).
fn run_with(evs: &[Ev], auto_tx_paused: bool) -> Result<(), String> {
    let mut t = app();
    let mut r = Radio::default();
    for c in t.cmds() {
        r.take(&c);
    }
    for (i, &e) in evs.iter().enumerate() {
        let mode_before = t.app.radio.mode;
        let s = store();
        let mut data_sent = false;
        match e {
            Ev::Open(d) => t.open(DESTS[d as usize]),
            Ev::Back => t.tap(Key::Exit),
            Ev::HoldBack => t.hold(Key::Exit),
            Ev::Ptt(down) => {
                if down != t.app.keys.is_held(Key::PTT) {
                    if down {
                        t.app.key_down(Key::PTT, t.t, &s);
                    } else {
                        t.app.key_up(Key::PTT, t.t, &s);
                    }
                    t.t += 50;
                    t.app.tick(t.t, &t.live, &s);
                }
            }
            Ev::SlotKey => t.tap(Key::Slot),
            Ev::Side1(hold) => {
                if hold {
                    t.hold(Key::Side(1))
                } else {
                    t.tap(Key::Side(1))
                }
            }
            Ev::Hash => t.tap(Key::Hash),
            Ev::Layout(l) => t.app.set_layout(l),
            Ev::Lpm(on) => {
                t.app.set_lpm_mode(on as u8);
                t.wait(50);
            }
            Ev::Aprs(v) => {
                t.app.set_aprs_select(v);
                t.wait(50);
            }
            Ev::Bcast(on) => {
                let f = if on { 10_050_000 } else { 14_652_000 };
                let a = t.app.active;
                crate::screens::entry::tune_vfo(&mut t.app, a, f);
                t.wait(50);
            }
            Ev::DataKey(w) => {
                if !t.app.radio.data_keyed {
                    data_sent = crate::screens::transmit::data_key(&mut t.app, APRS_SLOT, 0, WHAT[w as usize]);
                    t.wait(20);
                }
            }
            Ev::DataUnkey => {
                if t.app.radio.data_keyed {
                    crate::screens::transmit::data_unkey(&mut t.app, APRS_SLOT);
                    t.wait(20);
                }
            }
            Ev::Wait(ms) => t.wait(ms as u32),
        }
        for c in t.cmds() {
            r.take(&c);
        }
        let a = &t.app;
        let at = |m: &str| format!("event {i} ({e:?}): {m}; screen {:?}, mode {:?}, wanted {:?}, radio {r:?}, lpm {}", a.screen, a.radio.mode, a.wanted_mode(), a.low_power());
        REACHED[a.radio.mode as usize].fetch_add(1, Relaxed);
        REACHED[10].fetch_add(data_sent as u32, Relaxed);
        REACHED[11].fetch_add(a.low_power() as u32, Relaxed);
        REACHED[12].fetch_add((r.monitor != 0) as u32, Relaxed);
        REACHED[13].fetch_add((a.low_power() && r.tone) as u32, Relaxed); // PWR-24.4 doesn't name tone search
        // MODE-1, MODE-2: the mode the App runs is the one it wants, and the commands sent agree.
        if a.radio.mode != a.wanted_mode() {
            return Err(at("the mode running isn't the one wanted"));
        }
        let want = match a.radio.mode {
            Mode::Strip => Mode::Scope,
            Mode::Fm => Mode::Off,
            m => m,
        };
        if r.mode() != Some(want) {
            return Err(at("the Radio task's modes don't match the App's"));
        }
        if r.fm != a.radio.fm_on {
            return Err(at("the broadcast's commands don't match the App's"));
        }
        // PWR-24.4: Low Power Mode runs nothing that sweeps or listens non-stop, and no broadcast.
        if a.low_power() && (r.sweep || r.counter || r.hf || r.sat || r.xb || r.fm) {
            return Err(at("a sweeping or listening mode in Low Power Mode"));
        }
        // SQL-14.1 (ferrite#129): a latched monitor only ever on the active slot.
        if a.monitor & !(1 << a.active) != 0 || r.monitor & !(1 << a.active) != 0 {
            return Err(at(&format!("a monitor on a slot that isn't active ({:#b}, sent {:#b}, active {})", a.monitor, r.monitor, a.active)));
        }
        // HF-6, SCOPE-13, SAT-14 (ferrite#131): no automatic transmission while these modes run.
        if auto_tx_paused && data_sent && matches!(mode_before, Mode::Hf | Mode::Scope | Mode::Counter | Mode::Tone | Mode::Sat) {
            return Err(at(&format!("an automatic transmission keyed during {mode_before:?}")));
        }
    }
    Ok(())
}

fn run(evs: &[Ev]) -> Result<(), String> {
    run_with(evs, true)
}

#[test]
fn modes_random_streams_keep_the_rules() {
    check("modes", 61, 3_000, gen, run);
    let n: Vec<u32> = REACHED.iter().map(|x| x.load(Relaxed)).collect();
    std::println!("modes: steps in each mode {:?}, {} data key-ups, {} in Low Power Mode, {} with a monitor, {} with tone search in Low Power Mode", &n[..9], n[10], n[11], n[12], n[13]);
    assert!(n[..9].iter().all(|&x| x > 3) && n[10] > 10 && n[11] > 50 && n[12] > 20, "the sequences reach every mode: {n:?}");
}

/// ferrite#131: beacons, KISS frames and digipeats key the radio during HF mode, the band scope, the counter, tone
/// search and the satellite screen (HF-6, SCOPE-13, SAT-14). E.g. APRS mode on, the band scope open, a beacon due.
/// TX-51: `data_key` refuses while they run.
#[test]
fn modes_no_automatic_transmission_in_exclusive_modes() {
    let evs = [Ev::Aprs(2), Ev::Open(0), Ev::DataKey(0)];
    if let Err(e) = run_with(&evs, true) {
        panic!("{e}\n{evs:?}");
    }
    check("modes, ferrite#131", 62, 300, gen, |e| run_with(e, true));
}

/// TX-51, TX-51.4 (HF-6, SCOPE-13, SAT-14): automatic transmissions are paused while each mode's screen is up,
/// also through a person's key-up under the scope or HF mode, and not on home or the crossband screen.
#[test]
fn auto_tx_paused_follows_the_mode_screens() {
    for (d, paused) in DESTS.iter().zip([true, true, true, true, true, false, false]) {
        let mut t = app();
        assert!(!t.app.auto_tx_paused());
        t.open(*d);
        t.wait(100);
        assert_eq!(t.app.auto_tx_paused(), paused, "{d:?}");
        let s = store();
        t.app.key_down(Key::PTT, t.t, &s);
        t.t += 50;
        t.app.tick(t.t, &t.live, &s);
        // The scope and HF mode stay up through the key-up (the others' screens give way to the transmit screen).
        assert_eq!(t.app.auto_tx_paused(), paused && matches!(d, Dest::BandScope | Dest::HfMode), "{d:?}, keyed");
        t.app.key_up(Key::PTT, t.t, &s);
        t.hold(Key::Exit);
        t.wait(100);
        assert!(!t.app.auto_tx_paused(), "{d:?}, home again");
    }
}

/// SCAN-6: SIDE2 stops a scan on home with the slot on the scan's last hit (Return to "Last hit", 0 1 6 4) or where
/// it started ("Start"), whatever hits came and went; the Radio task modelled as reporting each hit while held.
#[test]
fn scan_stop_ends_where_scan6_says() {
    let runs = 2_000 * scale();
    let mut stops = 0;
    for i in 0..runs {
        let seed = 71u64.wrapping_mul(1_000_003).wrapping_add(i as u64);
        let mut g = Rng(seed);
        let mut t = app();
        t.app.set.scan_return_hit = g.below(2) == 0;
        let mut log: Vec<String> = Vec::new();
        let (mut start, mut last) = (t.freq(), None::<Freq10>);
        let mut scanning = false;
        for _ in 0..40 {
            let e = g.below(5);
            log.push(format!("{e}"));
            match e {
                0 => {
                    // SIDE2 on home: start, or stop
                    let before = t.freq();
                    t.tap(Key::Side(2));
                    if scanning {
                        stops += 1;
                        let want = if t.app.set.scan_return_hit { last.unwrap_or(start) } else { start };
                        t.live.slots[0] = SlotLive::default();
                        t.wait(50);
                        assert_eq!(t.freq(), want, "seed {seed}: stopped at {} (return to hit {}, started {start}, last hit {last:?}): {log:?}", t.freq(), t.app.set.scan_return_hit);
                        scanning = false;
                    } else if t.cmds().contains(&RadioCmd::Scan { slot: 0, on: true }) {
                        (scanning, start, last) = (true, before, None);
                        t.live.slots[0] = SlotLive { scanning: true, freq: before, ..Default::default() };
                        t.wait(50);
                    }
                }
                1 if scanning => {
                    // a hit, held while the carrier lasts
                    let f = start + (1 + g.below(40)) * 1_250;
                    t.live.slots[0] = SlotLive { scanning: true, held: true, freq: f, squelch_open: true, carrier: true, ..Default::default() };
                    t.wait(100 + g.below(500));
                    last = Some(f);
                }
                2 if scanning => {
                    // on again past the hit
                    t.live.slots[0] = SlotLive { scanning: true, freq: start + g.below(40) * 1_250, ..Default::default() };
                    t.wait(100 + g.below(500));
                }
                _ => t.wait(50 + g.below(400)),
            }
            if t.app.screen != Screen::Home && !scanning {
                t.tap(Key::Exit);
            }
        }
    }
    std::println!("scan: {stops} scans stopped");
    assert!(stops > runs, "the sequences stop scans: {stops}");
}

