//! The end-to-end harness: a radio in the simulator driven the way a person drives it (keys, the knob, the
//! world's events, time passing), read back the way a person reads it (the screen's name and text, the LEDs),
//! plus the App's state, the commands it sent the radio and the store.
//!
//! A script is a few steps separated by `;` or new lines:
//!
//! ```text
//! press OK; hold 1; wait 2s; expect screen "Band scope"; expect text "146.520"
//! ```
//!
//! Steps: `press K` (a short press), `hold K` (held 0.8 s: the long press), `down K` / `up K`, `type 14652`
//! (digits, `*` and `#`, each a short press), `knob N` (N steps, negative turns down), `wait 2s` / `wait 300ms`,
//! `event NAME` (message, alert, gps, bluetooth, battery, power, crossband, layout, beacon, led, fm), `expect screen
//! "Name"`, `expect text "s"`, `expect no text "s"`, `home` (to the home screen). Keys: OK (MENU), BACK, UP,
//! DOWN, LEFT, RIGHT, SLOT, VM, STAR, HASH, 0-9, SIDE1, SIDE2, PTT (PTT A), PTTB.
#![allow(dead_code)]

use ferrite_core::keys::Key;
use ferrite_sim::simcore::{Event, SimCore, Stored};

/// DISP-31's misses known and logged (docs/fix-batch.md), each with what it is: a miss on that screen inside rows y0..y1 is left out,
/// anything else on it still fails. Shrink this list; never widen an entry to make a test pass.
pub const REPAINT_KNOWN: &[(&str, u16, u16, &str)] = &[(
    "C4 APRS station",
    296,
    319,
    "a station with a two-line comment and a two-line message runs past the hint bar: the page doesn't scroll",
)];

/// The step the harness advances time by, as a frame (ms).
pub const FRAME: u32 = 20;

/// A frame hook: the video recorder's.
pub type Recorder = Box<dyn FnMut(&SimCore)>;

pub struct Radio {
    pub sim: SimCore,
    pub t: u32,
    /// Called with the simulator after every frame (the video recorder, `tools/promo`).
    pub rec: Option<Recorder>,
}

impl Radio {
    /// The sample radio, switched on and past the boot screen, with the commands it sends logged.
    pub fn new() -> Self {
        Self::with(Stored::Sample)
    }

    /// A radio just installed: blank flash, so setup runs (BOOT-12).
    pub fn blank() -> Self {
        Self::with(Stored::Blank)
    }

    pub fn with(stored: Stored) -> Self {
        // Pictures and screen text don't change with each commit (SELF-5's build ID reads 0000000).
        ferrite_core::BUILD_FIXED.store(true, core::sync::atomic::Ordering::Relaxed);
        let mut sim = SimCore::with_flash(1, false, stored);
        sim.set_clock(1_791_400_000); // a fixed clock: Oct 8, 2026, 19:06 UTC
        sim.log_cmds(true);
        let mut r = Radio { sim, t: 0, rec: None };
        #[cfg(test)] // the tests, not the video recorder (tools/promo), which builds this file without clipcheck
        ferrite_ui::clipcheck::start(); // text cut off fails the test (Drop below)
        r.wait(3000);
        r
    }

    /// Time passes, a frame at a time.
    pub fn wait(&mut self, ms: u32) -> &mut Self {
        let end = self.t + ms;
        while self.t < end {
            self.t += FRAME;
            #[cfg(test)]
            ferrite_ui::clipcheck::context(self.sim.screen_name());
            self.sim.step(self.t);
            if let Some(rec) = &mut self.rec {
                rec(&self.sim);
            }
        }
        self
    }

    pub fn down(&mut self, k: Key) -> &mut Self {
        self.sim.key(k, true);
        self.wait(FRAME)
    }

    pub fn up(&mut self, k: Key) -> &mut Self {
        self.sim.key(k, false);
        self.wait(FRAME)
    }

    /// A short press: down for 100 ms.
    pub fn press(&mut self, k: Key) -> &mut Self {
        self.sim.key(k, true);
        self.wait(100);
        self.sim.key(k, false);
        self.wait(100)
    }

    /// A long press: held 800 ms (the long press fires at 700 ms, KEY-3).
    pub fn hold(&mut self, k: Key) -> &mut Self {
        self.sim.key(k, true);
        self.wait(800);
        self.sim.key(k, false);
        self.wait(100)
    }

    /// Digits, `*`, `#` and `.` (= `*`), each a short press.
    pub fn type_(&mut self, s: &str) -> &mut Self {
        for c in s.chars() {
            let k = match c {
                '0'..='9' => Key::Digit(c as u8 - b'0'),
                '*' | '.' => Key::Star,
                '#' => Key::Hash,
                _ => panic!("type: no key for {c:?}"),
            };
            self.press(k);
        }
        self
    }

    pub fn knob(&mut self, n: i32) -> &mut Self {
        for _ in 0..n.unsigned_abs() {
            self.sim.knob(n.signum() as i8);
            self.wait(60);
        }
        self.wait(100)
    }

    pub fn event(&mut self, e: Event) -> &mut Self {
        self.sim.event(e);
        self.wait(200)
    }

    /// Hold BACK: home from anywhere (KEY-9).
    pub fn home(&mut self) -> &mut Self {
        self.hold(Key::Exit)
    }

    /// A number path from home, as the manual writes it: `menu("0 5 3")` is MENU (quick settings), 0
    /// (advanced), 5, 3.
    pub fn menu(&mut self, path: &str) -> &mut Self {
        self.home();
        self.press(Key::Menu);
        for d in path.split_whitespace() {
            self.type_(d);
        }
        self
    }

    pub fn screen(&self) -> &'static str {
        self.sim.screen_name()
    }

    pub fn text(&mut self) -> String {
        self.sim.screen_text()
    }

    pub fn has(&mut self, s: &str) -> bool {
        self.text().contains(s)
    }

    #[track_caller]
    pub fn expect_screen(&mut self, name: &str) -> &mut Self {
        assert_eq!(self.screen(), name, "screen; on it:\n{}", self.text());
        self
    }

    #[track_caller]
    pub fn expect_text(&mut self, s: &str) -> &mut Self {
        assert!(self.has(s), "{s:?} not on {} ; on it:\n{}", self.screen(), self.text());
        self
    }

    #[track_caller]
    pub fn expect_no_text(&mut self, s: &str) -> &mut Self {
        assert!(!self.has(s), "{s:?} on {} ; on it:\n{}", self.screen(), self.text());
        self
    }

    /// The commands sent to the radio since the last call.
    pub fn take_cmds(&mut self) -> Vec<ferrite_core::msg::RadioCmd> {
        core::mem::take(&mut self.sim.cmds)
    }

    /// Runs a script (see the module comment).
    #[track_caller]
    pub fn run(&mut self, script: &str) -> &mut Self {
        for step in script.split([';', '\n']).map(str::trim).filter(|s| !s.is_empty()) {
            let (cmd, arg) = step.split_once(' ').unwrap_or((step, ""));
            let arg = arg.trim();
            match cmd {
                "press" => {
                    self.press(key(arg));
                }
                "hold" => {
                    self.hold(key(arg));
                }
                "down" => {
                    self.down(key(arg));
                }
                "up" => {
                    self.up(key(arg));
                }
                "type" => {
                    self.type_(arg);
                }
                "knob" => {
                    self.knob(arg.parse().expect("knob N"));
                }
                "wait" => {
                    self.wait(ms(arg));
                }
                "event" => {
                    self.event(event(arg));
                }
                "home" => {
                    self.home();
                }
                "menu" => {
                    self.menu(arg);
                }
                "expect" => {
                    let (what, rest) = arg.split_once(' ').expect("expect what");
                    let q = quoted(rest.trim_start_matches("text").trim());
                    match what {
                        "screen" => self.expect_screen(&quoted(rest)),
                        "text" => self.expect_text(&quoted(rest)),
                        "no" => self.expect_no_text(&q),
                        _ => panic!("expect {what}?"),
                    };
                }
                _ => panic!("unknown step {step:?}"),
            }
        }
        self
    }
}

/// No text the radio drew was cut off without a [`ferrite_ui::widgets::fit`] asking for it (ferrite-ui's
/// `clipcheck`): every journey is also a check of the screens it passes through.
#[cfg(test)]
impl Drop for Radio {
    fn drop(&mut self) {
        let clips = ferrite_ui::clipcheck::take();
        if !clips.is_empty() && !std::thread::panicking() {
            panic!("text cut off (shorten it, widen its field, or w::fit it when the cut is meant):\n{}", ferrite_ui::clipcheck::report(&clips));
        }
    }
}

fn quoted(s: &str) -> String {
    s.trim().trim_matches('"').to_string()
}

fn ms(s: &str) -> u32 {
    if let Some(v) = s.strip_suffix("ms") {
        v.parse().unwrap()
    } else if let Some(v) = s.strip_suffix('s') {
        (v.parse::<f32>().unwrap() * 1000.0) as u32
    } else {
        s.parse().unwrap()
    }
}

pub fn key(s: &str) -> Key {
    match s.to_ascii_uppercase().as_str() {
        "OK" | "MENU" => Key::Menu,
        "BACK" | "EXIT" => Key::Exit,
        "UP" => Key::Up,
        "DOWN" => Key::Down,
        "LEFT" => Key::Left,
        "RIGHT" => Key::Right,
        "SLOT" => Key::Slot,
        "VM" | "V/M" => Key::VfoMem,
        "STAR" | "*" => Key::Star,
        "HASH" | "#" => Key::Hash,
        "SIDE1" => Key::Side(1),
        "SIDE2" => Key::Side(2),
        "PTT" | "PTTA" => Key::Ptt(ferrite_core::keys::PTT_A),
        "PTTB" => Key::Ptt(ferrite_core::keys::PTT_B),
        d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() => Key::Digit(d.as_bytes()[0] - b'0'),
        _ => panic!("no key {s:?}"),
    }
}

fn event(s: &str) -> Event {
    match s.to_ascii_lowercase().as_str() {
        "message" => Event::Message,
        "alert" => Event::Alert,
        "gps" => Event::Gps,
        "bluetooth" => Event::Bluetooth,
        "battery" => Event::Battery,
        "power" => Event::PowerSwitch,
        "crossband" => Event::Crossband,
        "layout" => Event::Layout,
        "beacon" => Event::Beacon,
        "led" => Event::ScreenOff,
        "linkwrite" => Event::LinkWrite,
        "empty" => Event::EmptyBattery,
        "ssb" => Event::SsbPatch,
        "fm" => Event::FmStation,
        "spkmic" => Event::SpeakerMic,
        "tonehint" => Event::ToneHint,
        _ => panic!("no event {s:?}"),
    }
}
