//! HF mode (chapter 20a): the SSB patch's load, the modes it brings, typing a frequency, the options page, chip
//! 1's range, and the readback.

use super::*;
use crate::screens::hf::{self, Hf};
use ferrite_core::freq::mhz;
use ferrite_core::msg::ModeLive;

fn hf_live(t: &mut T, loading: u8) {
    let f = t.app.set.hf.freq;
    t.live.mode = ModeLive::Hf { freq: f, rssi: -80, snr: 12, si4732: true, loading };
}

/// HF mode open, with the SSB patch (loaded unless `loading` < 100).
fn open(loading: u8) -> T {
    let mut t = T::ready();
    t.dev.selftest.ssb_patch = true;
    t.draw();
    hf_live(&mut t, loading);
    t.open(Dest::HfMode);
    t.wait(100);
    assert_eq!(t.name(), "HF mode");
    t
}

/// HF-3, HF-19: the SSB patch streams in behind a bar; the knob waits; BACK cancels, one level back.
#[test]
fn patch_loading() {
    let mut t = open(40);
    assert!(matches!(t.screen(), Screen::Hf(Hf { loaded: false, .. })));
    t.shows("Loading SSB patch");
    t.shows("40 %");
    t.reads("hints \"BACK\" \"Cancel\"");
    t.reads("hf");
    let f = t.app.set.hf.freq;
    t.knob(1);
    t.tap(Key::Up);
    assert_eq!(t.app.set.hf.freq, f, "nothing until it has loaded");
    t.tap(Key::Exit);
    assert_eq!(t.screen(), Screen::Home);
}

/// HF-20: with the patch, * goes through AM, USB, LSB, CW (FM only from 18 MHz); SSB shows its BFO, steps and
/// filters.
#[test]
fn ssb_modes() {
    let mut t = open(255);
    assert_eq!(t.screen().mockup(), Some("HfMode.png"));
    t.reads("title=\"HF\"");
    t.shows("SI4732");
    t.shows("SNR 12");
    // HF-12 (decision 349): SIDE2 steps the mode, its row shown a moment
    for (m, step) in [("USB", "100 Hz"), ("LSB", "100 Hz"), ("CW", "100 Hz")] {
        t.tap(Key::Side(2));
        assert_eq!(hf::mode_name(t.app.set.hf.mode), m);
        t.shows(m);
        t.shows("MODE   SIDE2 next");
        t.shows(step);
        t.shows("3 kHz");
        t.reads(m);
    }
    t.tap(Key::Side(2));
    assert_eq!(hf::mode_name(t.app.set.hf.mode), "AM", "no FM below 18 MHz");
}

/// VFO-24, HF-12: digits type a frequency in kHz, * puts a point (MHz), * again after a digit past it enters;
/// out of range says so.
#[test]
fn typing_a_frequency() {
    let mut t = open(255);
    for d in [7, 0, 7, 4] {
        t.tap(Key::Digit(d));
    }
    t.shows("Go to 7074 kHz");
    t.reads("hints \"*\" \"Point\" \"OK\" \"Go\" \"BACK\" \"Cancel\"");
    t.tap(Key::Menu);
    assert_eq!(t.app.set.hf.freq, 707_400);
    t.tap(Key::Digit(1));
    t.tap(Key::Digit(4));
    t.tap(Key::Star);
    t.tap(Key::Digit(2));
    t.tap(Key::Digit(5));
    t.shows("Go to 14.25 MHz");
    t.reads("hints \"*/OK\" \"Go\" \"BACK\" \"Cancel\"");
    t.tap(Key::Star);
    assert_eq!(t.app.set.hf.freq, mhz(14) + 25_000);
    t.tap(Key::Digit(9));
    t.tap(Key::Digit(9));
    t.tap(Key::Star);
    t.tap(Key::Menu); // 99. : out of range
    assert_eq!(t.toast(), "HF mode / 0.15 to 64 MHz");
    t.tap(Key::Digit(3));
    t.tap(Key::Left); // cancels
    assert!(matches!(t.screen(), Screen::Hf(Hf { n: 0, .. })));
}

/// HF-14: the options page: step, bandwidth and AGC turn with \u{25c0}\u{25b6} or OK; the recent list opens with OK
/// and # clears it; 0 is the full menu.
#[test]
fn options_page() {
    let mut t = open(255);
    t.hold(Key::Menu);
    assert!(matches!(t.screen(), Screen::Hf(Hf { menu: true, row: 0, .. })));
    t.shows("Full menu");
    t.reads("hints \"Knob\" \"Change\" \"0\" \"Full menu\" \"BACK\" \"Back\"");
    let h0 = t.app.set.hf;
    t.tap(Key::Menu); // the step, one on
    assert_eq!(t.app.set.hf.step, (h0.step + 1) % 4);
    t.tap(Key::Down);
    t.tap(Key::Right);
    assert_eq!(t.app.set.hf.bw, (h0.bw + 1) % 4);
    t.shows("4 kHz");
    t.tap(Key::Digit(3));
    t.knob(1);
    assert_eq!(t.app.set.hf.agc, (h0.agc + 1) % (crate::settings::AGC_MAX + 1));
    t.tap(Key::Up);
    t.tap(Key::Up);
    t.tap(Key::Up); // wraps to the recent list
    assert!(matches!(t.screen(), Screen::Hf(Hf { row: 3, .. })));
    t.tap(Key::Right); // nothing to turn
    t.app.set.hf.recent[0] = 707_400 | 3 << 28;
    t.tap(Key::Hash);
    assert!(matches!(t.screen(), Screen::Confirm(_)), "HF-28.1: asked first");
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "Recent list / cleared");
    assert!(hf::recent(&t.app.set.hf).next().is_none());
    t.tap(Key::Menu);
    assert_eq!(t.name(), Dest::HfRecent.name());
    t.tap(Key::Exit);
    t.hold(Key::Menu);
    t.tap(Key::Digit(0));
    assert_eq!(t.name(), Dest::Advanced.name());
}

/// HF-9, HF-10: from 18 MHz in FM, chip 1 receives: its tag, an S-meter; PTT transmits there; the bandwidth is
/// wide or narrow.
#[test]
fn chip1_range() {
    let mut t = open(255);
    t.app.set.hf.freq = mhz(29) + 60_000;
    t.app.set.hf.mode = hf::FM;
    t.app.set.hf.bw = 0;
    t.wait(100);
    t.shows("chip 1"); // on the step line (HF-15, decision 349)
    t.shows("Wide");
    t.reads("CHIP 1");
    t.hold(Key::Menu);
    t.tap(Key::Down);
    t.tap(Key::Right);
    t.tap(Key::Right); // two bandwidths: back to wide
    assert_eq!(t.app.set.hf.bw, 0);
}

/// HF-10.1 (decision 446): on 15 m, SIDE2 reaches USB on the SI4732 past 18 MHz; going into it says it's an
/// experiment, and the receiver shows as one; PTT refuses there (receive only); past 30 MHz USB isn't offered.
#[test]
fn ssb_past_18_mhz_is_the_si4732_experiment() {
    let mut t = open(255);
    t.app.set.hf.freq = 2_107_400;
    t.app.set.hf.mode = hf::AM;
    t.wait(100);
    t.reads("CHIP 1");
    for _ in 0..4 {
        if t.app.set.hf.mode == hf::USB {
            break;
        }
        t.tap(Key::Side(2));
    }
    assert_eq!(t.app.set.hf.mode, hf::USB, "SIDE2 offers USB on 15 m");
    assert_eq!(t.toast(), "Experimental / please report");
    t.wait(100);
    t.shows("SI4732 exp");
    t.reads("SI4732 EXP");
    // 1 MHz steps up (SIDE2 held, the knob) to 30 MHz: USB isn't offered there, chip 1 takes it in FM
    t.app.key_down(Key::Side(2), t.t, &t.s);
    t.knob(9);
    t.app.key_up(Key::Side(2), t.t, &t.s);
    assert!(t.app.set.hf.freq >= mhz(30));
    assert_eq!(t.app.set.hf.mode, hf::FM);
    t.wait(100);
    t.reads("CHIP 1");
}

/// NAV-3: SIDE2 held, the knob steps 1 MHz in HF mode too.
#[test]
fn side2_knob_big_step() {
    let mut t = open(255);
    let f = t.app.set.hf.freq;
    t.app.key_down(Key::Side(2), t.t, &t.s);
    t.knob(1);
    t.app.key_up(Key::Side(2), t.t, &t.s);
    assert_eq!(t.app.set.hf.freq, f + 100_000);
}

/// HF-32: hold SIDE1 outside any band in USB goes to the amateur band below.
#[test]
fn band_down_from_outside_the_bands() {
    let mut t = open(255);
    t.tap(Key::Side(2)); // USB
    t.app.set.hf.freq = mhz(16);
    t.hold(Key::Side(1));
    assert_eq!(t.app.set.hf.freq, 1_407_400, "20 m's start");
}

/// CHAN-26: an HF channel recalled while in HF mode tunes there and says so.
#[test]
fn recall_in_hf_mode() {
    let mut t = open(255);
    let c = crate::chan::channel(40, 0, 707_400, "FT8 40", crate::slot::DEFAULT_CFG);
    hf::recall(&mut t.app, &c);
    assert_eq!(t.app.set.hf.freq, 707_400);
    assert_eq!(t.name(), "HF mode");
    assert!(t.toast().starts_with("HF mode / "), "{}", t.toast());
}
