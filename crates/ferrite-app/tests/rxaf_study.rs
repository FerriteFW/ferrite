//! The APRS slot's receive filter study (Oct 27, 2026; decision 309): what the BK4829's receive audio filters do to
//! the AFSK decoder's frame count, modelled on recorded audio. Each variant filters the recording at its own rate (the
//! difference the chip's bit makes), then runs the radio's path as `rate_study.rs` does (48 kHz ADC counts, the
//! decimating low-pass, the decoder at 24 kHz, `Config::new`). The chip's de-emphasis is modelled two ways: as
//! measured on the black radio (REG_2B 0x0000 against 0x0100 in a raw, gain-free 16 kHz capture of the decoders'
//! samples, LINK-13.9: +7.5 dB at 400 Hz, -6.4 dB at 2 kHz against 1 kHz) and as a textbook 6 dB/octave (one pole at
//! 300 Hz). Taking de-emphasis away also boosts the noise up to the chip's fixed receive wall (about 3.1 kHz, whatever
//! REG_2B says), so those variants end with that wall (fourth-order, 3.1 kHz). Ignored: it needs the test data's
//! recordings (the off-air ones private, never redistributed). Run with
//! `cargo test --release -p ferrite-app --test rxaf_study -- --ignored --nocapture` (about 90 s; from a worktree, set
//! `TESTDATA` to the test data checkout's absolute path).

use ferrite_core::audio::ADC_HZ;
use std::f64::consts::PI;

/// A recording of the test data (`ferrite_fuzzkit::testdata_dir`'s rule), when present, else a skip note: the
/// off-air APRS recordings (`aprs/`) are private, the WA8LMF CD comes from the testdata repo's `fetch/all.sh`.
fn testdata(rel: &str) -> Option<std::path::PathBuf> {
    if rel.starts_with("aprs/") { ferrite_fuzzkit::testdata_private_file(rel) } else { ferrite_fuzzkit::testdata_file(rel) }
}

fn read_wav(path: &std::path::Path) -> Option<(Vec<i16>, u32)> {
    let d = std::fs::read(path).ok()?;
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
    Some((data, rate))
}

/// The measured de-emphasis (dB against 1 kHz: REG_2B 0x0000 minus 0x0100, the raw 16 kHz capture of Oct 27, 2026),
/// held below 315 Hz and above 3.15 kHz: about 6 dB an octave across the band.
const DEEMPH_DB: [(f64, f64); 12] = [
    (315.0, 9.3), (400.0, 7.5), (500.0, 5.4), (630.0, 3.3), (800.0, 1.5), (1000.0, 0.0), (1250.0, -1.75), (1600.0, -4.1),
    (2000.0, -6.4), (2500.0, -8.3), (3150.0, -12.0), (4000.0, -12.0),
];

fn measured_db(f: f64) -> f64 {
    if f <= DEEMPH_DB[0].0 {
        return DEEMPH_DB[0].1;
    }
    for w in DEEMPH_DB.windows(2) {
        let ((f0, g0), (f1, g1)) = (w[0], w[1]);
        if f <= f1 {
            let t = (f.ln() - f0.ln()) / (f1.ln() - f0.ln());
            return g0 + (g1 - g0) * t;
        }
    }
    DEEMPH_DB[DEEMPH_DB.len() - 1].1
}

/// A linear-phase FIR (511 taps, Hann window) with gain `db(f)` at `rate`, by frequency sampling.
fn fir(rate: u32, db: impl Fn(f64) -> f64) -> Vec<f64> {
    let (n, m) = (511usize, 4096usize);
    let h: Vec<f64> = (0..n)
        .map(|i| {
            let t = i as f64 - (n - 1) as f64 / 2.0;
            let mut acc = 0.0;
            for k in 0..=m / 2 {
                let f = k as f64 * rate as f64 / m as f64;
                let g = 10f64.powf(db(f) / 20.0);
                let w = if k == 0 || k == m / 2 { 1.0 } else { 2.0 };
                acc += w * g * (2.0 * PI * k as f64 * t / m as f64).cos();
            }
            acc / m as f64 * (0.5 - 0.5 * (2.0 * PI * i as f64 / (n - 1) as f64).cos())
        })
        .collect();
    h
}

fn convolve(x: &[f64], h: &[f64]) -> Vec<f64> {
    let d = h.len() / 2;
    let mut p = vec![0.0; d];
    p.extend_from_slice(x);
    p.resize(x.len() + 2 * d, 0.0);
    let hr: Vec<f64> = h.iter().rev().copied().collect();
    p.windows(h.len()).map(|w| w.iter().zip(&hr).map(|(a, b)| a * b).sum()).collect()
}

/// One biquad (RBJ cookbook), Butterworth Q, low- or high-pass.
fn biquad(x: &[f64], rate: u32, hz: f64, q: f64, high: bool) -> Vec<f64> {
    let w = 2.0 * PI * hz / rate as f64;
    let (c, a) = (w.cos(), w.sin() / (2.0 * q));
    let (b0, b1, b2) = if high { ((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0) } else { ((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0) };
    let (a0, a1, a2) = (1.0 + a, -2.0 * c, 1.0 - a);
    let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
    x.iter()
        .map(|&x0| {
            let y = (b0 * x0 + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2) / a0;
            (x2, x1, y2, y1) = (x1, x0, y1, y);
            y
        })
        .collect()
}

/// Fourth-order Butterworth (two biquads).
fn butter4(x: &[f64], rate: u32, hz: f64, high: bool) -> Vec<f64> {
    let y = biquad(x, rate, hz, 0.5412, high);
    biquad(&y, rate, hz, 1.3066, high)
}

/// A 6 dB/octave de-emphasis, one pole at 300 Hz (bilinear), gain 1 at 1 kHz; `undo` is its exact inverse.
fn six_db(x: &[f64], rate: u32, undo: bool) -> Vec<f64> {
    let k = (PI * 300.0 / rate as f64).tan();
    // H(z) = k (1 + z^-1) / ((1 + k) + (k - 1) z^-1)
    let (b0, b1, a0, a1) = (k, k, 1.0 + k, k - 1.0);
    let wn = 2.0 * PI * 1000.0 / rate as f64;
    let h = |z: f64| {
        let (nr, ni) = (b0 + b1 * z.cos(), -b1 * z.sin());
        let (dr, di) = (a0 + a1 * z.cos(), -a1 * z.sin());
        ((nr * nr + ni * ni) / (dr * dr + di * di)).sqrt()
    };
    let g = 1.0 / h(wn);
    let (mut xp, mut yp) = (0.0, 0.0);
    x.iter()
        .map(|&x0| {
            let y = if undo { (a0 * x0 / g + a1 * xp / g - b1 * yp) / b0 } else { g * (b0 * x0 + b1 * xp) / a0 - a1 / a0 * yp };
            (xp, yp) = (x0, y);
            y
        })
        .collect()
}

/// As the ADC samples it: 48 kHz (linear), the audio's rms at `rms` counts around 2048 (each variant at the same level).
fn as_adc(s: &[f64], rate: u32, rms: f64) -> Vec<u16> {
    let r = (s.iter().map(|x| x * x).sum::<f64>() / s.len() as f64).sqrt().max(1.0);
    let n = (s.len() as u64 * ADC_HZ as u64 / rate as u64) as usize;
    (0..n)
        .map(|k| {
            let t = k as f64 * rate as f64 / ADC_HZ as f64;
            let (i, f) = (t as usize, t.fract());
            let a = s[i.min(s.len() - 1)];
            let b = s[(i + 1).min(s.len() - 1)];
            (2048.0 + (a + (b - a) * f) * rms / r).round().clamp(0.0, 4095.0) as u16
        })
        .collect()
}

/// `ferrite_core::audio`'s decimating low-pass (private there), copied.
static TAPS: [i32; 31] = [-4, 7, 46, 104, 135, 69, -150, -490, -788, -780, -199, 1070, 2877, 4799, 6275, 6826, 6275, 4799, 2877, 1070, -199, -780, -788, -490, -150, 69, 135, 104, 46, 7, -4];

/// Frames with a good CRC through the radio's path at 24 kHz (rate_study.rs's, with the radio's 31-tap low-pass).
fn frames(adc: &[u16], highpass: bool) -> u32 {
    let rate = ferrite_core::decoders::RATE_HZ;
    let step = (ADC_HZ / rate) as usize;
    let t = &TAPS;
    let mut hist = vec![0i16; t.len()];
    let (mut at, mut phase, mut dc) = (0usize, 0usize, 2048i32 << 16);
    use ferrite_core::audio::uafsk::{fsk::Config, Demodulator};
    let mut d = Box::new(Demodulator::<3, 1>::new(Config { highpass, ..Config::new(rate) }));
    let mut n = 0;
    for &a in adc {
        let x = (a as i32) << 16;
        dc += (x - dc) >> 11;
        hist[at] = ((x - dc) >> 12).clamp(-32768, 32767) as i16;
        at = (at + 1) % t.len();
        phase += 1;
        if phase == step {
            phase = 0;
            let acc: i32 = (0..t.len()).map(|k| hist[(at + k) % t.len()] as i32 * t[k]).sum();
            let y = (acc >> 15).clamp(-32768, 32767) as i16;
            d.push(y, |_| n += 1);
            d.release_fx25();
        }
    }
    n
}

#[test]
#[ignore]
fn rx_filter_study() {
    let audio = [
        ("WA8LMF track 1 (flat)", "wa8lmf/track01.wav"),
        ("WA8LMF track 2 (de-emph.)", "wa8lmf/track02.wav"),
        ("Off-air 1 (45 min)", "aprs/10212026_113050.wav"),
        ("Off-air 2 (25 min)", "aprs/10212026_175026.wav"),
    ];
    type V = (&'static str, bool, fn(&[f64], u32) -> Vec<f64>);
    let variants: [V; 10] = [
        ("as is", true, |x, _| x.to_vec()),
        ("+ chip de-emphasis (measured)", true, |x, r| convolve(x, &fir(r, measured_db))),
        ("- chip de-emphasis (measured)", true, |x, r| butter4(&convolve(x, &fir(r, |f| -measured_db(f))), r, 3100.0, false)),
        ("+ 6 dB/oct de-emphasis", true, |x, r| six_db(x, r, false)),
        ("- 6 dB/oct (pre-emphasis)", true, |x, r| butter4(&six_db(x, r, true), r, 3100.0, false)),
        ("+ 3 kHz low-pass", true, |x, r| butter4(x, r, 3000.0, false)),
        ("+ 300 Hz high-pass", true, |x, r| butter4(x, r, 300.0, true)),
        ("as is, decoder HP off", false, |x, _| x.to_vec()),
        ("- chip de-emph., decoder HP off", false, |x, r| butter4(&convolve(x, &fir(r, |f| -measured_db(f))), r, 3100.0, false)),
        ("- 6 dB/oct, decoder HP off", false, |x, r| butter4(&six_db(x, r, true), r, 3100.0, false)),
    ];
    let mut table = vec![vec![None::<u32>; audio.len()]; variants.len()];
    std::thread::scope(|sc| {
        let mut hs = Vec::new();
        for (ai, (_, rel)) in audio.iter().enumerate() {
            let Some((s, rate)) = testdata(rel).and_then(|p| read_wav(&p)) else { continue };
            let s: std::sync::Arc<Vec<f64>> = std::sync::Arc::new(s.iter().map(|&v| v as f64).collect());
            // rate_study.rs's level for the recording as it is (full scale at 1500 counts), kept for every variant.
            let rms = (s.iter().map(|x| x * x).sum::<f64>() / s.len() as f64).sqrt() * 1500.0 / 32768.0;
            for (vi, &(_, hp, f)) in variants.iter().enumerate() {
                let s = s.clone();
                hs.push(sc.spawn(move || (vi, ai, frames(&as_adc(&f(&s, rate), rate, rms), hp))));
            }
        }
        for h in hs {
            let (vi, ai, n) = h.join().unwrap();
            table[vi][ai] = Some(n);
        }
    });
    print!("\n{:<34}", "variant");
    for (name, _) in &audio {
        print!("{name:>28}");
    }
    println!();
    for ((name, _, _), row) in variants.iter().zip(&table) {
        print!("{name:<34}");
        for n in row {
            print!("{:>28}", n.map_or("-".into(), |n| n.to_string()));
        }
        println!();
    }
}
