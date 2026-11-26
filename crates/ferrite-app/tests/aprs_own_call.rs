//! ferrite#73 (a Zbit radio's safety stops with APRS on; the reporter's earlier freezes came while a second radio
//! sent with the same callsign and SSID): the APRS receive and tick paths fed frames from our own callsign (APRS-36,
//! APRS-53.1), messages, acks, rejects and queries to us (APRS-6, APRS-13, APRS-33, APRS-57, APRS-58), between
//! beacons sent and messages queued, with random digipeater, beacon, path and FX.25 settings. The packet fuzz
//! harness (`fuzz_aprs_live`) almost never reaches these: its seeds are other stations' packets. No panic.
use ferrite_app::aprs::AprsCfg;
use ferrite_app::aprs_live::{AprsLive, Sending, FRAME_MAX};
use ferrite_app::data::{Fix, Pos};
use ferrite_fuzzkit::{hex_lines, mutate, root, testdata, Fuzz, Rng};
use uaprs::ax25::{Address, Frame, Tnc2};

const ME: &[u8] = b"VE7ABC-7";

/// An AX.25 frame (no FCS) from its parts.
fn frame(src: &[u8], dest: &[u8], path: &[&[u8]], info: &[u8]) -> Option<Vec<u8>> {
    let src = Address::from_text(src)?;
    let dest = Address::from_text(dest)?;
    let mut p = Vec::new();
    for x in path {
        p.push(Address::from_text(x)?);
    }
    let f = Frame::new(src, dest, &p, info)?;
    let mut out = vec![0u8; f.len()];
    f.write(&mut out)?;
    Some(out)
}

/// The test packets and data set lines re-sent from our call, its base call and another station, over four paths;
/// then the messages to us.
fn seeds() -> Vec<Vec<u8>> {
    let spec = std::fs::read_to_string(root().join("digimodes/uaprs/tests/spec_packets.txt")).unwrap();
    let mut tnc2: Vec<Vec<u8>> = spec.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')).map(|l| l.as_bytes().to_vec()).collect();
    if let Some(td) = testdata() {
        for f in ["spec.hex", "suite.hex", "wa8lmf.hex"] {
            tnc2.extend(hex_lines(&td.join("aprs").join(f)).into_iter().step_by(3));
        }
    }
    let mut out = Vec::new();
    let paths: [&[&[u8]]; 4] = [&[], &[b"WIDE1-1*"], &[b"VE6RPT*", b"WIDE2-1"], &[b"WIDE1*", b"WIDE2*"]];
    for line in &tnc2 {
        let Some(t) = Tnc2::split(line) else { continue };
        for p in paths {
            for src in [ME, b"VE7ABC".as_slice(), b"VE6ABC-9".as_slice()] {
                if let Some(f) = frame(src, t.dest, p, t.info) {
                    out.push(f);
                }
            }
        }
    }
    // messages, acks, rejects and queries to us; a bulletin, a CQ, a third-party message
    for info in [&b":VE7ABC-7:hello{12"[..], b":VE7ABC-7:ack1", b":VE7ABC-7:rej1", b":VE7ABC-7:?APRSP", b":VE7ABC-7:hi{AB}", b":BLN1     :hi", b":CQ       :hello", b"?APRS?", b"}VE6ZZZ>APRS,TCPIP,VE6ABC*::VE7ABC-7:yo{3", b":VE7ABC  :x{1"] {
        for src in [ME, b"VE7ABC-9".as_slice(), b"VE6ABC-9".as_slice(), b"SMSGTE".as_slice()] {
            for p in paths {
                if let Some(f) = frame(src, b"APRS", p, info) {
                    out.push(f);
                }
            }
        }
    }
    out
}

/// One radio's session: up to 60 frames heard, beacons and messages between them, a tick after each.
fn run(r: &mut Rng, corpus: &[Vec<u8>]) {
    let live = AprsLive::new();
    let mut cfg = AprsCfg::DEFAULT;
    cfg.digi = r.u32() as u8;
    cfg.beacon = r.u32() as u8;
    cfg.path = r.u32() as u8;
    cfg.fx25 = r.u32() as u8;
    let fix = Fix { pos: Pos { lat: 514_252_000, lon: -1_136_901_000 }, three_d: true, alt_m: 1000, speed_kmh: 3, heading: 90, hdop_x10: 10, used: 8, age_s: 0, utc_s: 0, phone: false };
    let mut now = r.u32() % 100_000;
    let mut out = [0u8; FRAME_MAX];
    let sends = [Sending::All, Sending::All, Sending::Beacons, Sending::Nothing];
    for _ in 0..r.range(1, 60) {
        match r.below(8) {
            0 => {
                live.beacon_now();
            }
            1 => {
                live.send(b"VE6ABC-9", b"test");
            }
            2 => live.beacon_sent(now),
            _ => {
                let mut d = r.pick(corpus).clone();
                if r.one_in(3) {
                    mutate(r, &mut d, 400, corpus);
                }
                let rx = live.receive(&d, now, ME, &cfg);
                let _ = (rx.row, rx.inbound, rx.state, rx.echo, rx.notice);
            }
        }
        let t = live.tick(now, *r.pick(&sends), r.one_in(2), ME, if r.one_in(4) { None } else { Some(fix) }, &cfg, &mut out);
        if t.what == "Beacon" {
            live.beacon_sent(now);
        }
        let _ = live.reply_now(now);
        for i in 0..live.stations().min(4) {
            let _ = live.station(i);
        }
        let _ = live.beacon(&cfg);
        for i in 0..live.beacons_sent() {
            let _ = live.sent_beacon(i);
        }
        now = now.wrapping_add(r.range(0, 40) as u32);
    }
}

#[test]
fn aprs_own_call_and_to_us_never_panic() {
    let s = seeds();
    let mut corpus = s.clone();
    corpus.sort();
    Fuzz::new("aprs_own_call", 0x7373, 40_000).run(|r| {
        let mut b = r.u32().to_le_bytes().to_vec();
        b.extend(r.bytes(8));
        b
    }, |seed| {
        let mut r = Rng::new(u64::from_le_bytes(seed[..8].try_into().unwrap()));
        run(&mut r, &corpus);
    });
}
