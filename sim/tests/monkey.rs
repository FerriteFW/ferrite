//! Monkey sessions: long seeded runs of everything at once (keys, holds, the knob, PTT A and B held and let go at
//! odd moments, the Radio task's refusals, squelch opening and closing on random slots, scans meeting carriers, the
//! world's events (messages, alerts, beacons, the battery, the power switch, a link write, the crossband
//! repeater, broadcast FM), settings and slots changed from the terminal, the clock moved on to a satellite pass,
//! screens opened from anywhere), with `crash::check`'s invariants after every step and a fresh-repaint check
//! (DISP-31) on one step in eight. A failure prints the seed, the step, and the steps shrunk to a short sequence
//! that still fails, as a script to replay.
//!
//! `FERRITE_FUZZ_SCALE=N` runs N times the seeds; `FERRITE_MONKEY_SEED=S` runs just seed S (printing its steps).

mod common;
mod crash;

use common::Radio;
use crash::{attempt, check, refuse_now, Rng, KEYS, REFUSALS};
use ferrite_core::keys::{Key, PTT_A, PTT_B};
use ferrite_core::txplan::Refusal;
use ferrite_sim::simcore::{jump_names, Event};

/// Steps in a session (times `FERRITE_FUZZ_SCALE` sessions).
const STEPS: usize = 400;

/// Terminal lines a session sends (as Console or a person at the cable would).
const TERM: &[&str] = &[
    "set 0 7 2 Watch",
    "set 0 7 2 Single + scope",
    "set 0 7 2 Watch + activity",
    "set 0 7 1 Daylight",
    "set 0 7 1 Dark",
    "set 0 7 1 Night red",
    "set 0 5 3 Off",
    "set 0 5 3 Tracker",
    "set 0 5 3 On",
    "set 0 5 3 KISS",
    "set 0 7 7 2 On",
    "set 0 7 7 2 Off",
    "set 0 1 1 A+B+C",
    "set 0 1 1 A+B",
    "slot A 146.52",
    "slot A 7.155",
    "slot B active",
    "slot C 162.55",
    "slot A 121.5",
    "ch 12 power high",
    "ch 12 rxonly on",
    "ch 12 rxonly off",
    "xb start",
    "xb stop",
    "quick 3 \"On my way\"",
];

const EVENTS: [Event; 16] = [
    Event::Message,
    Event::Alert,
    Event::Gps,
    Event::Bluetooth,
    Event::Battery,
    Event::PowerSwitch,
    Event::EmptyBattery,
    Event::SsbPatch,
    Event::LinkWrite,
    Event::Crossband,
    Event::Layout,
    Event::Beacon,
    Event::ScreenOff,
    Event::ToneHint,
    Event::FmStation,
    Event::SpeakerMic,
];

#[derive(Clone, Copy, Debug)]
enum Step {
    Tap(Key),
    Hold(Key),
    Knob(i32),
    /// A PTT pressed (0 A, 1 B), left down until its `Up`.
    Down(u8),
    Up(u8),
    /// The Radio task refuses whatever key-up is on.
    Refuse(Refusal),
    Event(Event),
    /// The slots with a carrier on them now (the fake's `carrier_on`).
    Carriers(u8),
    Jump(usize),
    Term(&'static str),
    /// The clock moved on, minutes (the satellites' passes come and go).
    Clock(u32),
    Wait(u32),
}

impl Step {
    fn run(self, r: &mut Radio) {
        let ptt = |p: u8| Key::Ptt(if p == 0 { PTT_A } else { PTT_B });
        match self {
            Step::Tap(k) => {
                r.press(k);
            }
            Step::Hold(k) => {
                r.hold(k);
            }
            Step::Knob(n) => {
                r.knob(n);
            }
            Step::Down(p) => {
                r.down(ptt(p));
                r.wait(200);
            }
            Step::Up(p) => {
                r.up(ptt(p));
                r.wait(100);
            }
            Step::Refuse(w) => {
                refuse_now(r, w);
                r.wait(100);
            }
            Step::Event(e) => {
                r.event(e);
                if e == Event::PowerSwitch && r.screen() == "Off" {
                    r.wait(500);
                    r.event(Event::PowerSwitch); // on again
                    r.wait(3000);
                }
            }
            Step::Carriers(m) => {
                r.sim.radio_mut().carrier_on = m;
                r.wait(200);
            }
            Step::Jump(i) => {
                r.sim.jump(i);
                r.wait(300);
            }
            Step::Term(l) => crash::term(r, l),
            Step::Clock(min) => {
                r.sim.set_clock(1_791_400_000 + r.t as u64 / 1000 + min as u64 * 60);
                r.wait(200);
            }
            Step::Wait(ms) => {
                r.wait(ms);
            }
        }
    }

    fn script(self) -> String {
        let k = crash::key_name;
        match self {
            Step::Tap(x) => format!("press {}", k(x)),
            Step::Hold(x) => format!("hold {}", k(x)),
            Step::Knob(n) => format!("knob {n}"),
            Step::Down(p) => format!("down {}; wait 200ms", ["PTT", "PTTB"][p as usize]),
            Step::Up(p) => format!("up {}; wait 100ms", ["PTT", "PTTB"][p as usize]),
            Step::Refuse(w) => format!("(the Radio task refuses: {w:?}); wait 100ms"),
            Step::Event(e) => format!("event {e:?}"),
            Step::Carriers(m) => format!("carrier_on = {m:#05b}; wait 200ms"),
            Step::Jump(i) => format!("jump {i} ({}); wait 300ms", jump_names().nth(i).unwrap_or_default()),
            Step::Term(l) => format!("term {l}"),
            Step::Clock(m) => format!("clock +{m}"),
            Step::Wait(ms) => format!("wait {ms}ms"),
        }
    }
}

/// A session's steps: drawn before it runs, so a failing one can be replayed and shrunk as data.
fn steps(seed: u64, n: usize) -> Vec<Step> {
    let mut g = Rng(seed.wrapping_mul(0x2545_F491_4F6C_DD1D) ^ 0xFE_221E);
    let screens = jump_names().count();
    let mut held = [false; 2];
    let mut v = Vec::with_capacity(n);
    while v.len() < n {
        let s = match g.below(100) {
            0..=34 => Step::Tap(g.pick(&KEYS)),
            35..=42 => Step::Hold(g.pick(&KEYS)),
            43..=51 => Step::Knob([-3, -1, -1, 1, 1, 3][g.below(6)]),
            52..=58 => {
                let p = (g.below(4) == 0) as u8; // B a quarter of the time
                held[p as usize] = !held[p as usize];
                if held[p as usize] {
                    Step::Down(p)
                } else {
                    Step::Up(p)
                }
            }
            59..=61 => Step::Refuse(g.pick(&REFUSALS)),
            62..=69 => Step::Event(g.pick(&EVENTS)),
            70..=73 => Step::Carriers((g.below(8) as u8) & if g.below(2) == 0 { 0b111 } else { 0b001 }),
            74..=78 => Step::Jump(g.below(screens)),
            79..=82 => Step::Term(g.pick(TERM)),
            83 => Step::Clock([30, 64, 90, 240][g.below(4)]),
            _ => Step::Wait([100, 300, 700, 1500, 4000][g.below(5)]),
        };
        v.push(s);
    }
    v
}

/// Runs the steps on a new radio; the first failure as (its step, the error).
fn session(steps: &[Step], show: bool, repaint_all: bool) -> Option<(usize, String)> {
    let mut r = Radio::new();
    for (i, &s) in steps.iter().enumerate() {
        if show {
            eprintln!("{i:4} {}", s.script());
        }
        let repaint = repaint_all || i % 8 == 7;
        r.sim.check_repaint(repaint);
        let res = attempt(&mut r, |r| s.run(r)).and_then(|_| check(&mut r));
        r.sim.check_repaint(false);
        if let Err(e) = res {
            let _ = ferrite_ui::clipcheck::take();
            return Some((i, e));
        }
    }
    let _ = ferrite_ui::clipcheck::take();
    None
}

/// A failure's kind, to keep it while shrinking: its first line, numbers left out.
fn kind(e: &str) -> String {
    e.lines().next().unwrap_or("").chars().filter(|c| !c.is_ascii_digit()).collect()
}

/// The steps cut down while the same kind of failure still comes (dropping halves, then quarters, ... then single
/// steps), with a bounded number of replays.
fn shrink(mut steps: Vec<Step>, want: &str) -> Vec<Step> {
    let quiet = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut budget = 120;
    let mut chunk = steps.len() / 2;
    while chunk >= 1 && budget > 0 {
        let mut at = 0;
        while at < steps.len() && budget > 0 {
            let mut t = steps.clone();
            t.drain(at..(at + chunk).min(t.len()));
            budget -= 1;
            match session(&t, false, true) {
                Some((i, e)) if kind(&e) == want => {
                    t.truncate(i + 1);
                    steps = t;
                }
                _ => at += chunk,
            }
        }
        chunk /= 2;
    }
    std::panic::set_hook(quiet);
    steps
}

/// Seeds whose sessions found a bug not fixed yet: left out of the runs above (the bug would end the session before
/// it reached anything else) and kept as the `found_*` tests below, ignored until fixed. Shrink this list.
const FOUND: &[u64] = &[];

fn monkey(first: u64, count: u64) {
    let only: Option<u64> = std::env::var("FERRITE_MONKEY_SEED").ok().map(|s| s.parse().expect("a seed number"));
    let n = count * crash::scale() as u64;
    let seeds: Vec<u64> = match only {
        Some(s) if s % 1000 == first => vec![s],
        Some(_) => vec![],
        None => (0..).map(|k| first + k * 1000).filter(|s| !FOUND.contains(s)).take(n as usize).collect(),
    };
    let mut failures = Vec::new();
    for seed in seeds {
        let all = steps(seed, STEPS);
        if let Some((i, e)) = session(&all, only.is_some(), false) {
            // shrunk with every frame's repaint checked, so a repaint miss doesn't hide behind the sampling
            let short = match session(&all[..=i], false, true) {
                Some((j, e2)) if kind(&e2) == kind(&e) => shrink(all[..=j].to_vec(), &kind(&e)),
                _ => all[..=i].to_vec(),
            };
            let script: Vec<String> = short.iter().map(|s| s.script()).collect();
            failures.push(format!(
                "seed {seed}, step {i}: {e}\n  shrunk to {} steps (a new sample radio, then):\n    {}\n  replay the whole session: FERRITE_MONKEY_SEED={seed}",
                short.len(),
                script.join("\n    ")
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

// Four sessions a test (seeds first, first + 1000, ...), so the runner spreads them over its threads.
#[test]
fn monkey_1() {
    monkey(1, 4);
}
#[test]
fn monkey_2() {
    monkey(2, 4);
}
#[test]
fn monkey_3() {
    monkey(3, 4);
}
#[test]
fn monkey_4() {
    monkey(4, 4);
}

/// A shrunk session found by the monkey, replayed with every frame's repaint checked.
fn found(steps: &[Step]) {
    if let Some((i, e)) = session(steps, true, true) {
        panic!("step {i}: {e}");
    }
}

use Step::*;

#[test]
fn found_home_stale_after_battery_low() {
    found(&[Tap(Key::Hash), Knob(3), Tap(Key::Left), Event(ferrite_sim::simcore::Event::Battery), Wait(100), Wait(4000)]);
}

#[test]
fn found_transmit_header_after_fm_retune() {
    use ferrite_sim::simcore::Event as E;
    let d = |n| Tap(Key::Digit(n));
    found(&[Event(E::PowerSwitch), Wait(4000), Event(E::PowerSwitch), Wait(300), d(6), d(4), d(3), Carriers(0b100), Tap(Key::Hash), Tap(Key::Exit), Knob(-1), Tap(Key::VfoMem), Down(1), Event(E::FmStation)]);
}

#[test]
fn found_tx_refused_dialog_after_fm_retune() {
    use ferrite_sim::simcore::Event as E;
    found(&[Event(E::PowerSwitch), Hold(Key::Digit(1)), Knob(3), Event(E::PowerSwitch), Event(E::FmStation), Down(0), Event(E::FmStation)]);
}

#[test]
fn found_home_card_mark_after_crossband() {
    use ferrite_sim::simcore::Event as E;
    let k = Key::Digit;
    found(&[
        Down(1), Event(E::Alert), Jump(45), Tap(k(1)), Tap(Key::Side(2)), Hold(k(3)), Wait(4000), Carriers(0b001),
        Term("set 0 7 2 Watch"), Event(E::Battery), Tap(Key::Slot), Refuse(Refusal::NotCalibrated), Wait(1500),
        Term("slot C 162.55"), Up(0), Tap(Key::Star), Event(E::Crossband), Tap(Key::Exit), Hold(Key::Menu), Tap(k(4)),
        Tap(Key::Down), Knob(-1), Carriers(0b101), Wait(1500), Wait(300), Wait(100), Hold(Key::Exit), Wait(4000),
    ]);
}
