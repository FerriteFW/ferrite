//! Low Power Mode's home (PWR-24.7): quiet, receiving, transmitting, the last heard, and the leaving box.

use super::*;
use crate::screens::lowpower::LP_AMBER;

/// Low Power Mode on (0 7 7 2 On), home drawn its way.
fn lpm() -> T {
    let mut t = T::ready();
    t.app.set.lpm = 0;
    t.wait(100);
    t.open(Dest::Group(7));
    assert!(crate::screens::groups::change(&mut t.app, &[7, 7, 2], -1));
    assert!(t.app.low_power());
    t.wait(3500); // the toast goes
    t.draw();
    t
}

/// PWR-24.7: the frame in its own amber, LOW POWER and the battery in the top band, home's hints with PTT Talk
/// at the bottom; redrawn with nothing changed, it draws nothing new.
#[test]
fn quiet_home() {
    let mut t = lpm();
    assert!(t.px(0, 26, LP_AMBER) > 2000, "the top band");
    assert!(t.px(294, 320, LP_AMBER) > 2000, "the bottom band");
    t.shows("LOW POWER");
    t.shows("80%");
    t.shows("146.520");
    let frames = t.app.frames;
    t.draw();
    assert_eq!(t.app.frames, frames, "nothing changed: nothing drawn");
}

/// PWR-24.7: receiving shows RX and the S-meter's text with the blocks lit; transmitting, TX and the time keyed.
#[test]
fn rx_and_tx() {
    let mut t = lpm();
    let a = t.app.active as usize;
    let f = t.app.slot(a as u8).freq();
    t.live.slots[a] = ferrite_core::msg::SlotLive { freq: f, squelch_open: true, carrier: true, rssi: -70, ..Default::default() };
    t.wait(200);
    t.shows("RX");
    t.shows("S9");
    assert!(t.px(228, 260, ferrite_ui::gfx::rgb565(0x4C, 0xFF, 0x8F)) > 100, "the blocks lit green");
    t.live.slots[a].squelch_open = false;
    t.wait(200);
    t.ptt(true);
    t.hold(Key::Exit); // TX-25: hold BACK goes home, still keyed
    assert_eq!(t.screen(), Screen::Home);
    t.wait(1500);
    t.shows("TX");
    t.shows("0:02");
    t.ptt(false);
}

/// PWR-24.8: once the signal goes, the last heard: how long ago and how strong.
#[test]
fn last_heard_line() {
    let mut t = lpm();
    let a = t.app.active as usize;
    let f = t.app.slot(a as u8).freq();
    t.live.slots[a] = ferrite_core::msg::SlotLive { freq: f, squelch_open: true, carrier: true, rssi: -70, ..Default::default() };
    t.wait(500);
    t.live.slots[a].squelch_open = false;
    t.wait(3000);
    t.shows("Heard ");
    t.shows("s ago \u{b7} S9");
    t.wait(120_000);
    t.shows("min ago");
    t.app.last_rx.2 = t.app.now.wrapping_sub(2 * 3600 * 1000);
    t.draw();
    t.shows("2 h ago");
}

/// PWR-24.6: holding SIDE1 + SIDE2 shows the leaving box filling; let go early, home comes back whole.
#[test]
fn leaving_box() {
    let mut t = lpm();
    t.app.key_down(Key::Side(1), t.t, &t.s);
    t.t += 50;
    t.app.key_down(Key::Side(2), t.t, &t.s);
    t.wait(500);
    assert!(t.app.lpm_leave_progress().is_some());
    t.shows("Leaving");
    t.shows("Keep both held.");
    t.app.key_up(Key::Side(2), t.t, &t.s);
    t.app.key_up(Key::Side(1), t.t, &t.s);
    t.wait(100);
    assert!(t.app.low_power(), "let go early: still on");
    t.hides("Leaving");
    t.shows("146.520");
}
