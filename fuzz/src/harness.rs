//! The fuzz targets' bodies, one function each: cargo-fuzz runs them from `fuzz_targets/` (tools/fuzz.sh), and the
//! host tests run the same functions on seeded, mutated inputs in `cargo nextest run`
//! (`crates/ferrite-app/tests/fuzz_packets.rs`, which includes this file). Each panics on a broken invariant.
#![allow(dead_code)]

use ferrite_app::aprs::{AprsCfg, DIGI_ON};
use ferrite_app::aprs_live::{AprsLive, Sending, FRAME_MAX};
use ferrite_core::decoders::Found;
use std::fmt::Write;
use uafsk::hdlc::{ax25_addresses_ok, Deframer, MAX_FRAME};
use uaprs::ax25::{Address, Frame, Tnc2};
use uaprs::kiss::{encode, Command, Params};
use uaprs::parse::{parse, Packet};
use ufx25::Receiver;
use ufx25::{decode, rs::Rs, MODES};

/// uaprs::parse on any information field, with any destination (Mic-E reads its latitude from the destination).
/// Input: one byte for the destination's length (low 3 bits, 0 to 7), the destination, then the information field.
/// Every packet type is reached through the type byte; third-party packets are parsed again, a few levels deep.
pub fn aprs_parse(data: &[u8]) {
    let Some((&n, rest)) = data.split_first() else { return };
    let n = ((n & 7) as usize).min(rest.len());
    let (mut dest, mut info) = rest.split_at(n);
    let mut sink = String::new();
    for _ in 0..4 {
        let Ok(p) = parse(dest, info) else { return };
        let _ = write!(sink, "{p:?}");
        match p {
            Packet::Position { comment, .. } | Packet::Object { comment, .. } | Packet::Item { comment, .. } => {
                let v: Vec<u8> = comment.bytes().collect();
                assert_eq!(v.len(), comment.len());
                assert_eq!(v.is_empty(), comment.is_empty());
                assert!(v.len() <= comment.raw().len());
                if let Packet::Position { pos, .. } = p {
                    let _ = write!(sink, "{:?}", pos);
                }
                return;
            }
            Packet::ThirdParty(t) => {
                dest = t.dest;
                info = t.info;
            }
            _ => return,
        }
    }
}

/// AX.25 frame decode (address field, control and PID, information field), the radio's TNC2 text of it, and the
/// TNC2 splitter and callsign reader. A parsed frame must write back to the same bytes, apart from the H bits of the
/// destination and source (which `Frame::parse` clears) and the reserved bits.
pub fn ax25_frame(data: &[u8]) {
    if let Some(f) = Frame::parse(data) {
        assert!(f.digis().len() <= uaprs::ax25::MAX_DIGIS);
        let mut out = [0u8; 400];
        let mut big = vec![0u8; f.len()];
        let n = f.write(&mut big).expect("room for the frame");
        assert_eq!(n, data.len());
        // control, PID and info as received (address bytes may differ in their reserved and H bits)
        let at = 14 + 7 * f.digis().len();
        assert_eq!(&big[at..], &data[at..], "control, PID and info round trip");
        let _ = f.write(&mut out);
        let mut t = Vec::new();
        f.emit_tnc2(|c| t.push(c));
        let mut s = String::new();
        let _ = write!(s, "{f}");
        assert_eq!(s.as_bytes(), &t[..], "Display and emit_tnc2 agree");
        let _ = Frame::new(f.src, f.dest, f.digis(), f.info);
        for d in f.digis() {
            let _ = d.call();
            let mut e = Vec::new();
            d.emit(&mut |c| e.push(c));
            let back = Address::from_text(&e).expect("a parsed address reads back as text");
            assert!(back.call().eq_ignore_ascii_case(d.call()) && back.ssid == d.ssid);
        }
    }
    if let Some(t) = Tnc2::split(data) {
        let _ = Address::from_text(t.src);
        let _ = Address::from_text(t.dest);
        for p in t.path.split(|&c| c == b',') {
            let _ = Address::from_text(p);
        }
    }
    let _ = Address::from_text(data);
    let _ = Address::from_field(data);
}

/// The KISS decoder (cable and Bluetooth TNC), byte by byte, and the parameter commands it carries; data frames go
/// on to the AX.25 parser as the APRS task's KISS path does.
pub fn kiss(data: &[u8]) {
    let mut d: uaprs::kiss::Decoder = uaprs::kiss::Decoder::new();
    let mut p = Params::default();
    for &b in data {
        if let Some(r) = d.push(b) {
            let _ = r.for_single_port();
            if let Command::Data(f) = p.apply(&r) {
                let _ = Frame::parse(f);
                // a frame encodes and decodes back to itself
                let mut e = Vec::new();
                encode(0, f, |c| e.push(c));
                let mut d2: uaprs::kiss::Decoder = uaprs::kiss::Decoder::new();
                let mut got = None;
                for &c in &e {
                    if let Some(r2) = d2.push(c) {
                        got = Some(r2.data.to_vec());
                    }
                }
                if !f.is_empty() {
                    assert_eq!(got.as_deref(), Some(f));
                }
            }
            let _ = p.flags(1200);
        }
    }
}

/// FX.25 correction and deframing of a received block (ufx25::decode, the APRS task's Fx25Block::decode path):
/// any bytes, any of the eleven codes, any tag error count. Input: one byte picking the code, then the block (padded
/// or cut to the code's length).
pub fn fx25_block(data: &[u8]) {
    let Some((&sel, rest)) = data.split_first() else { return };
    let mode = MODES[sel as usize % MODES.len()];
    let n = mode.n as usize;
    let mut block = [0u8; 255];
    let m = rest.len().min(n);
    block[..m].copy_from_slice(&rest[..m]);
    let mut b = block;
    if let Some(d) = decode(&mut b[..n], mode, sel >> 4) {
        assert!(d.frame.len() + 2 <= mode.k as usize);
        assert!(d.corrected as usize <= mode.check() / 2);
        let _ = uaprs::ax25::Frame::parse(d.frame);
    }
    // the Reed-Solomon decoder on its own, and a correction must leave a valid code word
    let rs = Rs::new(mode.check());
    let mut c = block;
    if rs.decode(&mut c[..n], mode.k as usize).is_ok() {
        let mut again = c;
        assert_eq!(rs.decode(&mut again[..n], mode.k as usize).ok(), Some(0), "a corrected block has no errors left");
    }
}

/// The FX.25 receiver bit by bit (tag hunter, collector, correction, deframing): each input byte gives 8 bits, least
/// significant first. The first byte sets the tag errors accepted (0 to 15).
pub fn fx25_bits(data: &[u8]) {
    let Some((&t, rest)) = data.split_first() else { return };
    let mut rx = Receiver::with_tag_errors(t & 15);
    for &byte in rest {
        for i in 0..8 {
            if let Some(d) = rx.push((byte >> i) & 1) {
                assert!(d.frame.len() <= 255);
                let _ = uaprs::ax25::Frame::parse(d.frame);
            }
            let _ = rx.busy();
        }
    }
    let _ = rx.block();
}

/// The HDLC deframer (flags, bit stuffing, FCS, frame length) bit by bit, plain and with soft-decision repair. Each
/// input byte gives one bit (bit 0) and a confidence (the other 7 bits); the first byte sets the repair depth.
pub fn hdlc_bits(data: &[u8]) {
    let Some((&k, rest)) = data.split_first() else { return };
    let mut plain = Deframer::new();
    let mut soft = Deframer::new();
    soft.set_repair(k & 7);
    for &b in rest {
        let bit = b & 1;
        if let Some(f) = plain.push(bit) {
            assert!(f.len() <= MAX_FRAME);
            let _ = ax25_addresses_ok(f);
            let _ = uaprs::ax25::Frame::parse(f);
        }
        if let Some((f, _fcs)) = soft.push_soft(bit, (b >> 1) as u32 * 1000) {
            assert!(f.len() <= MAX_FRAME);
            let _ = ax25_addresses_ok(f);
            let _ = soft.repaired();
        }
    }
}

/// The radio's AFSK decoder as the DSP task runs it (ferrite_core::decoders::Decoder::afsk at 24 kHz: three
/// slicers, repair, FX.25), on any audio: input bytes are little-endian 16-bit samples, fed in blocks. FX.25 blocks
/// it hands over are corrected as the APRS task does.
pub fn afsk_audio(data: &[u8]) {
    let samples: Vec<i16> = data.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
    let mut d = ferrite_core::decoders::Decoder::afsk();
    for block in samples.chunks(96) {
        d.push(block, |f| match f {
            Found::Frame(f) => {
                assert!(f.len() <= uafsk::hdlc::MAX_FRAME);
                let _ = uaprs::ax25::Frame::parse(f);
            }
            Found::Fx25(b) => {
                let mut b = *b;
                if let Some(fr) = b.decode() {
                    let _ = uaprs::ax25::Frame::parse(fr.frame);
                }
            }
            _ => {}
        });
    }
}

/// The radio's receive path for a decoded frame: AprsLive::receive (the AX.25 parse, digipeating, the heard list,
/// messages and acks, our own echoes) then a tick (acks, digipeats and beacons queued), as the APRS task calls them.
/// Input: frames separated by 0xC0 (as KISS would delimit them); the digipeater and our callsign are on, so every
/// branch can run.
pub fn aprs_live(data: &[u8]) {
    let live = AprsLive::new();
    let mut cfg = AprsCfg::DEFAULT;
    cfg.digi = DIGI_ON | data.first().copied().unwrap_or(0) & 2;
    let me = b"VE7ABC-9";
    let mut now = 1000u32;
    let mut out = [0u8; FRAME_MAX];
    for f in data.split(|&b| b == 0xC0) {
        let rx = live.receive(f, now, me, &cfg);
        let _ = (rx.row, rx.inbound, rx.inbound_id, rx.state);
        let _ = live.tick(now, Sending::All, true, me, None, &cfg, &mut out);
        let _ = ferrite_app::aprs::wx(f);
        for i in 0..live.stations().min(4) {
            let _ = live.station(i);
        }
        now += 7;
    }
}
