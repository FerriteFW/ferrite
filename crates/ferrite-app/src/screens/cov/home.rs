//! Home's cards (chapter 6): the broadcast card large and small, a scanning card, a split's header, the
//! clarifier, the APRS card's large rows, and the steps.

use super::*;
use crate::aprs::{AprsCfg, APRS_SLOT};
use ferrite_core::freq::mhz;
use ferrite_core::msg::ModeLive;

const FM1005: ferrite_core::freq::Freq10 = mhz(100) + 50_000;

fn live_fm(t: &mut T, rt: &[u8], ps: &[u8; 8], rssi: u8) {
    let mut r = [0u8; 64];
    r[..rt.len()].copy_from_slice(rt);
    t.live.mode = ModeLive::Fm { freq: 10_050, ps: *ps, rt: r, rssi, snr: 20, stereo: true, pty: 0, heard: true };
    t.live.bcast_heard = true;
}

/// A on 100.5 MHz broadcast, playing.
fn on_fm() -> T {
    let mut t = T::ready();
    t.app.set_vfo(0, FM1005);
    t.wait(100);
    assert_eq!(t.app.bcast_slot(), Some(0));
    t
}

/// BCR-26.1 - BCR-26.3: the large broadcast card: the frequency, STEREO, the SNR, and the radio text whole; the
/// station name alone when there is no radio text; "No RDS" with neither.
#[test]
fn broadcast_card_large() {
    let mut t = on_fm();
    live_fm(&mut t, b"Some radio text that runs on for more than one line of the card", b"STATION ", 40);
    t.wait(100);
    t.shows("100.5");
    t.shows("STEREO");
    t.shows("SNR 20 dB");
    t.shows("Some radio text");
    assert!(t.px(0, 300, ferrite_ui::gfx::GREEN) > 0, "the signal green while it plays");
    live_fm(&mut t, b"", b"STATION ", 40);
    t.wait(100);
    t.shows("STATION");
    live_fm(&mut t, b"", b"        ", 0);
    t.wait(100);
    t.shows("No RDS");
    t.hides("SNR");
}

/// BCR-22, BCR-26.3: on a small card the text is one line; another slot in the band says where the broadcast
/// plays instead.
#[test]
fn broadcast_card_small_and_elsewhere() {
    let mut t = on_fm();
    live_fm(&mut t, b"Some text", b"STATION ", 40);
    t.app.set_vfo(1, mhz(99) + 10_000);
    t.wait(100);
    assert_eq!(t.app.bcast_slot(), Some(1), "the last into the band has it");
    t.shows("Broadcast plays on B"); // on A's large card
    t.shows("99.1");
    t.tap(Key::Slot); // B active: A's card is small
    t.wait(100);
    t.shows("Broadcast plays on B");
    t.shows("Some text");
}

/// BCR-26.9 (ferrite#178): a slot on a saved broadcast channel shows its name beside the frequency, which stays
/// large: in the SNR's place on the large card, in the stereo tag's on the small one. A VFO shows the SNR as before.
#[test]
fn broadcast_channel_shows_its_name() {
    use crate::chan::{ChanWrite, Channel, NO_ZONE};
    let mut t = T::ready();
    let mut name = [0u8; 16];
    name[..11].copy_from_slice(b"CBC Radio 2");
    let ch = Channel { num: 70, zone: NO_ZONE, rx: FM1005, name, cfg: crate::slot::DEFAULT_CFG, flags: 0, tones: 0 };
    t.s.apply(ChanWrite::Put { ch, from: None });
    t.taps(&[Key::VfoMem, Key::Digit(0), Key::Digit(7), Key::Digit(1), Key::Menu]); // channel mode, 071
    t.wait(100);
    assert_eq!(t.app.bcast_slot(), Some(0), "on the broadcast channel");
    live_fm(&mut t, b"Some text", b"STATION ", 40);
    t.wait(100);
    t.shows("100.5");
    t.shows("CBC Radio 2");
    t.shows("STEREO");
    t.hides("SNR"); // the name has its place
    t.tap(Key::Slot); // B active: A's card is small
    t.wait(100);
    t.shows("100.5");
    t.shows("CBC Radio 2");
}

/// HOME-4, HOME-4.1, LINK-19.4: a slot scanning on a small card shows its hits and the last hit's frequency, and
/// the readback carries the last hit as drawn.
#[test]
fn scanning_small_card() {
    let mut t = T::ready();
    let f = t.app.slot(1).freq();
    t.live.slots[1] = ferrite_core::msg::SlotLive { freq: f, scanning: true, hits: 3, last_hit: mhz(146) + 52_000, ..Default::default() };
    t.wait(100);
    t.shows("3 hits");
    t.shows("last 146.520");
    let card = |t: &T| t.readback().into_iter().find(|l| l.starts_with("card B")).unwrap();
    assert!(card(&t).contains("lasthit=\"146.520\""), "LINK-19.4: the readback has the last hit: {}", card(&t));
    t.live.slots[1].hits = 0;
    t.wait(100);
    t.shows("0 hits");
    t.hides("last ");
    assert!(!card(&t).contains("lasthit="), "{}", card(&t));
}

/// HOME-4: a split's TX frequency takes the band's place in the active card's header.
#[test]
fn split_header() {
    let mut t = T::ready();
    let a = t.app.active as usize;
    t.app.slots[a].cfg.split = true;
    t.app.slots[a].cfg.offset = (mhz(147) + 30_000) as i32;
    t.wait(100);
    t.shows("\u{b7} TX 147.300");
}

/// VFO-22: SIDE1 + \u{25c0}\u{25b6} move the clarifier; the active card shows it, in amber, beside the state.
#[test]
fn clarifier_on_the_card() {
    let mut t = T::ready();
    t.app.key_down(Key::Side(1), t.t, &t.s);
    t.tap(Key::Right);
    t.tap(Key::Right);
    t.app.key_up(Key::Side(1), t.t, &t.s);
    t.wait(100);
    assert_ne!(t.app.slot(t.app.active).clar, 0);
    t.shows("CLR +0.2");
    assert!(t.px(25, 50, ferrite_ui::gfx::AMBER) > 0, "the clarifier in amber");
}

/// NAV-3: SIDE2 held, \u{25b2} steps 1 MHz.
#[test]
fn side2_up_is_a_big_step() {
    let mut t = T::ready();
    let f = t.app.slot(0).freq();
    t.app.key_down(Key::Side(2), t.t, &t.s);
    t.tap(Key::Up);
    t.app.key_up(Key::Side(2), t.t, &t.s);
    t.wait(100);
    assert_eq!(t.app.slot(0).freq(), f + 100_000);
    assert_eq!(t.screen(), Screen::Home, "SIDE2's release after a step opens nothing");
    assert_eq!(t.screen().mockup(), Some("HomeA.png"));
}

/// APRS-17.2: APRS mode's large card shows both counts; KISS's shows its port; APRS mode with timed beacons, the
/// countdown (beacons need APRS mode, APRS-29.1).
#[test]
fn aprs_card_large_rows() {
    let mut t = T::ready();
    t.app.set.bt = true;
    t.app.set.kiss_port = 2;
    t.wait(100);
    while t.app.active() != APRS_SLOT {
        t.tap(Key::Slot);
    }
    t.wait(3500);
    t.shows("Bluetooth");
    t.app.set.kiss_port = 0;
    t.app.set_aprs_mode(true);
    t.app.set_aprs(AprsCfg { beacon: 5, ..*t.app.aprs_cfg() });
    t.s.aprs_tick(10, true, true);
    t.wait(100);
    t.shows("Beacon in");
}

/// NAV-1: \u{25b2}\u{25bc} on a channel slot step the zone's channels; a zone with none does nothing.
#[test]
fn channel_step_in_an_empty_zone() {
    let mut t = T::ready();
    let a = t.app.active as usize;
    let ch = crate::chan::ChannelStore::zone_at(&t.s, 0, 0).unwrap();
    t.app.slots[a].ch = Some(ch);
    t.app.slots[a].vfo_mode = false;
    t.app.slots[a].zone = 0;
    let empty = crate::chan::Table { zones: &[b"Empty"], channels: &[] };
    let before = t.app.slots[a].pos;
    t.app.key_down(Key::Up, t.t, &empty);
    t.app.key_up(Key::Up, t.t + 100, &empty);
    assert_eq!(t.app.slots[a].pos, before, "nothing to step to");
    // V/M with no channels at all says so.
    t.app.slots[a].vfo_mode = true;
    t.app.slots[a].ch = None;
    t.app.key_down(Key::VfoMem, t.t, &empty);
    t.app.key_up(Key::VfoMem, t.t + 100, &empty);
    assert_eq!(t.toast(), "No channels yet / ");
}

/// VFO-18.2: on the airband with the 8.33 kHz step every home layout shows the channel name ("118.010" for
/// 118.00833) without 10 Hz digits, and 121.500 as itself; with another step, the frequency.
#[test]
fn airband_833_names() {
    for l in 0..3 {
        let mut t = T::ready();
        t.app.set_layout(l);
        let a = t.app.active;
        t.app.set_vfo(a, mhz(118) + 833);
        t.app.slots[a as usize].cfg_mut().step = crate::slot::STEP_833;
        t.wait(100);
        t.shows("118.010");
        t.hides("118.008 33"); // the single layout's scope strip keeps its centre's frequency (a scale)
        t.hides("118.010 33");
        t.app.set_vfo(a, mhz(121) + 50_000);
        t.app.slots[a as usize].cfg_mut().step = crate::slot::STEP_833;
        t.wait(100);
        t.shows("121.500"); // a block's base keeps its 25 kHz name, not 121.505
        t.hides("121.505");
        t.app.set_vfo(a, mhz(118) + 833);
        t.app.slots[a as usize].cfg_mut().step = 11; // 25 kHz
        t.wait(100);
        t.shows("118.008 33");
        t.hides("118.010");
    }
}
