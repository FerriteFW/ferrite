//! The options pages (MENU-25): beacon now and its refusals, the beacon switch, deleting all messages, mark
//! here without a fix, the arrival alarm's steps, and a confirm holding still.

use super::*;
use crate::screens::opts::{self, Of, Options};
use ferrite_ui::text::Buf;

fn options(t: &mut T, of: Of) {
    t.app.open_screen(of.screen());
    t.draw();
    assert_eq!(t.name(), "Options");
}

/// Picks row `n` (1-based) and runs it.
fn run(t: &mut T, n: u8) {
    t.tap(Key::Digit(n));
    t.tap(Key::Menu);
}

/// APRS-17.7, APRS-17.9, KISS-10: the APRS slot's list is headed "APRS slot"; Beacon now asks the APRS task,
/// refused while KISS holds C, and without a callsign says to set one.
#[test]
fn beacon_now() {
    let mut t = T::new();
    t.app.set_aprs_mode(true);
    t.wait(100);
    options(&mut t, Of::AprsCard);
    t.shows("APRS slot");
    run(&mut t, 4);
    assert!(t.s.beacon_asked());
    assert_eq!(t.toast(), "Beacon now / done");
    t.s.call.set(None);
    t.app.set.set_callsign(b"");
    opts::beacon_now(&mut t.app, &t.s);
    assert_eq!(t.toast(), "Beacon now / set a callsign first");
    t.s.call.set(Some(Buf::from(b"VA7FER")));
    t.app.set.bt = true;
    t.app.set.kiss_port = 2;
    opts::beacon_now(&mut t.app, &t.s);
    assert_eq!(t.toast(), "Not sent / KISS on: the app sends");
}

/// APRS-46: Delete all messages asks first; then the inbox count is gone.
#[test]
fn delete_all_messages() {
    let mut t = T::new();
    t.app.inbox = 2;
    options(&mut t, Of::Messages);
    run(&mut t, 5);
    assert!(matches!(t.screen(), Screen::Options(Options { confirm: true, .. })));
    let row = match t.screen() {
        Screen::Options(o) => o.row,
        _ => unreachable!(),
    };
    t.knob(1); // a confirm holds still
    assert!(matches!(t.screen(), Screen::Options(o) if o.row == row));
    t.tap(Key::Menu);
    assert_eq!(t.app.inbox, 0);
    assert_eq!(t.toast(), "Delete all messages / done");
}

/// GPS-10: Mark here without a fix says it needs one.
#[test]
fn mark_here_needs_a_fix() {
    let mut t = T::new();
    t.s.fix.set(None);
    options(&mut t, Of::Gps);
    run(&mut t, 1);
    assert_eq!(t.toast(), "Mark here / needs a GPS fix");
}

/// GPS-20: the arrival alarm steps through its distances, then off.
#[test]
fn arrival_alarm_steps() {
    let mut t = T::new();
    let mut seen = std::vec::Vec::new();
    for _ in 0..crate::settings::ARRIVAL_M.len() {
        options(&mut t, Of::Waypoints);
        run(&mut t, 3);
        seen.push(t.toast());
    }
    assert!(seen.contains(&"Arrival alarm / off".into()), "{seen:?}");
    assert!(seen.contains(&"Arrival alarm / 1 km".into()), "{seen:?}");
    assert!(seen.iter().any(|s| s.ends_with("0 m")), "{seen:?}");
}
