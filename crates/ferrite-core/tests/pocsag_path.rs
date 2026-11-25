//! The pager decoder through the radio's receive path (DEC-4, roadmap 6b): audio as ADC1 samples PA2 (48 kHz,
//! 12-bit counts around mid-scale), [`Decim`]'s 4:1 step, then the decoder slot's POCSAG receiver at 12 kHz and
//! [`Page`]. A synthetic transmission always runs; the recordings in the test data's `pocsag/`
//! (`ferrite_fuzzkit::testdata_dir`'s rule; the Signal Identification Wiki ones from the testdata repo's
//! `fetch/all.sh`; each skipped when missing) run with
//! `cargo test --release -p ferrite-core --test pocsag_path -- --ignored --nocapture`.

use ferrite_core::audio::{Decim, ADC_HZ, BLOCK_IN, BLOCK_OUT};
use ferrite_core::decoders::{Decoder, Found, Page};
use std::path::{Path, PathBuf};

/// Samples through the radio's path: the decoded pages.
fn through(adc: &[u16]) -> Vec<Page> {
    let mut dec = Decim::new();
    let mut d = Box::new(Decoder::pocsag());
    let mut pages = Vec::new();
    for block in adc.chunks(BLOCK_IN) {
        let mut out = [0i16; BLOCK_OUT];
        let mut n = 0;
        dec.block(block, |x| {
            out[n] = x;
            n += 1;
        });
        d.push(&out[..n], |f| {
            if let Found::Page(m) = f {
                pages.push(Page::from(m));
            }
        });
    }
    // Silence after, so the last message ends.
    let quiet = [2048u16; BLOCK_IN];
    for _ in 0..500 {
        let mut out = [0i16; BLOCK_OUT];
        let mut n = 0;
        dec.block(&quiet, |x| {
            out[n] = x;
            n += 1;
        });
        d.push(&out[..n], |f| {
            if let Found::Page(m) = f {
                pages.push(Page::from(m));
            }
        });
    }
    pages
}

/// A transmission as discriminator audio at the ADC: the preamble and codewords as two levels (`peak` counts
/// each side of mid-scale), most significant bit first, softened by a one-pole low-pass at about the baud rate.
fn synth(baud: u32, words: &[u32], peak: f64) -> Vec<u16> {
    let mut bits: Vec<bool> = (0..upocsag::encode::PREAMBLE_BITS).map(|i| i % 2 == 0).collect();
    for w in words {
        bits.extend((0..32).rev().map(|k| w >> k & 1 != 0));
    }
    let spb = ADC_HZ as f64 / baud as f64;
    let n = (bits.len() as f64 * spb) as usize;
    let a = 1.0 - (-2.0 * std::f64::consts::PI * baud as f64 / ADC_HZ as f64).exp();
    let mut y = 0.0;
    let mut out = vec![2048u16; ADC_HZ as usize / 10]; // 100 ms of quiet first
    for k in 0..n {
        // POCSAG: a 1 is the lower frequency, which the discriminator gives as the lower level.
        let x = if bits[(k as f64 / spb) as usize] { -peak } else { peak };
        y += a * (x - y);
        out.push((2048.0 + y).round().clamp(0.0, 4095.0) as u16);
    }
    out
}

#[test]
fn synthetic_pages_at_every_rate() {
    for baud in [512, 1200, 2400] {
        let mut words = Vec::new();
        upocsag::encode::encode(1_234_567, 3, upocsag::encode::Content::Alpha(b"STN 4 RESPOND HWY 417"), |w| words.push(w));
        let pages = through(&synth(baud, &words, 600.0));
        let p = pages.iter().find(|p| p.capcode == 1_234_567).unwrap_or_else(|| panic!("{baud} Bd: nothing decoded"));
        assert_eq!((p.baud, p.function, p.text()), (baud as u16, 3, &b"STN 4 RESPOND HWY 417"[..]), "{baud} Bd");
    }
}

/// A file of the test data's `pocsag/` (`ferrite_fuzzkit::testdata_dir`'s rule).
fn testdata(rel: &str) -> PathBuf {
    ferrite_fuzzkit::testdata_dir().join("pocsag").join(rel)
}

fn read_wav(path: &Path) -> Option<(Vec<i16>, u32)> {
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

/// A recording as the ADC would sample it: resampled to 48 kHz (linear), full scale swinging `peak` counts.
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

/// The recordings' messages, against multimon-ng's count (the polarity that decodes more), and against upocsag
/// fed the same recording directly at its own rate.
#[test]
#[ignore]
fn recordings_through_the_radio_path() {
    const FILES: [&str; 7] = ["commons_512", "sigid_512", "sigid_1200", "sigid_2400", "sigid_1200_variant", "sigid_148", "sigid_sound"];
    println!("\nPOCSAG recordings through the radio's path (48 kHz ADC, Decim, POCSAG at 12 kHz)");
    println!("{:<20} {:>9} {:>12} {:>14} {:>14}", "recording", "multimon", "direct 22k", "radio, 1500", "radio, 400");
    for name in FILES {
        let Some((s, rate)) = ferrite_fuzzkit::testdata_file(&format!("pocsag/{name}.22050.wav")).and_then(|p| read_wav(&p)) else {
            continue;
        };
        let refs = ["multimon", "multimon_inv"]
            .map(|k| std::fs::read_to_string(testdata(&format!("{name}.{k}.txt"))).map(|t| t.lines().count()).unwrap_or(0))
            .into_iter()
            .max()
            .unwrap();
        let mut direct = 0;
        let mut rx: upocsag::Receiver = upocsag::Receiver::new(rate);
        for &x in &s {
            rx.push(x, |_| direct += 1);
        }
        rx.flush(|_| direct += 1);
        let ours: Vec<usize> = [1500, 400].iter().map(|&peak| through(&as_adc(&s, rate, peak)).len()).collect();
        println!("{name:<20} {refs:>9} {direct:>12} {:>14} {:>14}", ours[0], ours[1]);
    }
}
