//! Every APRS menu setting stepped and reset, recorded whole: the settings block, its state bits and what went to
//! the Radio task. The size review (Nov 23, 2026) rewrote the menu's APRS arms to edit in place
//! (`App::edit_aprs`); the golden was blessed on main before that change, so any arm that now sets another field,
//! misses the dirty mark or the slot send fails here. Bless a deliberate change with FERRITE_BLESS=1.

use super::*;
use crate::screens::groups;
use std::string::String;
use std::vec::Vec;
use std::{format, vec};

/// Every menu path the APRS arms can sit under: 0 5 x, 0 5 x y, and the chime at 0 7 5 5.
fn paths() -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    for a in 0..10u8 {
        v.push(vec![5, a]);
        for b in 0..10u8 {
            v.push(vec![5, a, b]);
        }
    }
    v.push(vec![7, 5, 5]);
    v
}

fn app() -> App {
    let mut app = App::new(crate::tests::set(), crate::tests::alert());
    while app.pop_cmd().is_some() {}
    app
}

/// The whole outcome of one call, as one line.
fn line(app: &mut App, what: &str, changed: Option<bool>) -> String {
    let mut cmds = Vec::new();
    while let Some(c) = app.pop_cmd() {
        cmds.push(format!("{c:?}"));
    }
    format!("{what} -> {changed:?} st={:#04x} {:?} cmds=[{}]\n", app.aprs_st, app.aprs_cfg(), cmds.join(", "))
}

fn sweep(out: &mut String, name: &str, start: &dyn Fn() -> App) {
    for p in paths() {
        for d in [-1, 1, 5] {
            let mut app = start();
            let r = groups::change(&mut app, &p, d);
            out.push_str(&line(&mut app, &format!("{name} change {p:?} {d:+}"), Some(r)));
        }
        let mut app = start();
        groups::reset(&mut app, &p);
        out.push_str(&line(&mut app, &format!("{name} reset {p:?}"), None));
    }
}

#[test]
fn aprs_menu_edits_match_golden() {
    let mut out = String::new();
    // Off, every setting at its default
    sweep(&mut out, "off", &app);
    // On (APRS on slot C, so each edit retunes C), every setting stepped once away from its default
    let busy = || {
        let mut app = app();
        app.set_aprs_select(2);
        for p in paths() {
            groups::change(&mut app, &p, 1);
        }
        while app.pop_cmd().is_some() {}
        app.aprs_st &= !2; // the dirty bit, so each case shows its own
        app
    };
    sweep(&mut out, "on", &busy);

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/tests/aprs_edit.golden");
    if std::env::var("FERRITE_BLESS").is_ok() {
        std::fs::write(path, &out).unwrap();
    }
    let want = std::fs::read_to_string(path).unwrap();
    for (i, (a, b)) in out.lines().zip(want.lines()).enumerate() {
        assert_eq!(a, b, "line {}", i + 1);
    }
    assert_eq!(out.lines().count(), want.lines().count());
}
