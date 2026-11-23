//! DISP-31: the screen always matches the same state drawn from scratch. Every frame is drawn twice
//! ([`SimCore::check_repaint`]): as the radio draws it (only what changed) and then from scratch; any pixel that
//! differs is drawing that went stale. Two bugs of Nov 18, 2026 were this: the satellite pass page's countdown drawing
//! over the alarm picker, and home keeping the old layout's cards after a layout change from the terminal.

mod common;

use common::Radio;
use ferrite_core::keys::Key;
use ferrite_sim::simcore::{jump_names, rgb565_to_rgb, RepaintMiss};
use std::path::PathBuf;

/// The first miss's two screens, side by side with the differing pixels in magenta, for a person to look at:
/// `target/repaint-diff/<what>.png` (left as drawn, right drawn from scratch).
fn save(m: &RepaintMiss, what: &str) -> Option<PathBuf> {
    let (a, b) = m.shots.as_ref()?;
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/repaint-diff");
    std::fs::create_dir_all(&dir).ok()?;
    let slug: String = what.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let path = dir.join(format!("{slug}.png"));
    let (w, h) = (240usize, 320usize);
    let mut px = vec![0u8; w * 3 * h * 3];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let diff = a[i] != b[i];
            for (k, c) in [rgb565_to_rgb(a[i]), rgb565_to_rgb(b[i]), if diff { 0xFF00FF } else { (rgb565_to_rgb(a[i]) >> 2) & 0x3F3F3F }].into_iter().enumerate() {
                let o = (y * w * 3 + k * w + x) * 3;
                px[o..o + 3].copy_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, c as u8]);
            }
        }
    }
    let mut enc = png::Encoder::new(std::fs::File::create(&path).ok()?, (w * 3) as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.write_header().ok()?.write_image_data(&px).ok()?;
    Some(path)
}

fn report(misses: &[RepaintMiss], what: &str) -> Option<String> {
    let m = misses.first()?;
    let shot = save(m, what).map(|p| format!(" (see {})", p.display())).unwrap_or_default();
    Some(format!(
        "{what}: {} frames differ from a fresh repaint; first at {} ms on {}: {} pixels in x {}..{}, y {}..{}",
        misses.len(),
        m.at,
        m.screen,
        m.pixels,
        m.area.0,
        m.area.2,
        m.area.1,
        m.area.3
    ) + &shot)
}

fn known(name: &str, m: &RepaintMiss) -> bool {
    common::REPAINT_KNOWN.iter().any(|&(n, y0, y1, _)| name.ends_with(n) && m.area.1 >= y0 && m.area.3 <= y1)
}

/// DISP-31: every screen the simulator can open: let it run (live values, countdowns, timers), then a few keys and
/// the knob.
#[test]
fn every_screen_matches_a_fresh_repaint() {
    let mut failures = Vec::new();
    for (i, name) in jump_names().enumerate() {
        let mut r = Radio::new();
        r.sim.jump(i);
        r.sim.check_repaint(true);
        r.wait(3000);
        r.press(Key::Down).wait(500).press(Key::Up).wait(500).knob(1).wait(500).knob(-1).wait(1500);
        let what = format!("{i} {name}");
        let misses: Vec<RepaintMiss> = r.sim.repaint_misses.iter().filter(|m| !known(&what, m)).cloned().collect();
        failures.extend(report(&misses, &what));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A small seeded generator (xorshift32): the same seed walks the same way, so a failure can be replayed.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    fn pick<T: Copy>(&mut self, v: &[T]) -> T {
        v[self.next() as usize % v.len()]
    }
}

const KEYS: &[Key] = &[
    Key::Menu,
    Key::Exit,
    Key::Up,
    Key::Down,
    Key::Left,
    Key::Right,
    Key::Slot,
    Key::VfoMem,
    Key::Star,
    Key::Hash,
    Key::Side(1),
    Key::Side(2),
    Key::Digit(0),
    Key::Digit(1),
    Key::Digit(2),
    Key::Digit(3),
    Key::Digit(4),
    Key::Digit(5),
    Key::Digit(6),
    Key::Digit(7),
    Key::Digit(8),
    Key::Digit(9),
];

/// Settings changed from the terminal while a screen is up (as Console or the terminal would): the layout, the
/// theme, the clock format.
const TERM: &[&str] = &[
    "set 0 7 2 Watch",
    "set 0 7 2 Single + scope",
    "set 0 7 2 Watch + activity",
    "set 0 7 1 Daylight",
    "set 0 7 1 Dark",
];

/// One step of a walk, said as it would be replayed.
fn step(r: &mut Radio, g: &mut Rng) -> String {
    match g.next() % 20 {
        0..=9 => {
            let k = g.pick(KEYS);
            r.press(k);
            format!("press {k:?}")
        }
        10..=11 => {
            let k = g.pick(KEYS);
            r.hold(k);
            format!("hold {k:?}")
        }
        12..=13 => {
            let d = (g.next() % 5) as i32 - 2;
            r.knob(d);
            format!("knob {d}")
        }
        14 => {
            let line = g.pick(TERM);
            r.sim.term_feed(format!("\r:{line}\r").as_bytes());
            format!("terminal {line}")
        }
        15 => {
            // PTT a moment (the simulator's Dry TX; HOME and refusals redraw over whatever was up)
            r.sim.key(Key::Ptt(0), true);
            r.wait(300);
            r.sim.key(Key::Ptt(0), false);
            "ptt 300 ms".into()
        }
        _ => {
            let ms = [200, 700, 1500, 4000][g.next() as usize % 4];
            r.wait(ms);
            format!("wait {ms}")
        }
    }
}

/// DISP-31 over random walks: keys, holds, the knob, PTT, waits and settings from the terminal, across screens; every
/// frame drawn twice. A failure names its seed and the steps up to it (`FERRITE_WALK_SEED=N` replays one walk, in
/// whichever test holds it).
fn walks(seeds: core::ops::RangeInclusive<u32>) {
    let seeds: Vec<u32> = match std::env::var("FERRITE_WALK_SEED") {
        Ok(s) => vec![s.parse().expect("a number")].into_iter().filter(|s| seeds.contains(s)).collect(),
        Err(_) => seeds.collect(),
    };
    let mut failures = Vec::new();
    for seed in seeds {
        let mut g = Rng(seed.wrapping_mul(2_654_435_761) | 1);
        let mut r = Radio::new();
        r.sim.check_repaint(true);
        let mut log = Vec::new();
        for i in 0..250 {
            log.push(step(&mut r, &mut g));
            let misses: Vec<RepaintMiss> = r.sim.repaint_misses.iter().filter(|m| !known(m.screen, m)).cloned().collect();
            if let Some(f) = report(&misses, &format!("seed {seed} step {i}")) {
                let tail = log[log.len().saturating_sub(8)..].join(", ");
                failures.push(format!("{f}\n    last steps: {tail}"));
                break;
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// Three walks a test, so the runner spreads them over its threads.
#[test]
fn random_walks_1_to_3() {
    walks(1..=3);
}
#[test]
fn random_walks_4_to_6() {
    walks(4..=6);
}
#[test]
fn random_walks_7_to_9() {
    walks(7..=9);
}
#[test]
fn random_walks_10_to_12() {
    walks(10..=12);
}
