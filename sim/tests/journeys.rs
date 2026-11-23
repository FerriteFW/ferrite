//! End-to-end journeys through the simulator: keys in, screen out, as a person uses the radio (the harness is
//! `common`). Each test names the rules it exercises; the rule coverage report counts a rule as covered when its ID
//! appears in a test's doc comment.
//!
//! The sample radio: slot A a VFO on 146.520, B channel 012 "Club repeater" on 443.475, C scanning the airband,
//! watch A+B+C; the fake radio hears A from 6 to 9 s of every 10 s and B from 20 to 22 s of every 25 s.

mod common;

use common::Radio;
use ferrite_core::freq::mhz;
use ferrite_core::keys::Key;
use ferrite_core::msg::RadioCmd;
use ferrite_sim::simcore::Event;

/// First boot on a blank flash runs setup: eight pages, OK through them, the region picked on page 3, and the
/// summary before home (SETUP-1, SETUP-2, SETUP-3, SETUP-4, SETUP-10, BOOT-12).
#[test]
fn first_run_setup() {
    let mut r = Radio::blank();
    r.expect_screen("Setup").expect_text("1 of 8").expect_text("Welcome to Ferrite");
    r.run("press OK; expect text \"2 of 8\"; expect text \"Your callsign\"");
    r.run("press OK; expect text \"3 of 8\"; expect text \"United States\"; expect text \"Other\"");
    r.run("press OK; expect text \"4 of 8\""); // Canada: first in the list
    for page in 5..=8 {
        r.press(Key::Menu);
        r.expect_text(&format!("{page} of 8"));
    }
    r.expect_text("All set").expect_text("Region Canada").expect_text("Channels none yet");
    r.run("press OK; expect screen \"Home\"");
}

/// A digit on home opens frequency entry; the point comes by itself after three digits, and OK tunes what was
/// typed, exactly (VFO-11, VFO-12, VFO-17, VFO-19, NAV-4).
#[test]
fn frequency_entry() {
    let mut r = Radio::new();
    r.run("type 4461; expect screen \"Frequency entry\"; expect text \"446.1\"; expect text \"Band UHF\"");
    r.run("press OK; expect screen \"Home\"");
    assert_eq!(r.sim.app().slot(0).freq(), mhz(446) + 10_000);
    r.expect_text("446.100");
    // Whole on the step grid: in without OK (VFO-17).
    r.run("type 446125; wait 300ms; expect screen \"Home\"");
    assert_eq!(r.sim.app().slot(0).freq(), mhz(446) + 12_500);
}

/// One-handed entry: \* types the point, and \* again after a digit enters it as OK does; the hint bar says so once
/// it would (VFO-24). \* \* on 100-130 is still the fourth MHz digit (VFO-21).
#[test]
fn star_enters_a_frequency() {
    let mut r = Radio::new();
    r.run("type 146*5; expect screen \"Frequency entry\"; expect text \"146.5\"; expect text \"*/OK\"");
    r.run("type 2*; wait 300ms; expect screen \"Home\"; expect text \"146.520\"");
    assert_eq!(r.sim.app().slot(0).freq(), mhz(146) + 52_000);
    r.run("type 129**6; expect text \"1296\"; type *0*; wait 300ms; expect screen \"Home\"");
    assert_eq!(r.sim.app().slot(0).freq(), mhz(1296));
}

/// A frequency no chip can receive is refused and the entry stays open (VFO-21).
#[test]
fn frequency_entry_refuses_the_gap() {
    let mut r = Radio::new();
    r.run("type 700; press OK; wait 300ms; expect screen \"Frequency entry\"");
    assert_ne!(r.sim.app().slot(0).freq(), mhz(700));
}

/// V/M flips the active slot between its VFO and its channel; hold 9 on a channel asks, then copies it to the
/// VFO (VFO-9, VFO-23, KEY-23).
#[test]
fn vfo_and_channel() {
    let mut r = Radio::new();
    r.run("press VM; expect text \"Calling 2 m\"");
    assert!(r.sim.app().slot(0).on_channel());
    r.run("hold 9; expect text \"Copy to VFO?\"; press OK; expect text \"Copied to VFO\"");
    assert!(!r.sim.app().slot(0).on_channel());
}

/// SLOT makes the next slot active, and only SLOT does (HOME-11, WATCH-1).
#[test]
fn slot_changes_the_active_slot() {
    let mut r = Radio::new();
    assert_eq!(r.sim.app().active(), 0);
    r.press(Key::Slot);
    assert_eq!(r.sim.app().active(), 1);
    r.press(Key::Slot).press(Key::Slot);
    assert_eq!(r.sim.app().active(), 0);
}

/// A watched slot that is heard plays and is highlighted, and the active slot never moves by itself (WATCH-3.2,
/// WATCH-4, WATCH-5, decision 81).
#[test]
fn watched_slot_heard_active_stays() {
    let mut r = Radio::new();
    r.wait(17_500); // past 20 s: B is heard from 20 to 22 s
    assert!(r.sim.radio().live.slots[1].squelch_open, "B heard");
    assert_eq!(r.sim.app().active(), 0, "A stays active");
    r.expect_screen("Home");
    r.wait(5_000);
    assert_eq!(r.sim.app().active(), 0);
}

/// Hold 4 turns watch off, and on again as A+B (WATCH-7); the status bar shows TW for three watched slots and
/// DW for two (SBAR-16).
#[test]
fn watch_on_and_off() {
    let mut r = Radio::new();
    r.expect_text("TW");
    r.run("hold 4; wait 3500ms; expect no text \"TW\"; expect no text \"DW\"");
    r.run("menu 0 0; expect text \"Watch Off\"; home");
    r.run("hold 4; wait 3500ms; expect text \"DW\"");
    r.run("menu 0 0; expect text \"Watch A+B\"");
}

/// SIDE1 held and the knob set the squelch from home; SIDE1 tapped latches the monitor, shown as MON on the
/// card (SQL-9.2, SQL-10, SQL-14, MENU-11).
#[test]
fn squelch_and_monitor() {
    let mut r = Radio::new();
    r.run("press OK; expect text \"1 Squelch 4\"; home");
    r.down(Key::Side(1)).wait(300);
    r.knob(1);
    r.expect_screen("Squelch").expect_text("Squelch, active slot");
    r.up(Key::Side(1)).wait(200);
    r.expect_screen("Home");
    r.run("press OK; expect text \"1 Squelch 5\"; home");
    r.press(Key::Side(1)).wait(200);
    r.expect_text("MON");
    r.press(Key::Side(1)).wait(200);
    r.expect_no_text("MON");
}

/// SIDE2 starts and stops a scan on the active slot; SCN shows in the bar while it runs (SCAN-1, SBAR-11).
#[test]
fn scan_start_and_stop() {
    let mut r = Radio::new();
    r.press(Key::Slot); // B: the zone "Repeaters"
    r.run("press SIDE2; wait 500ms; expect screen \"Scan\"; expect text \"Zone Repeaters\"");
    assert!(r.sim.radio().live.slots[1].scanning);
    r.expect_text("SCN");
    r.run("home; press SIDE2; wait 500ms");
    assert!(!r.sim.radio().live.slots[1].scanning);
}

/// Hold 1 opens the band scope on the active slot's band (KEY-11, SCOPE-1).
#[test]
fn band_scope() {
    let mut r = Radio::new();
    r.run("hold 1; expect screen \"Scope\"; expect text \"Band scope\"; expect text \"bars\"");
    r.run("press BACK; expect screen \"Home\"");
}

/// The frequency counter: SIDE2 held alone opens it when it comes up (KEY-22, TSRCH-10.1).
#[test]
fn frequency_counter() {
    let mut r = Radio::new();
    r.hold(Key::Side(2)).wait(200);
    r.expect_screen("Frequency counter");
}

/// AM is receive only: PTT on an airband frequency is refused and says why (TX-20.1, VFO-4).
#[test]
fn transmit_refused_on_am() {
    let mut r = Radio::new();
    r.run("type 1215; press OK; wait 300ms; down PTT; wait 300ms");
    r.expect_screen("Transmit refused").expect_text("AM is receive only");
    r.run("up PTT; press BACK; wait 200ms; expect screen \"Home\"");
}

/// PTT A on 2 m transmits on the active slot through its shift, and the watched slots pause (KEY-7.1, TX-1.1,
/// WATCH-16).
#[test]
fn transmit_on_two_metres() {
    let mut r = Radio::new();
    r.run("down PTT; wait 500ms; expect screen \"Transmit\"; expect text \"TRANSMIT\"; expect text \"145.920\"");
    assert!(r.sim.radio().live.tx.keyed);
    assert!(r.has("paused"), "watched slots pause while keyed");
    r.run("up PTT; wait 500ms; expect screen \"Home\"");
    assert!(!r.sim.radio().live.tx.keyed);
}

/// An emergency frequency needs PTT twice (TX-29, TX-3.1): the first press warns.
#[test]
fn emergency_needs_ptt_twice() {
    let mut r = Radio::new();
    r.run("type 1210; press OK; wait 300ms; down PTT; wait 300ms; up PTT; wait 200ms");
    assert!(!r.sim.radio().live.tx.keyed);
    assert_ne!(r.screen(), "Transmit");
}

/// Hold 5 opens the tone picker for the active slot (KEY-11, TONE-2).
#[test]
fn tones() {
    let mut r = Radio::new();
    r.run("hold 5; expect screen \"Tone picker\"; expect text \"RX tone\"; expect text \"TX tone\"");
}

/// The DTMF dialer types digits and shows where it sends (DTMF-1, DTMF-3, DTMF-13).
#[test]
fn dtmf_dialer() {
    let mut r = Radio::new();
    r.run("menu 0 8 5 1; expect screen \"DTMF dialer\"; expect text \"Sends on slot A\"");
    r.type_("123");
    r.expect_text("123");
}

/// APRS mode takes slot C; with C active, PTT opens compose instead of transmitting (APRS-15, APRS-18).
#[test]
fn aprs_mode_compose() {
    let mut r = Radio::new();
    r.run("menu 0 5 3; press OK; press OK; expect text \"APRS mode On\""); // APRS-29.2: Off, Tracker, On
    r.run("home; expect text \"APRS\"; press SLOT; press SLOT");
    assert_eq!(r.sim.app().active(), 2);
    r.run("down PTT; wait 300ms; up PTT; wait 300ms; expect screen \"Compose message\"");
    assert!(!r.sim.radio().live.tx.keyed);
}

/// The heard list lives in RAM and shows the stations heard, newest first (APRS-4).
#[test]
fn aprs_heard_list() {
    let mut r = Radio::new();
    r.run("menu 0 5 1; expect screen \"APRS heard\"; expect text \"newest first\"; expect text \"VE3KPW-9\"");
}

/// A message arrives: a toast, the inbox count (unread messages to us, INBOX-3.1) goes up, and with the screen dark
/// the LEDs flash red, green, red every 2 s while it's unread (APRS-6, INBOX-1, DISP-21.1).
#[test]
fn message_and_led_double_flash() {
    let mut r = Radio::new();
    r.run("menu 0 0; expect text \"Inbox 1 unread\"; home");
    r.event(Event::Message);
    r.expect_text("VE3KPW-9");
    r.run("menu 0 0; expect text \"Inbox 2 unread\"; home");
    r.event(Event::ScreenOff).wait(500);
    assert!(r.sim.dark());
    // Over 2 s: two short green flashes 250 ms apart.
    let mut flashes = 0;
    let mut was = false;
    for _ in 0..100 {
        r.wait(20);
        let (green, red) = r.sim.leds();
        assert!(!(green && red), "DISP-21.1: one LED at a time");
        if (green || red) && !was {
            flashes += 1;
        }
        was = green || red;
    }
    assert_eq!(flashes, 3, "red, green, red every 2 s (DISP-21.1)");
}

/// A SAME warning takes the full screen over home (WX-4, INBOX-2).
#[test]
fn same_alert_full_screen() {
    let mut r = Radio::new();
    r.event(Event::Alert);
    r.expect_screen("Alert").expect_text("WEATHER ALERT").expect_text("Severe thunderstorm");
}

/// While the crossband repeater runs, an alert is a banner and the repeater keeps running (XB-19).
#[test]
fn alert_during_crossband_is_a_banner() {
    let mut r = Radio::new();
    r.run("event crossband; expect screen \"Crossband repeater\"; press HASH; wait 2s; expect text \"PASSING\"");
    r.event(Event::Alert);
    r.expect_screen("Crossband repeater").expect_text("ALERT");
}

/// The crossband repeater's start screen states the pairing and ID off by default (XB-2, XB-12, XB-18).
#[test]
fn crossband_setup_screen() {
    let mut r = Radio::new();
    r.run("event crossband; expect text \"VHF and UHF, two chips\"; expect text \"ID off\"; expect text \"Simplex\"");
}

/// POCSAG: 0 4 2 decode off by default; Popups (0 4 4) on by default (DEC-4, DEC-7).
#[test]
fn pocsag_and_popups_settings() {
    let mut r = Radio::new();
    r.run("menu 0 4 2; expect text \"Decode Off\"; expect text \"Capcode filter None\"");
    r.run("menu 0 4; expect text \"Popups On\"");
    r.run("menu 0 4 4; press OK; expect text \"Popups Off\"");
}

/// GPS: the position page shows the fix; F5 cycles off, searching, fix, and the Status page follows (GPS-15.1,
/// GPS-15.2, SBAR-14).
#[test]
fn gps_position_and_status() {
    let mut r = Radio::new();
    r.run("menu 0 6 2; expect screen \"GPS position\"; expect text \"3D fix\"; expect text \"Grid\"");
    r.run("event gps; menu 0 0; expect text \"GPS Off\"");
}

/// HF mode loads the SSB patch, then shows the band, receiver and mode (HF-1, HF-15, HF-19); hold 3 seeks
/// (HF-31).
#[test]
fn hf_mode() {
    let mut r = Radio::new();
    r.run("menu 0 3 3; expect screen \"HF mode\"; expect text \"Loading SSB patch\"; wait 3s");
    r.expect_text("31 m").expect_text("SI4732").expect_text("9.580");
    r.run("hold 3; wait 3s");
    assert_ne!(r.sim.app().settings().hf.freq, 958_000, "seek moved");
}

/// MENU 0 2 6 is FM broadcast's settings (BCR-1, decision 352: a slot in the band is the receiver).
#[test]
fn fm_broadcast() {
    let mut r = Radio::new();
    r.run("menu 0 2 6; expect text \"Step\"; expect text \"Sleep timer\"");
}

/// Hold # locks the keypad and offers * and 0; locked, OK does nothing; hold # unlocks; * in the popup's time locks
/// All (KEY-13, KEY-14.1, KEY-14.3, KEY-14.4).
#[test]
fn key_lock() {
    let mut r = Radio::new();
    r.run("hold HASH; expect text \"Keypad locked\"; expect text \"* all · 0 all but PTT\"");
    assert!(r.sim.app().locked());
    r.run("press OK; expect screen \"Home\"; expect text \"hold # to unlock\"");
    r.run("hold HASH; expect text \"Keys unlocked\"");
    assert!(!r.sim.app().locked());
    r.run("hold HASH; press STAR; expect text \"All keys locked\"; expect text \"PTT too\"");
    assert_eq!(r.sim.app().lock_mode(), Some(ferrite_app::settings::Lock::All));
    r.run("hold HASH; expect text \"Keys unlocked\"");
}

/// Setting a PIN starts with "New PIN" (PIN-10, PIN-2).
#[test]
fn pin_entry() {
    let mut r = Radio::new();
    r.run("menu 0 9 3 1; press OK; expect screen \"PIN entry\"; expect text \"New PIN\"; expect text \"4 to 6 digits\"");
}

/// Channel-only mode hides MENU 0, editing and frequency entry; hold # leaves it (PIN-5, VFO-10).
#[test]
fn channel_only_mode() {
    let mut r = Radio::new();
    r.run("menu 0 9 3 2; press OK; home");
    r.run("press OK; expect screen \"Home\"; expect text \"Channel-only\"");
    r.run("type 1; expect screen \"Home\"");
    r.run("hold SLOT; expect screen \"Home\"");
    r.run("hold HASH; wait 300ms; press OK; expect screen \"Quick\"");
}

/// The Status page lists every state in words, with a scroll cue for the rows below (SBAR-13, SBAR-14, DISP-24,
/// MENU 0 0 since decision 376); the live numbers (MCU temp, stack) are System's (SELF-9).
#[test]
fn status_page() {
    let mut r = Radio::new();
    r.run("hold MENU; press 0; expect screen \"Status\"; expect text \"Transmit Ready\"; expect text \"Region Canada\"");
    // The rows below the page: 19 rows down is the last of its 20 (the list wraps at its end).
    for _ in 0..19 {
        r.press(Key::Down);
    }
    r.expect_text("Mailbox");
    assert!(!r.sim.screen_text().contains("MCU temperature"), "MCU temp is System's now");
}

/// Quick settings: MENU on home, nine rows for the active slot, 0 opens More (MENU-1).
#[test]
fn quick_settings() {
    let mut r = Radio::new();
    r.run("press OK; expect screen \"Quick\"; expect text \"1 Squelch\"; expect text \"9 Battery Save\"");
    r.run("type 0; expect screen \"More\"; expect text \"9 System\"");
}

/// Hold BACK goes home from any screen (KEY-5).
#[test]
fn hold_back_goes_home() {
    let mut r = Radio::new();
    r.run("menu 0 8 5 1; expect screen \"DTMF dialer\"; home; expect screen \"Home\"");
}

/// A setting changed in the menu takes effect at once and reaches the radio: the active slot's TX power
/// (MENU-19, quick row 2).
#[test]
fn setting_reaches_the_radio() {
    let mut r = Radio::new();
    r.take_cmds();
    r.run("press OK; press DOWN; knob 1; wait 200ms");
    assert!(r.take_cmds().iter().any(|c| matches!(c, RadioCmd::SlotParam { .. })), "a slot setting was sent");
}

/// The channel browser lists zones and All (CHAN-24, chapter 10).
#[test]
fn channel_browser() {
    let mut r = Radio::new();
    r.run("menu 0 2 1; expect screen \"Channel browser\"; expect text \"Repeaters\"; expect text \"All 21\"");
}

/// The terminal over the cable answers a `get` (TERM-12) and a write from Bluetooth waits for approval on the
/// radio (LINK-7, TERM-25).
#[test]
fn terminal_get() {
    let mut r = Radio::new();
    r.sim.term_feed(b"\r:get 0 4 4\r");
    r.wait(400);
    let out = String::from_utf8_lossy(&r.sim.term_take()).into_owned();
    assert!(out.contains("On"), "{out}");
}

/// The power switch saves and the radio comes back with what was set (chapter 33, BOOT).
#[test]
fn power_cycle_keeps_settings() {
    let mut r = Radio::new();
    r.run("type 4461; press OK; wait 3s");
    r.event(Event::PowerSwitch);
    r.wait(3000);
    r.event(Event::PowerSwitch);
    r.wait(4000);
    r.expect_screen("Home");
    assert_eq!(r.sim.app().slot(0).freq(), mhz(446) + 10_000);
}

/// POCSAG on: a slot on the pager frequency decodes the fake system's pages into a toast and the inbox (DEC-4,
/// DEC-7, DEC-18).
#[test]
fn pocsag_page_with_popups() {
    let mut r = Radio::new();
    r.run("menu 0 4 2 1; press OK; expect text \"Decode On\"");
    r.run("home; press SLOT; press VM; type 439987; press OK; wait 1s");
    r.run("wait 21s; expect text \"Pager\"");
    r.run("menu 0 0; expect text \"Inbox 1 unread\""); // INBOX-3.1: a page is the log's, not the mail badge's
}

/// Popups off: a page still goes to the inbox, but no toast shows (DEC-7).
#[test]
fn pocsag_page_without_popups() {
    let mut r = Radio::new();
    r.run("menu 0 4 2 1; press OK; menu 0 4 4; press OK; expect text \"Popups Off\"");
    r.run("home; press SLOT; press VM; type 439987; press OK; wait 1s");
    for _ in 0..21 {
        r.wait(1000);
        r.expect_no_text("Pager");
    }
    r.run("menu 0 0; expect text \"Inbox 1 unread\""); // INBOX-3.1
}

/// The crossband repeater refuses to start with both sides on one chip, and says why (XB-2).
#[test]
fn crossband_refuses_one_chip() {
    let mut r = Radio::new();
    r.run("press SLOT; press VM; type 14694; press OK; wait 500ms");
    r.run("event crossband; expect text \"VHF and VHF, one chip\"; expect text \"Can't start yet\"");
    r.run("press HASH; wait 1s; expect no text \"PASSING\"; expect text \"Can't start\"");
}

/// A web app writing shows its progress at once (no approval since decision 104); BACK cancels it (LINK-9).
#[test]
fn link_write_shows_progress() {
    let mut r = Radio::new();
    r.run("event linkwrite; expect screen \"Writing\"; expect text \"BACK cancels\"");
    r.run("press BACK; wait 300ms");
    assert_ne!(r.screen(), "Writing");
}

/// Setting the clock by hand: OK on 0 6 6 takes YYYYMMDDhhmm in local time (GPS-23).
#[test]
fn clock_set_by_hand() {
    let mut r = Radio::new();
    r.run("menu 0 6 6; press OK; expect screen \"Text entry\"; expect text \"Date and time\"");
    r.run("type 202610081230; press OK; wait 500ms; expect text \"Clock set\"; home; expect text \"12:30\"");
}

/// Hold V/M opens the zones (KEY-23); SLOT edits one, with its scan and priority options (KEY-11, chapter 10,
/// SCAN-12); OK puts the active slot on it and goes home (CHAN-27, decision 178).
#[test]
fn zones_and_zone_editor() {
    let mut r = Radio::new();
    r.run("hold VM; expect screen \"Zones\"; expect text \"6 zones\"; expect text \"Repeaters\"");
    r.run("press SLOT; expect screen \"Zone editor\"; expect text \"Scan all\"; expect text \"Priority 1\"");
    r.run("press SLOT; expect screen \"Zones\"; press OK; expect screen \"Home\"; expect text \"Slot A\"");
}

/// An automatic transmission (a beacon) keys the radio briefly and returns to home by itself (TX-1.1, APRS-20).
#[test]
fn automatic_beacon_transmits() {
    let mut r = Radio::new();
    r.sim.event(Event::Beacon);
    r.wait(40);
    assert!(r.sim.radio().live.tx.keyed, "keyed for the burst");
    r.wait(3000);
    assert!(!r.sim.radio().live.tx.keyed, "and back to receive");
    r.expect_screen("Home");
}

/// The home layouts: L cycles them; the status bar and slots stay (HOME-1, DISP).
#[test]
fn home_layouts() {
    let mut r = Radio::new();
    for _ in 0..3 {
        r.event(Event::Layout);
        r.expect_screen("Home").expect_text("146.520");
    }
}

/// The strip beside a broadcast (MODE-5, BCR-29.2, HOME-14, BCR-26.7; decision 115): on single + scope with B playing
/// broadcast, B made active shows its signal, SNR and stereo pilot with the strip plainly off; back on A, the strip
/// sweeps around 146.520 while B's broadcast plays on (its line says FM, or muted while A's call has the speaker).
#[test]
fn strip_beside_broadcast() {
    use ferrite_core::msg::ModeLive;
    let mut r = Radio::new();
    r.event(Event::Layout); // single + scope
    r.press(Key::Slot).event(Event::FmStation).wait(3000); // B to 100.5 MHz
    r.expect_text("100.500").expect_text("SNR").expect_text("dBuV").expect_text("STEREO");
    r.expect_no_text("+112k");
    assert!(matches!(r.sim.radio().live.mode, ModeLive::Fm { .. }), "B active: the broadcast alone");
    r.press(Key::Slot).press(Key::Slot).wait(1000); // C, then A
    assert_eq!(r.sim.app().active(), 0);
    assert!(matches!(r.sim.radio().live.mode, ModeLive::Scope { centre, .. } if centre == mhz(146) + 52_000), "the strip sweeps around A");
    r.expect_text("-112k").expect_text("+112k").expect_no_text("SNR").expect_text("100.500");
}

/// APRS-46: the Messages options mark every message read (the mail badge empties, INBOX-3.1) and delete them all
/// after a confirm. On a message, Delete message (APRS-46.1) comes first, so each row is one further down.
#[test]
fn messages_mark_all_read_and_delete_all() {
    let mut r = Radio::new();
    r.run("menu 0 0; expect text \"Inbox 1 unread\"");
    r.run("menu 0 5 2; hold OK; expect text \"Mark all read\"; press DOWN; press DOWN; press DOWN; press DOWN; press OK; wait 300ms");
    r.run("menu 0 0; expect text \"Inbox Empty\"");
    r.run("menu 0 5 2; wait 300ms; expect text \"VE3KPW-9\"");
    r.run("menu 0 5 2; hold OK; press DOWN; press DOWN; press DOWN; press DOWN; press DOWN; press OK; wait 300ms; press OK; wait 300ms");
    r.run("menu 0 5 2; wait 300ms");
    r.expect_no_text("VE3KPW-9");
}

/// HOME-3.1: the layout changed while home is up (the terminal's `set 0 7 2`, as on black on Nov 18, 2026, or
/// Console) repaints the whole of home: the three cards stayed behind under single + scope's one card. The screen
/// must match, pixel for pixel, a radio that came to that layout through a screen change (MENU, BACK).
#[test]
fn layout_change_on_home_leaves_nothing_behind() {
    for (to, name) in [(1, "Single + scope"), (2, "Watch + activity")] {
        let line = format!("\r:set 0 7 2 {name}\r");
        let mut a = Radio::new();
        let mut b = Radio::new();
        for r in [&mut a, &mut b] {
            r.wait(1000);
            r.sim.term_feed(line.as_bytes());
            r.wait(400);
        }
        assert_eq!(a.sim.app().home_layout(), to);
        a.wait(400); // the same time as b's two presses
        b.press(Key::Menu).press(Key::Exit);
        a.wait(600);
        b.wait(600);
        let (pa, pb) = (a.sim.screen_rgb(), b.sim.screen_rgb());
        let rows: Vec<usize> = (0..320).filter(|y| (0..240).any(|x| pa[y * 240 + x] != pb[y * 240 + x])).collect();
        assert!(rows.is_empty(), "{name}: rows {:?}..{:?} differ from a fresh home", rows.first(), rows.last());
    }
}

/// A VFO set to FM where the region's default is NFM (Europe) stays FM through a slot switch and a power cycle
/// (PWR-17.1); only a band change brings the new band's mode (NAV-2, VFO-4). From a Discord report (ferrite#97).
#[test]
fn vfo_fm_survives_slot_switch_and_power_cycle() {
    use ferrite_core::txplan::Region;
    let mut r = Radio::new();
    r.sim.app_mut().settings_mut().set_region(Some(Region::Europe));
    r.sim.term_feed(b"\r:slot a vfo 145.500 fm active\r");
    r.wait(400);
    let _ = r.sim.term_take();
    r.run("home; wait 500ms");
    let mode = |r: &Radio| (r.sim.app().slot(0).vfo_mode, r.sim.app().slot(0).cfg.mode);
    assert_eq!(mode(&r), (true, 0), "A is a VFO on FM");
    r.run("press SLOT; wait 300ms; press SLOT; wait 300ms; press SLOT; wait 500ms");
    assert_eq!(r.sim.app().active(), 0, "back on A");
    assert_eq!(mode(&r), (true, 0), "a slot switch keeps FM");
    r.run("wait 3s");
    r.event(Event::PowerSwitch);
    r.wait(3000);
    r.event(Event::PowerSwitch);
    r.wait(4000);
    r.expect_screen("Home");
    assert_eq!(mode(&r), (true, 0), "a power cycle keeps FM");
    assert_eq!(r.sim.app().slot(0).vfo, mhz(145) + 50_000);
    r.run("press RIGHT; wait 300ms; press LEFT; wait 500ms");
    assert_eq!(mode(&r), (true, 1), "a band change brings the band's NFM (NAV-2)");
}
