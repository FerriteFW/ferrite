//! Home's other two layouts (HOME-3): single + scope and watch + activity, on a channel, scanning, on
//! broadcast FM, and with the S-meter in dBm.

use super::*;
use ferrite_core::freq::mhz;
use ferrite_core::msg::{ModeLive, SlotLive};

fn layout(l: u8) -> T {
    let mut t = T::ready();
    t.app.set_layout(l);
    t.wait(100);
    t
}

fn on_channel(t: &mut T, s: u8) -> String {
    t.app.set_channel(s, 0, 0, &t.s);
    t.wait(100);
    String::from_utf8_lossy(t.app.slot(s).ch.unwrap().name()).into_owned()
}

fn fm_live(t: &mut T, s: u8) {
    t.app.set_vfo(s, mhz(100) + 50_000);
    let mut rt = [0u8; 64];
    rt[..9].copy_from_slice(b"Some text");
    t.live.mode = ModeLive::Fm { freq: 10_050, ps: *b"STATION ", rt, rssi: 40, snr: 20, stereo: true, pty: 0, heard: true };
    t.live.bcast_heard = true;
    t.wait(200);
    assert_eq!(t.app.bcast_slot(), Some(s));
}

/// HOME-3, HOME-4: single + scope on a channel: its name large, the settings with a tone and a shift and a step
/// under 1 kHz, the other slots a line each with their channel's name.
#[test]
fn single_on_a_channel() {
    let mut t = layout(1);
    let a = t.app.active;
    let name = on_channel(&mut t, a);
    t.shows(&name);
    let other = (a + 1) % 3;
    let oname = on_channel(&mut t, other);
    t.shows(&oname);
    // a VFO with a tone, a shift and a 100 Hz step
    t.app.set_vfo(a, mhz(146) + 94_000);
    let s = &mut t.app.slots[a as usize];
    (s.cfg.tx_tone, s.cfg.offset, s.cfg.step) = (1000, -60_000, 2);
    t.wait(100);
    t.shows("100 Hz");
    t.hides("Simplex");
    t.hides("None");
}

/// SQL-12.3: with 0 7 3 1 dBm the single layout's S-meter is the number.
#[test]
fn single_dbm_meter() {
    let mut t = layout(1);
    t.app.set.smeter_dbm = true;
    let a = t.app.active as usize;
    let f = t.app.slot(a as u8).freq();
    t.live.slots[a] = SlotLive { freq: f, squelch_open: true, carrier: true, rssi: -93, ..Default::default() };
    t.wait(200);
    t.shows("-93 dBm");
    t.app.set.smeter_dbm = false;
    t.wait(100);
    t.hides("dBm");
}

/// SQL-12.3: watch + activity: a receiving card's state carries the level as dBm when 0 7 3 1 says so, else
/// its S-reading (ferrite#216).
#[test]
fn activity_dbm_state() {
    let mut t = layout(2);
    let a = t.app.active as usize;
    let f = t.app.slot(a as u8).freq();
    t.live.slots[a] = SlotLive { freq: f, squelch_open: true, carrier: true, rssi: -93, ..Default::default() };
    t.wait(200);
    t.shows("RX S");
    t.hides("dBm");
    t.app.set.smeter_dbm = true;
    t.wait(100);
    t.shows("RX -93 dBm");
    t.live.slots[a].rssi = -127;
    t.wait(100);
    t.shows("RX -127 dBm");
}

/// BCR-26.7: single + scope on the broadcast slot: dBuV with the SNR and STEREO under the frequency, the RDS
/// text, the dBuV scale, and no strip; "No RDS" without text.
#[test]
fn single_broadcast() {
    let mut t = layout(1);
    let a = t.app.active;
    fm_live(&mut t, a);
    t.shows("40 dBuV \u{b7} SNR 20 dB");
    t.shows("STEREO");
    t.shows("Some text");
    t.shows("60");
    t.live.mode = ModeLive::Fm { freq: 10_050, ps: [b' '; 8], rt: [0; 64], rssi: 0, snr: 0, stereo: false, pty: 0, heard: true };
    t.wait(100);
    t.shows("No RDS");
    t.app.set_vfo(a, mhz(146) + 52_000); // out of the band: the settings come back
    t.wait(100);
    t.shows("Squelch");
}

/// HOME-3, HOME-4.1: watch + activity: a scanning card shows its hits and last hit; a channel card its name and
/// frequency small; the broadcast card FM, its RDS text and STEREO.
#[test]
fn activity_cards() {
    let mut t = layout(2);
    let other = (t.app.active + 1) % 3;
    let name = on_channel(&mut t, other);
    t.shows(&name);
    let f = t.app.slot(other).freq();
    t.shows(&String::from_utf8_lossy(ferrite_ui::widgets::freq3(f).as_bytes()));
    let third = (t.app.active + 2) % 3;
    t.live.slots[third as usize] = SlotLive { freq: mhz(147), scanning: true, hits: 4, last_hit: mhz(146) + 52_000, ..Default::default() };
    t.wait(100);
    t.shows("4 hits");
    t.shows("last 146.520");
    t.live.slots[third as usize] = SlotLive::default();
    let a = t.app.active;
    fm_live(&mut t, a);
    t.shows("WFM");
    t.shows("Some text");
    t.shows("STEREO");
}
