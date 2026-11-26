//! The Status page (SBAR-14): each row's states, the lower rows scrolled to, and HF mode pausing the rest.

use super::*;
use crate::screens::status::Status;
use ferrite_core::gpspower::GpsPhase;
use ferrite_core::msg::{CalState, SlotLive};

fn status(t: &mut T, row: u8) -> String {
    t.app.screen = Screen::Status(Status { row, from_menu: false });
    t.draw();
    t.texts()
}

fn open() -> T {
    let mut t = T::ready();
    t.open(Dest::Status);
    assert_eq!(t.name(), "Status");
    assert_eq!(t.screen().mockup(), Some("StatusPage.png"));
    t
}

#[track_caller]
fn has(s: &str, want: &str) {
    assert!(s.contains(want), "{want:?} not on the page: {s}");
}

/// SBAR-14, BOOT-11: transmit Ready once the checks pass; the inbox's count; each scanning slot where it is
/// (one alone with its band); the watch paused; Low Power Mode in Battery Save's place.
#[test]
fn upper_rows() {
    let mut t = open();
    t.dev.selftest.mcu_ok = true;
    t.dev.selftest.cal = CalState::Ok;
    t.app.inbox = 3;
    t.live.slots[1] = SlotLive { freq: ferrite_core::freq::mhz(446), scanning: true, ..Default::default() };
    t.wait(100);
    let s = status(&mut t, 0);
    has(&s, "Ready");
    has(&s, "3 unread");
    has(&s, "B: 446.000 \u{b7} UHF");
    t.live.slots[0] = SlotLive { freq: ferrite_core::freq::mhz(146), scanning: true, ..Default::default() };
    t.wait(100);
    let s = status(&mut t, 0);
    has(&s, "A: 146.000, B: 446.000");
    t.app.paused_until = Some(t.app.now + 60_000);
    has(&status(&mut t, 0), "\u{b7} paused");
    t.app.set.lpm = 1; // On
    t.wait(100);
    has(&status(&mut t, 8), "Low Power Mode");
}

/// GPS-26, BCR-27, SBAR-14: GPS off, asleep, waking; broadcast FM on its slot; the clock not set; the battery's
/// low and cut-off in colour; the key lock; the weather alerts by region and setting.
#[test]
fn lower_rows() {
    let mut t = open();
    t.dev.gps = ferrite_core::msg::GpsState::Off;
    has(&status(&mut t, 9), "GPS Off");
    t.dev.gps = ferrite_core::msg::GpsState::Searching;
    t.dev.gps_phase = GpsPhase::Standby(42);
    has(&status(&mut t, 9), "Sleep 42 s");
    t.dev.gps_phase = GpsPhase::Waking;
    has(&status(&mut t, 9), "Waking");
    t.app.set_vfo(0, ferrite_core::freq::mhz(100) + 50_000);
    t.wait(100);
    has(&status(&mut t, 10), "WFM 100.5 on A");
    t.dev.clock_min = None;
    has(&status(&mut t, 11), "Not set");
    t.dev.battery_low = true;
    let s = status(&mut t, 19);
    has(&s, "Battery levels");
    has(&s, "Not measured yet");
    has(&s, "Key lock");
    has(&s, "Mailbox");
    assert!(t.px(20, 300, ferrite_ui::gfx::AMBER) > 0, "a low battery in amber");
    t.dev.battery_cutoff = true;
    status(&mut t, 19);
    assert!(t.px(20, 300, ferrite_ui::gfx::RED) > 0, "at the cut-off in red");
    // Weather alerts: US with SAME on; then off; then outside the US and Canada.
    t.app.set.same = true;
    has(&status(&mut t, 18), "Weather alerts On");
    t.app.set.same = false;
    has(&status(&mut t, 18), "Weather alerts Off");
    t.app.set_region(Some(ferrite_core::txplan::Region::Europe), false);
    has(&status(&mut t, 18), "Not in region");
}

/// HF-6, SBAR-18: in HF mode scan, watch, jobs, beacon and weather alerts say they are paused; Broadcast says HF
/// mode.
#[test]
fn hf_mode_pauses() {
    let mut t = open();
    t.app.set.same = true;
    t.open(Dest::HfMode);
    t.wait(100);
    let s = status(&mut t, 0);
    assert!(s.matches("Paused: HF mode").count() >= 4, "{s}");
    has(&status(&mut t, 10), "HF mode");
    has(&status(&mut t, 18), "Paused: HF mode");
}

/// SBAR-14: the Beacon row names the beacon's mode as 0 5 6 1 does: Off, Manual, Every N min, Smart (it read
/// "Off" whatever the mode until 0.8.2).
#[test]
fn beacon_row_follows_the_mode() {
    let mut t = open();
    let rows: std::vec::Vec<&str> = super::super::status::row_names().collect();
    let row = rows.iter().position(|n| *n == "Beacon").expect("a Beacon row") as u8;
    for (mode, want) in [(0u8, "Off"), (1, "Manual"), (5, "Every 10 min"), (crate::aprs::SMART as u8, "Smart")] {
        let c = crate::aprs::AprsCfg { beacon: mode, ..*t.app.aprs_cfg() };
        t.app.set_aprs(c);
        let s = status(&mut t, row);
        has(&s, want);
    }
}
