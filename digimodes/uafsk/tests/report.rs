//! Measurements on the WA8LMF TNC Test CD (the testdata repo's `fetch/wa8lmf.sh`; found by
//! `ferrite_fuzzkit::testdata_dir`'s rule, skipped when missing). Run with
//! `cargo test --release -p uafsk --test report -- --ignored --nocapture`.

use ferrite_fuzzkit::{testdata_file, testdata_private_file};
use std::collections::HashMap;
use uafsk::Demodulator;

/// A WA8LMF track and Dire Wolf's decode of it, when fetched (else a skip note).
fn track(name: &str) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    Some((testdata_file(&format!("wa8lmf/{name}.wav"))?, testdata_file(&format!("wa8lmf/ref/{name}.atest.txt"))?))
}

fn read_wav(path: &std::path::Path) -> (Vec<i16>, u32) {
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

/// Frames in Dire Wolf's `atest -h` output: hex rows "  000:  82 a0 ..." after each "[0] ..." line.
fn atest_frames(path: &std::path::Path) -> Vec<Vec<u8>> {
    let text = std::fs::read(path).unwrap();
    let text = String::from_utf8_lossy(&text);
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut cur: Option<Vec<u8>> = None;
    for line in text.lines() {
        let t = line.trim_start();
        if line.starts_with('[') {
            if let Some(f) = cur.take() {
                frames.push(f);
            }
            cur = Some(Vec::new());
        } else if let (Some(f), Some((off, rest))) = (cur.as_mut(), t.split_once(":  ")) {
            if off.len() == 3 && off.chars().all(|c| c.is_ascii_hexdigit()) {
                for h in rest.split("  ").next().unwrap_or("").split_whitespace() {
                    if let Ok(b) = u8::from_str_radix(h, 16) {
                        f.push(b);
                    }
                }
            }
        }
    }
    frames.extend(cur);
    frames.retain(|f| !f.is_empty());
    frames
}

fn count(frames: &[Vec<u8>]) -> HashMap<&[u8], usize> {
    let mut m = HashMap::new();
    for f in frames {
        *m.entry(f.as_slice()).or_insert(0) += 1;
    }
    m
}

/// Frames in common (as multisets).
fn common(a: &[Vec<u8>], b: &[Vec<u8>]) -> usize {
    let (ca, cb) = (count(a), count(b));
    ca.iter().map(|(k, n)| (*n).min(*cb.get(k).unwrap_or(&0))).sum()
}

#[test]
#[ignore]
fn wa8lmf() {
    use uafsk::demod::Config;
    println!("\nWA8LMF TNC Test CD: frames with a correct CRC (in brackets: also decoded by Dire Wolf)");
    print!("{:32}", "");
    let mut tracks = Vec::new();
    for t in ["track01", "track02"] {
        for suffix in ["_9600", ""] {
            let name = format!("{t}{suffix}");
            let Some((wav, atest)) = track(&name) else { return };
            let (s, rate) = read_wav(&wav);
            let dw = atest_frames(&atest);
            print!("{:>16}", format!("{t} {}k", rate / 1000));
            tracks.push((s, rate, dw));
        }
    }
    println!();
    let names = ["track01_9600", "track01_44100", "track02_9600", "track02_44100"];
    print!("{:32}", "Dire Wolf atest (E+)");
    for (_, _, dw) in &tracks {
        print!("{:>16}", dw.len());
    }
    println!();
    for (n, (_, _, dw)) in names.iter().zip(&tracks) {
        println!("RESULT wa8lmf.{n}.dire_wolf {}", dw.len());
    }
    fn run<const S: usize, const C: usize>(cfg: Config, s: &[i16]) -> Vec<Vec<u8>> {
        let mut d = Box::new(Demodulator::<S, C>::new(cfg));
        let mut got = Vec::new();
        for &x in s {
            d.push(x, |f| got.push(f.to_vec()));
        }
        got
    }
    type Run = fn(u32, &[i16]) -> Vec<Vec<u8>>;
    let rows: [(&str, &str, Run); 5] = [
        ("1 slicer, high-pass", "s1", |r, s| run::<1, 1>(Config::new(r), s)),
        ("3 slicers, high-pass (default)", "default", |r, s| run::<3, 1>(Config::new(r), s)),
        ("3 slicers, no high-pass", "s3_flat", |r, s| run::<3, 1>(Config { highpass: false, ..Config::new(r) }, s)),
        ("3 slicers, dual chain", "dual", |r, s| run::<3, 2>(Config::new(r), s)),
        ("4 slicers (+ raw), high-pass", "s4", |r, s| run::<4, 1>(Config::new(r), s)),
    ];
    let mut lines = Vec::new();
    for (name, key, f) in rows {
        print!("{name:32}");
        for (n, (s, rate, dw)) in names.iter().zip(&tracks) {
            let got = f(*rate, s);
            print!("{:>16}", format!("{} ({})", got.len(), common(&got, dw)));
            lines.push(format!("RESULT wa8lmf.{n}.{key} {}", got.len()));
            lines.push(format!("RESULT wa8lmf.{n}.{key}.also_dire_wolf {}", common(&got, dw)));
        }
        println!();
    }
    for l in lines {
        println!("{l}");
    }
}

/// AX.25 address-field sanity: 2 to 10 addresses of 7 bytes, the extension bit (bit 0) clear except on the last
/// address's final byte, callsign characters shifted ASCII A-Z, 0-9 or space.
fn ax25_ok(f: &[u8]) -> bool {
    let mut n = 0;
    loop {
        let a = match f.get(n * 7..n * 7 + 7) {
            Some(a) => a,
            None => return false,
        };
        if !a[..6].iter().all(|&c| c & 1 == 0 && matches!(c >> 1, b'A'..=b'Z' | b'0'..=b'9' | b' ')) {
            return false;
        }
        n += 1;
        if a[6] & 1 == 1 {
            return (2..=10).contains(&n) && f.len() > n * 7;
        }
        if n == 10 {
            return false;
        }
    }
}

#[test]
#[ignore]
fn extras() {
    use uafsk::demod::Config;
    for name in ["track01_9600", "track02_9600"] {
        let Some((wav, atest)) = track(name) else { continue };
        let (s, rate) = read_wav(&wav);
        let dw = atest_frames(&atest);
        let mut d = Box::new(Demodulator::<3>::new(Config::new(rate)));
        let mut got = Vec::new();
        for &x in &s {
            d.push(x, |f| got.push(f.to_vec()));
        }
        let extra: Vec<&Vec<u8>> = got.iter().filter(|f| !dw.contains(f)).collect();
        let bad = extra.iter().filter(|f| !ax25_ok(f)).count();
        let dw_bad = dw.iter().filter(|f| !ax25_ok(f)).count();
        println!("{name}: {} frames not in Dire Wolf's output, {bad} of them fail AX.25 address checks (Dire Wolf's own frames failing: {dw_bad})", extra.len());
    }
}

/// Carrier detect on real traffic and on what is not a packet.
#[test]
#[ignore]
fn carrier_detect() {
    println!("\nCarrier detect, Demodulator<3, 1>:\n");
    println!("{:<16} {:>8} {:>18} {:>16} {:>14}", "audio", "frames", "busy at decode", "busy in frames", "busy outside");
    let mut results = Vec::new();
    for name in ["track01_9600", "track02_9600"] {
        let Some((wav, atest)) = track(name) else { continue };
        let (s, rate) = read_wav(&wav);
        let mut d: Demodulator = Demodulator::new(uafsk::demod::Config::new(rate));
        let mut busy = vec![false; s.len()];
        // (end sample, length in bytes) of each frame, and whether the demodulator was busy as it ended.
        let mut frames: Vec<(usize, usize, bool)> = Vec::new();
        for (i, &x) in s.iter().enumerate() {
            let mut got = Vec::new();
            d.push(x, |f| got.push(f.len()));
            busy[i] = d.busy();
            frames.extend(got.into_iter().map(|n| (i, n, busy[i])));
        }
        // A frame's span: its bytes and FCS with about 3% bit stuffing, plus the opening flag.
        let mut inside = vec![false; s.len()];
        for &(end, n, _) in &frames {
            let bits = ((n + 2) * 8 * 103 / 100 + 8) as u64;
            let start = end.saturating_sub((bits * rate as u64 / 1200) as usize);
            inside[start..=end].iter_mut().for_each(|v| *v = true);
        }
        // "Outside": more than half a second from any frame we or Dire Wolf decoded (preambles are allowed to be
        // busy). What is left is still not all quiet: collisions and packets neither decoder gets.
        let margin = rate as usize / 2;
        let mut near = vec![false; s.len()];
        let atest = std::fs::read(&atest).unwrap();
        for line in String::from_utf8_lossy(&atest).lines() {
            // "DECODED[12] 1:23.456 ...": the time the frame ended.
            let Some(at) = line.find("DECODED[") else { continue };
            let t = line[at..].split_whitespace().nth(1).unwrap_or("");
            let Some((m, sec)) = t.split_once(':') else { continue };
            let (Ok(m), Ok(sec)) = (m.parse::<f64>(), sec.parse::<f64>()) else { continue };
            let end = ((m * 60.0 + sec) * rate as f64) as usize;
            let start = end.saturating_sub(rate as usize * 3 / 4);
            near[start.saturating_sub(margin).min(s.len())..(end + margin).min(s.len())].iter_mut().for_each(|v| *v = true);
        }
        for &(end, n, _) in &frames {
            let bits = ((n + 2) * 8 * 103 / 100 + 8) as u64;
            let start = end.saturating_sub((bits * rate as u64 / 1200) as usize);
            near[start.saturating_sub(margin)..(end + margin).min(s.len())].iter_mut().for_each(|v| *v = true);
        }
        let at_decode = frames.iter().filter(|f| f.2).count();
        let (mut in_busy, mut in_n, mut out_busy, mut out_n) = (0usize, 0usize, 0usize, 0usize);
        for i in 0..s.len() {
            if inside[i] {
                in_n += 1;
                in_busy += busy[i] as usize;
            } else if !near[i] {
                out_n += 1;
                out_busy += busy[i] as usize;
            }
        }
        // Busy stretches outside: short blips would be false triggers, long ones transmissions nobody decoded.
        let mut runs = Vec::new();
        let mut run = 0usize;
        for i in 0..s.len() {
            if busy[i] && !inside[i] && !near[i] {
                run += 1;
            } else if run > 0 {
                runs.push(run * 1000 / rate as usize);
                run = 0;
            }
        }
        runs.sort();
        let short = runs.iter().filter(|&&r| r < 100).count();
        if std::env::var("UAFSK_SHOW").is_ok() {
            eprintln!("  {name}: {} busy stretches outside frames; median {} ms; {} under 100 ms; longest {} ms", runs.len(), runs.get(runs.len() / 2).unwrap_or(&0), short, runs.last().unwrap_or(&0));
        }
        let pct = |a: usize, b: usize| 100.0 * a as f64 / b.max(1) as f64;
        println!(
            "{name:<16} {:>8} {:>17.1}% {:>15.1}% {:>13.1}%",
            frames.len(),
            pct(at_decode, frames.len()),
            pct(in_busy, in_n),
            pct(out_busy, out_n)
        );
        results.push(format!("RESULT dcd.{name}.busy_at_decode_permille {}", at_decode * 1000 / frames.len().max(1)));
        results.push(format!("RESULT dcd.{name}.busy_in_frames_permille {}", in_busy * 1000 / in_n.max(1)));
        results.push(format!("RESULT dcd.{name}.busy_outside_permille {}", out_busy * 1000 / out_n.max(1)));
    }
    // Not packets: noise, silence, steady tones, a voice-like sweep.
    let rate = 9600u32;
    let mut seed = 99u64;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let n = rate as usize * 60;
    let noise: Vec<i16> = (0..n).map(|_| ((rnd() - 0.5) * 20000.0) as i16).collect();
    let tone = |f: f64| -> Vec<i16> { (0..n).map(|i| (8000.0 * (2.0 * std::f64::consts::PI * f * i as f64 / rate as f64).sin()) as i16).collect() };
    let sweep: Vec<i16> = (0..n)
        .map(|i| {
            let t = i as f64 / rate as f64;
            let f = 600.0 + 400.0 * (2.0 * std::f64::consts::PI * 3.0 * t).sin();
            (8000.0 * (2.0 * std::f64::consts::PI * f * t + 200.0 * (t * 0.7).sin()).sin()) as i16
        })
        .collect();
    for (label, audio) in [("noise", noise), ("silence", vec![0; n]), ("1200 Hz", tone(1200.0)), ("2200 Hz", tone(2200.0)), ("1000 Hz", tone(1000.0)), ("sweep", sweep)] {
        let mut d: Demodulator = Demodulator::new(uafsk::demod::Config::new(rate));
        let busy = audio.iter().filter(|&&x| {
            d.push(x, |_| {});
            d.busy()
        }).count();
        let pm = busy * 1000 / audio.len();
        println!("{label:<16} busy {:>5.1}%", pm as f64 / 10.0);
        results.push(format!("RESULT dcd.{}.busy_permille {pm}", label.replace(' ', "_")));
    }
    println!();
    for r in results {
        println!("{r}");
    }
}

/// Off-air recordings in the private test data's aprs/ (any `NAME.wav`, mono 16-bit, any rate; never redistributed), each against
/// Dire Wolf's `atest -h NAME.wav` in testdata/aprs/ref/NAME.atest.txt: frames with a correct CRC, how many Dire Wolf
/// also decoded, and what only one side got. Run with
/// `cargo test --release -p uafsk --test report recordings -- --ignored --nocapture`.
#[test]
#[ignore]
fn recordings() {
    use uafsk::demod::Config;
    // the recordings' references: only the private test data has them
    let Some(refs) = testdata_private_file("aprs/ref") else { return };
    let dir = refs.parent().unwrap().to_path_buf();
    let mut wavs: Vec<_> = std::fs::read_dir(&dir).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    wavs.retain(|p| p.extension().is_some_and(|e| e == "wav") && dir.join("ref").join(format!("{}.atest.txt", p.file_stem().unwrap().to_string_lossy())).exists());
    wavs.sort();
    if wavs.is_empty() {
        println!("no recordings with a Dire Wolf reference in {}", dir.display());
        return;
    }
    fn run<const S: usize, const C: usize>(cfg: Config, s: &[i16]) -> Vec<Vec<u8>> {
        let mut d = Box::new(Demodulator::<S, C>::new(cfg));
        let mut got = Vec::new();
        for &x in s {
            d.push(x, |f| got.push(f.to_vec()));
        }
        got
    }
    type Run = fn(u32, &[i16]) -> Vec<Vec<u8>>;
    let rows: [(&str, &str, Run); 4] = [
        ("1 slicer, high-pass", "s1", |r, s| run::<1, 1>(Config::new(r), s)),
        ("3 slicers, high-pass (default)", "default", |r, s| run::<3, 1>(Config::new(r), s)),
        ("3 slicers, dual chain", "dual", |r, s| run::<3, 2>(Config::new(r), s)),
        ("4 slicers (+ raw), high-pass", "s4", |r, s| run::<4, 1>(Config::new(r), s)),
    ];
    for w in &wavs {
        let stem = w.file_stem().unwrap().to_string_lossy().to_string();
        let (s, rate) = read_wav(w);
        let dw = atest_frames(&dir.join("ref").join(format!("{stem}.atest.txt")));
        println!("\n{stem}: {:.0} min at {} Hz; Dire Wolf atest: {} frames", s.len() as f64 / rate as f64 / 60.0, rate, dw.len());
        println!("RESULT aprs.{stem}.dire_wolf {}", dw.len());
        for (name, key, f) in rows {
            let got = f(rate, &s);
            let both = common(&got, &dw);
            println!("  {name:32} {:4} frames, {:4} also Dire Wolf's, {:3} only ours, {:3} only Dire Wolf's",
                got.len(), both, got.len() - both, dw.len() - both);
            println!("RESULT aprs.{stem}.{key} {} {}", got.len(), both);
            // UAFSK_MISSED=1: one line per Dire Wolf frame, 1 when the default decoded it too (for analysis).
            if key == "default" && std::env::var("UAFSK_MISSED").is_ok() {
                let mut have = count(&got);
                let marks: Vec<String> = dw.iter().map(|f| match have.get_mut(f.as_slice()) {
                    Some(n) if *n > 0 => { *n -= 1; "1".into() }
                    _ => "0".into(),
                }).collect();
                std::fs::write(dir.join("ref").join(format!("{stem}.ours.txt")), marks.join("\n")).unwrap();
            }
        }
    }
}

/// The CRC, checked independently on the off-air recordings: every frame the default front end's deframers hand
/// out (all slicers, before the demodulator drops repeats; repaired frames included) has its CRC-16/X.25
/// recomputed here, with this test's own bit-by-bit routine, and compared with the FCS that came off the air (or
/// out of the repair); plus the AX.25 address check. Run with
/// `cargo test --release -p uafsk --test report recordings_crc -- --ignored --nocapture`.
#[test]
#[ignore]
fn recordings_crc() {
    use uafsk::demod::Config;
    use uafsk::fsk::{Fsk, BELL202};
    use uafsk::hdlc::Deframer;
    // CRC-16/X.25 written out here, not udsp's: reflected 0x1021, init 0xFFFF, final XOR 0xFFFF.
    fn crc(data: &[u8]) -> u16 {
        let mut c: u16 = 0xFFFF;
        for &b in data {
            for i in 0..8 {
                let bit = ((b >> i) & 1) as u16;
                let top = c & 1;
                c >>= 1;
                if top ^ bit != 0 {
                    c ^= 0x8408;
                }
            }
        }
        !c
    }
    let Some(refs) = testdata_private_file("aprs/ref") else { return };
    let dir = refs.parent().unwrap().to_path_buf();
    let mut wavs: Vec<_> = std::fs::read_dir(&dir).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    wavs.retain(|p| p.extension().is_some_and(|e| e == "wav"));
    wavs.sort();
    for w in &wavs {
        let (s, rate) = read_wav(w);
        let cfg = Config::new(rate);
        let mut fsk = Box::new(Fsk::<3, 1>::new(cfg, BELL202));
        let mut dfs: [Deframer; 3] = core::array::from_fn(|_| Deframer::new());
        for d in dfs.iter_mut() {
            d.set_repair(cfg.repair);
        }
        let mut last = [false; 3];
        let (mut n, mut bad_crc, mut bad_addr, mut repaired) = (0usize, 0usize, 0usize, 0usize);
        for &x in &s {
            fsk.push_soft(x, |i, mark, margin| {
                let bit = (mark == last[i]) as u8;
                last[i] = mark;
                let df = &mut dfs[i];
                let got = df.push_soft(bit, margin.unsigned_abs()).map(|(f, fcs)| (f.to_vec(), fcs));
                if let Some((frame, fcs)) = got {
                    let frame = &frame[..];
                    n += 1;
                    repaired += df.repaired() as usize;
                    if crc(frame) != fcs {
                        bad_crc += 1;
                    }
                    if !ax25_ok(frame) {
                        bad_addr += 1;
                    }
                }
            });
        }
        println!("{}: {n} frames from the 3 slicers ({repaired} repaired); {bad_crc} with an FCS that doesn't match an independent CRC; {bad_addr} failing the AX.25 address check",
            w.file_name().unwrap().to_string_lossy());
        println!("RESULT aprs_crc.{} {n} {bad_crc} {bad_addr} {repaired}", w.file_stem().unwrap().to_string_lossy());
        assert_eq!(bad_crc, 0, "a frame passed the deframer with a wrong FCS");
    }
}
