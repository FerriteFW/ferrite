//! Live View completeness (LINK-19.4, decision 289): for every screen the simulator reaches (the jump list, home in
//! each slot and holder, broadcast, scan, entry with digits, toasts and alerts, About's pages), the texts the firmware
//! drew and the UI readback it would send, written to `target/liveview/corpus.json`. Ferrite Console's
//! `liveview-complete` test draws each readback with its Live View and fails on any drawn text it doesn't show, so a
//! screen the radio changes and Console doesn't follow fails there. Run by Console's test, or by hand:
//!
//! ```text
//! cargo test -p ferrite-sim --test liveview_corpus
//! ```

mod common;

use common::Radio;
use ferrite_sim::simcore::jump_names;
use std::fmt::Write as _;
use std::path::Path;

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// One screen: its name, what the firmware drew there (top to bottom) and the readback.
fn entry(name: &str, r: &mut Radio) -> String {
    // HOME-8: home waits under a popup, so its cards on the panel can be older than the readback (a scan moved on):
    // one whole frame drawn first, so the texts and the readback are the same moment.
    if r.sim.screen_name() == "Home" && r.sim.readback().lines().any(|l| l.starts_with("popup ")) {
        r.sim.app_mut().redraw_all();
        r.wait(common::FRAME);
    }
    let texts: Vec<String> = r.sim.screen_texts().into_iter().map(|t| json_str(&t.s)).collect();
    // LINK-19.5: drawing the screen again for the texts changes nothing in the App
    let before = r.sim.app_bytes();
    let rt = r.sim.readback_texts();
    assert!(r.sim.app_bytes() == before, "{name}: drawing the screen again for the readback changed the App");
    // and every line fits Live View whole (LINK-48: a line is cut past LINE_ROOM coded bytes), within its lines
    let n = rt.lines().count();
    assert!(n <= 200, "{name}: {n} readback lines, Live View carries {}", ferrite_core::liveview::LINES);
    for l in rt.lines() {
        let mut c = [0u8; 256];
        let k = ferrite_core::liveview::compact(l.as_bytes(), &mut c);
        let e = ferrite_core::liveview::encoded_len(&c[..k]);
        assert!(k == l.len() || l.starts_with("state"), "{name}: kept {k} of {} bytes, {e} coded: {l}", l.len());
        assert!(e <= ferrite_core::liveview::LINE_ROOM, "{name}: a line Live View would cut ({e} coded bytes): {l}");
    }
    format!(
        "{{\"name\":{},\"texts\":[{}],\"readback\":{},\"readback_texts\":{}}}",
        json_str(name),
        texts.join(","),
        json_str(&r.sim.readback()),
        json_str(&rt)
    )
}

/// The states beyond the jump list: a fresh sample radio, then a script (`common::Radio::run`).
const SCRIPTS: &[(&str, &str)] = &[
    ("home, B active", "press SLOT; wait 1s"),
    ("home, C active", "press SLOT; press SLOT; wait 1s"),
    ("home, layout 1", "event layout; home; wait 1s"),
    ("home, layout 2", "event layout; event layout; home; wait 1s"),
    ("APRS mode, A active", "menu 0 5 3; press OK; press OK; home; wait 2s"),
    ("APRS Tracker, C active", "menu 0 5 3; press OK; home; press SLOT; press SLOT; wait 2s"), // APRS-29.2
    ("APRS mode, C active", "menu 0 5 3; press OK; press OK; home; press SLOT; press SLOT; wait 2s"),
    ("APRS mode, C active, layout 1", "menu 0 5 3; press OK; press OK; event layout; home; press SLOT; press SLOT; wait 2s"),
    ("APRS mode, A active, layout 1", "menu 0 5 3; press OK; press OK; event layout; home; wait 2s"), // MODE-5.1: APRS waits
    ("APRS mode, layout 1, strip paused", "menu 0 5 3; press OK; press OK; event layout; home; wait 1s; press HASH; wait 3s"),
    ("APRS mode, APRS list", "menu 0 5 3; press OK; press OK; home; press SLOT; press SLOT; press OK; wait 1s"),
    ("broadcast FM on A", "event fm; wait 3s"),
    ("broadcast FM, quick settings", "event fm; wait 3s; press MENU; wait 1s"),
    ("scan on A", "press SIDE2; wait 2s"),
    ("frequency entry, 1 digit", "type 1; wait 1s"),
    ("frequency entry, 146.52", "type 14652; wait 1s"),
    ("frequency entry, HF", "type 7; wait 1s"),
    ("quick menu", "press MENU; wait 1s"),
    ("message toast", "event message; wait 500ms"),
    ("alert", "event alert; wait 1s"),
    ("crossband running", "event crossband; wait 2s"),
    ("tone hint", "event tonehint; wait 1s"),
    ("low battery", "event battery; wait 1s"),
    ("monitor held", "down SIDE1; wait 1s"),
    ("key lock, choosing", "hold #; wait 500ms"),
    ("key lock, all but PTT", "hold #; press 0; wait 500ms"),
];

/// LINK-19.4: the corpus Console's Live View completeness check reads (every screen's drawn texts and readback).
#[test]
fn liveview_corpus() {
    let mut out = Vec::new();
    for (i, n) in jump_names().enumerate() {
        let mut r = Radio::new();
        r.sim.jump(i);
        r.wait(1000);
        out.push(entry(&format!("{i:03} {n}"), &mut r));
    }
    for (name, script) in SCRIPTS {
        let mut r = Radio::new();
        r.home();
        r.run(script);
        out.push(entry(name, &mut r));
    }
    // About's pages, ▶ by ▶ (HELP-1 to HELP-5).
    let about = jump_names().position(|n| n.ends_with("About")).expect("About in the jump list");
    let mut r = Radio::new();
    r.sim.jump(about);
    r.wait(1000);
    for p in 1..=5 {
        r.run("press RIGHT; wait 500ms");
        out.push(entry(&format!("About, page {}", p + 1), &mut r));
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/liveview");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("corpus.json"), format!("[\n{}\n]\n", out.join(",\n"))).unwrap();
    assert!(out.len() > 80, "{} screens", out.len());
}
