//! Property tests for the radio-side engines: random event streams, fixed seeds, invariants checked on every
//! step. A failure prints the seed and the (shrunk) event sequence, so it can be replayed by hand.
//!
//! - The watch engine and the receive-path hop (WATCH-2, WATCH-3, WATCH-11, WATCH-16, WATCH-17, APRS-21): one
//!   slot never on two chips, a slot only on its band's chip, off slots never listened to or heard, the audio
//!   always an open, unmuted slot's (the active one's first), only the active slot while keyed or cancelled, and
//!   the active slot never deaf past its turn while nothing else holds a chip.
//! - The transmit sequencer (TX-25, TX-30, TX-31, TX-32, TX-48): every unkey ends with the PA at 0, no chip
//!   transmitting and the receive path back; a refusal never keys; the timeout, battery and duty backstops fire
//!   on time; Dry TX never drives the PA.
//! - The scan engine (SCAN-9, SCAN-10, SCAN-11, SCAN-19, SCAN-20.3): a skipped position (a channel's Skip flag, a
//!   preset's skipped frequency, a nuisance skip this scan) is never tuned or stopped on; PTT ends on the hit or
//!   the start; a list with nothing to stop on ends the scan.
//!
//! `FERRITE_FUZZ_SCALE=N` (default 1) runs N times as many sequences.

use ferrite_core::freq::{band, mhz, Freq10};
use std::fmt::Debug;
use std::string::String;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::vec::Vec;

/// What the sequences reached (each test asserts its own share, so a generator that stops reaching a case fails).
static REACHED: [AtomicU32; 12] = [const { AtomicU32::new(0) }; 12];

fn reached(i: usize) {
    REACHED[i].fetch_add(1, Relaxed);
}

fn max_at(i: usize, v: u32) {
    REACHED[i].fetch_max(v, Relaxed);
}

/// SplitMix64: the same sequences every run, std only.
struct Rng(u64);
impl Rng {
    fn u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u32) -> u32 {
        (self.u64() % n.max(1) as u64) as u32
    }
    fn one_in(&mut self, n: u32) -> bool {
        self.below(n) == 0
    }
}

fn scale() -> u32 {
    std::env::var("FERRITE_FUZZ_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(1).max(1)
}

/// Shrink a failing sequence: drop chunks (halves, then smaller) while it still fails.
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

/// Run `runs` seeded sequences of `gen`; on the first failure shrink it and panic with the seed and events.
fn check<E: Clone + Debug>(name: &str, base: u64, runs: u32, gen: impl Fn(&mut Rng) -> Vec<E>, run: impl Fn(&[E]) -> Result<(), String>) {
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

// ---------------------------------------------------------------------------------------------------------------
// The watch engine.

mod watch {
    use super::*;
    use ferrite_core::watch::{Cfg, Hop, Watch, APRS_FIRST_MS, HOLD_MS, LOOK_MS, REACQUIRE_MS, AFTER_TX_MS, PRIO_DWELL_MS, WATCHED_DWELL_MS, ACTIVE_DWELL_MS};

    /// The frequencies the slots are put on: chip 0 (2 m, the APRS frequency, 1.25 m) and chip 1 (70 cm, and HF,
    /// which needs the relay: WATCH-11).
    const F: [Freq10; 7] = [mhz(146) + 52_000, mhz(147), mhz(144) + 39_000, mhz(446), mhz(442), mhz(28) + 40_000, mhz(223) + 50_000];
    const APRS: usize = 2;

    #[derive(Clone, Copy, Debug)]
    pub enum Ev {
        /// Ticks of 10 ms.
        Wait(u16),
        /// A carrier on F[i] comes or goes.
        Carrier(u8, bool),
        Active(u8),
        Set(u8),
        Muted(u8),
        Off(u8),
        /// A latched monitor on the active slot (SQL-14; SQL-14.1: it goes off when the active slot changes).
        Monitor(bool),
        /// The APRS holder of C: 0 none, 1 beacons (no priority, APRS-29), 2 APRS mode or KISS (APRS-21.1).
        Aprs(u8),
        First(bool),
        Freq(u8, u8),
        /// The active slot runs a range scan (SCAN-21).
        Scan(bool),
        Pause(u8),
        AfterTx,
        Keyed(bool),
    }

    pub fn gen(r: &mut Rng) -> Vec<Ev> {
        let n = 40 + r.below(80);
        let mut v = Vec::new();
        for _ in 0..n {
            v.push(match r.below(20) {
                0..=5 => Ev::Wait(1 + r.below(300) as u16),
                6..=8 => Ev::Carrier(r.below(F.len() as u32) as u8, r.one_in(2)),
                9 => Ev::Active(r.below(3) as u8),
                10 => Ev::Set(r.below(8) as u8),
                11 => Ev::Muted(if r.one_in(2) { 0 } else { r.below(8) as u8 }),
                12 => Ev::Off(if r.one_in(2) { 0 } else { r.below(8) as u8 }),
                13 => Ev::Monitor(r.one_in(2)),
                14 => Ev::Aprs(r.below(3) as u8),
                15 => Ev::First(r.one_in(2)),
                16 => Ev::Freq(r.below(3) as u8, r.below(F.len() as u32) as u8),
                17 => Ev::Scan(r.one_in(3)),
                18 => {
                    if r.one_in(2) {
                        Ev::Pause(r.below(3) as u8)
                    } else {
                        Ev::AfterTx
                    }
                }
                _ => Ev::Keyed(r.one_in(2)),
            });
        }
        v
    }

    /// How long the active slot may go unheard while nothing else holds a chip (no other slot open, closing within
    /// its hold, rising, or a priority slot's after-frame window). WATCH-17's turns: with APRS first a priority
    /// slot's 400 ms either side of another slot's 100 ms look (A, C, B, C, A); otherwise the turns are shorter.
    /// Two retunes' reacquire time on top.
    const ACTIVE_DEAF_MS: u32 = 2 * APRS_FIRST_MS + LOOK_MS + 2 * REACQUIRE_MS;
    /// A·B first (WATCH-17.2's turns): another watched slot's 70 ms and the priority slot's 100 ms, twice the
    /// watched turn when both others share the chip, and one reacquire.
    const ACTIVE_DEAF_VOICE_MS: u32 = PRIO_DWELL_MS + 2 * WATCHED_DWELL_MS + REACQUIRE_MS;

    /// The priority slot (APRS-21.1, WATCH-17.2, WATCH-17.3) never deaf longer than this while nothing else holds
    /// a chip: another slot's turn cut short by the receive path's hop to the other chip (a turn there counts only
    /// while its chip has the path, so it starts whole again after), the other chip's turn, that whole turn, and a
    /// retune's reacquire. APRS first, 100 ms looks; A·B first, the active slot's 150 ms and a watched slot's 70.
    /// The manual's "about 140 ms" and "about 260 ms" are the steady turns with A and B both on 2 m; these runs
    /// see up to some 300 ms after a change (a slot joining at a pause's end, a key-up).
    const C_DEAF_MS: u32 = 3 * LOOK_MS + REACQUIRE_MS;
    const C_DEAF_VOICE_MS: u32 = 2 * ACTIVE_DWELL_MS + WATCHED_DWELL_MS + REACQUIRE_MS;

    pub fn run(evs: &[Ev]) -> Result<(), String> {
        run_with(evs, false)
    }

    /// `scan_too`: the priority slot's bound holds while the active slot runs a range scan as well (SCAN-21: "an
    /// APRS priority slot keeps its turns").
    pub fn run_with(evs: &[Ev], scan_too: bool) -> Result<(), String> {
        let mut c = Cfg { set: 0b111, active: 0, freq: [F[0], F[3], F[2]], first: true, ..Cfg::default() };
        let (mut w, mut hop) = (Watch::new(), Hop::new());
        let mut path: Option<u8> = None;
        let mut got: [(Option<u8>, u32); 2] = [(None, 0); 2];
        let (mut air, mut keyed, mut monitor, mut aprs, mut scan) = (0u8, false, false, 0u8, false);
        let mut t = 10_000u32;
        // When the active slot was last listened to, and the last time anything excused it from being.
        let (mut heard_at, mut excused_at) = (t, t);
        // When each slot last read open: its hold runs 2 s from there, whichever slot was active then.
        let mut open_at = [0u32; 3];
        let (mut c_heard, mut c_excused, mut changed_at) = (t, t, t);
        let mut after_at: Option<u32> = None;
        let mut pause_until: Option<u32> = None;
        for (i, &e) in evs.iter().enumerate() {
            let ticks = match e {
                Ev::Wait(n) => n as u32,
                _ => 1,
            };
            match e {
                Ev::Wait(_) => {}
                Ev::Carrier(f, on) => air = air & !(1 << f) | (on as u8) << f,
                Ev::Active(s) => {
                    if s != c.active {
                        monitor = false; // SQL-14.1
                    }
                    c.active = s;
                }
                Ev::Set(m) => c.set = m,
                Ev::Muted(m) => c.muted = m,
                Ev::Off(m) => c.off = m,
                Ev::Monitor(on) => monitor = on,
                Ev::Aprs(a) => {
                    aprs = a;
                    if a != 0 {
                        c.freq[2] = F[APRS]; // APRS-15: C on the APRS frequency while held
                    }
                }
                Ev::First(f) => c.first = f,
                Ev::Freq(s, f) => {
                    if !(s == 2 && aprs != 0) {
                        c.freq[s as usize] = F[f as usize];
                    }
                }
                Ev::Scan(on) => scan = on,
                Ev::Pause(s) => {
                    w.pause(t, s as u16);
                    pause_until = (s > 0).then(|| t + s as u32 * 1000);
                }
                Ev::AfterTx => {
                    if aprs == 2 {
                        w.after_tx(t);
                        after_at = Some(t);
                    }
                }
                Ev::Keyed(k) => keyed = k,
            }
            c.held = if monitor { 1 << c.active } else { 0 };
            (c.aprs, c.prio) = match aprs {
                0 => (0, 0),
                1 => (0b100, 0),
                _ => (0b100, 0b100),
            };
            c.scan = if scan { 1 << c.active } else { 0 };
            if !matches!(e, Ev::Wait(_) | Ev::Carrier(..)) {
                (excused_at, changed_at) = (t, t); // a new configuration: a whole cycle's grace
            }
            for _ in 0..ticks {
                t += 10;
                let listened = [path == Some(0), path == Some(1)];
                let (mut sq, mut rising) = ([false; 2], [false; 2]);
                for n in 0..2 {
                    if let (true, (Some(s), since)) = (listened[n], got[n]) {
                        let f = c.freq[s as usize];
                        let on = F.iter().enumerate().any(|(k, &g)| g == f && air & 1 << k != 0);
                        let settle = if c.prio & 1 << s != 0 { 40 } else { 80 };
                        sq[n] = on && t >= since + settle;
                        rising[n] = on && t + 30 >= since + settle && t < since + settle; // WATCH-17.5
                    }
                }
                w.rising(rising);
                let out = w.turn(t, &c, sq, listened, keyed);
                let was = path;
                path = hop.next(t, &out, c.active);
                for n in 0..2u8 {
                    if out.chip[n as usize] != got[n as usize].0 || path == Some(n) && was != Some(n) {
                        got[n as usize] = (out.chip[n as usize], t);
                    }
                }
                let at = |m: &str| format!("event {i} ({e:?}), t {t}: {m}; cfg {c:?}, out {out:?}, path {path:?}");
                let a = c.active;
                // One slot never on two chips; a slot only on its band's chip.
                if out.chip[0].is_some() && out.chip[0] == out.chip[1] {
                    return Err(at("one slot on both chips"));
                }
                for n in 0..2u8 {
                    if let Some(s) = out.chip[n as usize] {
                        if band(c.freq[s as usize]).1 != n {
                            return Err(at("a slot on the other band's chip"));
                        }
                        // WATCH-2.1, BCR-23: an off slot is never listened to.
                        if c.off & 1 << s != 0 {
                            return Err(at("an off slot on a chip"));
                        }
                        // WATCH-16: only the active slot while keyed; WATCH-11: and while cancelled.
                        if (keyed || out.cancelled) && s != a {
                            return Err(at("a watched slot on a chip while keyed or cancelled"));
                        }
                    }
                }
                // Audio: an open (or held), unmuted, listened slot's, or the APRS slot's for a slot deferring to it.
                let open = |s: u8| w.open(s) || c.held & 1 << s != 0;
                let heard = |s: u8| open(s) && c.muted & 1 << s == 0 && c.off & 1 << s == 0;
                let via = |s: u8| if out.defer & 1 << s != 0 { out.defer_to.unwrap_or(s) } else { s };
                if let Some(x) = out.audio {
                    if !(0..3u8).any(|s| heard(s) && via(s) == x) {
                        return Err(at("audio from a slot that isn't open and unmuted"));
                    }
                    if keyed && x != a && via(a) != x {
                        return Err(at("a watched slot heard while keyed"));
                    }
                }
                // WATCH-3.1: the active slot's audio first.
                if heard(a) && out.audio != Some(via(a)) {
                    return Err(at("the active slot open and unmuted, but not heard"));
                }
                // The active slot never deaf past its turns while nothing else holds a chip.
                let on_path = |s: u8| path.is_some_and(|n| out.chip[n as usize] == Some(s));
                if on_path(a) || out.defer & 1 << a != 0 && out.defer_to.is_some_and(on_path) {
                    heard_at = t;
                }
                for s in 0..3u8 {
                    if w.open(s) || c.held & 1 << s != 0 {
                        open_at[s as usize] = t;
                    }
                }
                let excused = keyed
                    || out.paused
                    || out.cancelled
                    || pause_until.is_some_and(|p| t < p + 20)
                    || after_at.is_some_and(|x| t < x + AFTER_TX_MS + 20)
                    || c.muted & 1 << a != 0
                    || c.off & 1 << a != 0
                    || (0..3u8).any(|s| s != a && open_at[s as usize] != 0 && t < open_at[s as usize] + HOLD_MS + 20)
                    || rising.iter().any(|&r| r)
                    || (0..2).any(|n| out.chip[n].is_some_and(|s| s != a && sq[n]));
                if excused {
                    excused_at = t;
                }
                // The priority slot (APRS-21.1): how long it goes unheard while nothing else holds a chip.
                if on_path(2) {
                    c_heard = t;
                }
                let c_excused_now = keyed
                    || out.cancelled
                    || c.prio == 0
                    || c.set & 4 == 0
                    || c.off & 4 != 0
                    || c.held != 0
                    || c.scan != 0 && !scan_too
                    || rising.iter().any(|&r| r)
                    || (0..3u8).any(|s| open_at[s as usize] != 0 && t < open_at[s as usize] + HOLD_MS + 20);
                // A new configuration, or the pause ending (the watched slots back): from there.
                if c_excused_now || changed_at == t || pause_until.is_some_and(|p| t < p + 20) {
                    c_excused = t;
                }
                max_at(if c.first { 3 } else { 4 }, t - c_heard.max(c_excused));
                let (c_deaf, c_limit) = (t - c_heard.max(c_excused), if c.first { C_DEAF_MS } else { C_DEAF_VOICE_MS });
                if c_deaf > c_limit {
                    return Err(at(&format!("the priority slot C deaf {c_deaf} ms")));
                }
                let since = t - heard_at.max(excused_at);
                if !excused {
                    reached(0);
                }
                max_at(if c.first && c.prio != 0 { 1 } else { 2 }, since);
                let limit = if c.first && c.prio != 0 { ACTIVE_DEAF_MS } else { ACTIVE_DEAF_VOICE_MS };
                if since > limit {
                    return Err(at(&format!("the active slot deaf {since} ms")));
                }
            }
        }
        Ok(())
    }
}

#[test]
fn watch_random_streams_keep_the_rules() {
    check("watch", 17, 5_000, watch::gen, watch::run);
    let (checked, deaf) = (REACHED[0].load(Relaxed), REACHED[1].load(Relaxed));
    std::println!("watch: {checked} ticks checked for the active slot's turns, deaf at most {deaf} ms (APRS first), {} ms otherwise; C deaf at most {} ms (APRS first), {} ms otherwise", REACHED[2].load(Relaxed), REACHED[3].load(Relaxed), REACHED[4].load(Relaxed));
    assert!(checked > 100_000, "the sequences reach the turn check: {checked}");
}

/// SCAN-21 and WATCH-17.2 (decision 450): an APRS priority slot keeps its turns beside the active slot's range
/// scan, never deaf longer than its limit, whichever slot its chip is on now. Found here when the scan's 1 s turn
/// was skipped only while the other chip was on C itself: C sharing its chip with a watched slot, the chip on that
/// slot, was deaf about 1.1 s. E.g. A on 146.52 and C (APRS mode) on chip 0, B active on 446.00 scanning on chip 1,
/// nothing on the air.
#[test]
fn watch_priority_slot_keeps_its_turns_beside_a_scan() {
    use watch::Ev::*;
    let evs = [Aprs(2), Active(1), Scan(true), Wait(500)];
    if let Err(e) = watch::run_with(&evs, true) {
        panic!("{e}\n{evs:?}");
    }
    check("watch, C beside a scan", 18, 300, watch::gen, |e| watch::run_with(e, true));
}


// ---------------------------------------------------------------------------------------------------------------
// The transmit sequencer and the path machine.

mod tx {
    use super::*;
    use ferrite_core::path::{Act, Cause, Path, Rx, Sel, Steps};
    use ferrite_core::txplan::{Refusal, Region};
    use ferrite_core::txseq::{pa_drive, Cfg, Event, Kind, Req, TxSeq, BATT_LOW_MS, DUTY_DRAIN_DIV, DUTY_FULL_MS, DUTY_WEIGHT};

    /// Transmit frequencies (US): 2 m, 70 cm, 1.25 m, 10 m (the relay), and one outside every band.
    const F: [Freq10; 5] = [mhz(146) + 52_000, mhz(446), mhz(223) + 50_000, mhz(28) + 40_000, mhz(300)];

    #[derive(Clone, Copy, Debug)]
    pub enum Ev {
        /// A key-up on F[tx] receiving on F[rx]; `bad` breaks one of the checks (0 none).
        Key { tx: u8, rx: u8, data: bool, power: u8, bad: u8 },
        Release,
        /// LINK-20: a remote key-up lost.
        Cut,
        /// `n` Radio task ticks `ms` apart while the battery reads `batt_ok`.
        Ticks { n: u16, ms: u8, batt_ok: bool },
        /// Time with no ticks (unkeyed: the duty bucket drains).
        Idle(u16),
        Safe,
        Dry(bool),
    }

    pub fn gen(r: &mut Rng) -> Vec<Ev> {
        let n = 30 + r.below(60);
        (0..n)
            .map(|_| match r.below(12) {
                0..=2 => Ev::Key {
                    tx: if r.one_in(4) { r.below(F.len() as u32) as u8 } else { r.below(2) as u8 },
                    rx: if r.one_in(5) { r.below(F.len() as u32) as u8 } else { 255 },
                    data: r.one_in(3),
                    power: r.below(3) as u8,
                    bad: if r.one_in(4) { 1 + r.below(8) as u8 } else { 0 },
                },
                3 | 4 => Ev::Release,
                5 => Ev::Cut,
                6..=8 => {
                    let most = if r.one_in(4) { 6_000 } else { 300 };
                    Ev::Ticks { n: 1 + r.below(most) as u16, ms: 10 + r.below(41) as u8, batt_ok: !r.one_in(6) }
                }
                9 => Ev::Idle(r.below(60_000) as u16),
                10 => {
                    if r.one_in(3) {
                        Ev::Safe
                    } else {
                        Ev::Dry(r.one_in(2))
                    }
                }
                _ => Ev::Release,
            })
            .collect()
    }

    /// The pins as the plans leave them, and the PA drive the firmware's choke point writes.
    #[derive(Default)]
    struct Pins {
        tx_sel: Option<Sel>,
        rx_sel: Option<Sel>,
        drive: u8,
        chip_tx: [bool; 2],
    }

    impl Pins {
        fn run(&mut self, s: &Steps, dry: bool, level: u8) -> Result<(), String> {
            for &a in s.as_slice() {
                match a {
                    Act::Sel(x, on) if x.is_tx() => self.tx_sel = if on { Some(x) } else { self.tx_sel.filter(|&y| y != x) },
                    Act::Sel(x, on) => self.rx_sel = if on { Some(x) } else { self.rx_sel.filter(|&y| y != x) },
                    Act::PaRamp => {
                        self.drive = pa_drive(level, dry);
                        // TX-48: Dry TX never drives the PA.
                        if dry && self.drive != 0 {
                            return Err("PA drive with Dry TX on".into());
                        }
                        // Never PA drive with the relay wrong for the transmit select (TX-37, SAFE-21).
                        if self.tx_sel.is_none() {
                            return Err("PA ramp with no transmit select".into());
                        }
                    }
                    Act::PaZero => self.drive = 0,
                    Act::ChipTx(n) => self.chip_tx[n as usize] = true,
                    Act::ChipRx(n) => self.chip_tx[n as usize] = false,
                    _ => {}
                }
            }
            Ok(())
        }
    }

    /// TX-31's bucket, as the manual words it, in f64: keyed time fills it by the power's weight, idle time
    /// drains it at a quarter of High's rate; full at 2 min, cool again at half.
    struct Bucket {
        duty: f64,
        hot: bool,
    }

    impl Bucket {
        fn pass(&mut self, dt: u32, keyed: Option<u8>) {
            self.duty = match keyed {
                Some(p) => self.duty + dt as f64 * DUTY_WEIGHT[p as usize] as f64 / 4.0,
                None => (self.duty - dt as f64 / DUTY_DRAIN_DIV as f64).max(0.0),
            };
            if self.duty >= DUTY_FULL_MS as f64 {
                self.hot = true;
            } else if self.duty <= DUTY_FULL_MS as f64 / 2.0 {
                self.hot = false;
            }
        }
    }

    /// The bucket's rounding over many short ticks.
    const SLACK_MS: f64 = 1_000.0;

    pub fn run(evs: &[Ev]) -> Result<(), String> {
        let cfg = Cfg { timeout_s: 180, prealert_s: 10, end_tone: true, cw_id: true, tail: false, cw_each: false };
        let (mut path, mut seq, mut pins) = (Path::new(), TxSeq::new(), Pins::default());
        let mut b = Bucket { duty: 0.0, hot: false };
        let mut t = 1_000u32;
        let (mut dry, mut level) = (false, 0u8);
        // Keyed since, at which power; the battery low since; timed out (PTT to be released).
        let (mut keyed_at, mut power): (Option<u32>, u8) = (None, 0);
        let mut low_since: Option<u32> = None;
        let mut timed_out = false;
        let mut rx_before: Option<Rx> = None;
        let (mut keys, mut cuts) = (0u32, [0u32; 4]);
        for (i, &e) in evs.iter().enumerate() {
            let t0 = t;
            let at = |m: &str| format!("event {i} ({e:?}), from t {t0}: {m}");
            let was_keyed = seq.keyed();
            let mut s = Steps::default();
            match e {
                Ev::Key { tx, rx, data, power: p, bad } => {
                    let tx_f = F[tx as usize];
                    let rx_f = if rx == 255 { tx_f } else { F[rx as usize % F.len()] };
                    if !seq.keyed() {
                        // The Radio task tunes the slot first: its receive path.
                        let mut s0 = Steps::default();
                        let _ = path.rx(Rx::Bk(Sel::rx(rx_f)), Cause::ActiveSlot, t, &mut s0);
                        pins.run(&s0, dry, level).map_err(|m| at(&m))?;
                    }
                    let r = Req {
                        tx: tx_f,
                        rx: rx_f,
                        kind: if data { Kind::Data } else { Kind::Voice },
                        person: true,
                        confirmed: false,
                        am: bad == 1,
                        rx_only: bad == 2,
                        busy: bad == 3,
                        hf_si: bad == 4,
                        si_on: bad == 5,
                        region: Region::Us,
                        extend: false,
                        cal_ok: bad != 6,
                        cal_window: bad != 7,
                        batt_ok: bad != 8,
                        power: p,
                    };
                    let before = path.rx_now();
                    let hot_before = b.hot;
                    let res = seq.key(&r, &mut path, t, &mut s);
                    level = [40, 120, 255][p as usize];
                    match res {
                        Ok(()) => {
                            if was_keyed {
                                return Err(at("keyed again while keyed"));
                            }
                            if bad != 0 {
                                return Err(at("keyed with a check broken"));
                            }
                            if timed_out {
                                return Err(at("keyed again after a timeout without PTT released (TX-30)"));
                            }
                            if hot_before && b.duty >= DUTY_FULL_MS as f64 / 2.0 + SLACK_MS {
                                return Err(at(&format!("keyed with the duty bucket full ({:.0} ms)", b.duty)));
                            }
                            (keyed_at, power, low_since, rx_before) = (Some(t), p, None, Some(before));
                            keys += 1;
                        }
                        Err(why) => {
                            // A refusal never keys: no transmit select, no PA, no chip in transmit.
                            if s.as_slice().iter().any(|a| matches!(a, Act::PaRamp | Act::ChipTx(_)) || matches!(a, Act::Sel(x, true) if x.is_tx())) {
                                return Err(at("a refused key-up keyed something"));
                            }
                            if seq.keyed() != was_keyed {
                                return Err(at("a refusal changed the keyed state"));
                            }
                            // Liveness: with every check passing and the bucket cool, only the timeout's
                            // hold or a key-up in progress refuses.
                            let plain = bad == 0 && tx < 2 && rx == 255 && !was_keyed && !timed_out;
                            if plain && !b.hot && b.duty <= DUTY_FULL_MS as f64 / 2.0 - SLACK_MS {
                                return Err(at(&format!("a good key-up refused: {why:?} (the bucket at {:.0} ms, hot {})", b.duty, b.hot)));
                            }
                            if why == Refusal::Hot && b.duty < DUTY_FULL_MS as f64 / 2.0 - SLACK_MS {
                                return Err(at(&format!("refused as hot with the bucket at {:.0} ms", b.duty)));
                            }
                        }
                    }
                }
                Ev::Release => {
                    seq.release(&cfg, &mut path, t, &mut s);
                    timed_out = false;
                }
                Ev::Cut => {
                    seq.cut(&cfg, &mut path, t, &mut s);
                    timed_out = false;
                }
                Ev::Safe => {
                    seq.safe(&mut path, t, &mut s);
                    rx_before = None;
                }
                Ev::Dry(d) => dry = d,
                Ev::Idle(ms) => {
                    if !seq.keyed() {
                        b.pass(ms as u32, None);
                        t += ms as u32;
                    }
                }
                Ev::Ticks { n, ms, batt_ok } => {
                    for _ in 0..n {
                        b.pass(ms as u32, keyed_at.map(|_| power));
                        t += ms as u32;
                        if keyed_at.is_some() && !batt_ok {
                            low_since.get_or_insert(t);
                        } else {
                            low_since = None;
                        }
                        if !seq.keyed() {
                            continue; // the Radio task ticks the sequencer only while keyed (`tx_tick`)
                        }
                        let mut s = Steps::default();
                        let ev = seq.tick(&cfg, batt_ok, &mut path, t, &mut s);
                        pins.run(&s, dry, level).map_err(|m| at(&m))?;
                        if let Some(ev) = ev.filter(|&x| x != Event::PreAlert) {
                            cuts[ev as usize] += 1;
                            timed_out |= ev == Event::Timeout;
                        }
                        let what = |m: &str| at(&format!("tick at {t}: {m}"));
                        if let Some(k) = keyed_at.filter(|_| seq.keyed()) {
                            // TX-30: the timeout fires on time.
                            if t - k >= cfg.timeout_s as u32 * 1000 {
                                return Err(what("still keyed past the timeout"));
                            }
                            // TX-32: low for BATT_LOW_MS without a break unkeys.
                            if low_since.is_some_and(|l| t - l >= BATT_LOW_MS) {
                                return Err(what("still keyed with the battery low past its time"));
                            }
                            // TX-31: a full bucket unkeys.
                            if b.duty >= DUTY_FULL_MS as f64 + SLACK_MS {
                                return Err(what(&format!("still keyed with the duty bucket at {:.0} ms", b.duty)));
                            }
                        }
                        if !seq.keyed() {
                            if keyed_at.take().is_some() {
                                after_unkey(&pins, &path, rx_before.take()).map_err(|m| what(&m))?;
                            }
                            if !batt_ok {
                                low_since = None;
                            }
                        }
                    }
                }
            }
            pins.run(&s, dry, level).map_err(|m| at(&m))?;
            if seq.keyed() != path.keyed().is_some() {
                return Err(at("the sequencer and the path disagree"));
            }
            if !seq.keyed() {
                if keyed_at.take().is_some() && !matches!(e, Ev::Safe) {
                    after_unkey(&pins, &path, rx_before.take()).map_err(|m| at(&m))?;
                }
                if pins.drive != 0 || pins.chip_tx.contains(&true) || pins.tx_sel.is_some() {
                    return Err(at("unkeyed with the PA, a chip or a transmit select on"));
                }
            }
        }
        reached_tx(keys, cuts);
        Ok(())
    }

    /// TX-26: every unkey ends with the PA at 0, no chip transmitting, no transmit select and the receive path
    /// back as it was before the key-up.
    fn after_unkey(p: &Pins, path: &Path, rx: Option<Rx>) -> Result<(), String> {
        if p.drive != 0 || p.chip_tx.contains(&true) || p.tx_sel.is_some() {
            return Err("an unkey left the PA, a chip or a transmit select on".into());
        }
        if let Some(rx) = rx {
            if path.rx_now() != rx {
                return Err(format!("the receive path not back after the unkey: {:?}, was {rx:?}", path.rx_now()));
            }
            if let Rx::Bk(s) = rx {
                if p.rx_sel != Some(s) {
                    return Err(format!("the receive select not back after the unkey: {:?}, was {s:?}", p.rx_sel));
                }
            }
        }
        Ok(())
    }

    fn reached_tx(keys: u32, cuts: [u32; 4]) {
        REACHED[5].fetch_add(keys, Relaxed);
        REACHED[6].fetch_add(cuts[Event::Timeout as usize] + cuts[Event::Battery as usize], Relaxed);
        REACHED[7].fetch_add(cuts[Event::Hot as usize], Relaxed);
    }
}

#[test]
fn tx_random_streams_keep_the_rules() {
    check("tx", 31, 10_000, tx::gen, tx::run);
    let (keys, cuts, hot) = (REACHED[5].load(Relaxed), REACHED[6].load(Relaxed), REACHED[7].load(Relaxed));
    std::println!("tx: {keys} key-ups, {cuts} timeout or battery cuts, {hot} duty cuts");
    assert!(keys > 2_000 && cuts > 50 && hot > 5, "the sequences reach it all: {keys} key-ups, {cuts} cuts, {hot} duty cuts");
}

// ---------------------------------------------------------------------------------------------------------------
// The scan engine.

mod scan {
    use super::*;
    use ferrite_core::scan::{List, Out, Range, Resume, Scan, NUISANCE};

    /// A channel list: each channel's frequency (None: empty), its Skip flag (SCAN-9) and the priority channels.
    #[derive(Clone, Debug)]
    struct Chans {
        f: Vec<Option<Freq10>>,
        skip: Vec<bool>,
        prio: Vec<u16>,
    }

    impl List for Chans {
        fn len(&self) -> u16 {
            self.f.len() as u16
        }
        fn at(&self, i: u16) -> Option<Freq10> {
            self.f.get(i as usize).copied().flatten().filter(|_| !self.skip[i as usize])
        }
        fn priority(&self, k: u8) -> Option<u16> {
            self.prio.get(k as usize).copied()
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub enum Ev {
        Start(u16),
        Ticks(u16),
        /// A carrier on position i's frequency comes or goes.
        Carrier(u16, bool),
        SkipOnce,
        /// \# (SCAN-17): stored as the caller does (the Skip flag, the preset's list) before the next tick.
        SkipLasting,
        Dir(bool),
        Ptt,
        Stop,
        Restep,
        Pass,
    }

    /// One sequence's list and resume rule, from its first numbers.
    #[derive(Clone, Debug)]
    pub struct Case {
        range: bool,
        n: u16,
        skip: Vec<bool>,
        prio: Vec<u16>,
        resume: u8,
        prio_ms: u32,
        fast: bool,
        evs: Vec<Ev>,
    }

    pub fn gen(r: &mut Rng) -> Case {
        let range = r.one_in(2);
        let most = if r.one_in(4) { 4 } else { 40 };
        let n = 1 + r.below(most) as u16;
        let dense = r.below(4);
        let skip = (0..n).map(|_| r.below(4) < dense).collect();
        let prio = if !range && r.one_in(2) { (0..1 + r.below(3)).map(|_| r.below(n as u32) as u16).collect() } else { Vec::new() };
        let evs = (0..20 + r.below(80))
            .map(|_| match r.below(14) {
                0 => Ev::Start(r.below(n as u32) as u16),
                1..=4 => Ev::Ticks(1 + r.below(400) as u16),
                5..=7 => Ev::Carrier(r.below(n as u32) as u16, !r.one_in(3)),
                8 => Ev::SkipOnce,
                9 => Ev::SkipLasting,
                10 => Ev::Dir(r.one_in(2)),
                11 => {
                    if r.one_in(2) {
                        Ev::Ptt
                    } else {
                        Ev::Stop
                    }
                }
                12 => Ev::Restep,
                _ => Ev::Pass,
            })
            .collect();
        Case { range, n, skip, prio, resume: r.below(13) as u8, prio_ms: if r.one_in(2) { 0 } else { 500 + r.below(3_000) }, fast: range && r.one_in(2), evs }
    }

    const LO: Freq10 = mhz(146);
    const STEP: Freq10 = 1_250;

    /// The case with its events shrunk: the list and settings stay as generated.
    pub fn check_case(seed: u64, c: Case) {
        if let Err(e) = run(&c) {
            let evs = shrink(c.evs.clone(), |evs| run(&Case { evs: evs.to_vec(), ..c.clone() }).is_err());
            let small = Case { evs, ..c };
            let why = run(&small).err().unwrap_or(e);
            panic!("scan: seed {seed}: {why}\n{small:?}");
        }
    }

    /// The engine call for one event on list `l`: what it said, and a position \# hands back to store.
    fn act<L: List>(s: &mut Scan, l: &L, e: Ev, t: &mut u32, tuned: &mut Freq10, air: &[Freq10]) -> (Vec<Out>, Option<u16>) {
        let mut outs = Vec::new();
        let mut lasting = None;
        match e {
            Ev::Ticks(n) => {
                for _ in 0..n {
                    *t += 10;
                    let o = s.tick(l, *t, air.contains(tuned));
                    if let Some(f) = o.tune {
                        *tuned = f;
                    }
                    outs.push(o);
                }
            }
            Ev::SkipOnce => outs.push(s.skip_once(l, *t)),
            Ev::SkipLasting => {
                let (p, o) = s.skip_lasting(l, *t);
                (lasting, _) = (p, outs.push(o));
            }
            Ev::Dir(up) => outs.push(s.dir(up, l, *t)),
            Ev::Pass if s.running() => outs.push(s.pass(l, *t)),
            _ => {}
        }
        for o in &outs {
            if let Some(f) = o.tune {
                *tuned = f;
            }
        }
        (outs, lasting)
    }

    pub fn run(c: &Case) -> Result<(), String> {
        let freq = |i: u16| LO + i as u32 * STEP;
        // The caller's stored skips: a range preset's list (SCAN-10) or the channels' flags (SCAN-9).
        let mut skipped: Vec<Freq10> = (0..c.n).filter(|&i| c.skip[i as usize]).map(freq).collect();
        let mut chans = Chans { f: (0..c.n).map(|i| Some(freq(i))).collect(), skip: c.skip.clone(), prio: c.prio.clone() };
        let mut s = Scan::new();
        (s.resume, s.prio_ms, s.fast) = (Resume::from_code(c.resume), c.prio_ms, c.fast);
        let (mut t, mut tuned, mut air) = (1_000u32, freq(0), Vec::<Freq10>::new());
        let mut nuisance: Vec<u16> = Vec::new();
        let mut start = 0u16;
        for (i, &e) in c.evs.iter().enumerate() {
            // What the list says now: a position is stopped on if it has a frequency, isn't stored as skipped
            // and isn't skipped once this scan.
            let at_now = |p: u16, skipped: &[Freq10], chans: &Chans| -> Option<Freq10> {
                if c.range {
                    Range { lo: LO, hi: freq(c.n - 1), step: STEP, skipped }.at(p)
                } else {
                    chans.at(p)
                }
            };
            let t0 = t;
            let (was_hit, pos) = (s.on_hit(), s.pos());
            let at = |m: &str| format!("event {i} ({e:?}), from t {t0}: {m}");
            match e {
                Ev::Start(p) if !s.running() && at_now(p, &skipped, &chans).is_some() => {
                    // The app starts from the slot's channel or frequency, a stoppable one here (a channel with
                    // Skip is still tunable by hand: not modelled).
                    s.start(p, t);
                    (tuned, start) = (freq(p), p);
                    nuisance.clear();
                }
                Ev::Carrier(p, on) => {
                    air.retain(|&f| f != freq(p));
                    if on {
                        air.push(freq(p));
                    }
                }
                Ev::SkipOnce if s.on_hit() && nuisance.len() < NUISANCE => nuisance.push(s.pos()), // SCAN-20.3
                Ev::Ptt => {
                    let p = s.ptt();
                    // SCAN-19: the hit, or where the scan started.
                    if p != if was_hit { pos } else { start } || s.running() {
                        return Err(at(&format!("PTT ended on {p}, the hit {was_hit} at {pos}, started at {start}")));
                    }
                }
                Ev::Stop => s.stop(),
                Ev::Restep => s.restep(t),
                _ => {}
            }
            let (outs, lasting) = if c.range {
                let r = Range { lo: LO, hi: freq(c.n - 1), step: STEP, skipped: &skipped };
                act(&mut s, &r, e, &mut t, &mut tuned, &air)
            } else {
                act(&mut s, &chans, e, &mut t, &mut tuned, &air)
            };
            if let Some(p) = lasting {
                if c.range {
                    skipped.push(freq(p));
                } else {
                    chans.skip[p as usize] = true;
                }
            }
            let ok = |p: u16| at_now(p, &skipped, &chans).is_some() && !nuisance.contains(&p);
            for o in &outs {
                if let Some(f) = o.tune {
                    let p = ((f - LO) / STEP) as u16;
                    if !ok(p) {
                        return Err(at(&format!("tuned to {p}, a skipped position")));
                    }
                }
                if o.hit.is_some() {
                    reached(8);
                }
                if let Some(p) = o.hit.filter(|&p| !ok(p)) {
                    return Err(at(&format!("a hit on {p}, a skipped position")));
                }
            }
            if s.on_hit() && !ok(s.pos()) {
                return Err(at(&format!("stopped on {}, a skipped position", s.pos())));
            }
            // A list with nothing to stop on ends the scan at the step's end.
            if s.running() && !s.on_hit() && !(0..c.n).any(ok) && outs.iter().any(|o| o.tune.is_some()) {
                return Err(at("still scanning a list with nothing to stop on"));
            }
        }
        reached(if c.range { 9 } else { 10 });
        Ok(())
    }
}

#[test]
fn scan_random_streams_keep_the_rules() {
    for i in 0..20_000 * scale() {
        let seed = 47u64.wrapping_mul(1_000_003).wrapping_add(i as u64);
        scan::check_case(seed, scan::gen(&mut Rng(seed)));
    }
    let (range, chans) = (REACHED[9].load(Relaxed), REACHED[10].load(Relaxed));
    std::println!("scan: {range} range scans, {chans} channel scans, {} hits", REACHED[8].load(Relaxed));
    assert!(range > 500 && chans > 500 && REACHED[8].load(Relaxed) > 5_000, "the sequences reach it all");
}
