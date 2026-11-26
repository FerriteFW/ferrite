//! Measurement on real recordings, against multimon-ng. Needs the test data's `pocsag/` (found by
//! `ferrite_fuzzkit::testdata_dir`'s rule; the Signal Identification Wiki recordings come from the testdata repo's
//! `fetch/all.sh`); a recording that is missing is skipped.
//!
//! `cargo test --release -p upocsag --test report -- --ignored --nocapture`

use std::collections::HashMap;
use ferrite_fuzzkit::testdata_file;
use std::path::{Path, PathBuf};
use upocsag::{Message, Receiver};

const FILES: [&str; 7] =
    ["commons_512", "sigid_512", "sigid_1200", "sigid_2400", "sigid_1200_variant", "sigid_148", "sigid_sound"];

fn read_wav(path: &Path) -> (Vec<i16>, u32) {
    let d = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut i = 12;
    let (mut rate, mut data) = (0, Vec::new());
    while i + 8 <= d.len() {
        let len = u32::from_le_bytes(d[i + 4..i + 8].try_into().unwrap()) as usize;
        let body = &d[i + 8..(i + 8 + len).min(d.len())];
        match &d[i..i + 4] {
            b"fmt " => rate = u32::from_le_bytes(body[4..8].try_into().unwrap()),
            b"data" => data = body.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c)).collect(),
            _ => {}
        }
        i += 8 + len + (len & 1);
    }
    (data, rate)
}

/// A message as multimon-ng prints it with `-f alpha`: control characters by name.
fn line(m: &Message) -> String {
    const NAMES: [&str; 32] = [
        "NUL", "SOH", "STX", "ETX", "EOT", "ENQ", "ACK", "BEL", "BS", "HT", "LF", "VT", "FF", "CR", "SO", "SI", "DLE",
        "DC1", "DC2", "DC3", "DC4", "NAK", "SYN", "ETB", "CAN", "EM", "SUB", "ESC", "FS", "GS", "RS", "US",
    ];
    let text: String = m
        .alpha()
        .map(|c| match c {
            0..=31 => format!("<{}>", NAMES[c as usize]),
            127 => "<DEL>".into(),
            c => (c as char).to_string(),
        })
        .collect();
    let head = format!("POCSAG{}: Address: {:>7}  Function: {}", m.baud, m.address, m.function);
    if m.bits() == 0 {
        format!("{head} ")
    } else {
        format!("{head}  Alpha:   {text}")
    }
}

fn ours(dir: &Path, name: &str, rate: u32) -> Vec<String> {
    let (s, r) = read_wav(&dir.join(format!("{name}.{rate}.wav")));
    assert_eq!(r, rate);
    let mut rx: Receiver = Receiver::new(rate);
    let mut out = Vec::new();
    for x in s {
        rx.push(x, |m| out.push(line(m)));
    }
    rx.flush(|m| out.push(line(m)));
    out
}

fn common(a: &[String], b: &[String]) -> usize {
    let mut cb: HashMap<&String, usize> = HashMap::new();
    for x in b {
        *cb.entry(x).or_default() += 1;
    }
    a.iter().filter(|x| cb.get_mut(x).is_some_and(|n| *n > 0 && { *n -= 1; true })).count()
}

#[test]
#[ignore]
fn recordings() {
    println!("\nMessages decoded (in brackets: identical to multimon-ng's)\n");
    println!("{:<20} {:>9} {:>14} {:>14}", "recording", "multimon", "upocsag 22050", "upocsag 9600");
    let mut lines = Vec::new();
    for name in FILES {
        // the recording at both rates and multimon-ng's decodes, or a skip note
        let Some(dir) = ["22050.wav", "9600.wav", "multimon.txt", "multimon_inv.txt"]
            .iter()
            .map(|k| testdata_file(&format!("pocsag/{name}.{k}")))
            .collect::<Option<Vec<PathBuf>>>()
            .map(|p| p[0].parent().unwrap().to_path_buf())
        else {
            continue;
        };
        // multimon-ng as recorded and inverted: the polarity that decodes more.
        let refs: Vec<String> = ["multimon", "multimon_inv"]
            .map(|k| {
                let t = std::fs::read_to_string(dir.join(format!("{name}.{k}.txt"))).unwrap();
                t.lines().map(String::from).collect::<Vec<_>>()
            })
            .into_iter()
            .max_by_key(|v| v.len())
            .unwrap();
        let mut row = format!("{name:<20} {:>9}", refs.len());
        lines.push(format!("RESULT pocsag.{name}.multimon {}", refs.len()));
        for rate in [22050, 9600] {
            let got = ours(&dir, name, rate);
            let same = common(&got, &refs);
            row += &format!(" {:>14}", format!("{} ({})", got.len(), same));
            lines.push(format!("RESULT pocsag.{name}.{rate} {}", got.len()));
            lines.push(format!("RESULT pocsag.{name}.{rate}.matches {same}"));
            if std::env::var("UPOCSAG_SHOW").is_ok() {
                for g in got.iter().filter(|g| !refs.contains(g)) {
                    eprintln!("  only ours {rate}: {g}");
                }
                for r in refs.iter().filter(|r| !got.contains(r)) {
                    eprintln!("  only multimon {rate}: {r}");
                }
            }
        }
        println!("{row}");
    }
    println!();
    for l in lines {
        println!("{l}");
    }
}
