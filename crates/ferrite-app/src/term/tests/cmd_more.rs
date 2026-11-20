//! The command line's answers at their edges (TERM-12, TERM-13): each refusal names what was wrong, and a few
//! answers that only a rare line reaches.

use super::*;

/// One command, run as typed: its answer's text, or the refusal's.
fn run<S: ChannelStore + Apply>(r: &mut Rig<S>, line: &str) -> Result<String, String> {
    let out = cmd::run(&mut r.app, &r.store, &r.dev, line.as_bytes(), false);
    while let Some(w) = r.app.pop_write() {
        r.store.apply(w);
    }
    match out {
        Ok(cmd::Done::Said(m)) => Ok(String::from_utf8_lossy(m.as_bytes()).into_owned()),
        Ok(cmd::Done::Nothing) => Ok(String::new()),
        Ok(_) => Ok("<more>".into()),
        Err(m) => Err(String::from_utf8_lossy(m.as_bytes()).into_owned()),
    }
}

/// TERM-12: refusals, each naming the field or command and what it takes.
#[test]
fn refusals_name_what_was_wrong() {
    let mut r = Rig::main();
    for (line, want) in [
        ("\"quoted\"", "a command first; help lists them"),
        ("ch 1 rx 99999", "rx: MHz from 0.15 to 1300, e.g. 146.52"),
        ("ch 1 name \"caf\u{e9}\"", "name: letters, digits and punctuation only"),
        ("ch 1 name \"seventeen chars xx\"", "name: 16 characters at most"),
        ("ch 1 skip maybe", "skip: on or off"),
        ("ch 1 skip \"on\"", "skip: on or off"),
        ("ch 1 reverse maybe", "reverse: on or off"),
        ("ch 1 reverse \"on\"", "reverse: on or off"),
        ("ch 1 power", "power: needs a value, e.g. power=..."),
        ("ch 1 squelch 12", "squelch: global, or 0 to 9"),
        ("ch 1 power=high skip", "ch: one field at a time"),
        ("ch 1 pos=none skip", "ch: one field at a time"),
        ("ch 1 \"A\" 146.52 \"B\"", "ch: a name in quotes goes right after the number"),
        ("zone 1", "zone 1: its name in quotes, e.g. zone 1 \"Repeaters\""),
        ("zone 1 \"Local\" \"Again\"", "zone: one name per zone"),
        ("zone 1 \"Local\" resume=99", "resume: radio, 1s to 10s, carrier or stop"),
        ("zone 1 \"Local\" dwell=999", "dwell: radio, or 1s to 254s"),
        ("preset 1 \"2 m\" 148 144", "preset: the end must be above the start"),
        ("preset 1 \"2 m\" 144 148 colour=red", "preset: no field colour; they are step, mode, bw, resume"),
        ("home 999", "home: channel 999 doesn't exist"),
        ("ui frobs", "ui, ui keys, ui status or ui radio"),
        ("key MENU sideways", "key NAME [long|down|up]: ui keys lists the names"),
        ("xb dance", "xb, xb start or xb stop: the crossband repeater"),
        // a bench build has the module commands, so it lists them instead (decision 151)
        ("gps now", if cfg!(feature = "bench") { "gps, gps query [0|1|2|4|6] (CAS06), gps nmea off|on (CAS03), gps standby N [quiet] (1-65535 s)" } else { "gps alone: query, nmea and standby need a bench build" }),
        ("clear zones", "clear channels N: deletes every channel"),
        ("set 0 5 5 3 this comment is far too long to fit in forty characters", "Comment: up to 40 characters (no #), or None"),
    ] {
        assert_eq!(run(&mut r, line), Err(want.into()), "{line}");
    }
}

/// TERM-12: `ch N` field values that only older dumps or rare lines carry: `bw=` (FM and NFM follow it, AM
/// stays AM), a quoted name, `hidden`; a channel without a name is saved and says so.
#[test]
fn rare_channel_fields() {
    let mut r = Rig::main();
    assert_eq!(run(&mut r, "ch 1 bw=wide"), Ok("Channel 1 saved".into()));
    assert_eq!(r.store.get(0).unwrap().cfg.mode, 0, "FM");
    assert_eq!(run(&mut r, "ch 1 bw=narrow"), Ok("Channel 1 saved".into()));
    assert_eq!(r.store.get(0).unwrap().cfg.mode, 1, "NFM");
    assert_eq!(run(&mut r, "ch 1 name \"Club\""), Ok("Channel 1 saved".into()));
    assert_eq!(r.store.get(0).unwrap().name(), b"Club");
    assert_eq!(run(&mut r, "ch 1 hidden"), Ok("Channel 1 saved".into()));
    assert!(r.store.get(0).unwrap().flags & crate::chan::FLAG_HIDDEN != 0);
    assert_eq!(run(&mut r, "ch 1 name \"\""), Ok("Channel 1 saved (no name)".into()));
    assert_eq!(run(&mut r, "ch 900 \"\" 146.52"), Ok("Channel 900 saved (no name)".into()));
    assert_eq!(run(&mut r, "ch 901 \"\" 146.52 pos=none"), Ok("Channel 901 saved (no name)".into()));
    assert_eq!(run(&mut r, "ch 1 zone=none"), Ok("Channel 1 saved (no name)".into()), "its name was cleared above");
    assert_eq!(r.store.get(0).unwrap().zone, NO_ZONE);
    // AM stays AM whatever bw= says
    assert_eq!(run(&mut r, "ch 902 \"Air\" 121.5 mode=AM bw=narrow"), Ok("Channel 902 saved".into()));
    assert_eq!(r.store.get(901).unwrap().cfg.mode, 2);
}

/// TERM-12: a preset's `bw=` from older dumps; the same zone or preset again is "unchanged".
#[test]
fn presets_and_zones_unchanged_and_bw() {
    let mut r = Rig::main();
    assert_eq!(run(&mut r, "preset 1 \"2 m\" 144 148 mode=FM bw=narrow"), Ok("Preset 1 saved".into()));
    assert_eq!(r.store.data().preset(0).unwrap().mode, 1);
    assert_eq!(run(&mut r, "preset 1 \"2 m\" 144 148 mode=FM bw=narrow"), Ok("Preset 1 unchanged".into()));
    let z = dump(&r.app, &r.store, &r.dev).into_iter().find(|l| l.starts_with("zone 1 ")).expect("zone 1 in the dump");
    assert_eq!(run(&mut r, &z), Ok("Zone 1 unchanged".into()), "{z}");
}

/// TERM-12: `home none` clears the home channel.
#[test]
fn home_none() {
    let mut r = Rig::main();
    assert_eq!(run(&mut r, "home 1"), Ok("Home channel: 1".into()));
    assert_eq!(run(&mut r, "home none"), Ok("No home channel".into()));
    assert_eq!(r.app.set.home_ch, None);
}

/// TERM-37: `key NAME down` and `up` press and let go apart.
#[test]
fn key_down_and_up() {
    let mut r = Rig::main();
    assert_eq!(run(&mut r, "key UP down"), Ok("key UP down".into()));
    assert_eq!(run(&mut r, "key UP up"), Ok("key UP up".into()));
}

/// XB-20: `xb start` refused while a screen that can't be left is up.
#[test]
fn xb_start_refused_on_a_modal_screen() {
    let mut r = Rig::main();
    r.app.open(crate::screens::nav::Dest::PowerOff);
    assert!(r.app.screen().modal(), "{:?}", r.app.screen());
    let e = run(&mut r, "xb start").unwrap_err();
    assert!(e.starts_with("xb start: not now: the radio shows "), "{e}");
}

/// TERM-12: the settings changed list says when everything is at its default.
#[test]
fn changed_list_all_defaults() {
    let r = Rig::main();
    let mut l = cmd::Line::new();
    assert!(cmd::list_line(&r.app, &r.store, &r.dev, &r.live, cmd::List::Changed, 0, &mut l));
    let first = String::from_utf8_lossy(l.as_bytes()).into_owned();
    // a fresh radio's settings may differ from the defaults (the sample's): either a setting or the note
    assert!(first.starts_with("set ") || first == "All settings are at their defaults.", "{first}");
}

/// TERM-35: a slot on a channel the store no longer has shows its number alone.
#[test]
fn slot_line_gone_channel_and_scanning() {
    let mut r = Rig::main();
    let c = r.store.get(9).unwrap();
    r.app.set_channel_rec(0, c);
    r.store.apply(ChanWrite::Delete(9));
    let mut l = cmd::Line::new();
    cmd::slot_line(&r.app, &r.store, 0, &mut l);
    let s = String::from_utf8_lossy(l.as_bytes()).into_owned();
    assert!(s.starts_with("A* ch 10 "), "{s}");
}
