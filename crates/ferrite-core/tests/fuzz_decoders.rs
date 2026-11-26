//! The other decoders the radio runs on what it hears, fuzzed in the normal tests (ferrite-fuzzkit): POCSAG
//! (codewords, and audio through the decoder slot), DSC (bits through the framer, symbols through the message
//! reader, audio), SSTV (audio), SAME headers (the BK4829's FIFO words through the assembler, then the parser) and
//! the DTMF digit tracker. Seeds come from the decoders' own encoders and, when present, the test data's
//! recordings (`pocsag/`, `dsc/`, found by `testdata_dir`'s rule). Invariant: no panic, nothing slower than the
//! limit, results inside their buffers.

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
    let mut b: Vec<u8> = (0..40u8).map(|i| i.is_multiple_of(2) as u8).collect();
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
            for (i, w) in data.as_chunks::<4>().0.iter().enumerate() {
                if i % 17 == 16 {
                    d.sync();
                }
                d.word(u32::from_le_bytes(*w), check);
            }
            d.end(check);
        },
    );
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
        eprintln!("fuzz_dsc_audio: no dsc recordings in the test data, random audio only");
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
            for w in rest.as_chunks::<2>().0 {
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
            for e in d.as_chunks::<3>().0 {
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

// ---- Liveness and bounds (ferrite#73: a freeze after traffic): each decoder that has eaten garbage is back to idle,
// within a stated bound, for the next good signal; nothing it holds grows past its buffer.

/// DEC-4, DEC-23: the pager receiver (the BK4829 modem's FIFO words into pages) gives a good page after any
/// mix of syncs, words and ends: the modem's next sync starts afresh, whatever the last transmission left (half a
/// codeword, a batch cut short, a frame run past its end).
#[test]
fn fuzz_pager_recovers() {
    use ferrite_core::pagerrx::PagerRx;
    let page = pocsag_words(1_234_567, b"FERRITE 73", true);
    let first = page.iter().position(|&w| w == upocsag::SYNC).expect("a sync codeword");
    let words: Vec<u16> = page[first + 1..].iter().flat_map(|&cw| [((cw >> 16) as u16).swap_bytes(), (cw as u16).swap_bytes()]).collect();
    let corpus: Vec<Vec<u8>> = vec![words.iter().flat_map(|w| w.to_le_bytes()).collect()];
    Fuzz::new("pager_recovers", 0x9C5B, 20_000).run(
        |r| mutated(r, &corpus, 1200),
        |d| {
            let Some((&baud, rest)) = d.split_first() else { return };
            let mut rx = PagerRx::new(ferrite_core::pagerrx::BAUDS[baud as usize & 1]);
            // the garbage: each 3 bytes an event (a word, now and then a sync or an end)
            for e in rest.as_chunks::<3>().0 {
                match e[0] & 0x1F {
                    0 => rx.sync(|_| {}),
                    1 => rx.end(|_| {}),
                    _ => rx.word(u16::from_le_bytes([e[1], e[2]]), |p| assert!(p.len as usize <= p.text.len())),
                }
            }
            let mut got = Vec::new();
            rx.sync(|_| {});
            for &w in &words {
                rx.word(w, |p| got.push(p));
            }
            rx.end(|p| got.push(p));
            assert!(
                got.iter().any(|p| p.capcode == 1_234_567 && &p.text[..p.len as usize] == b"FERRITE 73"),
                "a good page lost after garbage ({} pages)",
                got.len()
            );
        },
    );
}

/// DEC-5: the DSC framer finds a good call after any bits, within a bound: a false lock (or a call cut short)
/// holds it at most 2 x (MAX_SYMBOLS + 8) words (960 bits, 8 s at 1200 baud), so the call comes after that many
/// bits of dot pattern. The call's symbols come out as they do with no garbage before.
#[test]
fn fuzz_dsc_recovers() {
    let corpus: Vec<Vec<u8>> = dsc_calls().iter().map(|c| dsc_bits(c)).collect();
    let calls = dsc_calls();
    Fuzz::new("dsc_recovers", 0xD5C4, 3_000).run(
        |r| {
            let mut v = vec![r.byte()];
            v.extend(mutated(r, &corpus, 4096));
            v
        },
        |d| {
            let Some((&pick, g)) = d.split_first() else { return };
            let call = &calls[pick as usize % calls.len()];
            let bits = dsc_bits(call);
            let clean: Vec<Vec<u8>> = {
                let mut f = udsc::framer::Framer::new();
                bits.iter().filter_map(|&b| f.push(b != 0).map(|c| c.symbols.to_vec())).collect()
            };
            let mut f = udsc::framer::Framer::new();
            for &b in g {
                if let Some(c) = f.push(b & 1 != 0) {
                    assert!(c.symbols.len() <= udsc::framer::MAX_SYMBOLS);
                }
            }
            let mut got = Vec::new();
            let dots = (0..2 * (udsc::framer::MAX_SYMBOLS + 8) * 10).map(|i| i % 2 == 0);
            for b in dots.chain(bits.iter().map(|&b| b != 0)) {
                if let Some(c) = f.push(b) {
                    got.push(c.symbols.to_vec());
                }
            }
            for c in &clean {
                assert!(got.contains(c), "a good DSC call lost after garbage");
            }
        },
    );
}

/// DEC-19: the SAME assembler, fed FIFO words with no sync between (a missed sync interrupt, words after a bad
/// burst), ends each burst within its buffer (Done or Bad before 4 + MAX_LEN bytes) and never panics; after the
/// next sync a good header comes out whole.
#[test]
fn fuzz_same_recovers() {
    let mut corpus: Vec<Vec<u8>> = Vec::new();
    for o in Orient::ALL {
        corpus.push(same_words(SAME, o));
    }
    Fuzz::new("same_recovers", 0x5A3F, 30_000).run(
        |r| {
            let mut v = vec![r.byte()];
            v.extend(mutated(r, &corpus, 1200));
            v
        },
        |d| {
            let Some((&o, g)) = d.split_first() else { return };
            let mut a = Assembler::new();
            let mut since = 0usize;
            for w in g.as_chunks::<2>().0 {
                // no start(): the words keep coming as the radio's FIFO could hand them
                since += 1;
                match a.push(u16::from_le_bytes([w[0], w[1]])) {
                    Step::More => assert!(since <= ferrite_core::same_rx::MAX_LEN / 2 + 8, "a burst that never ends"),
                    Step::Done | Step::Bad => since = 0,
                }
                assert!(a.text().len() <= 4 + ferrite_core::same_rx::MAX_LEN);
            }
            let o = Orient::ALL[o as usize % 4];
            a.start();
            let mut done = false;
            for w in same_words(SAME, o).as_chunks::<2>().0 {
                if a.push(u16::from_le_bytes([w[0], w[1]])) == Step::Done {
                    done = true;
                    break;
                }
            }
            assert!(done && a.text() == SAME, "a good header lost after garbage");
        },
    );
}

/// DEC-6, DEC-6.2: the DTMF digits clear HOLD_MS after the last digit once the carrier is gone, whatever the
/// clock did before (jumps either way, the 49.7-day wrap of the millisecond count, zero): the string can't stick.
#[test]
fn fuzz_dtmf_clears() {
    Fuzz::new("dtmf_clears", 0xD7F1, 20_000).run(
        |r| {
            let n = r.len(100);
            r.bytes(n * 5)
        },
        |d| {
            let mut t = Track::NEW;
            let mut now = 0u32;
            let mut last = None;
            for e in d.as_chunks::<5>().0 {
                let v = u32::from_le_bytes([e[1], e[2], e[3], e[4]]);
                now = match e[0] >> 6 {
                    0 => now.wrapping_add(v % 2000),
                    1 => now.wrapping_sub(v % 100_000),
                    2 => v,
                    _ => now,
                };
                if e[0] & 0x20 != 0 {
                    t.digit(e[0] & 0x0F, now);
                    last = Some(now);
                } else {
                    let _ = t.tick(now, e[0] & 0x10 != 0);
                }
            }
            let Some(at) = last else { return };
            let _ = t.tick(at.wrapping_add(ferrite_core::dtmfrx::HOLD_MS), false);
            assert!(t.heard.is_empty(), "the digits stuck after the carrier went");
        },
    );
}

/// TSRCH-3: tone search finds a tone from two agreeing reads with the carrier, whatever it read before (noise,
/// carrier flapping, other tones): no state keeps a new tone out.
#[test]
fn fuzz_tone_search_recovers() {
    use ferrite_core::tonesearch::Search;
    Fuzz::new("tone_search_recovers", 0x7053, 20_000).run(
        |r| {
            let n = r.len(64);
            r.bytes(n * 3)
        },
        |d| {
            let mut s = Search::new();
            let mut tone = 0u16;
            for e in d.as_chunks::<3>().0 {
                let w = u16::from_le_bytes([e[1], e[2]]);
                let word = (e[0] & 1 != 0).then_some(w);
                let _ = s.step(e[0] & 2 != 0, word);
                let _ = s.average(e[0] & 4 != 0, word);
                tone = w | 1; // never 0
            }
            s.step(true, Some(tone));
            s.step(true, Some(tone));
            assert_eq!(s.found, tone, "tone search stuck on an old reading");
        },
    );
}

/// GPS-2, GPS-18: the firmware's NMEA reader (ferrite_core::gps, the GPS task's; drivers/nmea is the older
/// crate) on any bytes: sentences cut short, overlong, no `$`, bad checksums, odd fields, binary. Its tables stay in
/// bounds, and the next good GGA and RMC still give the fix: no state outlives a `$`.
#[test]
fn fuzz_gps_recovers() {
    use ferrite_core::gps::Nmea;
    const LINES: [&str; 6] = [
        "GNGGA,183412.00,4900.60000,N,12305.40000,W,1,09,0.9,12.3,M,-17.0,M,,",
        "GNRMC,183413.50,A,4900.60000,N,12305.40000,W,23.4,271.5,290926,,,A",
        "GPGSV,3,1,12,01,40,083,46,02,17,308,41,12,07,344,39,14,22,228,45",
        "BDGSV,1,1,04,01,45,120,33,03,50,200,,06,10,330,18,08,60,010,40",
        "GNGSA,A,3,01,02,12,14,,,,,,,,,1.5,0.9,1.2,1",
        "GPRMC,183414.00,V,,,,,,,290926,,,N",
    ];
    let line = |body: &str| {
        let sum = body.bytes().fold(0u8, |a, b| a ^ b);
        format!("${body}*{sum:02X}\r\n").into_bytes()
    };
    let corpus: Vec<Vec<u8>> = vec![LINES.iter().flat_map(|l| line(l)).collect()];
    let good: Vec<u8> = [LINES[0], LINES[1]].iter().flat_map(|l| line(l)).collect();
    Fuzz::new("gps_recovers", 0x6950, 20_000).run(
        |r| mutated(r, &corpus, 2000),
        |d| {
            let mut n = Nmea::new();
            for &c in d {
                n.feed(c);
                assert!(n.nsat as usize <= n.sat.len());
            }
            let _ = (n.fix(1000), n.sky());
            let before = n.sentences;
            for &c in &good {
                n.feed(c);
            }
            assert_eq!(n.sentences.wrapping_sub(before), 2, "a good sentence lost after garbage");
            assert!(n.has_fix() && n.lat_e7 == 490_100_000 && n.lon_e7 == -1_230_900_000, "no fix from good sentences after garbage");
        },
    );
}
