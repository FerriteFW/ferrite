//! Satellite mode on the App (chapter 28a): the pass page's radio commands while working a pass (SAT-8, SAT-10,
//! SAT-11), the alarm's toast and the status bar's mark (SAT-12, SAT-20), with the sample satellites predicted on
//! the host as on the radio.

use super::*;
use crate::screens::sat::SatScr;
use ferrite_core::sat::data::{Live, SatData};
use ferrite_core::sat::look::Observer;
use ferrite_core::sat::pass::Pass;

/// Ottawa, as the simulator's fix.
const HERE: (i32, i32, i32) = (454_000_000, -757_000_000, 70);

/// A pass of satellite `sat` (the sample's index) over 30°, from the start of the sample's day.
fn a_pass(sat: u8) -> Pass {
    let mut d = Live::new(crate::sat::SimSats::new(crate::sat::sample()));
    d.tick(Some(crate::sat::SIM_T0), Some(Observer::new(HERE.0, HERE.1, HERE.2)), true, u32::MAX);
    (0..d.passes()).filter_map(|i| d.pass(i)).find(|p| p.sat == sat && p.max_el > 30).expect("a high pass")
}

/// An App with the sample satellites, its clock at `utc`, transmit built and allowed anywhere.
fn sat_app(utc: u32) -> T {
    let mut t = T::new();
    t.app.set.backlight = 0;
    t.app.set.extend_tx = true;
    t.app.radio.tx_built = true;
    t.app.sat_attach(std::boxed::Box::leak(std::boxed::Box::new(Live::new(crate::sat::SimSats::new(crate::sat::sample())))));
    t.app.sat_clock(Some(utc), true, Some(HERE));
    t
}

fn sat_cmds(t: &mut T) -> Vec<RadioCmd> {
    t.cmds().into_iter().filter(|c| matches!(c, RadioCmd::SatRx(_) | RadioCmd::SatUp(_) | RadioCmd::SatTone { .. } | RadioCmd::TxPlan { .. } | RadioCmd::Transmit { .. })).collect()
}

/// SAT-8, SAT-10, SAT-11: working SO-50 overhead: the downlink corrected and retuned, the uplink corrected at the
/// key-up; the first key-up of the pass sends the arming tone (74.4 Hz) for 2 s, then the access tone (67.0 Hz);
/// the next key-up the access tone only; * switches half duplex, even keyed; the trim moves the downlink.
#[test]
fn working_so50_overhead() {
    let p = a_pass(1);
    let mut t = sat_app(p.aos + 120);
    t.open(crate::screens::nav::Dest::Satellites);
    t.wait(3000); // predicted
    t.app.sat.cur = Some(p);
    t.app.screen = Screen::Sat(SatScr::pass_of(&p));
    t.wait(1100);
    let c = sat_cmds(&mut t);
    let down = c.iter().rev().find_map(|c| if let RadioCmd::SatRx(f) = c { Some(*f) } else { None }).expect("the downlink tuned");
    assert!(down.abs_diff(43_679_500) < 1_200 && down != 43_679_500, "436.795 corrected by Doppler: {down}");
    let s = store();
    t.app.key_down(Key::Ptt(ferrite_core::keys::PTT_A), t.t, &s);
    let c = sat_cmds(&mut t);
    assert!(c.contains(&RadioCmd::SatTone { tone: 744, hd: false, power: 0 }), "the arming tone first: {c:?}");
    let up = c.iter().find_map(|c| if let RadioCmd::TxPlan { tx, .. } = c { Some(*tx) } else { None }).expect("the uplink planned");
    assert!(up.abs_diff(14_585_000) < 400 && up != 14_585_000, "145.850 corrected: {up}");
    assert!(c.contains(&RadioCmd::Transmit { slot: crate::sat::SAT_SLOT, on: true }));
    assert_eq!(t.app.radio.keyed, Some(crate::sat::SAT_SLOT));
    t.wait(2500);
    let c = sat_cmds(&mut t);
    assert!(c.contains(&RadioCmd::SatTone { tone: 670, hd: false, power: 0 }), "then the access tone: {c:?} {:?} {:?} {:?}", t.app.radio.keyed, (t.app.sat.arm_until, t.app.now, t.app.sat.sent), t.app.screen());
    t.tap(Key::Star);
    assert!(sat_cmds(&mut t).contains(&RadioCmd::SatTone { tone: 670, hd: true, power: 0 }), "* while keyed: half duplex");
    t.app.key_up(Key::Ptt(ferrite_core::keys::PTT_A), t.t, &s);
    t.wait(200);
    assert!(sat_cmds(&mut t).contains(&RadioCmd::Transmit { slot: crate::sat::SAT_SLOT, on: false }));
    t.app.key_down(Key::Ptt(ferrite_core::keys::PTT_A), t.t, &s);
    let c = sat_cmds(&mut t);
    assert!(!c.contains(&RadioCmd::SatTone { tone: 744, hd: true, power: 0 }), "no arming tone the second time: {c:?}");
    t.app.key_up(Key::Ptt(ferrite_core::keys::PTT_A), t.t, &s);
    t.wait(1100);
    let _ = t.cmds();
    t.tap(Key::Right);
    let c = sat_cmds(&mut t);
    let d2 = c.iter().find_map(|c| if let RadioCmd::SatRx(f) = c { Some(*f) } else { None }).expect("retuned");
    assert!((d2 as i32 - down as i32).abs() < 1_000, "the trim and a second's Doppler: {down} -> {d2}");
    assert_eq!(t.app.sat_data().prefs(1).trim, 10, "100 Hz, kept with the satellite");
    t.hold(Key::Exit);
    assert!(t.cmds().contains(&RadioCmd::SatRx(0)), "home: both chips given back (SAT-1)");
}

/// SAT-12, SAT-20: an alarm 5 minutes before the rise: the status bar's mark counts down, the toast comes at its
/// lead with the alert tone (never taking the screen), OK on it opens the pass page; the mark goes after the pass.
#[test]
fn alarm_toast_and_mark() {
    let p = a_pass(0);
    let mut t = sat_app(p.aos - 3600);
    t.app.sat_set_alarm(p, 2);
    t.wait(200);
    let m = t.app.sat_mark() & ferrite_ui::widgets::SAT_UP;
    assert!((60..=61).contains(&m), "an hour to the rise: {m}");
    t.app.sat_clock(Some(p.aos - 299), true, Some(HERE));
    t.wait(200);
    assert!(t.app.toast.is_some_and(|x| x.head.as_bytes().starts_with(b"ISS 5 min")), "the toast");
    assert!(t.cmds().iter().any(|c| matches!(c, RadioCmd::Beep { .. })), "the alert tone (0 7 5 3 on)");
    assert_eq!(t.app.screen(), Screen::Home, "it never takes the screen");
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Sat(s) if s.page == 1 && s.aos == p.aos), "OK on it: the pass page");
    t.app.sat_clock(Some(p.aos + 30), true, Some(HERE));
    t.wait(200);
    assert_eq!(t.app.sat_mark() & ferrite_ui::widgets::SAT_UP, ferrite_ui::widgets::SAT_UP, "UP while overhead");
    t.app.sat_clock(Some(p.los() + 5), true, Some(HERE));
    t.hold(Key::Exit);
    t.wait(200);
    assert_eq!(t.app.sat_mark(), 0, "gone after the pass");
}

/// SAT-22: hold MENU on a pass page opens the satellite's options: Doppler on the 70 cm side only, the uplink's
/// power (Low to start, a press each: Mid, High), duplex, the transponder, the trim back to 0; each kept with the
/// satellite and sent to the radio at once; BACK returns to the page.
#[test]
fn options_page() {
    let p = a_pass(1);
    let mut t = sat_app(p.aos + 120);
    t.open(crate::screens::nav::Dest::Satellites);
    t.wait(3000);
    t.app.sat.cur = Some(p);
    t.app.screen = Screen::Sat(SatScr::pass_of(&p));
    t.wait(1100);
    t.tap(Key::Right);
    let _ = t.cmds();
    t.hold(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Sat(s) if s.opt == 1), "{:?}", t.app.screen());
    assert_eq!(t.app.sat_data().prefs(1).power, 0, "Low to start");
    t.tap(Key::Digit(2));
    t.tap(Key::Menu);
    assert_eq!(t.app.sat_data().prefs(1).power, 1);
    assert!(sat_cmds(&mut t).contains(&RadioCmd::SatTone { tone: 670, hd: false, power: 1 }), "the next key-up's power");
    t.tap(Key::Up);
    t.tap(Key::Menu);
    assert!(t.app.sat_data().prefs(1).uhf_only, "Doppler on the 70 cm side only");
    t.tap(Key::Digit(3));
    t.tap(Key::Menu);
    assert!(t.app.sat_data().prefs(1).hd);
    t.tap(Key::Digit(5));
    assert_eq!(t.app.sat_data().prefs(1).trim, 10);
    t.tap(Key::Menu);
    assert_eq!(t.app.sat_data().prefs(1).trim, 0, "OK on the trim: back to 0");
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Sat(s) if s.opt == 0 && s.page == 1), "back on the pass page");
}


/// The satellite source counting its record reads (each one an SPI flash read on the radio).
struct Counting(crate::sat::SimSats, core::cell::Cell<u32>);

impl ferrite_core::sat::SatSource for Counting {
    fn count(&self) -> u8 {
        self.0.count()
    }
    fn get(&self, i: u8) -> Option<ferrite_core::sat::Sat> {
        self.1.set(self.1.get() + 1);
        self.0.get(i)
    }
    fn declination(&self) -> Option<i16> {
        self.0.declination()
    }
    fn prefs(&self, i: u8) -> Option<ferrite_core::sat::data::Prefs> {
        self.0.prefs(i)
    }
    fn set_prefs(&self, i: u8, p: ferrite_core::sat::data::Prefs) {
        self.0.set_prefs(i, p)
    }
}

/// SAT-6 (the radio's key-to-screen time, modelled on the host): ▼ on the pass list, while predicting and after:
/// the pixels sent to the panel (0.24 µs a byte on the radio, TERM-40) and the satellite records read (an SPI
/// flash read each), per key and per idle redraw.
#[test]
fn list_key_to_screen_model() {
    let src: &'static Counting = std::boxed::Box::leak(std::boxed::Box::new(Counting(crate::sat::SimSats::new(crate::sat::sample()), core::cell::Cell::new(0))));
    struct Ref(&'static Counting);
    impl ferrite_core::sat::SatSource for Ref {
        fn count(&self) -> u8 {
            self.0.count()
        }
        fn get(&self, i: u8) -> Option<ferrite_core::sat::Sat> {
            self.0.get(i)
        }
        fn declination(&self) -> Option<i16> {
            self.0.declination()
        }
        fn prefs(&self, i: u8) -> Option<ferrite_core::sat::data::Prefs> {
            self.0.prefs(i)
        }
        fn set_prefs(&self, i: u8, p: ferrite_core::sat::data::Prefs) {
            self.0.set_prefs(i, p)
        }
    }
    let mut t = T::new();
    t.app.set.backlight = 0;
    t.app.sat_attach(std::boxed::Box::leak(std::boxed::Box::new(Live::new(Ref(src)))));
    t.app.sat_clock(Some(crate::sat::SIM_T0), true, Some(HERE));
    let mut fb = FrameBuffer::new();
    t.open(crate::screens::nav::Dest::Satellites);
    t.render(&mut fb);
    let mut keys = 0;
    let (mut px, mut reads) = (0u64, 0u32);
    let (mut ipx, mut ireads, mut idle) = (0u64, 0u32, 0);
    for k in 0..40 {
        // an idle redraw (the frame every 50 ms), then a key and its redraw
        fb.begin_pass();
        let r0 = src.1.get();
        t.wait(100);
        t.render(&mut fb);
        let rep = fb.end_pass(false);
        (ipx, ireads, idle) = (ipx + rep.writes, ireads + src.1.get() - r0, idle + 1);
        if k % 2 == 0 {
            fb.begin_pass();
            let r0 = src.1.get();
            t.tap(Key::Down);
            t.render(&mut fb);
            let rep = fb.end_pass(false);
            (px, reads, keys) = (px + rep.writes, reads + src.1.get() - r0, keys + 1);
        }
    }
    let ms = |p: u64, n: u32| p as f64 * 2.0 * 0.24 / 1000.0 / n as f64;
    std::println!(
        "SAT-6 model: a key {:.1} ms of panel bus and {:.1} records read; an idle redraw {:.1} ms and {:.1} records",
        ms(px, keys),
        reads as f64 / keys as f64,
        ms(ipx, idle),
        ireads as f64 / idle as f64
    );
    // an idle redraw read every satellite (the head line's elements age) until Nov 3: 14.8 a redraw for 4
    assert!(ireads <= idle, "{ireads} records in {idle} idle redraws");
    assert!(reads <= keys * 12, "only the rows drawn read their names");
}

/// MENU-25, SAT-22: hold MENU opens the satellite's options from every satellite page: the pass list (while it
/// predicts and after), the pass page before the rise and overhead, the pointing page and the alarm picker (which
/// it closes); keyed it says so and stays; 0 there is the full menu.
#[test]
fn hold_menu_on_every_satellite_page() {
    let p = a_pass(1);
    let mut t = sat_app(p.aos - 600);
    let opts = |t: &T| matches!(t.app.screen(), Screen::Sat(s) if s.opt == 1);
    t.open(crate::screens::nav::Dest::Satellites);
    t.hold(Key::Menu);
    assert!(opts(&t), "the list while predicting: {:?}", t.app.screen());
    t.tap(Key::Exit);
    t.wait(3000);
    t.hold(Key::Menu);
    assert!(opts(&t), "the list");
    t.tap(Key::Digit(0));
    assert!(matches!(t.app.screen(), Screen::Advanced(_)), "0: the full menu");
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Sat(s) if s.opt == 0), "BACK: the list: {:?}", t.app.screen());
    t.tap(Key::Hash);
    assert!(matches!(t.app.screen(), Screen::Sat(s) if s.picker != 0));
    t.hold(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Sat(s) if s.opt == 1 && s.picker == 0), "the picker: closed, the options");
    t.tap(Key::Exit);
    t.app.sat.cur = Some(p);
    t.app.screen = Screen::Sat(SatScr::pass_of(&p));
    t.wait(200);
    t.hold(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Sat(s) if s.opt == 1 && s.sat == p.sat), "the pass page before the rise");
    t.tap(Key::Exit);
    t.tap(Key::Digit(0));
    assert!(matches!(t.app.screen(), Screen::Sat(s) if s.page == 2));
    t.hold(Key::Menu);
    assert!(opts(&t), "the pointing page");
    t.tap(Key::Exit);
    t.tap(Key::Digit(0));
    t.app.sat_clock(Some(p.aos + 60), true, Some(HERE));
    t.wait(1100);
    t.hold(Key::Menu);
    assert!(opts(&t), "the pass page overhead");
    t.tap(Key::Exit);
    let s = store();
    t.app.key_down(Key::Ptt(ferrite_core::keys::PTT_A), t.t, &s);
    t.wait(200);
    assert!(matches!(t.app.screen(), Screen::Sat(s) if s.page & 0x80 != 0), "keyed");
    t.hold(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Sat(s) if s.opt == 0 && s.page & 0x80 != 0), "keyed: no options");
    assert!(t.app.toast.is_some_and(|x| x.body.as_bytes().starts_with(b"after you release")), "it says why");
    t.app.key_up(Key::Ptt(ferrite_core::keys::PTT_A), t.t, &s);
}

/// SAT-6, SAT-22 (decision 341): the options' Lowest pass, 10° to start: no pass under it is listed; OK steps it
/// (15°) and the list is predicted again without the lower ones; 0° lists them all.
#[test]
fn minimum_elevation_on_the_options_page() {
    let mut t = sat_app(crate::sat::SIM_T0);
    t.open(crate::screens::nav::Dest::Satellites);
    t.wait(3000);
    let d = |t: &T| (0..t.app.sat_data().passes()).filter_map(|i| t.app.sat_data().pass(i)).collect::<Vec<_>>();
    let ten = d(&t);
    assert!(!ten.is_empty() && ten.iter().all(|p| p.max_el >= 10), "10° to start");
    t.hold(Key::Menu);
    t.tap(Key::Digit(6));
    let mut v = Buf::<16>::new();
    crate::screens::sat::opt_value(&t.app, 0, 6, &mut v);
    assert_eq!(v.as_bytes(), "10\u{b0}".as_bytes());
    t.tap(Key::Menu);
    assert_eq!(t.app.settings().sat_min_el, 3);
    t.wait(3000);
    let fifteen = d(&t);
    assert!(fifteen.iter().all(|p| p.max_el >= 15) && fifteen.len() < ten.len(), "predicted again, the lower ones gone");
    for _ in 0..3 {
        t.tap(Key::Menu); // 20, then round to 0, 5
    }
    assert_eq!(t.app.settings().sat_min_el, 1);
    for _ in 0..4 {
        t.tap(Key::Menu); // 10, 15, 20, then 0
    }
    assert_eq!(t.app.settings().sat_min_el, 0);
    t.wait(3000);
    assert!(d(&t).iter().any(|p| p.max_el < 10), "0°: the low ones too");
}

/// SAT-6: the selection is a pass, not a row: once the first rows are in it stays on its pass while the rest of
/// the prediction adds rows, and ▼ moves to the next pass by rise time.
#[test]
fn selection_stays_on_its_pass_while_rows_arrive() {
    let mut t = sat_app(crate::sat::SIM_T0);
    t.open(crate::screens::nav::Dest::Satellites);
    while t.app.sat_data().passes() == 0 {
        t.wait(100);
    }
    t.wait(100);
    let Screen::Sat(first) = t.app.screen() else { panic!() };
    assert_ne!(first.aos, 0, "pinned to a pass");
    while t.app.sat_data().state() == ferrite_core::sat::data::State::Busy {
        t.wait(100);
        let Screen::Sat(s) = t.app.screen() else { panic!() };
        assert_eq!((s.sat, s.aos), (first.sat, first.aos), "the same pass while rows arrive");
    }
    let list: Vec<_> = (0..t.app.sat_data().passes()).filter_map(|i| t.app.sat_data().pass(i)).collect();
    let at = list.iter().position(|p| p.sat == first.sat && p.aos == first.aos).unwrap();
    t.tap(Key::Down);
    let Screen::Sat(s) = t.app.screen() else { panic!() };
    assert_eq!((s.sat, s.aos), (list[at + 1].sat, list[at + 1].aos), "▼: the next pass");
}

/// SAT-6, PWR-26.1: the radio predicts in time slices ([`App::sat_work`]): a slice runs while the platform says
/// there is time, a long enough one finishes; closed, nothing is predicted and the governor's reason is gone.
#[test]
fn prediction_in_time_slices() {
    let mut t = sat_app(crate::sat::SIM_T0);
    t.wait(200);
    // until Nov 3 the source said busy whenever the screen wasn't open: the governor's predict reason held the
    // build's speed from power on whenever satellites, a clock and a position were there
    assert!(!t.app.sat_busy(), "closed: not predicting (the clock may drop)");
    t.open(crate::screens::nav::Dest::Satellites);
    assert!(t.app.sat_busy());
    let mut n = 0;
    assert!(t.app.sat_work(&mut || {
        n += 1;
        n <= 3
    }));
    assert_eq!(n, 4, "three chunks, then out of time");
    assert!(!t.app.sat_work(&mut || true), "a long slice finishes it");
    assert!(t.app.sat_data().passes() > 10);
    assert_eq!(t.app.clock_reasons(&t.live, t.t) & ferrite_core::clocktree::why::PREDICT, 0);
    t.hold(Key::Exit);
    assert!(!t.app.sat_busy(), "closed again");
}

/// SAT-1's list: ◀▶ from all shows the selected pass's satellite and all its passes, the selection staying on that
/// pass (it always showed the first satellite, Alex, Nov 3, 2026); ◀▶ again step through the satellites.
#[test]
fn arrows_filter_to_the_selected_satellite() {
    let p = a_pass(0);
    let mut t = sat_app(p.aos - 600);
    t.open(crate::screens::nav::Dest::Satellites);
    t.wait(3000);
    let first = match t.app.screen() { Screen::Sat(s) => s.sat, _ => panic!("{:?}", t.app.screen()) };
    // down until a pass of another satellite is selected
    let mut picked = None;
    for _ in 0..20 {
        t.tap(Key::Down);
        if let Screen::Sat(s) = t.app.screen() {
            if s.sat != first {
                picked = Some((s.sat, s.aos));
                break;
            }
        }
    }
    let (sat, aos) = picked.expect("a second satellite's pass in the list");
    t.tap(Key::Right);
    match t.app.screen() {
        Screen::Sat(s) => {
            assert_eq!(s.filter, sat, "the selected pass's satellite");
            assert_eq!((s.sat, s.aos), (sat, aos), "the selection stays on its pass");
        }
        sc => panic!("{sc:?}"),
    }
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Sat(s) if s.filter == 0xFF && (s.sat, s.aos) == (sat, aos)), "BACK: all the passes again, the selection kept");
    t.tap(Key::Right);
    t.tap(Key::Left);
    assert!(matches!(t.app.screen(), Screen::Sat(s) if s.filter != sat), "◀ steps to another satellite (or all)");
}

/// SAT-1 to SAT-22 (DISP-25): every satellite page draws a whole screen in each of its states: the list while it
/// predicts and after, filtered to one satellite, the pass page before the rise (its timeline), the pointing page
/// (the sky plot) before and during the pass, the pass page overhead, keyed (the transmit page), the alarm picker,
/// each option row; and BACK from each lands where it should.
#[test]
fn every_satellite_page_draws() {
    let p = a_pass(1);
    let mut t = sat_app(p.aos - 600);
    let mut fb = FrameBuffer::new();
    let mut drew = |t: &mut T, what: &str| {
        t.render(&mut fb);
        let n = count_px(&fb, 20, 300, |c| c != ferrite_ui::gfx::BG);
        assert!(n > 500, "{what}: drew little ({n} px): {:?}", t.app.screen());
    };
    t.open(crate::screens::nav::Dest::Satellites);
    drew(&mut t, "the list predicting");
    t.wait(3000);
    drew(&mut t, "the list");
    t.tap(Key::Right);
    drew(&mut t, "the list, one satellite");
    t.tap(Key::Left);
    t.tap(Key::Hash);
    drew(&mut t, "the alarm picker");
    for _ in 0..3 {
        t.tap(Key::Down);
        drew(&mut t, "the picker, another row");
    }
    t.tap(Key::Exit);
    t.app.sat.cur = Some(p);
    t.app.screen = Screen::Sat(SatScr::pass_of(&p));
    t.wait(1100);
    drew(&mut t, "the pass page before the rise");
    t.tap(Key::Digit(0));
    t.wait(1100);
    drew(&mut t, "the pointing page before the rise");
    t.app.sat_clock(Some(p.aos + (p.los() - p.aos) / 2), true, Some(HERE));
    t.wait(1100);
    drew(&mut t, "the pointing page overhead");
    t.tap(Key::Digit(0));
    t.wait(1100);
    drew(&mut t, "the pass page overhead");
    t.hold(Key::Menu);
    for _ in 0..8 {
        drew(&mut t, "an option row");
        t.tap(Key::Down);
    }
    t.tap(Key::Exit);
    let s = store();
    t.app.radio.tx_built = true; // the harness's device report clears it on the way
    t.app.key_down(Key::Ptt(ferrite_core::keys::PTT_A), t.t, &s);
    t.wait(1100);
    assert!(matches!(t.app.screen(), Screen::Sat(x) if x.page & 0x80 != 0), "keyed: {:?} {:?}", t.app.screen(), t.app.radio.keyed);
    drew(&mut t, "keyed: the transmit page");
    t.tap(Key::Right);
    t.wait(300);
    drew(&mut t, "keyed, the trim moved");
    t.app.key_up(Key::Ptt(ferrite_core::keys::PTT_A), t.t, &s);
    t.wait(300);
    t.app.sat_clock(Some(p.los() + 60), true, Some(HERE));
    t.wait(1100);
    drew(&mut t, "the pass page after the set");
    t.hold(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home);
}

/// SAT-8.2 (ferrite#79): through the minute around a pass's highest point, where Doppler moves fastest, the downlink
/// is retuned only once it has drifted 1 kHz from what the chip has (each retune is the chip's receive off and on,
/// a click with monitor on), never every second; and still within 1 kHz of the true Doppler.
#[test]
fn downlink_retunes_only_past_a_kilohertz() {
    let p = a_pass(1);
    let mut t = sat_app(p.aos + p.tmax as u32 - 30);
    t.open(crate::screens::nav::Dest::Satellites);
    t.wait(3000); // predicted
    t.app.sat.cur = Some(p);
    t.app.screen = Screen::Sat(SatScr::pass_of(&p));
    t.wait(1100);
    let first = sat_cmds(&mut t).iter().rev().find_map(|c| if let RadioCmd::SatRx(f) = c { Some(*f) } else { None }).expect("tuned");
    let (mut sent, mut last) = (std::vec![first], first);
    for _ in 0..60 {
        t.wait(1000);
        for c in sat_cmds(&mut t) {
            if let RadioCmd::SatRx(f) = c {
                assert!(f.abs_diff(last) >= crate::sat::DOWN_STEP, "a retune under 1 kHz: {last} -> {f}");
                sent.push(f);
                last = f;
            }
        }
        let (_, _, tp, pr) = t.app.sat_cur().expect("the pass");
        let now = t.app.sat_now().unwrap();
        let (want, _) = crate::sat::corrected(&tp, &t.app.sat_data().look(p.sat, now).unwrap_or_default(), &pr);
        assert!(want.abs_diff(last) < crate::sat::DOWN_STEP, "the chip within 1 kHz of Doppler: {want} vs {last}");
    }
    let moved = sent.last().unwrap().abs_diff(first);
    assert!(moved > 3 * crate::sat::DOWN_STEP, "Doppler at the pass's peak moves the downlink: {moved}");
    assert!(sent.len() < 20, "retunes in a minute at the peak: {} ({sent:?})", sent.len());
}

/// SAT-22.1: hold 2 on the pass page cycles the satellite's uplink power, Low, Mid, High and round, as hold 2 cycles
/// TX power on home (KEY-11): a popup names it, the options page shows the same value, the radio gets it for the
/// next key-up; a tap of 2 still picks the second transponder.
#[test]
fn hold_2_cycles_the_uplink_power() {
    let p = a_pass(1);
    let mut t = sat_app(p.aos + 120);
    t.open(crate::screens::nav::Dest::Satellites);
    t.wait(3000);
    t.app.sat.cur = Some(p);
    t.app.screen = Screen::Sat(SatScr::pass_of(&p));
    t.wait(1100);
    let _ = t.cmds();
    assert_eq!(t.app.sat_data().prefs(1).power, 0, "Low to start");
    for want in [1u8, 2, 0] {
        t.hold(Key::Digit(2));
        let pop = t.app.toast.filter(|x| x.center).map(|x| (std::string::String::from_utf8_lossy(x.head.as_bytes()).into_owned(), std::string::String::from_utf8_lossy(x.body.as_bytes()).into_owned()));
        assert_eq!(pop.as_ref().map(|p| p.0.as_str()), Some("Uplink power"), "{pop:?}");
        assert_eq!(pop.map(|p| p.1).as_deref(), Some(["Low", "Mid", "High"][want as usize]));
        assert_eq!(t.app.sat_data().prefs(1).power, want);
        t.wait(1100);
        assert!(sat_cmds(&mut t).iter().any(|c| matches!(c, RadioCmd::SatTone { power, .. } if *power == want)), "the next key-up's power: {want}");
    }
    assert!(matches!(t.app.screen(), Screen::Sat(s) if s.page == 1 && s.opt == 0), "still on the pass page");
}

/// SAT-24: the satellite screen keeps the display lit from 5 minutes before a pass's rise to 5 minutes after its
/// set: dark before the window (the 5 s backlight time), woken as it opens, lit through the pass and the 5 minutes
/// after, dark again past them; on the list for any pass listed, and dark when nothing listed is near; never with
/// the screen closed.
#[test]
fn display_stays_lit_around_a_pass() {
    let p = a_pass(1);
    let mut t = sat_app(p.aos - 900);
    t.app.set.backlight = 1; // 5 s
    t.open(crate::screens::nav::Dest::Satellites);
    while t.app.sat_data().state() == ferrite_core::sat::data::State::Busy || t.app.sat_data().passes() == 0 {
        t.wait(100);
    }
    t.app.screen = Screen::Sat(SatScr::pass_of(&p));
    // the clock set: a jump predicts the list again (SAT-6), waited for
    let at = |t: &mut T, utc: u32| {
        t.app.sat_clock(Some(utc), true, Some(HERE));
        t.wait(100);
        while t.app.sat_data().state() == ferrite_core::sat::data::State::Busy {
            t.wait(100);
        }
    };
    // the pass page
    at(&mut t, p.aos - 600);
    t.wait(6000);
    assert!(t.app.dark(), "10 min before the rise: the usual timer");
    at(&mut t, p.aos - 290);
    t.wait(200);
    assert!(!t.app.dark(), "5 min before: woken");
    t.wait(10_000);
    assert!(!t.app.dark(), "and kept lit");
    at(&mut t, p.aos + 60);
    t.wait(10_000);
    assert!(!t.app.dark(), "during the pass");
    at(&mut t, p.los() + 280);
    t.wait(10_000);
    assert!(!t.app.dark(), "up to 5 min after the set (the page has moved on to the next pass)");
    at(&mut t, p.los() + 310);
    t.wait(6000);
    assert!(t.app.dark(), "past the 5 min: the usual timer");
    // the list: any pass listed
    t.app.screen = Screen::Sat(SatScr::LIST);
    at(&mut t, p.aos + 60);
    t.wait(200);
    assert!(!t.app.dark(), "the list with a pass up");
    let d = t.app.sat_data();
    let min = t.app.min_el();
    let list: Vec<Pass> = (0..d.passes()).filter_map(|i| d.pass(i)).filter(|q| q.max_el >= min).collect();
    let quiet = (p.los() + 600..p.aos + 43_200)
        .step_by(60)
        .find(|&u| list.iter().all(|q| u + 400 < q.aos || u > q.los() + 400))
        .expect("a quiet time");
    at(&mut t, quiet);
    t.wait(6000);
    assert!(t.app.dark(), "nothing listed near: the usual timer");
    at(&mut t, p.aos + 60);
    t.wait(200);
    assert!(!t.app.dark(), "the pass near again: woken");
    // closed
    t.hold(Key::Exit);
    assert!(!matches!(t.app.screen(), Screen::Sat(_)));
    t.wait(6000);
    assert!(t.app.dark(), "the screen closed: the usual timer, pass or not");
    assert_eq!(t.app.sat.awake_until, 0, "the window's end let go with the screen");
}

/// SAT-12: the alarm set with the picker's keys as on the radio (# on the list, OK), then home with EXIT: the toast
/// comes at the lead (Discord, Nov 26-27: no tone or toast on the radio; the host gets it, the radio still to explain).
#[test]
fn alarm_by_keys_then_home() {
    let t0 = crate::sat::SIM_T0;
    let mut t = sat_app(t0);
    t.open(crate::screens::nav::Dest::Satellites);
    t.wait(3000); // predicted
    t.tap(Key::Hash);
    t.tap(Key::Menu);
    let p = t.app.sat_data().alarm().expect("an alarm set");
    std::println!("alarm on sat {} aos {} (+{} s) lead {} fired {}", p.sat, p.aos, p.aos - t0, t.app.sat.alarm_lead, t.app.sat.fired);
    t.tap(Key::Exit);
    t.tap(Key::Exit);
    std::println!("screen {:?}", t.app.screen());
    let mut toast_at = None;
    let mut s = 0;
    while t0 + s < p.aos + 60 {
        t.app.sat_clock(Some(t0 + s), true, Some(HERE));
        t.wait(1000);
        if toast_at.is_none() && t.app.toast.is_some_and(|x| x.icon) {
            toast_at = Some(p.aos as i64 - (t0 + s) as i64);
        }
        s += 1;
    }
    std::println!("toast {:?} s before the rise; lead now {}, alarm {:?}", toast_at, t.app.sat.alarm_lead, t.app.sat_data().alarm().map(|a| a.aos));
    assert!(toast_at.is_some(), "the alarm's toast came");
}
