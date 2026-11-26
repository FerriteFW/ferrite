//! The decoder rate study (Oct 22, 2026; decision 240): the radio's APRS receive path at 12, 16 and 24 kHz on recorded audio.
//! The audio goes as PA2's does (48 kHz, 12-bit counts around mid-scale, `Decim`'s DC removal), then a low-pass
//! designed for each step (Kaiser beta 6, 31 taps, Q15, as `audio::TAPS`) and the AFSK demodulator at the lower
//! rate. Frames with a good CRC are counted. Ignored: it needs the test data's recordings (the off-air ones private, never redistributed). Run
//! with `cargo test --release -p ferrite-app --test rate_study -- --ignored --nocapture`.

use ferrite_core::audio::ADC_HZ;

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

/// As the ADC samples it: 48 kHz (linear), `peak` counts around 2048.
fn as_adc(s: &[i16], rate: u32, peak: i32) -> Vec<u16> {
    let n = (s.len() as u64 * ADC_HZ as u64 / rate as u64) as usize;
    (0..n)
        .map(|k| {
            let t = k as f64 * rate as f64 / ADC_HZ as f64;
            let (i, f) = (t as usize, t.fract());
            let a = s[i.min(s.len() - 1)] as f64;
            let b = s[(i + 1).min(s.len() - 1)] as f64;
            (2048.0 + (a + (b - a) * f) * peak as f64 / 32768.0).round().clamp(0.0, 4095.0) as u16
        })
        .collect()
}

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term) = (1.0, 1.0);
    for k in 1..30 {
        term *= (x / 2.0 / k as f64).powi(2);
        sum += term;
    }
    sum
}

/// A 31-tap Kaiser-windowed sinc low-pass at `cut_hz` for 48 kHz, Q15, summing to 32768.
fn taps(cut_hz: f64) -> Vec<i32> {
    let n = 31;
    let fc = cut_hz / ADC_HZ as f64;
    let w: Vec<f64> = (0..n)
        .map(|i| {
            let m = i as f64 - (n - 1) as f64 / 2.0;
            let sinc = if m == 0.0 { 2.0 * fc } else { (2.0 * std::f64::consts::PI * fc * m).sin() / (std::f64::consts::PI * m) };
            let r = 2.0 * i as f64 / (n - 1) as f64 - 1.0;
            sinc * bessel_i0(6.0 * (1.0 - r * r).sqrt()) / bessel_i0(6.0)
        })
        .collect();
    let s: f64 = w.iter().sum();
    w.iter().map(|x| (x / s * 32768.0).round() as i32).collect()
}

/// Frames with a good CRC through the path at `rate` (48 kHz / an integer step).
fn frames(adc: &[u16], rate: u32, cut_hz: f64) -> u32 {
    let step = (ADC_HZ / rate) as usize;
    let t = taps(cut_hz);
    let mut hist = vec![0i16; t.len()];
    let (mut at, mut phase, mut dc) = (0usize, 0usize, 2048i32 << 16);
    let mut d = Box::new(ferrite_core::audio::uafsk::Demodulator::<3, 1>::new(ferrite_core::audio::uafsk::fsk::Config::new(rate)));
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
fn decoder_rate_study() {
    let audio = [
        ("WA8LMF track 1", "wa8lmf/track01.wav"),
        ("WA8LMF track 2", "wa8lmf/track02.wav"),
        ("Off-air 1 (45 min)", "aprs/10212026_113050.wav"),
        ("Off-air 2 (25 min)", "aprs/10212026_175026.wav"),
    ];
    println!("\n{:<20} {:>8} {:>8} {:>8} {:>8} {:>8}", "audio", "12k/5k", "16k/5k", "16k/6.5k", "24k/5k", "24k/9k");
    for (name, rel) in audio {
        let Some((s, rate)) = testdata(rel).and_then(|p| read_wav(&p)) else {
            println!("{name}: skipped");
            continue;
        };
        let adc = as_adc(&s, rate, 1500);
        let r = [(12_000, 5_000.0), (16_000, 5_000.0), (16_000, 6_500.0), (24_000, 5_000.0), (24_000, 9_000.0)].map(|(hz, cut)| frames(&adc, hz, cut));
        println!("{:<20} {:>8} {:>8} {:>8} {:>8} {:>8}", name, r[0], r[1], r[2], r[3], r[4]);
    }
}
