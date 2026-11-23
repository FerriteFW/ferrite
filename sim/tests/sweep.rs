//! Crash sweep: every screen of the jump list × every input (each key tapped and held, the knob both ways, PTT A
//! and B, a second PTT while keyed, every refusal the Radio task can give) × odd states (keyed, a slot off, APRS
//! holding C, Low Power Mode, a scan stopped on a hit, broadcast FM, monitor latched, the layouts and themes, the
//! link writing, ...), and the places that are a state of their own (the satellite pass page with the pass up,
//! the crossband repeater, HF mode keyed, the PIN entry), every input there. After each input: no panic, the
//! readback fits Live View, the mode settled, no text cut off (`crash::check`). Decision 438's fault (a second PTT
//! on the satellite pass page drew the TX refused screen for slot 4 and indexed the slots A-C) fails
//! `sweep_satellite_pass_up` when its fix is taken out.
//!
//! The places take every input, then every input again on each of the first four screens an input led to (key-ups
//! first); the plain state every key-up and refusal on every screen; the other states an eighth of the inputs on
//! each screen, a different eighth on the next (`FERRITE_FUZZ_SCALE=8` or more: all of them, and more places' pairs). One radio a state, the inputs one
//! after another on it (the state set up again whenever an input undid it). A failure names the state, the screen
//! and the input, and whether a new radio given only that input fails the same way.

mod common;
mod crash;

use common::Radio;
use crash::{all_acts, apply, attempt, check, fresh, wake, Act, State};
use ferrite_sim::simcore::jump_names;

/// Inputs on a screen: the radio set up for the state (again, if an input before undid it, or for a place if it
/// left it), then the inputs, then the checks.
fn case(r: &mut Radio, s: &State, screen: Option<usize>, acts: &[Act], place_screen: &str, kept: &str) -> Result<(), String> {
    attempt(r, |r| {
        wake(r);
        let undone = s.keeps.is_some_and(|k| k(r) != kept);
        let off_place = screen.is_none() && crash::place_of(r) != place_screen;
        if undone || off_place {
            r.home();
            apply(r, s.setup);
            if screen.is_none() && crash::place_of(r) != place_screen {
                // the inputs before changed what the setup leads to (a list's filter, a pass chosen): a new radio
                let _ = ferrite_ui::clipcheck::take();
                *r = fresh(s).unwrap_or_else(|e| panic!("{e}"));
            }
        }
        if let Some(i) = screen {
            r.sim.jump(i);
            r.wait(300);
        }
        apply(r, s.pre);
        for a in acts {
            a.run(r);
        }
        apply(r, s.post);
    })?;
    check(r)
}

/// The script that replays a case on a new radio, for the report.
fn replay(s: &State, screen: Option<usize>, acts: &[Act]) -> String {
    let jump = screen.map(|i| format!("r.sim.jump({i}) ({}); wait 300ms; ", jump_names().nth(i).unwrap_or_default())).unwrap_or_default();
    let acts: Vec<String> = acts.iter().map(|a| a.name()).collect();
    format!("Radio::new(); home; {}; {jump}{}; {}; {}", s.setup, s.pre, acts.join("; "), s.post)
}

struct Sweep {
    s: State,
    r: Radio,
    place_screen: String,
    kept: String,
    cases: usize,
    failures: Vec<String>,
    seen: Vec<String>,
}

impl Sweep {
    fn new(s: State) -> Self {
        let r = fresh(&s).unwrap_or_else(|e| panic!("{e}"));
        let place_screen = crash::place_of(&r);
        let kept = s.keeps.map(|k| k(&r)).unwrap_or_default();
        Sweep { s, r, place_screen, kept, cases: 0, failures: Vec::new(), seen: Vec::new() }
    }

    /// One case; a failure is noted (the first of each kind, with whether a new radio given only these inputs fails
    /// too) and the radio started again.
    fn run(&mut self, screen: Option<usize>, acts: &[Act]) -> bool {
        self.cases += 1;
        let Err(e) = case(&mut self.r, &self.s, screen, acts, &self.place_screen, &self.kept) else { return true };
        let key = e.lines().next().unwrap_or("").split(" at ").take(2).collect::<Vec<_>>().join(" at ");
        if !self.seen.contains(&key) && self.failures.len() < 20 {
            self.seen.push(key);
            let alone = fresh(&self.s).and_then(|mut n| case(&mut n, &self.s, screen, acts, &self.place_screen, &self.kept)).err();
            let how = if alone.is_some() { "a new radio given only these inputs fails too" } else { "only after the inputs before it on the same radio (the whole state's run replays it)" };
            let at = screen.map_or("its own screen".into(), |i| format!("screen {i} {}", jump_names().nth(i).unwrap()));
            let names: Vec<String> = acts.iter().map(|a| a.name()).collect();
            self.failures.push(format!("state \"{}\", {at}, {}: {e}\n    replay: {}\n    {how}", self.s.name, names.join(", "), replay(&self.s, screen, acts)));
        }
        let _ = ferrite_ui::clipcheck::take();
        self.r = fresh(&self.s).unwrap_or_else(|e| panic!("{e}"));
        false
    }
}

fn sweep(name: &str) {
    let states = crash::states();
    let si = states.iter().position(|x| x.name == name).expect("a state");
    let s = states[si];
    let scale = crash::scale() as usize;
    let acts = all_acts();
    let mut w = Sweep::new(s);
    if s.place {
        // Every input on the place; then every input again on each screen an input led to (the TX refused screen
        // a refused satellite key-up opens, the transmit screen, ...): decision 438 was PTT on that refused screen.
        let mut led: Vec<(String, Act)> = Vec::new();
        for &a in &acts {
            if w.run(None, &[a]) {
                let now = crash::place_of(&w.r);
                if now != w.place_screen && !led.iter().any(|(n, _)| *n == now) {
                    led.push((now, a));
                }
            }
        }
        // the screens a key-up led to first (they hold the transmit state), then the keys'
        led.sort_by_key(|(_, a)| a.is_key());
        for (_, first) in led.into_iter().take(4 * scale) {
            for &a in &acts {
                w.run(None, &[first, a]);
            }
        }
    } else {
        for screen in 0..jump_names().count() {
            for (ai, &a) in acts.iter().enumerate() {
                // an eighth of the inputs a screen, a different eighth on the next (FERRITE_FUZZ_SCALE=8: all); the
                // plain state every key-up and refusal (KEY-26's record has its keys)
                let all = si == 0 && !a.is_key();
                if !all && (ai + screen * 5 + si) % 8 >= scale {
                    continue;
                }
                w.run(Some(screen), &[a]);
            }
        }
    }
    let _ = ferrite_ui::clipcheck::take();
    assert!(w.failures.is_empty(), "{} kinds of failure in {} cases in state \"{}\" (the first of each):\n{}", w.failures.len(), w.cases, s.name, w.failures.join("\n"));
}

macro_rules! sweeps {
    ($($test:ident => $state:literal,)*) => {
        $(#[test] fn $test() { sweep($state); })*
        /// Every state has its test.
        #[test]
        fn every_state_swept() {
            let tested = [$($state),*];
            for s in crash::states() {
                assert!(tested.contains(&s.name), "no sweep test for state \"{}\": add one", s.name);
            }
        }
    };
}

sweeps! {
    sweep_plain => "plain",
    sweep_keyed_voice => "keyed (voice)",
    sweep_keyed_beacon => "keyed (beacon)",
    sweep_slot_a_off => "slot A off",
    sweep_slot_b_off_active => "slot B off and active",
    sweep_aprs_tracker => "APRS Tracker",
    sweep_aprs_on => "APRS mode on",
    sweep_aprs_kiss => "APRS KISS",
    sweep_low_power => "Low Power Mode",
    sweep_band_scan_hit => "band scan stopped on a hit",
    sweep_channel_scan_hit => "channel scan stopped on a hit",
    sweep_preset_scan_hit => "range preset scan stopped on a hit",
    sweep_broadcast_fm => "broadcast FM playing",
    sweep_monitor => "monitor latched",
    sweep_layout_scope => "layout: single + scope",
    sweep_layout_activity => "layout: watch + activity",
    sweep_theme_daylight => "theme: Daylight",
    sweep_theme_night_red => "theme: Night red",
    sweep_battery_cutoff => "battery cut-off",
    sweep_keys_locked => "keys locked",
    sweep_link_writing => "link writing",
    sweep_satellite_pass_up => "satellite pass up",
    sweep_satellite_before_rise => "satellite pass before it rises",
    sweep_crossband => "crossband repeater running",
    sweep_crossband_keyed => "crossband repeater running, keyed",
    sweep_hf_rx_only => "HF mode, receive only (AM on the SI4732)",
    sweep_hf_keyed => "HF mode keyed (29.6 MHz FM)",
    sweep_pin_entry => "PIN entry at power-on",
}
