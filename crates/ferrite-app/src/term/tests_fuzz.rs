//! The terminal fuzzed (ferrite-fuzzkit): the cable is untrusted, and its command line (TERM-12) also runs over
//! the link (`CMD`, LINK-34). Two levels:
//!
//! - Command lines: the radio's own dump (every line a valid command) and the tests' commands, their tokens
//!   swapped, cut, repeated and mutated, run as the terminal runs them (and as `load` runs them); a list, a dump
//!   or a pasted TLE they start is read to its end. A refused line changes nothing the radio keeps.
//! - Keystrokes: any bytes typed into the whole terminal (escape sequences, arrows, Enter, `:`, long lines, odd
//!   characters), with its pages and without (TERM-47).
//!
//! Invariants: no panic, no case slower than the limit, the sink never overfilled.

use super::*;
use crate::sample::Sample;
use ferrite_fuzzkit::{mutate, Fuzz, Rng};
use std::format;
use std::string::{String, ToString};
use std::sync::OnceLock;
use std::vec::Vec;

fn new_app() -> App {
    let mut app = App::new(crate::tests::set(), crate::tests::alert());
    while app.pop_cmd().is_some() {}
    app
}

fn dev() -> Device {
    Device { battery_mv: 7900, clock_min: Some(14 * 60 + 32), ..Default::default() }
}

/// What the radio keeps, as its dump says it (TERM-13).
fn kept<S: ChannelStore>(app: &App, st: &S, dev: &Device) -> Vec<u8> {
    let mut p = cmd::DumpPos::default();
    let mut l = cmd::Line::new();
    let mut v = Vec::new();
    while cmd::dump_line(app, st, dev, &mut p, &mut l) {
        v.extend_from_slice(l.as_bytes());
        v.push(b'\n');
    }
    v
}

/// Commands from the tests (`term/tests.rs`, `tests_line.rs`), beside the dump's own lines.
const LINES: &[&str] = &[
    "ch 12 pos none", "ch 12 pos=-33.8688197,151.2092955", "ch 12 power high", "ch 12 tone=D023I", "ch 300 delete", "ch 310 offset=+5",
    "ch 330 priority", "ch 330 skip off", "ch 999 rx=145.495 name=\"Fuzz\" tx=146.52", "get 0 5 1", "get 0 5 5 7", "get 0 6 9", "get", "gps",
    "help", "?", "quick 1", "quick 3 factory", "set 0 1 1 off", "set 0 2 6 1 200 kHz", "set 0 4 2 2 439.9875", "set 0 4 2 3 none",
    "set 0 5 4 145.495", "set 0 5 5 7 en route", "set 0 5 9 2 WIDE1-1,WIDE2-1,WIDE3-1", "set 0 6 9 45.4215,-75.6972", "set 0 6 9 FN31",
    "set 0 7 1 night red", "set 0 7 4 1 10 s", "set 0 8 8 5 On", "set 0 9 5 software", "slot", "slot 1", "ui", "ui keys", "ui status",
    "ui radio", "key MENU short", "key 5 long", "key ptt down", "knob 3", "knob -128", "xb", "zone 1 name=\"Air\" dwell=radio pri=none",
    "preset 1", "home", "dump", "sat", "sat add", "sat del 1", "clear", "bk settle", "mem 20000000 100", "gpio", "lcd bl 30",
];

fn corpus() -> &'static Vec<Vec<u8>> {
    static C: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    C.get_or_init(|| {
        let app = new_app();
        let st = Sample::new();
        let dump = kept(&app, &st, &dev());
        let mut v: Vec<Vec<u8>> = dump.split(|&c| c == b'\n').filter(|l| !l.is_empty() && l[0] != b'#').map(|l| l.to_vec()).collect();
        v.extend(LINES.iter().map(|l| l.as_bytes().to_vec()));
        v
    })
}

/// Every word any of the lines uses: the swaps' dictionary.
fn words() -> &'static Vec<String> {
    static W: OnceLock<Vec<String>> = OnceLock::new();
    W.get_or_init(|| {
        let mut w: Vec<String> = corpus().iter().flat_map(|l| String::from_utf8_lossy(l).split([' ', '=', ',']).map(String::from).collect::<Vec<_>>()).filter(|s| !s.is_empty()).collect();
        w.sort();
        w.dedup();
        w
    })
}

/// A line: a real one with tokens swapped, dropped, repeated or added, then (sometimes) bytes mutated.
fn line(r: &mut Rng) -> Vec<u8> {
    let c = corpus();
    if r.one_in(10) {
        let ws: Vec<&str> = words().iter().map(|s| s.as_str()).collect();
        return r.words(&ws, 12);
    }
    let base = r.pick(c).clone();
    let mut toks: Vec<Vec<u8>> = base.split(|&b| b == b' ').map(|t| t.to_vec()).collect();
    for _ in 0..r.below(4) {
        let i = r.below(toks.len());
        match r.below(6) {
            0 => toks[i] = r.pick(words()).as_bytes().to_vec(),
            1 if toks.len() > 1 => {
                toks.remove(i);
            }
            2 => {
                let t = toks[i].clone();
                toks.insert(i, t);
            }
            3 => toks.insert(i, r.edge(4).to_string().into_bytes()),
            4 => toks[i] = format!("{}={}", String::from_utf8_lossy(&toks[i]), r.pick(words())).into_bytes(),
            _ => toks.push(r.pick(words()).as_bytes().to_vec()),
        }
    }
    let mut l = toks.join(&b' ');
    if r.one_in(3) {
        mutate(r, &mut l, 300, c);
    }
    l.retain(|&b| b != b'\r' && b != b'\n');
    l
}

/// A case: up to six lines, newline separated; a first byte of 1 runs them as `load` does.
fn lines(r: &mut Rng) -> Vec<u8> {
    let mut v = std::vec![r.below(4) as u8];
    for i in 0..r.range(1, 6) {
        if i > 0 {
            v.push(b'\n');
        }
        v.extend(line(r));
    }
    v
}

fn run_lines(input: &[u8]) {
    let Some((&mode, rest)) = input.split_first() else { return };
    let loading = mode == 1;
    let mut app = new_app();
    let st = Sample::new();
    let dev = dev();
    for l in rest.split(|&b| b == b'\n') {
        let writes = cmd::writes(l);
        let before = kept(&app, &st, &dev);
        let res = cmd::run(&mut app, &st, &dev, l, loading);
        while let Some(w) = app.pop_write() {
            st.apply(w);
        }
        while app.pop_cmd().is_some() {}
        match res {
            Err(_) => {
                let after = kept(&app, &st, &dev);
                assert!(after == before, "a refused line ({}) changed what the radio keeps (writes: {writes})", String::from_utf8_lossy(l));
            }
            Ok(cmd::Done::List(list)) => {
                let mut out = cmd::Line::new();
                let mut i = 0;
                while cmd::list_line(&app, &st, &dev, &LiveAll::default(), list, i, &mut out) {
                    i += 1;
                    assert!(i < 5000, "a list that never ends");
                }
            }
            Ok(cmd::Done::Dump) => {
                let _ = kept(&app, &st, &dev);
            }
            Ok(cmd::Done::SatPaste) => {
                let mut p = cmd::SatPaste::default();
                for t in ["ISS (ZARYA)", "1 25544U 98067A   26327.50000000  .00016717  00000-0  10270-3 0  9005", "2 25544  51.6400 208.9163 0006317  69.9862  25.2906 15.49560532 12345", "end"] {
                    let _ = cmd::sat_line(&st, &mut p, t.as_bytes());
                }
            }
            Ok(_) => {}
        }
    }
}

#[test]
fn fuzz_term_lines() {
    assert!(corpus().len() > 100, "{} lines", corpus().len());
    Fuzz::new("term_lines", 0x7E31, 4_000).run(lines, run_lines);
}

/// A sink that holds what the terminal sends and checks it never sends more than there was room for.
struct Out {
    n: usize,
    room: usize,
}

impl Sink for Out {
    fn room(&self) -> usize {
        self.room
    }
    fn put(&mut self, b: &[u8]) {
        assert!(b.len() <= self.room, "the sink was overfilled");
        self.n += b.len();
    }
}

/// Keystrokes: printable runs, control characters, escape sequences, and lines typed after `:`.
fn keys(r: &mut Rng) -> Vec<u8> {
    let mut v = std::vec![r.byte()]; // the sink's room and whether the pages are there
    for _ in 0..r.range(1, 24) {
        match r.below(9) {
            0 => v.extend_from_slice(r.pick(&[&b"\x1b[A"[..], b"\x1b[B", b"\x1b[C", b"\x1b[D", b"\x1b[5~", b"\x1b[6~", b"\x1b[H", b"\x1b[F", b"\x1b[2~", b"\x1b[3~", b"\x1bOP", b"\x1b[24;80R", b"\x1b[999;999R", b"\x1b[0;0R", b"\x1b[", b"\x1b"])),
            1 => v.push(*r.pick(b"\r\n\x08\x7f\x03\x04\x09\x15\x17\x1a ")),
            2 | 3 => {
                v.push(b':');
                v.extend(line(r));
                v.push(b'\r');
            }
            4 => {
                let n = r.range(1, 300);
                v.extend((0..n).map(|_| *r.pick(b"abcxyz0123456789 .=-")));
            }
            5 => {
                let n = r.range(1, 8);
                v.extend(r.bytes(n));
            }
            6 => v.extend_from_slice(r.pick(&[&b"\r\r"[..], b"q", b"?", b"s", b"c", b"z", b"1", b"2", b"3", b"0"])),
            _ => v.extend_from_slice(b"\x1b[24;80R\r"),
        }
    }
    v
}

fn run_keys(input: &[u8]) {
    let Some((&k, typed)) = input.split_first() else { return };
    NO_VT.with(|v| v.set(k & 1 != 0));
    let mut app = new_app();
    let st = Sample::new();
    let dev = dev();
    let live = LiveAll::default();
    let mut term = Term::new();
    term.set_bluetooth(k & 2 != 0);
    let mut out = Out { n: 0, room: [64, 256, 1024, 1 << 20][(k >> 2 & 3) as usize] };
    let mut now = 1000u32;
    let poll = |term: &mut Term, app: &mut App, out: &mut Out, now: &mut u32| {
        *now += 100;
        term.poll(app, &st, &dev, &live, *now, out);
        while let Some(w) = app.pop_write() {
            st.apply(w);
        }
        while app.pop_cmd().is_some() {}
        let _ = term.debug_req();
        out.room = out.room.max(64); // the port drains between polls
    };
    for &c in typed {
        let mut tries = 0;
        while !term.ready() && tries < 64 {
            poll(&mut term, &mut app, &mut out, &mut now);
            tries += 1;
        }
        if !term.ready() {
            break;
        }
        term.feed(&mut app, c, now, &st, &dev, &live);
        while let Some(w) = app.pop_write() {
            st.apply(w);
        }
    }
    for _ in 0..32 {
        poll(&mut term, &mut app, &mut out, &mut now);
    }
    NO_VT.with(|v| v.set(false));
}

#[test]
fn fuzz_term_keys() {
    Fuzz::new("term_keys", 0x7E32, 1_000).slow_ms(5_000).run(keys, run_keys);
}
