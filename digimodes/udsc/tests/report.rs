//! udsc on calls built by an independent encoder (dsc_generator; the test data's `dsc/`, made by the
//! testdata repo's `tools/make-dsc.sh`, found by `ferrite_fuzzkit::testdata_dir`'s rule, skipped when missing): exact symbols and
//! fields, then in noise and with radio-like audio.
//!
//! `cargo test --release -p udsc --test report -- --ignored --nocapture`

use ferrite_fuzzkit::{testdata_dir, testdata_file};
use serde_json::Value;
use std::path::PathBuf;
use udsc::message::{Address, Message};
use udsc::Receiver;
use uafsk::fsk::Config;

fn dir() -> PathBuf {
    testdata_dir().join("dsc")
}

fn read_wav(name: &str) -> Vec<i16> {
    let d = std::fs::read(dir().join(format!("{name}.wav"))).unwrap_or_else(|e| panic!("{name}: {e} (the testdata repo's tools/make-dsc.sh)"));
    d[44..].as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c)).collect()
}

/// Every call decoded from `audio`: (symbols, ECC ok).
fn decode(audio: &[i16]) -> Vec<(Vec<u8>, bool)> {
    decode_with(audio, true)
}

fn decode_with(audio: &[i16], highpass: bool) -> Vec<(Vec<u8>, bool)> {
    let mut rx: Receiver = Receiver::new(Config { highpass, ..Config::new(9600) });
    let mut out = Vec::new();
    for &x in audio {
        rx.push(x, |c| out.push((c.symbols.to_vec(), c.ecc_ok)));
    }
    out
}

/// The fields we expect, as the vector file states them, against ours.
fn fields_match(m: &Message, f: &Value) -> Result<(), String> {
    let get = |k: &str| f.get(k).and_then(|v| v.as_i64());
    let mut errs: Vec<String> = Vec::new();
    fn check_(errs: &mut Vec<String>, name: &str, ours: Option<i64>, want: Option<i64>) {
        if want.is_some() && ours != want {
            errs.push(format!("{name}: ours {ours:?}, want {want:?}"));
        }
    }
    macro_rules! check {
        ($n:expr, $o:expr, $w:expr) => {
            check_(&mut errs, $n, $o, $w)
        };
    }
    check!("format", Some(m.format as i64), get("format"));
    check!("category", Some(m.category as i64), get("category"));
    check!("self_id", Some(m.self_id as i64), get("self_id"));
    check!("tc1", m.tc1.map(i64::from), get("tc1"));
    check!("tc2", m.tc2.map(i64::from), get("tc2"));
    check!("channel", m.channel.map(i64::from), get("channel"));
    check!("eos", Some(m.eos as i64), get("eos"));
    if let Some(a) = get("address") {
        check!("address", match m.address { Address::Mmsi(x) => Some(x as i64), _ => None }, Some(a));
    }
    if let Some(area) = f.get("area") {
        let want: Vec<i64> = area.as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
        let ours = match m.address {
            Address::Area { quadrant, lat, lon, dlat, dlon } => vec![quadrant as i64, lat as i64, lon as i64, dlat as i64, dlon as i64],
            _ => vec![],
        };
        if ours != want {
            errs.push(format!("area: ours {ours:?}, want {want:?}"));
        }
    }
    if get("nature").is_some() {
        let d = m.distress.ok_or("no distress fields")?;
        check!("nature", Some(d.nature as i64), get("nature"));
        check!("distressed", Some(d.mmsi as i64), get("distressed").or(get("self_id")));
        check!("lat_min", d.position.map(|p| p.lat_min as i64), get("lat_min"));
        check!("lon_min", d.position.map(|p| p.lon_min as i64), get("lon_min"));
        if get("lat_min").is_none() && d.position.is_some() {
            errs.push("position should be unknown".into());
        }
        let want_utc = f.get("utc").and_then(|u| u.as_array()).map(|u| (u[0].as_i64().unwrap() as u8, u[1].as_i64().unwrap() as u8));
        if d.utc != want_utc {
            errs.push(format!("utc: ours {:?}, want {want_utc:?}", d.utc));
        }
        check!("subsequent", Some(d.subsequent as i64), get("subsequent"));
    }
    if errs.is_empty() { Ok(()) } else { Err(errs.join("; ")) }
}

#[test]
#[ignore]
fn generated_calls() {
    let Some(path) = testdata_file("dsc/vectors.json") else { return };
    let vectors: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut results = Vec::new();
    println!("\nCalls from dsc_generator, clean:\n");
    let mut exact = 0;
    for v in &vectors {
        let name = v["name"].as_str().unwrap();
        let truth: Vec<u8> = v["symbols"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect();
        let want = &truth[..truth.len() - 3]; // format specifiers to EOS (then ECC, EOS, EOS)
        let got = decode(&read_wav(name));
        let ok = got.len() == 1 && got[0].0 == want && got[0].1;
        let fields = got.first().and_then(|g| Message::parse(&g.0)).map(|m| fields_match(&m, &v["fields"]));
        let fields_ok = matches!(fields, Some(Ok(())));
        println!("  {name:<28} {} {}", if ok { "symbols ok" } else { "SYMBOLS WRONG" }, match &fields {
            Some(Ok(())) => "fields ok".to_string(),
            Some(Err(e)) => format!("FIELDS: {e}"),
            None => "NO FIELDS".to_string(),
        });
        exact += (ok && fields_ok) as u32;
    }
    let three = decode(&read_wav("three_calls"));
    println!("  {:<28} {} calls", "three_calls", three.len());
    results.push(format!("RESULT dsc.clean.exact {exact}"));
    results.push(format!("RESULT dsc.clean.calls {}", vectors.len()));
    results.push(format!("RESULT dsc.three_calls {}", three.len()));

    // Noise: white noise over the 4.8 kHz band, SNR of the whole signal to it; and the same through a receiver's
    // de-emphasis (6 dB/octave above 300 Hz), as a speaker output would be. 20 tries per call and level.
    println!("\nCalls decoded exactly (of {} per row):\n", vectors.len() * 20);
    print!("{:<14}", "SNR (dB)");
    let snrs = [-3i32, 0, 3, 6, 9, 12];
    for s in snrs {
        print!("{s:>8}");
    }
    println!();
    for deemph in [false, true] {
        let label = if deemph { "de-emphasised" } else { "flat" };
        print!("{label:<14}");
        for snr in snrs {
            let mut seed = 7u64 + snr as u64;
            let mut gauss = || {
                let mut u = || {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    ((seed >> 11) as f64 + 0.5) / (1u64 << 53) as f64
                };
                (-2.0 * u().ln()).sqrt() * (2.0 * std::f64::consts::PI * u()).cos()
            };
            let mut n = 0;
            for v in &vectors {
                let truth: Vec<u8> = v["symbols"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect();
                let want = &truth[..truth.len() - 3];
                let clean = read_wav(v["name"].as_str().unwrap());
                let (a, b) = (clean.iter().position(|&x| x != 0).unwrap(), clean.iter().rposition(|&x| x != 0).unwrap());
                let p = clean[a..b].iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / (b - a) as f64;
                let sigma = (p / 10f64.powf(snr as f64 / 10.0)).sqrt();
                for _ in 0..20 {
                    // De-emphasis: a first-order low-pass at 300 Hz, gain-corrected at 1700 Hz.
                    let k = 1.0 - (-2.0 * std::f64::consts::PI * 300.0 / 9600.0f64).exp();
                    let mut y = 0.0;
                    let audio: Vec<i16> = clean
                        .iter()
                        .map(|&x| {
                            let s = x as f64 + sigma * gauss();
                            let s = if deemph {
                                y += k * (s - y);
                                y * 1700.0 / 300.0
                            } else {
                                s
                            };
                            s.clamp(-32768.0, 32767.0) as i16
                        })
                        .collect();
                    // Flat audio (a discriminator) without the high-pass; a speaker's de-emphasised audio with it.
                    n += decode_with(&audio, deemph).iter().any(|g| g.0 == want && g.1) as u32;
                }
            }
            print!("{n:>8}");
            results.push(format!("RESULT dsc.noise.{label}.snr{snr} {n}"));
        }
        println!();
    }
    println!();
    for r in results {
        println!("{r}");
    }
}
