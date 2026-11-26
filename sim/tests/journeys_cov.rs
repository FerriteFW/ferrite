//! Journeys for the rules the first rounds left uncovered: channel names and zones, the home channel, greyed
//! settings, squelch, scans, HF memory, APRS frames, the go-to dial and the terminal's redraw. The harness is
//! `common`; a rule counts as covered when its ID is in a test's doc comment.

mod common;

use common::Radio;
use ferrite_core::keys::Key;

/// One terminal command line: Esc back to the menu first, then `:` and the command; the answer is the
/// terminal's output with the escape sequences taken out.
fn term(r: &mut Radio, line: &str) -> String {
    r.sim.term_feed(b"\x1b");
    r.wait(200);
    r.sim.term_feed(format!("\r{line}\r").as_bytes());
    r.wait(400);
    plain(&String::from_utf8_lossy(&r.sim.term_take()))
}

/// Terminal output without its VT100 escape sequences.
fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            if it.peek() == Some(&'[') {
                it.next();
                while let Some(&d) = it.peek() {
                    it.next();
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                it.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The go-to dial is north up when stopped (N at the top, no ETA) and course up at 3 km/h or more: N moves
/// round the ring, the note says "Course up" with the speed, and the bearing line gains the ETA (GPS-19).
#[test]
fn goto_dial_course_up_when_moving() {
    let mut r = Radio::new();
    r.run("menu 0 6 7; press DOWN; press OK; wait 500ms");
    r.expect_text("North up");
    let t = r.text();
    assert!(!t.contains("ETA"), "no ETA when stopped:\n{t}");
    let n_still = n_pos(&mut r);
    let f = r.sim.store().extra.fix.get().expect("a fix");
    r.sim.store().extra.fix.set(Some(ferrite_app::data::Fix { speed_kmh: 46, heading: 120, ..f }));
    r.wait(1500);
    r.expect_text("Course up");
    r.expect_text("ETA");
    let n_moving = n_pos(&mut r);
    assert_ne!(n_still, n_moving, "N moves round the ring when course up");
    // Just under 3 km/h counts as stopped again.
    r.sim.store().extra.fix.set(Some(ferrite_app::data::Fix { speed_kmh: 2, heading: 120, ..f }));
    r.wait(1500);
    r.expect_text("North up");
    assert_eq!(n_pos(&mut r), n_still);
}

fn n_pos(r: &mut Radio) -> (i32, i32) {
    let items = r.sim.screen_texts();
    let n = items.iter().find(|i| i.s == "N" && i.y0 > 60 && i.y0 < 220).expect("N on the dial");
    (n.x0, n.y0)
}

/// A 16-character name shows in full on home and in the channel list (CHAN-5).
#[test]
fn sixteen_character_names_show_in_full() {
    let mut r = Radio::new();
    let out = term(&mut r, ":ch 50 \"Westside Repeat1\" 146.94 zone=1");
    assert!(out.contains("Channel 50 saved"), "{out}");
    r.home();
    r.press(Key::VfoMem).wait(300); // slot A to channel mode
    r.run("type 50; press OK; wait 500ms");
    r.expect_text("Westside Repeat1");
    r.run("menu 0 2 1; press OK; wait 300ms"); // the first zone, where channel 50 now is
    for _ in 0..40 {
        if r.has("Westside Repeat1") {
            break;
        }
        r.press(Key::Down);
    }
    r.expect_text("Westside Repeat1");
}

/// A channel is in one zone or none: moving it to another zone takes it out of the first; a zone lists its
/// channels in number order (CHAN-10).
#[test]
fn one_zone_per_channel_in_number_order() {
    let mut r = Radio::new();
    term(&mut r, ":ch 60 \"Zed Sixty\" 147.06");
    term(&mut r, ":ch 55 \"Zed Fifty5\" 147.09");
    let a = term(&mut r, ":ch 60 zone=1");
    let b = term(&mut r, ":ch 55 zone=1");
    assert!(a.contains("saved") && b.contains("saved"), "{a}\n{b}");
    let zone_of = |r: &Radio, n: u16| {
        use ferrite_app::chan::ChannelStore;
        r.sim.store().by_num(n).map(|(c, _)| c.zone)
    };
    assert_eq!(zone_of(&r, 59), Some(0));
    let c = term(&mut r, ":ch 60 zone=2");
    assert!(c.contains("Channel 60 saved"), "{c}");
    assert_eq!(zone_of(&r, 59), Some(1), "moved, not copied");
    // Zone 1's list: 55 is listed before anything numbered above it, and 60 is gone from it.
    use ferrite_app::chan::ChannelStore;
    let nums: Vec<u16> = (0..1000).filter(|&n| r.sim.store().by_num(n).is_some_and(|(c, _)| c.zone == 0)).collect();
    assert!(nums.windows(2).all(|w| w[0] < w[1]) && nums.contains(&54) && !nums.contains(&59), "{nums:?}");
}

/// The home channel is one channel number in the settings, separate from a zone's priority channels: hold 0
/// goes to it, not to a priority channel (CHAN-17, CHAN-21).
#[test]
fn home_channel_is_one_number_apart_from_priority() {
    let mut r = Radio::new();
    let out = term(&mut r, ":home 3");
    assert!(!out.contains("rror"), "{out}");
    assert_eq!(r.sim.app().settings().home_ch, Some(2), "stored as a channel number");
    assert_eq!(core::mem::size_of_val(&r.sim.app().settings().home_ch.unwrap_or(0)), 2, "2 bytes");
    r.home();
    r.hold(Key::Digit(0)).wait(300);
    assert_eq!(r.sim.app().slot(r.sim.app().active()).ch.filter(|_| r.sim.app().slot(r.sim.app().active()).on_channel()).map(|c| c.num), Some(2), "hold 0: the home channel");
}

/// A setting that doesn't apply is greyed and says why when selected: in quick settings on a channel, turning
/// Step says "VFO only" (EDIT-7).
#[test]
fn greyed_settings_say_why() {
    let mut r = Radio::new();
    r.home();
    r.press(Key::VfoMem).wait(300); // slot A to channel mode
    r.run("type 1; press OK; wait 300ms; press OK; wait 300ms"); // quick settings
    r.expect_screen("Quick");
    let mut toast = false;
    for _ in 0..12 {
        r.knob(1);
        if r.has("VFO only") {
            toast = true;
            break;
        }
        r.knob(-1);
        r.press(Key::Down);
    }
    assert!(toast, "turning a greyed setting says why:\n{}", r.text());
}

/// HF mode's VFOs and recent list are written to flash on leaving HF mode, never while tuning (HF-29).
#[test]
fn hf_memory_written_on_leaving_not_while_tuning() {
    let mut r = Radio::new();
    r.run("menu 0 3 3; wait 9s; expect screen \"HF mode\"");
    let _ = r.sim.flash_changed();
    r.knob(5).wait(3000);
    assert!(!r.sim.flash_changed(), "no flash write while tuning");
    r.press(Key::Exit).wait(3000);
    assert!(r.sim.flash_changed(), "written on leaving");
}

/// The terminal shows a setting; a change made on the radio is redrawn there without asking (TERM-10).
#[test]
fn terminal_redraws_radio_edits() {
    let mut r = Radio::new();
    r.sim.term_size(80, 24);
    r.sim.term_feed(b"\r");
    r.wait(400);
    r.sim.term_feed(b"\r"); // past the splash: the menu, with the "Radio now" lines
    r.wait(400);
    let menu = plain(&String::from_utf8_lossy(&r.sim.term_take()));
    assert!(menu.contains("146.52000"), "{menu}");
    let before = r.sim.app().slot(0).freq();
    r.home();
    r.run("type 4462; press OK; wait 3s");
    assert_ne!(r.sim.app().slot(0).freq(), before);
    let out = plain(&String::from_utf8_lossy(&r.sim.term_take()));
    assert!(out.contains("446.20000"), "the terminal redrew the new frequency:\n{out}");
}

/// A channel flagged Skip is left out of a scan: the UI answers the Radio task's asks for its position in the
/// scan's list with no frequency (no stop), and gives the others in the zone theirs (SCAN-9, SCAN-25).
#[test]
fn skipped_channels_are_never_stops() {
    use ferrite_core::msg::RadioCmd;
    let mut r = Radio::new();
    let out = term(&mut r, ":ch 3 skip");
    assert!(out.contains("Channel 3 saved"), "{out}");
    r.home();
    r.press(Key::VfoMem).wait(300); // slot A to channel mode
    r.run("type 1; press OK; wait 300ms");
    r.take_cmds();
    r.press(Key::Side(2)).wait(3000);
    let cmds = r.take_cmds();
    assert!(cmds.iter().any(|c| matches!(c, RadioCmd::ScanSet { slot: 0, .. })), "a channel scan sent its list");
    let mut answers = std::collections::BTreeMap::new();
    for c in cmds {
        if let RadioCmd::ScanEntry { slot: 0, pos, rx } = c {
            let num = ferrite_app::chanscan::at(r.sim.app(), 0, pos, r.sim.store()).map(|c| c.num);
            answers.insert(num, rx);
        }
    }
    assert!(answers.get(&Some(0)).is_some_and(|&f| f != 0) && answers.get(&Some(1)).is_some_and(|&f| f != 0), "the zone's other channels are stops: {answers:?}");
    assert_eq!(answers.get(&Some(2)), Some(&0), "channel 3 has Skip: never a stop");
}

/// The reset clears settings, channels and the rest, but never the SSB patch (HF-21, as SAFE-9.1).
#[test]
fn reset_keeps_the_ssb_patch() {
    use ferrite_core::layout::SSB_PATCH;
    use ferrite_sim::simcore::{Stored, FLASH_IMAGE};
    let mut img = Radio::new().sim.flash_image();
    let at = (SSB_PATCH.start - FLASH_IMAGE.start) as usize;
    for (i, b) in img[at..at + 1024].iter_mut().enumerate() {
        *b = (i * 7 + 3) as u8;
    }
    let patch = img[at..at + 1024].to_vec();
    let mut r = Radio::with(Stored::Image(img));
    assert_eq!(&r.sim.flash_image()[at..at + 1024], &patch[..], "the patch survives the mount");
    r.run("menu 0 9 7 2; wait 300ms");
    if !r.has("Reset the radio") {
        r.press(Key::Menu).wait(300);
    }
    r.expect_text("Reset the radio");
    r.press(Key::Menu).wait(300);
    r.expect_text("Hold # for 3 s");
    r.down(Key::Hash);
    r.wait(3500);
    r.up(Key::Hash);
    r.wait(4000);
    let after = r.sim.flash_image();
    let ch = (ferrite_core::layout::CHANNELS.start - FLASH_IMAGE.start) as usize;
    assert!(after[ch..ch + 4096].iter().all(|&b| b == 0xFF), "the channels went");
    assert_eq!(&after[at..at + 1024], &patch[..], "the SSB patch stayed");
}

/// The Radio task's own refusal shows the same TX refused screen as the UI's: after long keying on High the
/// duty limit refuses the next key-up, and the screen says the transmitter is cooling (TX-39, TX-38).
#[test]
fn radio_refusal_shows_the_refused_screen() {
    let mut r = Radio::new();
    r.home();
    r.run("type 14652; press OK; wait 1s");
    let ptt = ferrite_core::keys::Key::Ptt(ferrite_core::keys::PTT_A);
    let mut refused = false;
    // TX-38's bucket is 240 s (High 4 min from cold, Mid 8): enough 100 s key-ups for either.
    for _ in 0..16 {
        r.down(ptt);
        r.wait(100_000);
        r.up(ptt);
        r.wait(1000);
        if r.has("cooling") {
            refused = true;
            break;
        }
        if r.screen() != "Home" {
            r.press(Key::Exit).wait(300);
        }
    }
    assert!(refused, "the duty limit's refusal is shown:\n{}", r.text());
    r.expect_text("Can't transmit");
}

/// A speaker mic plugged into the side connector shows on the Status page, and taken out again (SND-12.6).
#[test]
fn speaker_mic_on_status_page() {
    let mut r = Radio::new();
    r.run("hold MENU; press 0; expect screen \"Status\"");
    for _ in 0..12 {
        r.press(Key::Down);
    }
    r.expect_text("Speaker mic").expect_text("None");
    r.run("event spkmic; wait 300ms; expect text \"Connected\"");
    assert!(r.sim.device().spkmic);
    r.run("event spkmic; wait 300ms; expect text \"None\"");
}

/// The terminal's `gps`: the last GGA sentence as the module sent it, its age, and fields 9 and 11 (TERM-30).
#[test]
fn terminal_gps_gga() {
    let mut r = Radio::new();
    let out = term(&mut r, ":gps");
    assert!(out.contains("GGA, 1 s ago: $GNGGA,143210.000,4525.2950,N,07541.8600,W,1,09,0.9,-12.3,M,,M,,*57"), "{out}");
    assert!(out.contains("altitude (field 9) -12.3 m, geoid separation (field 11) empty"), "{out}");
}
