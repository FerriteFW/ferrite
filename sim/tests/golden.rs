//! Screenshot regression: every screen the simulator's jump list offers, and home in each layout and theme,
//! drawn by the firmware's own UI code at 240 x 320 and compared pixel for pixel with the goldens in
//! `sim/golden/`. Each screen starts from a fresh radio at a fixed time, so the fake radio's traffic and the clock
//! are the same on every run.
//!
//! A difference fails the test and writes `target/golden-diff/<name>.png`: the new screen dimmed, with the
//! changed pixels in magenta. When a change is meant, bless it:
//!
//! ```text
//! FERRITE_BLESS=1 cargo test -p ferrite-sim --test golden
//! ```
//!
//! which rewrites the goldens that changed (and adds new ones); review them in the commit like any other change.

mod common;

use common::Radio;
use ferrite_sim::simcore::{jump_names, Event};
use std::path::{Path, PathBuf};

const W: usize = 240;
const H: usize = 320;

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("golden")
}

fn diff_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/golden-diff")
}

fn bless() -> bool {
    std::env::var_os("FERRITE_BLESS").is_some_and(|v| v != "0")
}

/// A name for a file: "D6 Status page" -> "d6-status-page".
fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn write_png(path: &Path, px: &[u32]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let data: Vec<u8> = px.iter().flat_map(|&c| [(c >> 16) as u8, (c >> 8) as u8, c as u8]).collect();
    let mut enc = png::Encoder::new(std::fs::File::create(path).unwrap(), W as u32, H as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_compression(png::Compression::Best);
    enc.write_header().unwrap().write_image_data(&data).unwrap();
}

fn read_png(path: &Path) -> Option<Vec<u32>> {
    let dec = png::Decoder::new(std::fs::File::open(path).ok()?);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    assert_eq!((info.width as usize, info.height as usize, info.color_type), (W, H, png::ColorType::Rgb), "{}", path.display());
    Some(buf[..info.buffer_size()].chunks(3).map(|p| (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32).collect())
}

/// Compares `px` with the golden `name`; returns a failure line, or None. Blessing writes the golden instead.
fn check(name: &str, px: &[u32]) -> Option<String> {
    let path = golden_dir().join(format!("{name}.png"));
    let old = read_png(&path);
    if old.as_deref() == Some(px) {
        return None;
    }
    if bless() {
        write_png(&path, px);
        println!("blessed {name}");
        return None;
    }
    let Some(old) = old else {
        return Some(format!("{name}: no golden (run with FERRITE_BLESS=1 to add it)"));
    };
    let changed = old.iter().zip(px).filter(|(a, b)| a != b).count();
    let diff: Vec<u32> = old.iter().zip(px).map(|(&a, &b)| if a != b { 0xFF00FF } else { (b >> 2) & 0x3F3F3F }).collect();
    let out = diff_dir().join(format!("{name}.png"));
    write_png(&out, &diff);
    write_png(&diff_dir().join(format!("{name}.new.png")), px);
    Some(format!("{name}: {changed} pixels differ (see {})", out.display()))
}

/// A screen of the jump list on a fresh radio, a second after it opened.
fn shot_jump(i: usize) -> Vec<u32> {
    let mut r = Radio::new();
    r.sim.jump(i);
    r.wait(1000);
    r.sim.screen_rgb()
}

/// Home in layout `layout` (0-2) and theme `theme` (0 dark, 1 daylight, 2 night red, 3 cyberpunk).
fn shot_home(layout: u8, theme: u8) -> Vec<u32> {
    let mut r = Radio::new();
    r.sim.app_mut().settings_mut().theme = theme;
    for _ in 0..layout {
        r.event(Event::Layout);
    }
    r.home();
    r.wait(1000);
    r.sim.screen_rgb()
}

/// Every screen as reviewed, pixel for pixel: the goldens hold the approved look, so a change to the fonts and
/// sizes (DISP-6), the corners (DISP-19) or the centring of text in boxes (DISP-20) fails here until it is
/// reviewed and blessed.
#[test]
fn every_screen_matches_its_golden() {
    let names: Vec<String> = jump_names().collect();
    let mut failures = Vec::new();
    for (i, n) in names.iter().enumerate() {
        let name = format!("{:03}-{}", i, slug(n));
        failures.extend(check(&name, &shot_jump(i)));
    }
    assert!(failures.is_empty(), "{} of {} screens changed:\n{}", failures.len(), names.len(), failures.join("\n"));
}

#[test]
fn home_layouts_and_themes_match_their_goldens() {
    let mut failures = Vec::new();
    for layout in 0..3u8 {
        for (theme, tname) in ["dark", "daylight", "night", "cyber"].iter().enumerate() {
            let name = format!("home-layout{layout}-{tname}");
            failures.extend(check(&name, &shot_home(layout, theme as u8)));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// No text on the goldens' screens is cut off unasked (ferrite-ui's `clipcheck`): every screen of the jump
/// list and home in each layout and theme, all the cuts listed at once. (Every [`Radio`] also checks its own when
/// dropped, so each journey checks the screens it passes through.)
#[test]
fn no_text_is_cut_off() {
    let mut all = Vec::new();
    for (i, n) in jump_names().enumerate() {
        let mut r = Radio::new();
        r.sim.jump(i);
        r.wait(1000);
        all.extend(ferrite_ui::clipcheck::take().into_iter().map(|c| format!("{n}: {c}")));
    }
    for layout in 0..3u8 {
        for theme in 0..4u8 {
            let mut r = Radio::new();
            r.sim.app_mut().settings_mut().theme = theme;
            for _ in 0..layout {
                r.event(Event::Layout);
            }
            r.home();
            r.wait(1000);
            all.extend(ferrite_ui::clipcheck::take().into_iter().map(|c| format!("home layout {layout} theme {theme}: {c}")));
        }
    }
    all.dedup();
    assert!(all.is_empty(), "{} texts cut off:\n{}", all.len(), all.join("\n"));
}

/// HELP-1 to HELP-5, SELF-9: About's pages, ▶ by ▶ from About: System, the manual's QR code and the key card pages.
#[test]
fn help_pages_match_their_goldens() {
    let about = jump_names().position(|n| n.ends_with("About")).expect("About in the jump list");
    let mut failures = Vec::new();
    for (i, name) in ["system", "manual", "home-keys", "side-keys", "shortcuts", "menu-numbers"].iter().enumerate() {
        let mut r = Radio::new();
        r.sim.jump(about);
        r.wait(200);
        for _ in 0..=i {
            r.press(ferrite_core::keys::Key::Right);
        }
        r.wait(1000);
        failures.extend(check(&format!("help-{name}"), &r.sim.screen_rgb()));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// TSRCH-13, TSRCH-16: the tone hint on the active slot's card (the T event: a station sending 88.5 where the
/// slot has 100.0), and quick settings' Tones row offering it.
#[test]
fn tone_hint_matches_its_goldens() {
    let mut r = Radio::new();
    r.home();
    r.event(Event::ToneHint);
    r.wait(1000);
    let mut failures = Vec::new();
    failures.extend(check("tone-hint-home", &r.sim.screen_rgb()));
    r.press(ferrite_core::keys::Key::Menu);
    r.press(ferrite_core::keys::Key::Digit(4));
    r.wait(1000);
    failures.extend(check("tone-hint-quick", &r.sim.screen_rgb()));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// BCR-26.1, BCR-26.2, BCR-26.3, BCR-26.6, BCR-27, BCR-28: slot A tuned to 100.5 MHz plays broadcast FM: its card with
/// the full radio text, stereo and SNR; 99.1 (no radio text: the station name); slot B tuned in too takes it ("WFM on B"
/// on A); quick settings and the other two layouts with the broadcast slot.
#[test]
fn broadcast_slot_matches_its_goldens() {
    use ferrite_core::keys::Key;
    let mut failures = Vec::new();
    let mut r = Radio::new();
    r.home();
    r.event(Event::FmStation);
    r.wait(3000);
    failures.extend(check("bcast-home", &r.sim.screen_rgb()));
    r.press(Key::Menu).wait(1000);
    failures.extend(check("bcast-quick", &r.sim.screen_rgb()));
    r.home();
    r.event(Event::FmStation); // 99.1: the station name only
    r.wait(3000);
    failures.extend(check("bcast-home-ps", &r.sim.screen_rgb()));
    r.press(Key::Slot);
    r.event(Event::FmStation); // B to 100.5: the broadcast moves to B
    r.wait(3000);
    failures.extend(check("bcast-home-handover", &r.sim.screen_rgb()));
    for l in 1..3 {
        r.event(Event::Layout);
        r.home().wait(1000);
        failures.extend(check(&format!("bcast-layout{l}"), &r.sim.screen_rgb()));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// MODE-5 (decision 115): single + scope with B playing broadcast and A active: the strip sweeps around A.
#[test]
fn strip_beside_broadcast_matches_its_golden() {
    use ferrite_core::keys::Key;
    let mut r = Radio::new();
    r.event(Event::Layout);
    r.press(Key::Slot).event(Event::FmStation).wait(1000);
    r.press(Key::Slot).press(Key::Slot).wait(2000);
    if let Some(f) = check("bcast-strip", &r.sim.screen_rgb()) {
        panic!("{f}");
    }
}

/// HOME-15: quick mute's state word in each layout: A (active, the large card) muted with \*, and B muted while
/// its fake carrier is on ("RX muted" on a compact card or line).
#[test]
fn quick_mute_matches_its_goldens() {
    use ferrite_core::keys::Key;
    let mut failures = Vec::new();
    for l in 0..3u8 {
        let mut r = Radio::new();
        for _ in 0..l {
            r.event(Event::Layout);
        }
        r.home();
        r.press(Key::Slot).press(Key::Star).press(Key::Slot).press(Key::Slot).press(Key::Star);
        // B's fake carrier: 20-22 s of every 25.
        while !r.sim.radio().live.slots[1].squelch_open {
            r.wait(20);
        }
        r.wait(1000);
        failures.extend(check(&format!("mute-layout{l}"), &r.sim.screen_rgb()));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// HOME-17: the quick menu over home (hold *), in dark and in night red (PTT B in the menu turns it on).
#[test]
fn quick_menu_matches_its_goldens() {
    use ferrite_core::keys::{Key, PTT_B};
    let mut r = Radio::new();
    r.home();
    r.hold(Key::Star).wait(300);
    let mut failures: Vec<String> = check("quick-menu", &r.sim.screen_rgb()).into_iter().collect();
    r.press(Key::Ptt(PTT_B)).wait(300);
    failures.extend(check("quick-menu-night-red", &r.sim.screen_rgb()));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The same screen drawn twice from scratch is the same: the goldens can only change when the code does.
#[test]
fn shots_are_deterministic() {
    assert_eq!(shot_jump(0), shot_jump(0));
    let n = jump_names().count();
    assert_eq!(shot_jump(n / 2), shot_jump(n / 2));
}

/// CHAN-27 to CHAN-30: the zones page with location (B active on Repeaters: its chip, the places in the preview),
/// on the Nearby row (its preview), and with the GPS off and no fixed position (the plain list, no places); CHAN-34.1,
/// CHAN-34.4: on Local (its nearest four), home with B on Local, and Local without a position.
#[test]
fn zones_page_matches_its_goldens() {
    use ferrite_core::keys::Key;
    let mut failures = Vec::new();
    let mut r = Radio::new();
    r.home();
    r.press(Key::Slot); // B, on "Club repeater" in Repeaters
    r.menu("0 2 3").press(Key::Down).wait(1000);
    failures.extend(check("zones-located", &r.sim.screen_rgb()));
    r.press(Key::Up).press(Key::Up).wait(1000);
    failures.extend(check("zones-nearby", &r.sim.screen_rgb()));
    // CHAN-34: Local, the last row (▲ from Nearby), and home with B on it (OK)
    r.press(Key::Up).wait(1000);
    failures.extend(check("zones-local", &r.sim.screen_rgb()));
    r.press(Key::Menu).wait(1000);
    failures.extend(check("home-on-local", &r.sim.screen_rgb()));
    r.sim.app_mut().settings_mut().gps = ferrite_app::settings::GPS_OFF; // no fix, no fixed position: no location anywhere
    r.home();
    r.menu("0 2 3").press(Key::Down).wait(1000);
    failures.extend(check("zones-no-fix", &r.sim.screen_rgb()));
    r.press(Key::Up).press(Key::Up).wait(1000);
    failures.extend(check("zones-local-no-position", &r.sim.screen_rgb()));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// PWR-24.7: Low Power Mode's own home (0 7 7 2 On): quiet, receiving (B's fake carrier, B active), and the
/// "Leaving" box half way through the SIDE1 + SIDE2 hold.
#[test]
fn low_power_home_matches_its_goldens() {
    use ferrite_core::keys::Key;
    let mut r = Radio::new();
    r.home();
    r.menu("0 7 7 2").knob(1).wait(6000); // Auto (the default) to On (PWR-24.1); the entering toast gone
    let mut failures: Vec<String> = check("lowpower-quiet", &r.sim.screen_rgb()).into_iter().collect();
    r.press(Key::Slot);
    while !r.sim.radio().live.slots[1].squelch_open {
        r.wait(20);
    }
    r.wait(1500);
    failures.extend(check("lowpower-rx", &r.sim.screen_rgb()));
    r.down(Key::Side(1)).down(Key::Side(2)).wait(500);
    failures.extend(check("lowpower-leaving", &r.sim.screen_rgb()));
    r.up(Key::Side(2)).up(Key::Side(1));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// KEY-11, VFO-26: hold 7 opens the shift's options; hold 8 the offset's amount, typed (7.6 here), all text uncut.
#[test]
fn offset_editors_match_their_goldens() {
    use ferrite_core::keys::Key;
    let mut failures = Vec::new();
    let mut r = Radio::new();
    r.home();
    r.hold(Key::Digit(7)).wait(1500);
    failures.extend(check("offset-hold7", &r.sim.screen_rgb()));
    r.press(Key::Exit).home();
    r.hold(Key::Digit(8)).wait(1500);
    failures.extend(check("offset-hold8", &r.sim.screen_rgb()));
    r.type_("7.6").wait(300);
    failures.extend(check("offset-hold8-typed", &r.sim.screen_rgb()));
    r.press(Key::Menu).wait(1500);
    failures.extend(check("offset-home-7m6", &r.sim.screen_rgb()));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// VFO-4, TX-20.6: the Mode picker's help names FM 25 kHz, NFM 12.5 kHz and WFM broadcast; PTT on a WFM
/// slot says it's broadcast, receive only (the toast, then the refused screen), all text uncut.
#[test]
fn wfm_mode_and_refusal_match_their_goldens() {
    use ferrite_core::keys::Key;
    let mut failures = Vec::new();
    let mut r = Radio::new();
    r.home();
    r.press(Key::Menu).press(Key::Digit(3)).wait(500);
    failures.extend(check("wfm-mode-help", &r.sim.screen_rgb()));
    r.knob(4).wait(300); // FM, NFM, AM, NAM, WFM
    failures.extend(check("wfm-mode-set", &r.sim.screen_rgb()));
    r.home().wait(1500);
    r.sim.key(Key::Ptt(0), true);
    r.wait(300);
    failures.extend(check("wfm-ptt-refused", &r.sim.screen_rgb()));
    r.sim.key(Key::Ptt(0), false);
    r.wait(300);
    failures.extend(check("wfm-ptt-released", &r.sim.screen_rgb()));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// DEC-20.1: C's pager card, small (A active) and large (C active), after the fake system's first page; DEC-21's
/// hint bar says "OK Pages".
#[test]
fn pager_card_matches_its_goldens() {
    use ferrite_core::keys::Key;
    let mut r = Radio::new();
    r.run("menu 0 4 2 2; press OK; home");
    r.wait(26_000);
    let mut failures: Vec<String> = check("pager-card-small", &r.sim.screen_rgb()).into_iter().collect();
    while r.sim.app().active() != 2 {
        r.press(Key::Slot).wait(300);
    }
    r.wait(4000); // the page's toast gone
    failures.extend(check("pager-card-large", &r.sim.screen_rgb()));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ACARS-4, ACARS-5, ACARS-7: the ACARS list with slot B's strip (#), and one message's page: the two-block message
/// from Air Canada (its airline from the fixture table), then the uplink, and the one with a bad CRC.
#[test]
fn acars_matches_its_goldens() {
    use ferrite_core::keys::Key;
    let at = jump_names().position(|n| n.ends_with("ACARS")).expect("ACARS in the jump list");
    let mut failures = Vec::new();
    let mut r = Radio::new();
    r.sim.jump(at);
    r.wait(200);
    r.press(Key::Hash);
    r.wait(1000);
    failures.extend(check("acars-strip", &r.sim.screen_rgb()));
    r.press(Key::Hash);
    for _ in 0..7 {
        r.press(Key::Down);
    }
    r.press(Key::Menu);
    r.wait(1000);
    failures.extend(check("acars-message", &r.sim.screen_rgb()));
    r.press(Key::Left);
    r.press(Key::Left);
    r.wait(1000);
    failures.extend(check("acars-uplink", &r.sim.screen_rgb()));
    for _ in 0..3 {
        r.press(Key::Left);
    }
    r.wait(1000);
    failures.extend(check("acars-crc-bad", &r.sim.screen_rgb()));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
