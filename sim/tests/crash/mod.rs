//! The crash hunts' shared part (`sweep.rs`, `monkey.rs`): odd states, the inputs, the checks after each input,
//! and a seeded generator. Decision 438 was the class they hunt: a refused key-up on the satellite pass page drew
//! the TX refused screen for slot 4 (the satellite's), which indexed the three slots A-C and faulted. An odd state
//! (keyed, refused, a slot outside A-C, a slot off, APRS holding C, Low Power Mode, a scan stopped on a hit, a mode
//! screen open) plus a key or event at the wrong moment.
//!
//! `FERRITE_FUZZ_SCALE=N` (default 1) multiplies the work: more acts a screen in the sweep, more and longer monkey
//! sessions (CI's nightly run).
#![allow(dead_code)]

use crate::common::Radio;
use ferrite_core::keys::{Key, PTT_A, PTT_B};
use ferrite_core::txplan::Refusal;
use ferrite_sim::simcore::{Event, RepaintMiss};
use std::panic::{catch_unwind, AssertUnwindSafe};

/// `FERRITE_FUZZ_SCALE` (default 1, at least 1).
pub fn scale() -> u32 {
    std::env::var("FERRITE_FUZZ_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(1).max(1)
}

/// splitmix64: small, std-only, and the same seed always gives the same run.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    pub fn pick<T: Copy>(&mut self, v: &[T]) -> T {
        v[self.below(v.len())]
    }
}

/// Every refusal the Radio task (or the UI) can give. The match below fails to build when one is added, so the
/// list can't fall behind.
pub const REFUSALS: [Refusal; 19] = [
    Refusal::OutOfBand,
    Refusal::NotCalibrated,
    Refusal::RxOnlyChannel,
    Refusal::Busy,
    Refusal::Battery,
    Refusal::Locked,
    Refusal::InProgress,
    Refusal::NoCallsign,
    Refusal::RxOnlyBuild,
    Refusal::ReceiveOnlyHere,
    Refusal::Crossband,
    Refusal::RelayMismatch,
    Refusal::AmRxOnly,
    Refusal::WfmRxOnly,
    Refusal::Emergency,
    Refusal::ReleasePtt,
    Refusal::PttAtBoot,
    Refusal::Hot,
    Refusal::Broadcast,
];

#[allow(clippy::match_same_arms)]
const fn _every_refusal_listed(r: Refusal) {
    match r {
        Refusal::OutOfBand
        | Refusal::NotCalibrated
        | Refusal::RxOnlyChannel
        | Refusal::Busy
        | Refusal::Battery
        | Refusal::Locked
        | Refusal::InProgress
        | Refusal::NoCallsign
        | Refusal::RxOnlyBuild
        | Refusal::ReceiveOnlyHere
        | Refusal::Crossband
        | Refusal::RelayMismatch
        | Refusal::AmRxOnly
        | Refusal::WfmRxOnly
        | Refusal::Emergency
        | Refusal::ReleasePtt
        | Refusal::PttAtBoot
        | Refusal::Hot
        | Refusal::Broadcast => {}
    }
}

/// The front keys and the side keys (PTT apart).
pub const KEYS: [Key; 22] = [
    Key::Menu,
    Key::Exit,
    Key::Up,
    Key::Down,
    Key::Left,
    Key::Right,
    Key::Slot,
    Key::VfoMem,
    Key::Star,
    Key::Hash,
    Key::Digit(0),
    Key::Digit(1),
    Key::Digit(2),
    Key::Digit(3),
    Key::Digit(4),
    Key::Digit(5),
    Key::Digit(6),
    Key::Digit(7),
    Key::Digit(8),
    Key::Digit(9),
    Key::Side(1),
    Key::Side(2),
];

pub fn key_name(k: Key) -> String {
    match k {
        Key::Digit(d) => d.to_string(),
        Key::Side(s) => format!("SIDE{s}"),
        Key::Menu => "OK".into(),
        Key::Exit => "BACK".into(),
        Key::VfoMem => "VM".into(),
        Key::Star => "STAR".into(),
        Key::Hash => "HASH".into(),
        Key::Ptt(p) if p == PTT_B => "PTTB".into(),
        Key::Ptt(_) => "PTT".into(),
        other => format!("{other:?}").to_uppercase(),
    }
}

/// The Radio task refuses the key-up in progress now (as its own checks do: calibration, battery under load, the
/// duty limit, a second key-up): the fake's live state, as the UI reads it.
pub fn refuse_now(r: &mut Radio, why: Refusal) {
    let tx = &mut r.sim.radio_mut().live.tx;
    tx.keyed = false;
    tx.refused = Some(why);
    tx.refusals = tx.refusals.wrapping_add(1);
}

/// One input: a key, the knob, a key-up and what can happen to it.
#[derive(Clone, Copy, Debug)]
pub enum Act {
    Tap(Key),
    Hold(Key),
    Knob(i32),
    /// A 300 ms key-up on PTT A (0) or PTT B (1).
    Ptt(u8),
    /// PTT A held, then PTT B too: the second key-up while the first is on (decision 438).
    PttBoth,
    /// PTT A released and pressed again at once: a key-up while the last one is ending.
    PttAgain,
    /// PTT A held; the Radio task refuses the key-up.
    Refused(Refusal),
    /// PTT A let go and the Radio task's refusal arrives just after.
    LateRefused(Refusal),
    /// A beacon (a data key-up), refused by the Radio task.
    DataRefused(Refusal),
}

impl Act {
    pub fn run(self, r: &mut Radio) {
        let (a, b) = (Key::Ptt(PTT_A), Key::Ptt(PTT_B));
        match self {
            Act::Tap(k) => {
                r.press(k);
            }
            Act::Hold(k) => {
                r.hold(k);
            }
            Act::Knob(n) => {
                r.knob(n);
            }
            Act::Ptt(p) => {
                let k = if p == 0 { a } else { b };
                r.sim.key(k, true);
                r.wait(300);
                r.sim.key(k, false);
                r.wait(100);
            }
            Act::PttBoth => {
                r.sim.key(a, true);
                r.wait(200);
                r.sim.key(b, true);
                r.wait(200);
                r.sim.key(b, false);
                r.wait(100);
                r.sim.key(a, false);
                r.wait(100);
            }
            Act::PttAgain => {
                r.sim.key(a, true);
                r.wait(300);
                r.sim.key(a, false);
                r.wait(crate::common::FRAME);
                r.sim.key(a, true);
                r.wait(300);
                r.sim.key(a, false);
                r.wait(100);
            }
            Act::Refused(why) => {
                r.sim.key(a, true);
                r.wait(200);
                refuse_now(r, why);
                r.wait(200);
                r.sim.key(a, false);
                r.wait(100);
            }
            Act::LateRefused(why) => {
                r.sim.key(a, true);
                r.wait(300);
                r.sim.key(a, false);
                refuse_now(r, why);
                r.wait(200);
            }
            Act::DataRefused(why) => {
                r.sim.event(Event::Beacon);
                r.wait(crate::common::FRAME);
                refuse_now(r, why);
                r.wait(200);
            }
        }
    }

    /// A front or side key, or the knob (not a key-up).
    pub fn is_key(self) -> bool {
        matches!(self, Act::Tap(_) | Act::Hold(_) | Act::Knob(_))
    }

    pub fn name(self) -> String {
        match self {
            Act::Tap(k) => format!("press {}", key_name(k)),
            Act::Hold(k) => format!("hold {}", key_name(k)),
            Act::Knob(n) => format!("knob {n}"),
            Act::Ptt(0) => "ptt A 300ms".into(),
            Act::Ptt(_) => "ptt B 300ms".into(),
            Act::PttBoth => "ptt A held + ptt B".into(),
            Act::PttAgain => "ptt A, released, again".into(),
            Act::Refused(w) => format!("ptt A held, refused {w:?}"),
            Act::LateRefused(w) => format!("ptt A let go, refused {w:?} after"),
            Act::DataRefused(w) => format!("beacon, refused {w:?}"),
        }
    }
}

/// Every input the sweep tries on every screen.
pub fn all_acts() -> Vec<Act> {
    let mut v: Vec<Act> = KEYS.iter().map(|&k| Act::Tap(k)).collect();
    v.extend(KEYS.iter().map(|&k| Act::Hold(k)));
    v.extend([Act::Knob(1), Act::Knob(-1), Act::Ptt(0), Act::Ptt(1), Act::PttBoth, Act::PttAgain]);
    v.extend(REFUSALS.iter().map(|&w| Act::Refused(w)));
    v.extend([Act::LateRefused(Refusal::InProgress), Act::LateRefused(Refusal::Hot)]);
    v.extend([Act::DataRefused(Refusal::Busy), Act::DataRefused(Refusal::Hot), Act::DataRefused(Refusal::InProgress)]);
    v
}

/// A line to the radio's terminal, as Console or a person at the cable types it; its answer is dropped.
pub fn term(r: &mut Radio, line: &str) {
    r.sim.term_feed(format!("\r:{line}\r").as_bytes());
    r.wait(100);
    r.sim.term_clear();
    r.sim.term_take();
}

/// A setup script: `common::Radio::run`'s steps, plus `term LINE` (the terminal), `carrier N` (a carrier on slot N:
/// 0-2, the fake's `carrier_on`), `clock +MIN` (the clock moved on: the satellites' passes), `hit N` (wait, up to
/// 20 s, until slot N's scan stops on a hit), `hf10m` (HF mode's memory on 29.6 MHz FM, where it transmits) and `pin`
/// (a PIN set and asked for, as at power-on).
pub fn apply(r: &mut Radio, script: &str) {
    for step in script.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let (cmd, arg) = step.split_once(' ').unwrap_or((step, ""));
        match cmd {
            "term" => term(r, arg),
            "carrier" => r.sim.radio_mut().carrier_on |= 1 << arg.parse::<u8>().unwrap(),
            "clock" => {
                let min: u64 = arg.trim_start_matches('+').parse().unwrap();
                r.sim.set_clock(1_791_400_000 + r.t as u64 / 1000 + min * 60);
                r.wait(200);
            }
            "hit" => {
                let i: usize = arg.parse().unwrap();
                let t0 = r.t;
                while r.t - t0 < 20_000 {
                    let s = &r.sim.radio().live.slots[i];
                    if s.scanning && s.squelch_open || !s.scanning && r.t - t0 > 1000 {
                        break; // stopped on a hit, or no scan running to stop
                    }
                    r.wait(100);
                }
            }
            "hf10m" => {
                // HF mode's memory on 29.600 MHz FM: the BK4829's side, where HF mode transmits (HF-23)
                let h = &mut r.sim.app_mut().settings_mut().hf;
                (h.freq, h.mode) = (2_960_000, 0);
            }
            "pin" => {
                let s = r.sim.app_mut().settings_mut();
                (s.pin, s.pin_len, s.pin_mode) = (1234, 4, 1);
                r.sim.app_mut().ask_pin();
                r.wait(200);
            }
            _ => {
                r.run(step);
            }
        }
    }
}

/// What a state keeps, to see whether an input undid it (then it is set up again before the next one).
pub type Keeps = fn(&Radio) -> String;

/// An odd state: set up from home by `setup`; `pre` and `post` run around each input (a PTT held through it).
/// `place`: the state is a screen of its own (the satellite pass page, the crossband repeater): the inputs go
/// there, not to every screen of the jump list.
#[derive(Clone, Copy)]
pub struct State {
    pub name: &'static str,
    pub setup: &'static str,
    pub pre: &'static str,
    pub post: &'static str,
    pub place: bool,
    pub keeps: Option<Keeps>,
}

const fn st(name: &'static str, setup: &'static str, keeps: Option<Keeps>) -> State {
    State { name, setup, pre: "", post: "", place: false, keeps }
}

const fn place(name: &'static str, setup: &'static str) -> State {
    State { name, setup, pre: "", post: "", place: true, keeps: None }
}

fn settings(r: &Radio) -> String {
    let a = r.sim.app();
    let s = a.settings();
    format!("{} {} {} {:?} {}", s.layout, s.theme, s.lpm, a.aprs_holder(), a.low_power())
}

fn radio_bits(r: &Radio) -> String {
    let f = r.sim.radio();
    format!("{} {} {}", f.monitor, f.slot_off, settings(r))
}

fn scanning(r: &Radio) -> String {
    let l = &r.sim.radio().live;
    format!("{}{}{} {}", l.slots[0].scanning as u8, l.slots[1].scanning as u8, l.slots[2].scanning as u8, settings(r))
}

fn fm(r: &Radio) -> String {
    format!("{} {}", r.sim.app().slot(0).freq(), settings(r))
}

/// The odd states. The jump list's screens give the rest (HF mode, the scope, the tone search, the counter, the
/// editors, the PIN entry, ...): each state is tried on each of them.
pub fn states() -> Vec<State> {
    vec![
        st("plain", "", None),
        State { name: "keyed (voice)", setup: "", pre: "down PTT; wait 200ms", post: "up PTT; wait 100ms", place: false, keeps: None },
        State { name: "keyed (beacon)", setup: "", pre: "event beacon", post: "", place: false, keeps: None },
        st("slot A off", "press STAR; press STAR", Some(radio_bits)),
        st("slot B off and active", "term slot B active; press STAR; press STAR", Some(radio_bits)),
        st("APRS Tracker", "term set 0 5 3 Tracker; wait 1s", Some(settings)),
        st("APRS mode on", "term set 0 5 3 On; wait 1s", Some(settings)),
        st("APRS KISS", "term set 0 5 3 KISS; wait 1s", Some(settings)),
        st("Low Power Mode", "term set 0 7 7 2 On; wait 1s", Some(settings)),
        st("band scan stopped on a hit", "press SIDE2; carrier 0; hit 0", Some(scanning)),
        st("channel scan stopped on a hit", "term slot B active; press SIDE2; carrier 1; hit 1", Some(scanning)),
        st("range preset scan stopped on a hit", "menu 0 1 4; press SIDE2; carrier 0; hit 0", Some(scanning)),
        st("broadcast FM playing", "event fm; wait 2s", Some(fm)),
        st("monitor latched", "press SIDE1; wait 300ms", Some(radio_bits)),
        st("layout: single + scope", "term set 0 7 2 Single + scope; wait 2s", Some(settings)),
        st("layout: watch + activity", "term set 0 7 2 Watch + activity; wait 1s", Some(settings)),
        st("theme: Daylight", "term set 0 7 1 Daylight", Some(settings)),
        st("theme: Night red", "term set 0 7 1 Night red", Some(settings)),
        st("battery cut-off", "event battery; event battery", None),
        st("keys locked", "hold HASH; press 0", None),
        State { name: "link writing", setup: "", pre: "event linkwrite", post: "", place: false, keeps: None },
        // The places: their own screen, every input there.
        place("satellite pass up", "clock +64; menu 0 3 6; wait 1s; press OK; wait 1s"),
        place("satellite pass before it rises", "menu 0 3 6; wait 1s; press OK; wait 1s"),
        place("crossband repeater running", "event crossband; wait 1s; press HASH; wait 1s"),
        place("crossband repeater running, keyed", "event crossband; wait 1s; press HASH; wait 1s; down PTT; wait 300ms"),
        place("HF mode, receive only (AM on the SI4732)", "menu 0 3 3; wait 4s"),
        place("HF mode keyed (29.6 MHz FM)", "hf10m; menu 0 3 3; wait 4s; down PTT; wait 300ms"),
        place("PIN entry at power-on", "pin"),
    ]
}

/// The repaint misses already known (`repaint.rs` keeps the list).
fn repaint_known(m: &RepaintMiss) -> bool {
    crate::common::REPAINT_KNOWN.iter().any(|&(n, y0, y1, _)| m.screen.ends_with(n) && m.area.1 >= y0 && m.area.3 <= y1)
}

/// Text cut off in odd states, found by these hunts (Nov 23, 2026) and not fixed yet: (the cut text, the file
/// that drew it, "" when the cut is past the screen's edge with no widget site; what it is). Each is a wording or
/// layout call, so left for a person. Shrink this list; never widen an entry to make a test pass. Empty since
/// ferrite#144 fixed the first five (the auto-transmit toast, the editors' slot strip, the crossband check line,
/// Low Power Mode's mode line, the satellite pass page with HD).
pub const CLIPS_KNOWN: &[(&str, &str, &str)] = &[];

fn clip_known(c: &ferrite_ui::clipcheck::Clip) -> bool {
    CLIPS_KNOWN.iter().any(|&(t, f, _)| c.text.contains(t) && c.at.map_or(f.is_empty(), |a| !f.is_empty() && a.file().ends_with(f)))
}

/// The checks after every input: the UI readback builds and each line fits Live View whole (LINK-48), the mode
/// the App wants is the one running (MODE-2), no text was cut off (clipcheck) and, when `repaint` is on for this
/// input, each frame matched a fresh repaint (DISP-31). A panic anywhere is caught by [`attempt`].
pub fn check(r: &mut Radio) -> Result<(), String> {
    let clips: Vec<_> = ferrite_ui::clipcheck::take().into_iter().filter(|c| !clip_known(c)).collect();
    if let Some(c) = clips.first() {
        return Err(format!("text cut off: {} {:?} at {} ({})", c.kind, c.text, c.at.map_or(String::new(), |a| a.file().to_string()), c.context));
    }
    if !r.sim.app().mode_settled() {
        return Err(format!("MODE-2: the mode running isn't the one wanted on {}", r.screen()));
    }
    let rt = r.sim.readback_texts();
    let n = rt.lines().count();
    if n > ferrite_core::liveview::LINES {
        return Err(format!("{n} readback lines, Live View carries {}", ferrite_core::liveview::LINES));
    }
    for l in rt.lines() {
        let mut c = [0u8; 256];
        let k = ferrite_core::liveview::compact(l.as_bytes(), &mut c);
        let e = ferrite_core::liveview::encoded_len(&c[..k]);
        if !(k == l.len() || l.starts_with("state")) || e > ferrite_core::liveview::LINE_ROOM {
            return Err(format!("a readback line Live View would cut ({e} coded bytes): {l}"));
        }
    }
    let misses: Vec<&RepaintMiss> = r.sim.repaint_misses.iter().filter(|m| !repaint_known(m)).collect();
    if let Some(m) = misses.first() {
        return Err(format!(
            "DISP-31: a frame differs from a fresh repaint at {} ms on {}: {} pixels in x {}..{}, y {}..{}",
            m.at, m.screen, m.pixels, m.area.0, m.area.2, m.area.1, m.area.3
        ));
    }
    r.sim.repaint_misses.clear();
    Ok(())
}

/// Runs `f` on the radio; a panic comes back as an error with its message (the radio is then not to be trusted:
/// the caller starts a new one).
pub fn attempt(r: &mut Radio, f: impl FnOnce(&mut Radio)) -> Result<(), String> {
    catch_unwind(AssertUnwindSafe(|| f(r))).map_err(|e| {
        let msg = e.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| e.downcast_ref::<String>().cloned()).unwrap_or_default();
        format!("panic: {msg}")
    })
}

/// A new sample radio with the state set up; `Err` if setting it up already failed.
pub fn fresh(s: &State) -> Result<Radio, String> {
    let mut r = Radio::new();
    attempt(&mut r, |r| {
        r.home();
        apply(r, s.setup);
    })
    .and_then(|_| check(&mut r))
    .map(|_| r)
    .map_err(|e| format!("setting up \"{}\": {e}", s.name))
}

/// Where the radio is, finer than the screen's name: the readback's first line (`screen SatPass "Satellites"
/// title="ISS"`).
pub fn place_of(r: &Radio) -> String {
    r.sim.readback().lines().next().unwrap_or("").to_string()
}

/// The radio back on (after a power switch or an empty battery), past its boot screen.
pub fn wake(r: &mut Radio) {
    if r.screen() == "Off" {
        r.event(Event::PowerSwitch);
    }
    let t0 = r.t;
    while r.sim.booting() && r.t - t0 < 10_000 {
        r.wait(100);
    }
}
