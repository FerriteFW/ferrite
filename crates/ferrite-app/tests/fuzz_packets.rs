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
use ferrite_app::aprs_live::{AprsLive, Sending, FRAME_MAX};
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

/// ACARS-2.1: any bytes through the ACARS parser and log never panic; seeds are the sample traffic's frames, alone
/// and in a row.
#[test]
fn fuzz_acars() {
    let mut corpus = Vec::new();
    let mut all = Vec::new();
    for (_, f, _) in ferrite_app::acars::sample::FRAMES.iter() {
        let mut b = [0u8; 256];
        let n = ferrite_core::acars::encode(f, &mut b);
        corpus.push(b[..n].to_vec());
        all.push(ferrite_core::acars::SOH);
        all.extend_from_slice(&b[..n]);
    }
    corpus.push(all);
    Fuzz::new("acars", 0xACA5, 40_000).run(|r| from(r, &corpus, 1200), harness::acars);
}

// ---- Liveness (ferrite#73: a freeze after APRS traffic): a decoder that has eaten garbage is back to idle for the
// next good frame. Each input is two bytes picking the good frame, then the garbage (a seed mutated, or random).

/// Garbage of up to `max` bytes after the two bytes that pick the frame.
fn garbage(r: &mut Rng, corpus: &[Vec<u8>], max: usize) -> Vec<u8> {
    let mut v = vec![r.byte(), r.byte()];
    v.extend(from(r, corpus, max));
    v
}

/// The good frame an input picks, and its garbage.
fn split(d: &[u8]) -> Option<(&'static [u8], &[u8])> {
    let s = seeds();
    let (pick, g) = (d.get(..2)?, &d[2..]);
    Some((&s.frames[u16::from_le_bytes([pick[0], pick[1]]) as usize % s.frames.len()], g))
}

/// APRS-3, APRS-56: the HDLC deframer (plain and with soft-decision repair) finds a good frame whatever bits came
/// before its flags: no state (a long frame, an abort, stuffing) outlives a flag.
#[test]
fn fuzz_hdlc_recovers() {
    let s = seeds();
    Fuzz::new("hdlc_recovers", 0x4D1D, 4_000).run(
        |r| garbage(r, &s.hdlc, 8192),
        |d| {
            let Some((f, g)) = split(d) else { return };
            let mut plain = uafsk::hdlc::Deframer::new();
            let mut soft = uafsk::hdlc::Deframer::new();
            soft.set_repair(g.first().map_or(0, |k| k & 7));
            for &b in g {
                let _ = plain.push(b & 1);
                let _ = soft.push_soft(b & 1, (b >> 1) as u32 * 1000);
            }
            let (mut a, mut b) = (false, false);
            for bit in uafsk::hdlc::Framer::new(f, 2, 2) {
                a |= plain.push(bit).is_some_and(|x| x == f);
                b |= soft.push_soft(bit, 100_000).is_some_and(|(x, _)| x == f);
            }
            assert!(a, "the plain deframer missed a good frame after garbage");
            assert!(b, "the soft deframer missed a good frame after garbage");
        },
    );
}

/// KISS-4, KISS-6: the KISS decoder hands over a good frame after any bytes (cut escapes, overlong frames, stray
/// FENDs): the FEND before it always starts afresh.
#[test]
fn fuzz_kiss_recovers() {
    let s = seeds();
    Fuzz::new("kiss_recovers", 0x4156, 20_000).run(
        |r| garbage(r, &s.kiss, 1500),
        |d| {
            let Some((f, g)) = split(d) else { return };
            let mut k: uaprs::kiss::Decoder = uaprs::kiss::Decoder::new();
            for &b in g {
                let _ = k.push(b);
            }
            let mut wire = vec![uaprs::kiss::FEND];
            uaprs::kiss::encode(0, f, |b| wire.push(b));
            let mut got = false;
            for b in wire {
                if let Some(r) = k.push(b) {
                    got |= r.command == 0 && r.data == f;
                }
            }
            assert!(got, "a good KISS frame lost after garbage");
        },
    );
}

/// APRS-44: the FX.25 receiver finds a good block after any bits (false tags, blocks cut short) within one block's
/// time: a tag (a false one in noise too) blinds the tag hunt for its block's bits (up to 255 bytes, 1.7 s; plain
/// AX.25 decodes meanwhile), then it hunts again. So the good block comes after that many bits of flags.
#[test]
fn fuzz_fx25_recovers() {
    let s = seeds();
    Fuzz::new("fx25_recovers", 0xF25D, 1_500).run(
        |r| garbage(r, &s.fx_bits, 4096),
        |d| {
            let Some((f, g)) = split(d) else { return };
            let check = [16, 32, 64][g.len() % 3];
            let Ok(e) = Encoder::new(f, check) else { return };
            let mut rx = ufx25::Receiver::with_tag_errors(g.first().map_or(0, |t| t & 15));
            for &byte in g {
                for i in 0..8 {
                    let _ = rx.push((byte >> i) & 1);
                }
            }
            let mut got = false;
            for bit in e.bits(2 + 255, 2) {
                got |= rx.push(bit).is_some_and(|d| d.frame == f);
            }
            assert!(got, "a good FX.25 block lost after garbage");
        },
    );
}

/// APRS-3, DEC-18: the radio's AFSK decoder (the DSP task's slot: three slicers, repair, FX.25) decodes a good
/// frame after up to a second of any audio (noise, clipping, DC, other packets cut short), with a usual TX delay
/// (30 flags, 200 ms) before it.
#[test]
fn fuzz_afsk_recovers() {
    let s = seeds();
    let tx = TxConfig { rate_hz: ferrite_core::decoders::RATE_HZ, preamble: 30, postamble: 2, amp: 12000 };
    Fuzz::new("afsk_recovers", 0xAF5D, 250).slow_ms(5_000).run(
        |r| {
            // pieces of audio up to a second in all: packets mutated (cut short, bits flipped), noise at any level,
            // a clipped tone, DC, silence
            let mut a: Vec<i16> = Vec::new();
            while a.len() < 24_000 && !r.one_in(6) {
                let n = r.range(1, 8000);
                match r.below(5) {
                    0 => {
                        let mut p = r.pick(&s.audio).clone();
                        mutate(r, &mut p, 16_000, &s.audio);
                        a.extend(ferrite_fuzzkit::bytes_to_samples(&p));
                    }
                    1 => {
                        let amp = r.range(1, 32768) as i32;
                        a.extend((0..n).map(|_| ((r.u16() as i16 as i32 * amp) >> 15) as i16));
                    }
                    2 => {
                        let half = r.range(2, 40);
                        a.extend((0..n).map(|i| if (i / half).is_multiple_of(2) { i16::MAX } else { i16::MIN }));
                    }
                    3 => a.extend(std::iter::repeat_n(r.u16() as i16, n)),
                    _ => a.extend(std::iter::repeat_n(0, n)),
                }
            }
            let mut d = vec![r.byte(), r.byte()];
            d.extend(samples_to_bytes(&a));
            d
        },
        |d| {
            let Some((_, g)) = split(d) else { return };
            // the short frames keep the cases quick: the garbage is what is tested
            let short: Vec<&Vec<u8>> = s.frames.iter().filter(|f| f.len() <= 80).collect();
            let f = &short[(d[0] as usize | (d[1] as usize) << 8) % short.len()][..];
            let mut dec = ferrite_core::decoders::Decoder::afsk();
            let mut audio = ferrite_fuzzkit::bytes_to_samples(g);
            audio.extend(Modulator::new(f, tx));
            audio.extend(std::iter::repeat_n(0i16, 2400));
            let at = g.len() / 2;
            let mut got = false;
            for (i, block) in audio.chunks(96).enumerate() {
                dec.push(block, |x| {
                    if let ferrite_core::decoders::Found::Frame(x) = x {
                        got |= i * 96 + 96 > at && x == f;
                    }
                });
            }
            assert!(got, "a good AFSK frame lost after {} samples of garbage", at);
        },
    );
}

// ---- Timing-driven state (ferrite#73): the APRS and ACARS logs with the clock fuzzed too. A turn can come late by
// any amount (a long transmission, an SSTV picture). ACARS's clock is the App's millisecond count (u32, wrapping
// every 49.7 days) over 1000; APRS's is the seconds since power-on (embassy's Instant: it was the wrapping
// millisecond count over 1000 too, and fuzz_aprs_clock found what that did at the wrap).

/// ACARS's seconds: the App's millisecond count over 1000 (App::acars_heard, App::acars_tick).
fn secs(ms: u32) -> u32 {
    ms / 1000
}

/// APRS's seconds (firmware/src/tasks/aprs.rs): since power-on, from a 64-bit count, so they never go back.
fn secs64(ms: u64) -> u32 {
    (ms / 1000) as u32
}

/// One clock step from a fuzz byte pair: a usual turn, a late one, hours, days, or the next second.
fn step_ms(op: u8, v: u8) -> u32 {
    match op % 6 {
        0 => 1000,
        1 => 1 + v as u32 * 37,
        2 => v as u32 * 1000,
        3 => v as u32 * 60_000,
        4 => v as u32 * 3_600_000,
        _ => 0,
    }
}

const ME: &[u8] = b"VE7ABC-9";

/// APRS-6, APRS-13, APRS-13.6, APRS-59, APRS-3: AprsLive (receive, our messages, acks, digipeats, beacons, the
/// Traffic ring) under any mix of frames, sends and turns with the clock jumping (late turns, hours, days). With
/// the old clock (the millisecond count over 1000, back to 0 after 49.7 days) a message being tried at the wrap
/// stayed in its slot for another 49.7 days (fixed in the firmware: its seconds now come from a 64-bit count). Its lists stay within their sizes throughout; afterwards, with the clock running
/// normally: a message to us is acked at once, every message of ours ends (acked or failed) within three hours
/// so all four slots are free, and a new one goes out at the next turn. Nothing stays stuck.
#[test]
fn fuzz_aprs_clock() {
    let s = seeds();
    let to_me: Vec<Vec<u8>> = [&b"VE7XYZ>APRS,WIDE1-1::VE7ABC-9 :hello there{12"[..], b"VE7XYZ>APRS::VE7ABC-9 :ack7", b"VE7XYZ>APRS::VE7ABC-9 :rej8", b"N0CALL>APRS::VE7ABC   :to the base{A1}"]
        .iter()
        .filter_map(|l| frame(l).map(|f| f.0))
        .collect();
    // not among the frames above: the liveness check's own, never heard before
    let fresh = frame(b"VE7QRP>APRS::VE7ABC-9 :are you there{99").unwrap().0;
    Fuzz::new("aprs_clock", 0xA11F, 1_500).run(
        |r| {
            let n = r.len(120);
            r.bytes(4 + n * 3)
        },
        |d| {
            let Some((start, ev)) = d.split_first_chunk::<4>() else { return };
            let live = AprsLive::new();
            let mut cfg = ferrite_app::aprs::AprsCfg::DEFAULT;
            cfg.digi = ferrite_app::aprs::DIGI_ON;
            let mut ms = u32::from_le_bytes(*start) as u64;
            let mut out = [0u8; FRAME_MAX];
            let sending = [Sending::All, Sending::All, Sending::Beacons, Sending::Nothing];
            for e in ev.as_chunks::<3>().0 {
                ms += step_ms(e[0] >> 5, e[1]) as u64;
                let now = secs64(ms);
                match e[0] & 7 {
                    0 | 1 => {
                        let mut f = if e[2] & 1 == 0 { s.frames[e[1] as usize % s.frames.len()].clone() } else { to_me[e[1] as usize % to_me.len()].clone() };
                        if e[2] & 6 == 6 {
                            let i = e[2] as usize % f.len();
                            f[i] ^= e[1] | 1;
                        }
                        let _ = live.receive(&f, now, ME, &cfg);
                    }
                    2 => {
                        let _ = live.send(b"VE7XYZ", if e[2] & 1 == 0 { &b"ping"[..] } else { b"bad|text" });
                    }
                    3 => live.beacon_now(),
                    4 => live.after_voice(),
                    5 => live.beacon_sent(now),
                    6 if e[2] == 0 => live.clear_out(),
                    _ => {
                        let _ = live.tick(now, sending[e[2] as usize & 3], e[2] & 4 == 0, if e[2] & 8 == 0 { ME } else { b"" }, None, &cfg, &mut out);
                    }
                }
                assert!(live.stations() <= ferrite_app::aprs::HEARD_MAX);
                assert!(live.notices() as usize <= ferrite_app::aprs_live::NOTICES);
                assert!(live.beacons_sent() as usize <= ferrite_app::aprs_live::SENT_MAX);
                let t = live.traffic();
                assert!(t.used() as usize <= ferrite_app::aprs_traffic::TRAFFIC_BYTES);
                for i in 0..t.len() {
                    assert!(t.get(i).is_some(), "Traffic entry {i} of {} missing", t.len());
                }
            }
            // The clock runs normally from here: a turn a second (every 30 s while nothing is due).
            ms += 1000;
            let rx = live.receive(&fresh, secs64(ms), ME, &cfg);
            assert!(rx.inbound.is_some(), "a message to us not taken");
            let mut acked = false;
            for _ in 0..6 {
                let t = live.tick(secs64(ms), Sending::All, true, ME, None, &cfg, &mut out);
                acked |= t.what == "Ack" && out[..t.len].windows(5).any(|w| w == b"ack99");
                ms += 1000;
            }
            assert!(acked, "a message to us not acked");
            for _ in 0..3 * 120 {
                let _ = live.tick(secs64(ms), Sending::All, true, ME, None, &cfg, &mut out);
                ms += 30_000;
            }
            for i in 0..4 {
                assert!(live.send(b"VE7XYZ", b"still here"), "our message slot {i} never freed: a message stuck being tried");
            }
            live.clear_out();
            assert!(live.send(b"VE7XYZ", b"after"));
            let mut sent = false;
            for _ in 0..3 {
                let t = live.tick(secs64(ms), Sending::All, true, ME, None, &cfg, &mut out);
                sent |= t.what == "Message";
                ms += 1000;
            }
            assert!(sent, "a new message not sent");
        },
    );
}

/// ACARS-2.1, ACARS-2.3, ACARS-2.4: the ACARS log under any frames (good, damaged, blocks of two-block messages)
/// with the clock jumping (late turns, hours, the wrap). The log stays within KEEP and its pool; a block heard
/// twice in the same second is kept once; afterwards a message's first block waiting for its second is kept on
/// its own within JOIN_S, and a single block is listed at once.
#[test]
fn fuzz_acars_clock() {
    use ferrite_app::acars::{AcarsLive, KEEP};
    use ferrite_core::acars::{encode, Fields, JOIN_MAX, JOIN_S, NAK};
    let enc = |f: &Fields| {
        let mut b = [0u8; 256];
        let n = encode(f, &mut b);
        b[..n].to_vec()
    };
    let base = Fields { mode: b'2', reg: b".C-FTJO", ack: NAK, label: *b"H1", block: b'1', msgno: b"M01A", flight: b"AC0123", text: b"FIRST BLOCK", more: true };
    let mut corpus: Vec<Vec<u8>> = ferrite_app::acars::sample::FRAMES.iter().map(|(_, f, _)| enc(f)).collect();
    corpus.push(enc(&base));
    corpus.push(enc(&Fields { msgno: b"M01B", block: b'2', text: b"SECOND", more: false, ..base }));
    Fuzz::new("acars_clock", 0xACA7, 3_000).run(
        |r| {
            let n = r.len(80);
            r.bytes(4 + n * 3)
        },
        |d| {
            let Some((start, ev)) = d.split_first_chunk::<4>() else { return };
            let live = AcarsLive::new();
            let mut ms = u32::from_le_bytes(*start);
            let mut text = [0u8; JOIN_MAX];
            for e in ev.as_chunks::<3>().0 {
                ms = ms.wrapping_add(step_ms(e[0] >> 5, e[1]));
                let now = secs(ms);
                let mut f = corpus[e[1] as usize % corpus.len()].clone();
                match e[0] & 3 {
                    0 => live.tick(now),
                    1 => {
                        let i = e[2] as usize % f.len();
                        f[i] ^= e[1] | 1;
                        let _ = live.frame(&f, e[2] % 7, now);
                    }
                    _ => {
                        let crc_ok = ferrite_core::acars::parse(&f).is_ok_and(|p| p.crc_ok);
                        let _ = live.frame(&f, e[2] % 7, now);
                        let (len, dups) = (live.len(), live.counts().2);
                        if crc_ok && e[0] & 4 != 0 {
                            let _ = live.frame(&f, e[2] % 7, now);
                            assert_eq!((live.len(), live.counts().2), (len, dups + 1), "a block heard twice in one second kept twice");
                        }
                    }
                }
                assert!(live.len() as usize <= KEEP);
                for i in 0..live.len() {
                    let (m, n) = live.get(i, &mut text).expect("every message counted is there");
                    assert!(n <= JOIN_MAX && m.blocks >= 1);
                }
            }
            // The clock runs normally: a first block alone is listed within JOIN_S; a single block at once.
            ms = ms.wrapping_add(1000);
            let first = enc(&Fields { flight: b"WS0099", msgno: b"M77A", ..base });
            live.frame(&first, 0, secs(ms)).expect("a good block");
            for _ in 0..=JOIN_S {
                ms = ms.wrapping_add(1000);
                live.tick(secs(ms));
            }
            let (m, _) = live.get(0, &mut text).expect("the waiting message listed");
            assert_eq!(&m.flight, b"WS0099", "a first block still waiting after JOIN_S");
            let one = enc(&Fields { flight: b"AC0777", msgno: b"M88A", more: false, ..base });
            live.frame(&one, 0, secs(ms)).expect("a good block");
            assert_eq!(&live.get(0, &mut text).unwrap().0.flight, b"AC0777", "a single block not listed at once");
        },
    );
}

/// APRS-10, APRS-29: a frame waiting for a clear moment (p-persistence on C's squelch) is sent or given up within
/// CLEAR_WAIT_MS of turns, whatever the clock did while it waited (late turns, the millisecond count's wrap) and
/// however busy the channel: never a wait that doesn't end.
#[test]
fn fuzz_clear_wait_ends() {
    use ferrite_app::aprs_live::{Clear, ClearWait, CLEAR_WAIT_MS};
    Fuzz::new("clear_wait_ends", 0xC1EA, 20_000).run(
        |r| {
            let n = r.len(60);
            r.bytes(4 + n * 3)
        },
        |d| {
            let Some((start, ev)) = d.split_first_chunk::<4>() else { return };
            let mut ms = u32::from_le_bytes(*start);
            let mut w = ClearWait::default();
            for e in ev.as_chunks::<3>().0 {
                ms = ms.wrapping_add(step_ms(e[0] >> 5, e[1]));
                if e[0] & 1 == 0 {
                    w.start(ms);
                }
                let _ = w.poll(ms, e[0] & 2 != 0, e[2]);
            }
            w.start(ms);
            // the APRS task's turns: every 10 to 100 ms, the channel busy (the worst case) or not, any random byte
            let mut waited = 0u32;
            let mut i = 0usize;
            loop {
                let busy = d.get(i % d.len()).is_none_or(|b| b & 1 == 1);
                let r = d.get((i * 7) % d.len()).copied().unwrap_or(255);
                match w.poll(ms, busy, r) {
                    Clear::Wait => {}
                    Clear::Send | Clear::GiveUp => break,
                }
                let dt = 10 + (i as u32 * 13) % 91;
                ms = ms.wrapping_add(dt);
                waited += dt;
                i += 1;
                assert!(waited <= CLEAR_WAIT_MS + 100, "a frame still waiting after {waited} ms");
            }
            assert!(!w.waiting());
        },
    );
}
