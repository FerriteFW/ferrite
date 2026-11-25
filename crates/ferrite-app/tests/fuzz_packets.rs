//! The packet decoders' fuzz targets (`fuzz/`, tools/fuzz.sh; digimodes/FUZZING.md) run deterministically in the
//! normal tests: the same harness functions (`fuzz/src/harness.rs`), fed the TNC2 test packets
//! (`digimodes/uaprs/tests/spec_packets.txt`, and when present the test data's `aprs/*.hex`) turned into each target's
//! format as `fuzz/src/bin/seeds.rs` does, then mutated (ferrite-fuzzkit). No panic, no broken round trip, no
//! case slower than the limit. `FERRITE_FUZZ_SCALE` runs more cases.

#[path = "../../../fuzz/src/harness.rs"]
mod harness;

use ferrite_fuzzkit::{hex_lines, mutate, root, samples_to_bytes, testdata, Fuzz, Rng};
use std::sync::OnceLock;
use uafsk::{Modulator, TxConfig};
use uaprs::ax25::{Address, Frame, Tnc2};
use ufx25::Encoder;

/// Every target's seeds, built once per test process.
struct Seeds {
    tnc2: Vec<Vec<u8>>,
    frames: Vec<Vec<u8>>,
    aprs: Vec<Vec<u8>>,
    kiss: Vec<Vec<u8>>,
    hdlc: Vec<Vec<u8>>,
    fx_block: Vec<Vec<u8>>,
    fx_bits: Vec<Vec<u8>>,
    audio: Vec<Vec<u8>>,
    live: Vec<Vec<u8>>,
}

/// A TNC2 line as an AX.25 frame (no FCS), its destination and information field.
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
        if i.is_multiple_of(8) {
            out.push(0);
        }
        *out.last_mut().unwrap() |= b << (i % 8);
    }
    out
}

fn seeds() -> &'static Seeds {
    static S: OnceLock<Seeds> = OnceLock::new();
    S.get_or_init(|| {
        let spec = std::fs::read_to_string(root().join("digimodes/uaprs/tests/spec_packets.txt")).unwrap();
        let mut tnc2: Vec<Vec<u8>> = spec.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')).map(|l| l.as_bytes().to_vec()).collect();
        if let Some(td) = testdata() {
            for f in ["spec.hex", "suite.hex", "wa8lmf.hex"] {
                // every 4th of the WA8LMF CD's 856: they repeat a lot
                let step = if f == "wa8lmf.hex" { 4 } else { 1 };
                tnc2.extend(hex_lines(&td.join("aprs").join(f)).into_iter().step_by(step));
            }
        }
        let tx = TxConfig { rate_hz: ferrite_core::decoders::RATE_HZ, preamble: 4, postamble: 2, amp: 12000 };
        let mut s = Seeds { tnc2: tnc2.clone(), frames: vec![], aprs: vec![], kiss: vec![], hdlc: vec![], fx_block: vec![], fx_bits: vec![], audio: vec![], live: vec![] };
        for line in &tnc2 {
            let Some((f, dest, info)) = frame(line) else { continue };
            let mut p = vec![dest.len() as u8];
            p.extend_from_slice(&dest);
            p.extend_from_slice(&info);
            s.aprs.push(p);
            let mut k = vec![uaprs::kiss::FEND];
            uaprs::kiss::encode(0, &f, |b| k.push(b));
            s.kiss.push(k);
            let mut l = f.clone();
            l.push(0xC0);
            l.extend_from_slice(&f);
            s.live.push(l);
            let mut h = vec![3u8];
            h.extend(uafsk::hdlc::Framer::new(&f, 2, 2).map(|b| 0xF0 | b));
            s.hdlc.push(h);
            for check in [16, 32, 64] {
                let Ok(e) = Encoder::new(&f, check) else { continue };
                let mut b = vec![e.mode().id - 1];
                b.extend_from_slice(e.block());
                s.fx_block.push(b);
                let mut bits = vec![8u8];
                bits.extend(pack_bits(e.bits(2, 2)));
                s.fx_bits.push(bits);
            }
            s.frames.push(f);
        }
        let mut short = s.frames.clone();
        short.sort_by_key(|f| f.len());
        for f in short.iter().take(6) {
            s.audio.push(samples_to_bytes(&Modulator::new(f, tx).collect::<Vec<_>>()));
            if let Ok(e) = Encoder::new(f, 16) {
                s.audio.push(samples_to_bytes(&Modulator::from_bits(e.bits(4, 2), tx).collect::<Vec<_>>()));
            }
        }
        let mut l = Vec::new();
        for f in short.iter().take(8) {
            l.extend_from_slice(f);
            l.push(0xC0);
        }
        s.live.push(l);
        s
    })
}

/// A seed mutated, or (one time in eight) random bytes.
fn from(r: &mut Rng, corpus: &[Vec<u8>], max: usize) -> Vec<u8> {
    if corpus.is_empty() || r.one_in(8) {
        let n = r.len(max.min(512));
        return r.bytes(n);
    }
    let mut d = r.pick(corpus).clone();
    mutate(r, &mut d, max, corpus);
    d
}

/// The seeds themselves decode cleanly through every target first (a broken seed builder would fuzz nothing).
#[test]
fn fuzz_seeds_are_valid() {
    let s = seeds();
    assert!(s.frames.len() >= 40, "{} frames", s.frames.len());
    for f in &s.frames {
        assert!(Frame::parse(f).is_some());
    }
}

#[test]
fn fuzz_aprs_parse() {
    let s = seeds();
    Fuzz::new("aprs_parse", 0xA9A5, 200_000).run(|r| from(r, &s.aprs, 400), harness::aprs_parse);
}

#[test]
fn fuzz_ax25_frame() {
    let s = seeds();
    let corpus: Vec<Vec<u8>> = s.frames.iter().chain(&s.tnc2).cloned().collect();
    Fuzz::new("ax25_frame", 0xA125, 200_000).run(|r| from(r, &corpus, 400), harness::ax25_frame);
}

#[test]
fn fuzz_kiss() {
    let s = seeds();
    Fuzz::new("kiss", 0x4155, 100_000).run(
        |r| {
            let mut d = from(r, &s.kiss, 1200);
            // commands other than data, and several frames in one stream
            if r.one_in(4) {
                let cmd = [0xC0, r.below(16) as u8 | (r.below(16) as u8) << 4, r.byte(), 0xC0];
                let at = r.below(d.len() + 1);
                d.splice(at..at, cmd);
            }
            d
        },
        harness::kiss,
    );
}

#[test]
fn fuzz_fx25_block() {
    let s = seeds();
    Fuzz::new("fx25_block", 0xF25B, 15_000).run(|r| from(r, &s.fx_block, 256), harness::fx25_block);
}

#[test]
fn fuzz_fx25_bits() {
    let s = seeds();
    Fuzz::new("fx25_bits", 0xF25C, 30_000).run(|r| from(r, &s.fx_bits, 2048), harness::fx25_bits);
}

#[test]
fn fuzz_hdlc_bits() {
    let s = seeds();
    Fuzz::new("hdlc_bits", 0x4D1C, 30_000).run(|r| from(r, &s.hdlc, 8192), harness::hdlc_bits);
}

#[test]
fn fuzz_afsk_audio() {
    let s = seeds();
    Fuzz::new("afsk_audio", 0xAF5C, 1_500).slow_ms(5_000).run(
        |r| {
            let mut d = from(r, &s.audio, 65_536);
            d.truncate(d.len() & !1);
            d
        },
        harness::afsk_audio,
    );
}

#[test]
fn fuzz_aprs_live() {
    let s = seeds();
    Fuzz::new("aprs_live", 0xA11E, 60_000).run(|r| from(r, &s.live, 1200), harness::aprs_live);
}
