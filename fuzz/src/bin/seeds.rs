//! Seed corpora for the fuzz targets, built from the decoders' test packets: TNC2 lines in, each target's input
//! format out (AX.25 frames, KISS, HDLC bits, FX.25 blocks and bit streams, AFSK audio at the radio's 24 kHz).
//!
//! `cargo run --release --bin seeds -- OUT_DIR TNC2_FILE...` (tools/fuzz.sh runs it). A file is TNC2 text, one
//! packet a line (`#` comments skipped), or `.hex`: one hex-encoded TNC2 line a line.

use std::fs;
use std::path::{Path, PathBuf};
use uafsk::{Modulator, TxConfig};
use uaprs::ax25::{Address, Frame, Tnc2};
use ufx25::Encoder;

fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

fn lines(path: &Path) -> Vec<Vec<u8>> {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let hex = path.extension().is_some_and(|e| e == "hex");
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| if hex { unhex(l) } else { Some(l.as_bytes().to_vec()) })
        .collect()
}

/// A TNC2 line as an AX.25 frame (without FCS); None when an address isn't an AX.25 callsign.
fn frame(line: &[u8]) -> Option<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let t = Tnc2::split(line)?;
    let src = Address::from_text(t.src)?;
    let dest = Address::from_text(t.dest)?;
    let mut path = Vec::new();
    if !t.path.is_empty() {
        for p in t.path.split(|&c| c == b',') {
            path.push(Address::from_text(p)?);
        }
    }
    let f = Frame::new(src, dest, &path, t.info)?;
    let mut out = vec![0u8; f.len()];
    f.write(&mut out)?;
    Some((out, dest.call().to_vec(), t.info.to_vec()))
}

fn pack_bits(bits: impl Iterator<Item = u8>) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, b) in bits.enumerate() {
        if i % 8 == 0 {
            out.push(0);
        }
        *out.last_mut().unwrap() |= b << (i % 8);
    }
    out
}

fn audio(samples: impl Iterator<Item = i16>) -> Vec<u8> {
    samples.flat_map(|s| s.to_le_bytes()).collect()
}

/// A 16-bit mono WAV's samples and rate.
fn wav(path: &Path) -> Option<(Vec<i16>, u32)> {
    let b = fs::read(path).ok()?;
    let (mut i, mut rate) = (12, 0);
    while i + 8 <= b.len() {
        let n = u32::from_le_bytes(b[i + 4..i + 8].try_into().ok()?) as usize;
        let body = b.get(i + 8..(i + 8 + n).min(b.len()))?;
        match &b[i..i + 4] {
            b"fmt " if body.len() >= 16 => {
                if u16::from_le_bytes([body[2], body[3]]) != 1 || u16::from_le_bytes([body[14], body[15]]) != 16 {
                    return None;
                }
                rate = u32::from_le_bytes(body[4..8].try_into().ok()?);
            }
            b"data" => return Some((body.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect(), rate)),
            _ => {}
        }
        i += 8 + n + (n & 1);
    }
    None
}

struct Out(PathBuf);

impl Out {
    fn put(&self, target: &str, name: &str, data: &[u8]) {
        let d = self.0.join(target);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join(name), data).unwrap();
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = Out(PathBuf::from(args.next().expect("usage: seeds OUT_DIR TNC2_FILE...")));
    let tx = TxConfig { rate_hz: ferrite_core::decoders::RATE_HZ, preamble: 4, postamble: 2, amp: 12000 };
    let mut frames = Vec::new();
    let (wavs, args): (Vec<String>, Vec<String>) = args.partition(|f| f.ends_with(".wav"));
    for file in args {
        for (i, line) in lines(Path::new(&file)).into_iter().enumerate() {
            let stem = Path::new(&file).file_stem().unwrap().to_string_lossy().into_owned();
            let name = format!("{stem}-{i:04}");
            out.put("ax25_frame", &format!("{name}.tnc2"), &line);
            let Some((f, dest, info)) = frame(&line) else { continue };
            let mut p = vec![dest.len() as u8];
            p.extend_from_slice(&dest);
            p.extend_from_slice(&info);
            out.put("aprs_parse", &name, &p);
            out.put("ax25_frame", &name, &f);
            let mut k = vec![uaprs::kiss::FEND];
            uaprs::kiss::encode(0, &f, |b| k.push(b));
            out.put("kiss", &name, &k);
            let mut l = f.clone();
            l.push(0xC0);
            l.extend_from_slice(&f); // the same frame again: a duplicate
            out.put("aprs_live", &name, &l);
            let mut h = vec![3u8];
            h.extend(uafsk::hdlc::Framer::new(&f, 2, 2).map(|b| 0xF0 | b));
            out.put("hdlc_bits", &name, &h);
            for check in [16, 32, 64] {
                let Ok(e) = Encoder::new(&f, check) else { continue };
                let mut b = vec![e.mode().id - 1];
                b.extend_from_slice(e.block());
                out.put("fx25_block", &format!("{name}-{check}"), &b);
                let mut bits = vec![8u8];
                bits.extend(pack_bits(e.bits(2, 2)));
                out.put("fx25_bits", &format!("{name}-{check}"), &bits);
            }
            frames.push((name, f));
        }
    }
    // audio: the shortest frames (a frame at 24 kHz is 40 bytes of input a bit), plain and FX.25
    frames.sort_by_key(|(_, f)| f.len());
    for (name, f) in frames.iter().take(24) {
        out.put("afsk_audio", name, &audio(Modulator::new(f, tx)));
        if let Ok(e) = Encoder::new(f, 16) {
            out.put("afsk_audio", &format!("{name}-fx16"), &audio(Modulator::from_bits(e.bits(4, 2), tx)));
        }
    }
    // a few frames back to back on the receive path
    let mut l = Vec::new();
    for (_, f) in frames.iter().take(8) {
        l.extend_from_slice(f);
        l.push(0xC0);
    }
    out.put("aprs_live", "several", &l);
    // ACARS: each sample frame alone, then all of them in a row (SOH between)
    let mut all = Vec::new();
    for (k, (_, f, _)) in ferrite_app::acars::sample::FRAMES.iter().enumerate() {
        let mut b = [0u8; 256];
        let n = ferrite_core::acars::encode(f, &mut b);
        out.put("acars", &format!("sample-{k:02}"), &b[..n]);
        all.push(ferrite_core::acars::SOH);
        all.extend_from_slice(&b[..n]);
    }
    out.put("acars", "samples", &all);
    // the ACARS screen: the sample frames heard one by one, a key and a few seconds between; and with * on, a link
    // test (no text) heard on an empty list
    for flags in 0..8u8 {
        let mut v = vec![flags];
        for (k, (_, f, _)) in ferrite_app::acars::sample::FRAMES.iter().enumerate() {
            let mut b = [0u8; 256];
            let n = ferrite_core::acars::encode(f, &mut b);
            v.extend([0, (n - 1) as u8]);
            v.extend_from_slice(&b[..n - 1]);
            v.extend([2, k as u8, 4, 30]);
        }
        out.put("acars_screen", &format!("samples-{flags}"), &v);
    }
    // ACARS audio: the recordings (`acars/*.wav`), 3 s windows at their own rate
    for file in wavs {
        let Some((x, rate)) = wav(Path::new(&file)) else { continue };
        let cfg = match rate {
            12_000 => 1,
            16_000 => 2,
            48_000 => 3,
            _ => 0,
        };
        let stem = Path::new(&file).file_stem().unwrap().to_string_lossy().into_owned();
        for (k, w) in x.chunks(rate as usize * 3).enumerate() {
            let mut v = vec![cfg | (k as u8 % 16) << 2];
            v.extend(audio(w.iter().copied()));
            out.put("acars_audio", &format!("{stem}-{k:03}"), &v);
        }
    }
    println!("{} frames", frames.len());
}
