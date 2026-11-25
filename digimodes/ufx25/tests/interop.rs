//! Interoperability with Dire Wolf (used only as a black box: its `gen_packets` and `atest` programs).
//!
//! - `dire_wolf_to_us`: WAVs from `gen_packets -X 16|32|64` (the test data's fx25/, made by tools/interop.sh) through
//!   uafsk's tone detector, NRZI, and [`ufx25::Receiver`]; every FX.25 frame must decode with a good FCS, also
//!   with noise bursts written over part of each block.
//! - `write_wavs`: our transmissions, modulated here, to the test data's fx25/ours/ for `atest -d x` (tools/interop.sh).
//!
//! `cargo test --release -p ufx25 --test interop -- --ignored --nocapture`

use ferrite_fuzzkit::testdata_file;
use std::path::Path;
use uafsk::demod::Config;
use uafsk::fsk::{Fsk, BELL202};
use uafsk::hdlc::Deframer;
use udsp::trig::{cos_sin_phase, phase_step};
use ufx25::{Encoder, Receiver};


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

fn write_wav(path: &Path, rate: u32, s: &[i16]) {
    let mut d = Vec::new();
    let data_len = (s.len() * 2) as u32;
    d.extend_from_slice(b"RIFF");
    d.extend_from_slice(&(36 + data_len).to_le_bytes());
    d.extend_from_slice(b"WAVEfmt ");
    d.extend_from_slice(&16u32.to_le_bytes());
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&rate.to_le_bytes());
    d.extend_from_slice(&(rate * 2).to_le_bytes());
    d.extend_from_slice(&2u16.to_le_bytes());
    d.extend_from_slice(&16u16.to_le_bytes());
    d.extend_from_slice(b"data");
    d.extend_from_slice(&data_len.to_le_bytes());
    for x in s {
        d.extend_from_slice(&x.to_le_bytes());
    }
    std::fs::write(path, d).unwrap();
}

#[derive(Default, Debug)]
struct Result {
    /// Frames from the FX.25 receivers (deduplicated by FCS), with the corrected byte counts.
    fx25: Vec<(Vec<u8>, u8)>,
    /// Frames from plain AX.25 deframers on the same bits.
    plain: Vec<Vec<u8>>,
    /// Sample index at which each FX.25 tag was found, and the block length (bytes).
    tags: Vec<(usize, usize)>,
}

/// uafsk's three slicers, NRZI, one FX.25 receiver and one plain deframer per slicer (as uafsk's report test
/// `recordings_crc` does). Frames found by several slicers count once.
fn decode(samples: &[i16], rate: u32) -> Result {
    let mut fsk = Box::new(Fsk::<3, 1>::new(Config::new(rate), BELL202));
    let mut rx: [Receiver; 3] = core::array::from_fn(|_| Receiver::new());
    let mut df: [Deframer; 3] = core::array::from_fn(|_| Deframer::new());
    let mut last = [false; 3];
    let mut out = Result::default();
    let mut seen_fx: Vec<(u16, usize)> = Vec::new();
    let mut seen_plain: Vec<(u16, usize)> = Vec::new();
    for (t, &x) in samples.iter().enumerate() {
        fsk.push(x, |i, mark| {
            let bit = (mark == last[i]) as u8;
            last[i] = mark;
            let was = rx[i].busy();
            if let Some(d) = rx[i].push(bit) {
                if !seen_fx.iter().any(|&(f, at)| f == d.fcs && t - at < rate as usize) {
                    seen_fx.push((d.fcs, t));
                    out.fx25.push((d.frame.to_vec(), d.corrected));
                }
            } else if !was && rx[i].busy() && i == 0 {
                out.tags.push((t, 0));
            }
            if let Some((f, fcs)) = df[i].push_fcs(bit) {
                if !seen_plain.iter().any(|&(c, at)| c == fcs && t - at < rate as usize) {
                    seen_plain.push((fcs, t));
                    out.plain.push(f.to_vec());
                }
            }
        });
    }
    out
}

/// AX.25 UI frame (no FCS): addresses, control 0x03, PID 0xF0, information.
fn ax25(dst: &str, src: &str, path: &[&str], info: &[u8]) -> Vec<u8> {
    let mut f = Vec::new();
    let calls: Vec<&str> = [dst, src].iter().copied().chain(path.iter().copied()).collect();
    for (n, c) in calls.iter().enumerate() {
        let (call, ssid) = c.split_once('-').map(|(a, b)| (a, b.parse::<u8>().unwrap())).unwrap_or((c, 0));
        let mut b = [b' ' << 1; 6];
        for (i, ch) in call.bytes().enumerate() {
            b[i] = ch << 1;
        }
        f.extend_from_slice(&b);
        f.push(0x60 | (ssid << 1) | (n == calls.len() - 1) as u8);
    }
    f.extend_from_slice(&[0x03, 0xF0]);
    f.extend_from_slice(info);
    f
}

/// Bell 202 audio for a bit sequence (NRZI here: a 0 changes the tone), phase-continuous, as uafsk's Modulator.
fn modulate(bits: impl Iterator<Item = u8>, rate: u32, out: &mut Vec<i16>) {
    let rate_mhz = rate as u64 * 1000;
    let (mark_step, space_step, bit_step) =
        (phase_step(1_200_000, rate_mhz), phase_step(2_200_000, rate_mhz), phase_step(1_200_000, rate_mhz));
    let (mut phase, mut bit_phase, mut mark) = (0u32, 0u32, true);
    for b in bits {
        if b == 0 {
            mark = !mark;
        }
        loop {
            let (_, s) = cos_sin_phase(phase);
            phase = phase.wrapping_add(if mark { mark_step } else { space_step });
            out.push(((s * 16000) >> 15) as i16);
            let before = bit_phase;
            bit_phase = bit_phase.wrapping_add(bit_step);
            if bit_phase < before {
                break;
            }
        }
    }
}

fn frames() -> Vec<Vec<u8>> {
    let long = b"=4903.50N/07201.75W-Ferrite FX.25 test: a longer comment so the frame needs a bigger block than the short ones do";
    vec![
        ax25("APRS", "N0CALL", &[], b">short"),
        ax25("APZFER", "N0CALL-9", &["WIDE1-1", "WIDE2-1"], b"!4237.14NS07120.83W#"),
        ax25("APZFER", "N0CALL-9", &["WIDE1-1", "WIDE2-1"], long),
        ax25("APZFER", "N0CALL-9", &["WIDE2-2"], &[b'~'; 60]),
    ]
}

/// xorshift32, for repeatable error patterns.
struct Rng(u32);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
}

#[test]
#[ignore]
fn dire_wolf_to_us() {
    let msgs = 4; // FX.25 frames in msgs.txt that fit a block of every size (the fifth is too long for 32 and 64)
    for name in ["fx16", "fx32", "fx64", "fx1"] {
        let Some(wav) = testdata_file(&format!("fx25/{name}.wav")) else { return };
        let (s, rate) = read_wav(&wav);
        let r = decode(&s, rate);
        println!("{name}: {} FX.25 frames (corrected {:?}), {} plain AX.25", r.fx25.len(), r.fx25.iter().map(|f| f.1).collect::<Vec<_>>(), r.plain.len());
        assert!(r.fx25.len() >= msgs, "{name}");
        for (f, c) in &r.fx25 {
            assert!(r.plain.contains(f), "{name}: an FX.25 frame the plain deframer did not see on clean audio");
            assert_eq!(*c, 0);
        }

        // Write noise over a stretch of each block (inside the AX.25 frame): the plain deframer loses those
        // frames, the FX.25 receiver repairs them.
        let mut noisy = s.clone();
        let spb = rate as usize / 1200;
        let mut rng = Rng(0x2545_F491);
        for &(t, _) in &r.tags {
            let start = t + 16 * 8 * spb; // 16 bytes into the block
            let len = 3 * 8 * spb; // 3 bytes: at most 4 wrong bytes, fewer than any code's limit of 8
            for x in &mut noisy[start..(start + len).min(s.len())] {
                *x = (rng.next() >> 16) as i16 / 2;
            }
        }
        let n = decode(&noisy, rate);
        let fixed: Vec<u8> = n.fx25.iter().map(|f| f.1).collect();
        println!("{name} with noise bursts: {} FX.25 frames (corrected {:?}), {} plain AX.25", n.fx25.len(), fixed, n.plain.len());
        assert_eq!(n.fx25.len(), r.fx25.len(), "{name}: every block repaired");
        assert!(n.fx25.iter().all(|(f, _)| r.fx25.iter().any(|(g, _)| g == f)));
        assert!(n.plain.len() < r.plain.len());
    }

    // gen_packets -n 100: the built-in frame 100 times with rising noise. Dire Wolf's atest decodes 44, 42 and 41
    // of them (16, 32, 64 check bytes; see the README).
    for name in ["noise16", "noise32", "noise64"] {
        let Some(wav) = testdata_file(&format!("fx25/{name}.wav")) else { continue };
        let (s, rate) = read_wav(&wav);
        let r = decode(&s, rate);
        let fixed = r.fx25.iter().filter(|f| f.1 > 0).count();
        let both: usize = r.plain.len() + r.fx25.iter().filter(|(f, _)| !r.plain.contains(f)).count();
        println!("{name}: {} FX.25 ({fixed} with corrections), {} plain AX.25, {both} together", r.fx25.len(), r.plain.len());
        println!("RESULT fx25.{name} {} {fixed} {} {both}", r.fx25.len(), r.plain.len());

        // uafsk's Demodulator as a firmware runs it: blocks taken after each batch of samples and decoded apart; the
        // slicers share one block buffer (a tag found while it is busy is skipped). As many frames as one receiver
        // per slicer above.
        let mut d: Box<uafsk::Demodulator> = Box::new(uafsk::Demodulator::new(Config::new(rate)));
        let (mut plain, mut fx, mut fixed) = (0, 0, 0);
        for chunk in s.chunks(24) {
            for &x in chunk {
                d.push(x, |_| plain += 1);
            }
            if let Some(mut b) = d.take_fx25() {
                if let Some(f) = b.decode() {
                    fixed += (f.corrected > 0) as usize;
                    fx += !f.duplicate as usize;
                }
            }
        }
        println!("{name} through uafsk::Demodulator: {plain} plain, {fx} more from FX.25 ({fixed} blocks corrected), {} together; {} tags skipped (buffer busy)", plain + fx, d.fx25_skipped());
        assert_eq!(plain + fx, both, "{name}: the shared block buffer loses nothing");
        println!("RESULT fx25.demod.{name} {plain} {fx} {}", plain + fx);
    }
}

#[test]
#[ignore]
fn write_wavs() {
    let rate = 12_000;
    // into the test data checkout (testdata_dir's rule), only when it is there
    let Some(dir) = testdata_file("fx25") else { return };
    let dir = dir.join("ours");
    std::fs::create_dir_all(&dir).unwrap();
    let gap = |out: &mut Vec<i16>| out.extend(std::iter::repeat_n(0, rate as usize / 2));
    let mut rng = Rng(0x1357_9BDF);
    for check in [16, 32, 64] {
        // Clean, then each frame with t, then t + 1, wrong bytes in its block.
        for (label, extra) in [("clean", None), ("t", Some(0)), ("t1", Some(1))] {
            let mut audio = Vec::new();
            let mut sizes = Vec::new();
            for f in frames() {
                let Ok(mut enc) = Encoder::new(&f, check) else { continue };
                sizes.push(enc.mode().n);
                if let Some(e) = extra {
                    let n = enc.block().len();
                    let mut hit = vec![false; n];
                    let mut left = check / 2 + e;
                    while left > 0 {
                        let j = rng.next() as usize % n;
                        if !hit[j] {
                            hit[j] = true;
                            enc.block_mut()[j] ^= 1 + (rng.next() % 255) as u8;
                            left -= 1;
                        }
                    }
                }
                gap(&mut audio);
                modulate(enc.bits(30, 3), rate, &mut audio);
            }
            gap(&mut audio);
            write_wav(&dir.join(format!("x{check}_{label}.wav")), rate, &audio);
            // Our own receiver agrees.
            let r = decode(&audio, rate);
            let expect = match label {
                "t1" => 0,
                _ => sizes.len(),
            };
            println!("x{check}_{label}: blocks {sizes:?}; we decode {} FX.25 frames, corrected {:?}", r.fx25.len(), r.fx25.iter().map(|f| f.1).collect::<Vec<_>>());
            if label == "t1" {
                assert!(r.fx25.len() <= expect + 1);
            } else {
                assert_eq!(r.fx25.len(), expect);
            }
        }
    }
    // Tag bit errors 0..=12, one frame each, to measure the tolerance of Dire Wolf's tag correlator.
    let f = &frames()[1];
    let enc = Encoder::new(f, 16).unwrap();
    let mut audio = Vec::new();
    for errs in 0..=12usize {
        let mut bits: Vec<u8> = enc.bits(30, 3).collect();
        for e in 0..errs {
            bits[240 + e * 5] ^= 1;
        }
        gap(&mut audio);
        modulate(bits.into_iter(), rate, &mut audio);
    }
    gap(&mut audio);
    write_wav(&dir.join("tagerr.wav"), rate, &audio);
}
