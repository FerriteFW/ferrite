//! The other decoders the radio runs on what it hears, fuzzed in the normal tests (ferrite-fuzzkit): POCSAG
//! (codewords, and audio through the decoder slot), DSC (bits through the framer, symbols through the message
//! reader, audio), SSTV (audio), SAME headers (the BK4829's FIFO words through the assembler, then the parser) and
//! the DTMF digit tracker. Seeds come from the decoders' own encoders and, when present, the testdata recordings
//! (`../testdata/pocsag`, `../testdata/dsc`). Invariant: no panic, nothing slower than the limit, results inside
//! their buffers.

use ferrite_core::decoders::{Decoder, Found, RATE_HZ};
use ferrite_core::dtmfrx::{Track, MAX as DTMF_MAX};
use ferrite_core::same_rx::{Assembler, Orient, Repeats, Step};
use ferrite_fuzzkit::{bytes_to_samples, mutate, read_wav, samples_to_bytes, testdata, Fuzz, Rng};
use std::sync::OnceLock;

/// A recording at 24 kHz (linear interpolation: enough to give the decoders their kind of signal).
fn resample(s: &[i16], rate: u32) -> Vec<i16> {
    if rate == RATE_HZ || s.is_empty() {
        return s.to_vec();
    }
    let n = (s.len() as u64 * RATE_HZ as u64 / rate as u64) as usize;
    (0..n)
        .map(|i| {
            let x = i as f64 * rate as f64 / RATE_HZ as f64;
            let k = x as usize;
            let f = x - k as f64;
            let a = s[k.min(s.len() - 1)] as f64;
            let b = s[(k + 1).min(s.len() - 1)] as f64;
            (a + (b - a) * f) as i16
        })
        .collect()
}

/// Up to `max` samples of each recording in `dir` (24 kHz, as bytes), from where the signal starts.
fn recordings(dir: &str, max: usize) -> Vec<Vec<u8>> {
    let Some(td) = testdata() else { return vec![] };
    let mut files: Vec<_> = std::fs::read_dir(td.join(dir)).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "wav")).collect();
    files.sort();
    files
        .iter()
        .filter_map(|p| read_wav(p))
        .map(|(s, rate)| {
            let s = resample(&s, rate);
            let start = s.iter().position(|x| x.unsigned_abs() > 2000).unwrap_or(0).saturating_sub(2400);
            samples_to_bytes(&s[start..(start + max).min(s.len())])
        })
        .collect()
}

/// POCSAG codewords as FSK audio at 24 kHz: two levels, as the discriminator gives them.
fn pocsag_audio(baud: u32, words: &[u32]) -> Vec<i16> {
    let mut bits: Vec<bool> = (0..upocsag::encode::PREAMBLE_BITS).map(|i| i % 2 == 0).collect();
    for w in words {
        bits.extend((0..32).rev().map(|k| w >> k & 1 != 0));
    }
    let spb = RATE_HZ as f64 / baud as f64;
    let mut out = vec![0i16; 1200];
    let mut y = 0.0;
    for k in 0..(bits.len() as f64 * spb) as usize {
        let target = if bits[(k as f64 / spb) as usize] { -8000.0 } else { 8000.0 };
        y += (target - y) * 0.3;
        out.push(y as i16);
    }
    out.extend([0i16; 1200]);
    out
}

fn pocsag_words(address: u32, text: &[u8], alpha: bool) -> Vec<u32> {
    let mut w = Vec::new();
    let c = if alpha { upocsag::encode::Content::Alpha(text) } else { upocsag::encode::Content::Numeric(text) };
    upocsag::encode::encode(address, if alpha { 3 } else { 0 }, c, |x| w.push(x));
    w
}

/// A DSC call's bits as sent: dot pattern, phasing, DX and RX copies (as udsc's unit tests build them).
fn dsc_bits(call: &[u8]) -> Vec<u8> {
    let ph = [125u8, 111, 125, 110, 125, 109, 125, 108, 125, 107, 125, 106, 125, 105, 125, 104];
    let mut seq: Vec<u8> = ph[..12].to_vec();
    seq.extend([call[0], ph[13], call[1], ph[15], call[2]]);
    for i in 0..call.len() - 3 {
        seq.push(call[i]);
        seq.push(call[i + 3]);
    }
    seq.push(call[call.len() - 3]);
    let mut b: Vec<u8> = (0..40).map(|i| (i % 2 == 0) as u8).collect();
    for v in seq {
        let w = udsc::framer::encode(v);
        b.extend((0..10).rev().map(|k| ((w >> k) & 1) as u8));
    }
    b.extend([0u8; 40]);
    b
}

/// DSC calls' symbols: from testdata's vectors when present, else a few built here. ECC and closing EOS added.
fn dsc_calls() -> &'static Vec<Vec<u8>> {
    static C: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    C.get_or_init(|| {
        let mut calls: Vec<Vec<u8>> = vec![
            vec![120, 120, 36, 61, 23, 45, 0, 100, 23, 50, 12, 34, 0, 100, 126, 90, 0, 72, 117],
            vec![116, 116, 108, 23, 50, 12, 34, 0, 118, 126, 126, 126, 126, 126, 126, 126, 127],
            vec![112, 112, 23, 50, 12, 34, 0, 101, 49, 12, 34, 56, 78, 12, 0, 109, 126, 127],
        ];
        if let Some(td) = testdata() {
            // "symbols": [ ... ] lists, read without a JSON parser
            let j = std::fs::read_to_string(td.join("dsc/vectors.json")).unwrap_or_default();
            for part in j.split("\"symbols\"").skip(1) {
                let Some(body) = part.split('[').nth(1).and_then(|b| b.split(']').next()) else { continue };
                let v: Vec<u8> = body.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if v.len() > 4 {
                    calls.push(v);
                }
            }
        }
        calls
            .into_iter()
            .map(|c| {
                if c.len() > 4 && c[c.len() - 2] == c[c.len() - 1] {
                    c // already whole (the vectors: ECC and EOS repeats in)
                } else {
                    let n = c.len();
                    let ecc = c[2..].iter().fold(c[0], |x, &v| x ^ v);
                    let mut w = c.clone();
                    w.extend([ecc, c[n - 1], c[n - 1]]);
                    w
                }
            })
            .collect()
    })
}

fn mutated(r: &mut Rng, corpus: &[Vec<u8>], max: usize) -> Vec<u8> {
    if corpus.is_empty() || r.one_in(8) {
        let n = r.len(max.min(4096));
        return r.bytes(n);
    }
    let mut d = r.pick(corpus).clone();
    mutate(r, &mut d, max, corpus);
    d
}

/// Audio through a decoder slot in the DSP task's blocks; every result inside its bounds.
fn through(mut d: Decoder, audio: &[u8]) {
    let s = bytes_to_samples(audio);
    for block in s.chunks(96) {
        d.push(block, |f| match f {
            Found::Frame(f) => assert!(f.len() <= uafsk::hdlc::MAX_FRAME),
            Found::Page(m) => {
                let p = ferrite_core::decoders::Page::from(m);
                let _ = (p.text(), m.numeric().count(), m.alpha().count());
            }
            Found::Call(c) => {
                assert!(c.symbols.len() <= udsc::framer::MAX_SYMBOLS);
                let _ = udsc::message::Message::parse(c.symbols);
            }
            Found::SstvLine(y, _) => assert!(y < 512),
            _ => {}
        });
        let _ = d.busy();
    }
}

#[test]
fn fuzz_pocsag_words() {
    let mut corpus: Vec<Vec<u8>> = Vec::new();
    for (a, t, alpha) in [(1_234_567u32, &b"HELLO WORLD 73"[..], true), (8, b"0123456789*U -[]", false), (2_097_151, b"", true), (77, b"A much longer message that takes several batches to send, more than one", true)] {
        corpus.push(pocsag_words(a, t, alpha).iter().flat_map(|w| w.to_le_bytes()).collect());
    }
    Fuzz::new("pocsag_words", 0x9C5A, 40_000).run(
        |r| {
            let mut d = mutated(r, &corpus, 1024);
            d.truncate(d.len() & !3);
            d
        },
        |data| {
            let mut d: upocsag::decode::Decoder = upocsag::decode::Decoder::new();
            d.sync();
            let check = |m: &upocsag::decode::Message| {
                assert!(m.data().len() * 8 >= m.bits());
                let _ = (m.numeric().count(), m.alpha().count());
            };
            for (i, w) in data.chunks_exact(4).enumerate() {
                if i % 17 == 16 {
                    d.sync();
                }
                d.word(u32::from_le_bytes(w.try_into().unwrap()), check);
            }
            d.end(check);
        },
    );
}

#[test]
fn fuzz_pocsag_audio() {
    let mut corpus = recordings("pocsag", 48_000);
    for baud in [512, 1200, 2400] {
        corpus.push(samples_to_bytes(&pocsag_audio(baud, &pocsag_words(1_234_567, b"Fuzz 73", true))));
    }
    Fuzz::new("pocsag_audio", 0x9C5B, 400).slow_ms(5_000).run(|r| mutated(r, &corpus, 200_000), |a| through(Decoder::pocsag(), a));
}

#[test]
fn fuzz_dsc_framer() {
    let corpus: Vec<Vec<u8>> = dsc_calls().iter().map(|c| dsc_bits(c)).collect();
    Fuzz::new("dsc_framer", 0xD5C1, 20_000).run(
        |r| mutated(r, &corpus, 4096),
        |bits| {
            let mut f = udsc::framer::Framer::new();
            for &b in bits {
                if let Some(c) = f.push(b & 1 != 0) {
                    assert!(c.symbols.len() <= udsc::framer::MAX_SYMBOLS);
                    let _ = udsc::message::Message::parse(c.symbols);
                }
            }
        },
    );
}

#[test]
fn fuzz_dsc_message() {
    let corpus = dsc_calls();
    Fuzz::new("dsc_message", 0xD5C2, 200_000).run(
        |r| {
            let mut d = mutated(r, corpus, 64);
            if r.one_in(3) {
                d.iter_mut().for_each(|s| *s %= 128); // symbols are 0 to 127
            }
            d
        },
        |s| {
            if let Some(m) = udsc::message::Message::parse(s) {
                let _ = format!("{m:?}");
            }
        },
    );
}

#[test]
fn fuzz_dsc_audio() {
    let corpus = recordings("dsc", 48_000);
    if corpus.is_empty() {
        eprintln!("fuzz_dsc_audio: no ../testdata/dsc recordings, random audio only");
    }
    Fuzz::new("dsc_audio", 0xD5C3, 300).slow_ms(5_000).run(|r| mutated(r, &corpus, 120_000), |a| through(Decoder::dsc(), a));
}

#[test]
fn fuzz_sstv_audio() {
    struct Card;
    impl usstv::Picture for Card {
        fn rgb(&mut self, x: usize, y: usize) -> (u8, u8, u8) {
            ((x * 3) as u8, (y * 5) as u8, (x ^ y) as u8)
        }
    }
    // each mode's VIS and its first lines (a whole picture is a minute or two of audio)
    let corpus: Vec<Vec<u8>> = usstv::mode::MODES
        .iter()
        .map(|m| {
            let mut e = usstv::Encoder::new(m, RATE_HZ, 12000);
            let mut s = Vec::new();
            while s.len() < 60_000 {
                let Some(x) = e.next(&mut Card) else { break };
                s.push(x);
            }
            samples_to_bytes(&s)
        })
        .collect();
    Fuzz::new("sstv_audio", 0x557F, 150).slow_ms(5_000).run(|r| mutated(r, &corpus, 160_000), |a| through(Decoder::sstv(), a));
}

const SAME: &[u8] = b"ZCZC-WXR-TOR-039173-039051-139069+0030-1591829-KCLE/NWS-";

/// A header's text after `ZCZC` as the chip's FIFO words in orientation `o` (the inverse of `Orient::bytes`).
fn same_words(text: &[u8], o: Orient) -> Vec<u8> {
    let mut d = text[4..].to_vec();
    if d.len() % 2 == 1 {
        d.push(0);
    }
    let mut out = Vec::new();
    for p in d.chunks(2) {
        let (mut a, mut b) = (p[0], p[1]);
        if o.reversed {
            (a, b) = (a.reverse_bits(), b.reverse_bits());
        }
        let (hi, lo) = if o.low_first { (b, a) } else { (a, b) };
        out.extend(u16::from_be_bytes([hi, lo]).to_le_bytes());
    }
    out
}

#[test]
fn fuzz_same() {
    let headers: [&[u8]; 3] = [SAME, b"ZCZC-EAS-RWT-000000+0015-0011200-WAEB/AM -", b"ZCZC-CIV-CEM-024031-024033-024510-024005+0100-3651430-WBAL/TV -"];
    let mut corpus: Vec<Vec<u8>> = Vec::new();
    for h in headers {
        for o in Orient::ALL {
            let mut v = vec![b'W'];
            v.extend(same_words(h, o));
            corpus.push(v);
        }
        let mut v = vec![b'T'];
        v.extend_from_slice(h);
        corpus.push(v);
    }
    Fuzz::new("same", 0x5A3E, 60_000).run(
        |r| mutated(r, &corpus, 300),
        |d| {
            let Some((&kind, rest)) = d.split_first() else { return };
            let mut rep = Repeats::new();
            if kind == b'T' {
                // the text alone, twice: the second is a repeat
                if let Some(h) = ferrite_core::same::parse(rest) {
                    let _ = format!("{h:?}");
                    assert!(rep.take(rest, 10).is_some());
                    assert!(rep.take(rest, 20).is_none(), "a repeat within 5 minutes");
                }
                return;
            }
            let mut a = Assembler::new();
            a.start();
            for w in rest.chunks_exact(2) {
                match a.push(u16::from_le_bytes([w[0], w[1]])) {
                    Step::More => {}
                    Step::Done => {
                        assert!(a.text().starts_with(b"ZCZC"));
                        if let Some(h) = rep.take(a.text(), 100) {
                            let _ = format!("{h:?}");
                        }
                        a.start();
                    }
                    Step::Bad => a.start(),
                }
            }
        },
    );
}

#[test]
fn fuzz_dtmf_track() {
    Fuzz::new("dtmf_track", 0xD7F0, 20_000).run(
        |r| {
            let n = r.len(200);
            r.bytes(n * 3)
        },
        |d| {
            let mut t = Track::NEW;
            let mut now = 0u32;
            for e in d.chunks_exact(3) {
                now = now.wrapping_add(u16::from_le_bytes([e[1], e[2]]) as u32 * if e[0] & 0x80 != 0 { 97 } else { 1 });
                if e[0] & 0x40 != 0 {
                    let _ = t.tick(now, e[0] & 0x20 != 0);
                } else {
                    t.digit(e[0] & 0x0F, now); // the detector gives 0 to 15
                }
                assert!(t.heard.len() <= DTMF_MAX);
                for i in 0..t.heard.len() {
                    assert!(b"0123456789ABCD*#".contains(&t.heard.char(i)), "{}", t.heard.char(i));
                }
            }
        },
    );
}
