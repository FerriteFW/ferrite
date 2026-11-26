//! The link protocol fuzzed (ferrite-fuzzkit): Bluetooth and the cable are untrusted, so every request type with
//! any payload, through the real session against the sample store. Payloads are built the way a PC builds them
//! (records read from the radio itself, split into `WRITE` parts or packed into `WRITE_MANY`, indexes, offsets and
//! lengths at their edges) and then mutated; the wire bytes are sometimes damaged too (COBS, CRC, delimiters).
//!
//! Invariants: no panic, no case slower than the limit; a refused `WRITE` (or a `WRITE_MANY` refused before its
//! first record) changes nothing the radio keeps (its dump, TERM-13); a record the radio took reads back as it
//! keeps it, and that copy written again reads back the same (the radio's own checks settle in one pass).

use super::*;
use ferrite_fuzzkit::{mutate, Fuzz, Rng};
use std::sync::OnceLock;
use std::vec::Vec;

const TYPES: [u8; 39] = [
    msg::HELLO, msg::INFO, msg::AUTH, msg::READ, msg::WRITE, msg::DELETE, msg::BEGIN, msg::END, msg::USED, msg::READ_MANY,
    msg::WRITE_MANY, msg::STREAM, msg::STREAM_MORE, msg::LOG_READ, msg::SLOT, msg::BAUD, msg::REBOOT, msg::CMD, msg::MORE,
    msg::KEY, msg::UI, msg::STATE, msg::TUNE, msg::BEACON, msg::PHONE_POS, msg::SUBSCRIBE, msg::EVENT, msg::HEARD,
    msg::MSGS, msg::MSG_SEND, msg::MSG_STOP, msg::MSG_DELETE, msg::AUDIO, msg::STOCK_RESTORE, msg::DATA_READ,
    msg::DATA_WRITE, msg::CLOCK, msg::SSB_WRITE, 0x7F,
];

/// Words a `CMD` (LINK-34) may carry.
const WORDS: &[&str] = &["set", "get", "ch", "zone", "preset", "home", "quick", "slot", "ui", "key", "knob", "xb", "help", "dump", "gps", "load", "sat", "clear", "0", "1", "2", "999", "145.495", "446.475", "name=X", "tx=146.52", "offset=split", "delete", "off", "on", "keys", "status", "radio"];

/// What a request's answers were: (type, seq, payload).
type Answers = Vec<(u8, u8, Vec<u8>)>;

impl Pc {
    /// Bytes on the wire, then polls until the session is ready again (or gives up): every frame that came back.
    fn exchange(&mut self, wire_bytes: &[u8]) -> Answers {
        let mut out = Wire(Vec::new());
        for &c in wire_bytes {
            let mut tries = 0;
            while !self.link.ready() && tries < 16 {
                self.step(&mut out);
                tries += 1;
            }
            if !self.link.ready() {
                break; // a frame the session holds and never takes is a hang: the time limit says so
            }
            self.link.feed(c, self.now);
        }
        for _ in 0..8 {
            self.step(&mut out);
        }
        let mut answers = Vec::new();
        for f in out.0.split_mut(|&b| b == 0) {
            if f.is_empty() {
                continue;
            }
            if let Ok(fr) = wire::parse(f) {
                answers.push((fr.ty, fr.seq, fr.payload.to_vec()));
            } else {
                panic!("the radio sent a frame that doesn't decode");
            }
        }
        answers
    }

    fn step(&mut self, out: &mut Wire) {
        self.now += 10;
        self.link.poll(&mut self.app, &self.st, &self.dev, &LiveAll::default(), self.now, out);
        while let Some(wr) = self.app.pop_write() {
            self.st.apply(wr);
        }
        while self.app.pop_cmd().is_some() {}
        if self.link.take_reboot() {
            self.link = Session::new(self.link.trusted);
        }
        let _ = (self.link.take_baud(), self.link.take_restore(), self.link.debug_req());
    }

    /// One request, its answer's status and payload (None if it didn't answer it).
    fn request(&mut self, ty: u8, payload: &[u8]) -> Option<(u8, Vec<u8>)> {
        self.seq = self.seq.wrapping_add(1);
        let mut w = [0u8; MAX_WIRE];
        let n = wire::build(ty, self.seq, payload, &mut w)?;
        let bytes = w[..n].to_vec(); // with its delimiter
        let seq = self.seq;
        self.exchange(&bytes).into_iter().find(|a| a.0 == ty | wire::ANSWER && a.1 == seq).map(|a| (a.2.first().copied().unwrap_or(0xFF), a.2.get(1..).unwrap_or_default().to_vec()))
    }

    /// What the radio keeps, as its dump says it (TERM-13).
    fn kept(&self) -> Vec<u8> {
        let mut v = Vec::new();
        #[cfg(feature = "line-cmds")]
        {
            let mut p = cmd::DumpPos::default();
            let mut l = cmd::Line::new();
            while cmd::dump_line(&self.app, &self.st, &self.dev, &mut p, &mut l) {
                v.extend_from_slice(l.as_bytes());
                v.push(b'\n');
            }
        }
        struct V<'a>(&'a mut Vec<u8>);
        impl PSink for V<'_> {
            fn put(&mut self, b: &[u8]) {
                self.0.extend_from_slice(b);
            }
        }
        self.app.set.save(&mut V(&mut v));
        v
    }
}

/// Records read from a fresh radio: (what, index, bytes), every kind the link has.
fn seeds() -> &'static Vec<(u8, u16, Vec<u8>)> {
    static S: OnceLock<Vec<(u8, u16, Vec<u8>)>> = OnceLock::new();
    S.get_or_init(|| {
        let mut pc = Pc::new(true);
        let mut v = Vec::new();
        for w in 1..=15u8 {
            if let Ok(r) = pc.read_parts(w, 0) {
                v.push((w, 0, r));
            }
            let mut p = std::vec![w, 0, 0];
            p.extend_from_slice(&8u16.to_le_bytes());
            if let Some((0, a)) = pc.request(msg::READ_MANY, &p) {
                let mut at = 2;
                while at + 3 <= a.len() {
                    let (idx, l) = (u16::from_le_bytes([a[at], a[at + 1]]), a[at + 2] as usize);
                    if at + 3 + l > a.len() {
                        break;
                    }
                    v.push((w, idx, a[at + 3..at + 3 + l].to_vec()));
                    at += 3 + l;
                }
            }
        }
        v
    })
}

/// A record kind and index at their edges: real ones, the next free, the last, past the end.
fn what_idx(r: &mut Rng) -> (u8, u16) {
    let w = if r.one_in(8) { r.byte() } else { r.range(1, 15) as u8 };
    let idx = match r.below(4) {
        0 => r.edge(2) as u16,
        1 => MAX_CHANNELS - 1 + r.below(3) as u16,
        _ => r.below(16) as u16,
    };
    (w, idx)
}

/// A request a PC might send, then mutated now and then: (type, payload).
fn request(r: &mut Rng) -> (u8, Vec<u8>) {
    let seeds = seeds();
    let ty = *r.pick(&TYPES);
    let mut p = match ty {
        msg::READ => {
            let (w, idx) = what_idx(r);
            let mut p = std::vec![w];
            p.extend(idx.to_le_bytes());
            p.extend((r.edge(2) as u16).to_le_bytes());
            p.push(r.edge(1) as u8);
            p
        }
        msg::WRITE => {
            // one part of a real record (or a whole one), its offset and total sometimes lying
            let (w, idx, rec) = r.pick(seeds).clone();
            let mut rec = rec;
            if r.one_in(2) {
                mutate(r, &mut rec, RECORD_MAX + 8, &[]);
            }
            let total = if r.one_in(6) { r.edge(2) as u16 } else { rec.len() as u16 };
            let off = if r.one_in(3) { r.below(rec.len() + 1) } else { 0 };
            let len = r.below(rec.len() - off.min(rec.len()) + 1).min(MAX_PAYLOAD - 7);
            let mut p = std::vec![if r.one_in(10) { r.byte() } else { w }];
            p.extend((if r.one_in(10) { r.edge(2) as u16 } else { idx }).to_le_bytes());
            p.extend((off as u16).to_le_bytes());
            p.extend(total.to_le_bytes());
            p.extend_from_slice(&rec[off.min(rec.len())..(off + len).min(rec.len())]);
            p
        }
        msg::WRITE_MANY => {
            // a kind `READ_MANY` gave (the settings and APRS go by `WRITE` alone)
            let many: Vec<u8> = seeds.iter().filter(|s| !matches!(s.0, what::SETTINGS | what::APRS)).map(|s| s.0).collect();
            let w = if r.one_in(10) { r.range(0, 16) as u8 } else { *r.pick(&many) };
            let mut p = std::vec![w];
            for _ in 0..r.range(1, 4) {
                let same: Vec<_> = seeds.iter().filter(|s| s.0 == w).collect();
                let Some(&&(_, idx, ref rec)) = same.get(r.below(same.len().max(1))) else { break };
                let mut rec = rec.clone();
                if r.one_in(2) {
                    mutate(r, &mut rec, 250, &[]);
                }
                let idx = if r.one_in(3) { r.below(20) as u16 } else { idx };
                p.extend(idx.to_le_bytes());
                p.push(if r.one_in(8) { r.byte() } else { rec.len() as u8 });
                p.extend_from_slice(&rec);
            }
            p
        }
        msg::READ_MANY | msg::DELETE | msg::USED => {
            let (w, idx) = what_idx(r);
            let mut p = std::vec![w];
            p.extend(idx.to_le_bytes());
            p.extend((r.edge(2) as u16).to_le_bytes());
            p.truncate(if ty == msg::USED { 1 } else { r.range(1, 5) });
            p
        }
        msg::STREAM => {
            let any = r.byte();
            let inner = *r.pick(&[msg::READ_MANY, msg::HEARD, msg::MSGS, msg::LOG_READ, msg::UI, any]);
            let mut p = std::vec![inner];
            let n = r.len(7);
            p.extend(r.bytes(n));
            p
        }
        msg::CMD => r.words(WORDS, 6),
        msg::DATA_READ | msg::DATA_WRITE => {
            let mut p = std::vec![r.below(5) as u8];
            let addr = match r.below(4) {
                0 => r.edge(4),
                1 => 0x00F000 + r.below(0x1000) as u32,
                2 => 0x300000 - r.below(256) as u32,
                _ => r.below(0x400000) as u32,
            };
            p.extend(addr.to_le_bytes());
            let n = r.len(80);
            p.extend(r.bytes(n));
            p
        }
        msg::MSG_SEND | msg::MSG_STOP | msg::MSG_DELETE => {
            // flags (a delete's), then two texts, each a length and its bytes
            let mut p = if ty == msg::MSG_DELETE { std::vec![r.below(3) as u8] } else { Vec::new() };
            for _ in 0..2 {
                let t = r.words(&["VE7ABC-9", "APRS", "WIDE1-1", "hello", "73", "{12", "ack1", "BLN1", ":"], 3);
                let n = if r.one_in(8) { r.byte() } else { t.len().min(255) as u8 };
                p.push(n);
                p.extend_from_slice(&t[..t.len().min(255)]);
            }
            p
        }
        msg::KEY => {
            use ferrite_core::link::key as k;
            let code = match r.below(4) {
                0 => k::KNOB,
                1 => k::PTT,
                _ => r.below(k::LAST_CODE as usize + 3) as u8,
            };
            let mut p = std::vec![code, r.below(8) as u8];
            if r.one_in(4) {
                let n = r.below(4);
                p.extend(r.bytes(n));
            }
            p
        }
        msg::SSB_WRITE => {
            // LINK-73: the SSB patch's steps in any order (a BEGIN never finished, a PROGRAM before any, pieces out of
            // its length): the known hash or another, a valid length or not
            use ferrite_core::link::ssb;
            match r.below(5) {
                0 => std::vec![ssb::CHECK],
                1 => {
                    let len = if r.one_in(2) { r.below(0x5000) as u32 & !7 } else { r.edge(4) };
                    let sha = if r.one_in(3) { [r.byte(); 32] } else { ferrite_core::ssb::KNOWN_SHA256 };
                    ssb::begin(len, &sha).to_vec()
                }
                2 => {
                    let mut out = [0u8; 3 + ssb::PROGRAM_MAX];
                    let n = r.below(ssb::PROGRAM_MAX + 1);
                    let b = r.bytes(n);
                    let k = ssb::program(r.edge(2) as u16 & !7, &b, &mut out);
                    out[..k].to_vec()
                }
                3 => std::vec![ssb::FINISH],
                _ => {
                    let n = r.len(80);
                    r.bytes(n)
                }
            }
        }
        msg::SLOT | msg::TUNE => {
            let mut p = std::vec![r.below(4) as u8, if ty == msg::TUNE { r.below(20) as u8 } else { r.byte() }];
            let n = if ty == msg::TUNE { r.below(3) } else { r.len(24) };
            p.extend(r.bytes(n));
            p
        }
        _ if r.one_in(2) => {
            // small fields first (a slot, a key, an op, a mask), then edge values
            let mut p = Vec::new();
            for _ in 0..r.range(1, 3) {
                p.push(r.below(40) as u8);
            }
            for _ in 0..r.below(6) {
                let k = *r.pick(&[1, 2, 4]);
                p.extend_from_slice(&r.edge(k).to_le_bytes()[..k]);
            }
            p
        }
        _ => {
            let n = r.len(MAX_PAYLOAD);
            r.bytes(n)
        }
    };
    if r.one_in(5) {
        mutate(r, &mut p, MAX_PAYLOAD + 4, &[]);
    }
    (ty, p)
}

/// A case's input: requests as (type, seq, payload length, payload, wire damage), so a failure's hex replays it.
fn case(r: &mut Rng) -> Vec<u8> {
    let mut v = Vec::new();
    v.push(r.below(4) as u8); // 0 cable; 1 Bluetooth; 2 Bluetooth with a PIN; 3 cable with a write session
    for _ in 0..r.range(1, 8) {
        let (ty, p) = request(r);
        let p = &p[..p.len().min(255)];
        v.push(ty);
        v.push(p.len() as u8);
        v.extend_from_slice(p);
        v.push(if r.one_in(6) { r.range(1, 255) as u8 } else { 0 }); // wire damage
    }
    v
}

fn run(input: &[u8]) {
    let Some((&mode, mut rest)) = input.split_first() else { return };
    let mut pc = Pc::new(mode & 1 == 0);
    if mode & 2 != 0 {
        if mode & 1 != 0 {
            pc.app.set.pin_len = 4;
            pc.app.set.pin = 1234;
        } else {
            let _ = pc.request(msg::BEGIN, &3u16.to_le_bytes());
        }
    }
    while rest.len() >= 2 {
        let (ty, n) = (rest[0], rest[1] as usize);
        let Some(p) = rest.get(2..2 + n) else { break };
        let damage = rest.get(2 + n).copied().unwrap_or(0);
        rest = rest.get(3 + n..).unwrap_or_default();
        let before = matches!(ty, msg::WRITE | msg::WRITE_MANY).then(|| pc.kept());
        if damage != 0 {
            // the frame damaged on the wire: a byte changed, dropped or added, or the delimiter lost
            pc.seq = pc.seq.wrapping_add(1);
            let mut w = [0u8; MAX_WIRE];
            let Some(k) = wire::build(ty, pc.seq, p, &mut w) else { continue };
            let mut b = w[..k - 1].to_vec();
            let i = damage as usize % b.len();
            match damage % 4 {
                0 => b[i] ^= damage | 1,
                1 => {
                    b.remove(i);
                }
                2 => b.insert(i, damage),
                _ => {}
            }
            if damage % 4 != 3 {
                b.push(0);
            }
            let _ = pc.exchange(&b);
            continue;
        }
        let Some((status, a)) = pc.request(ty, p) else { continue };
        if let Some(before) = before {
            let refused_whole = status != Status::Ok as u8 && (ty == msg::WRITE || a.first() == Some(&0));
            if refused_whole {
                assert!(pc.kept() == before, "a refused write ({ty:#x}, status {status}) changed what the radio keeps");
            }
            // a whole record taken: it reads back as kept, and that copy written again reads back the same
            let whole_write = ty == msg::WRITE && p.len() >= 7 && u16::from_le_bytes([p[3], p[4]]) as usize + p.len() - 7 == u16::from_le_bytes([p[5], p[6]]) as usize;
            if status == Status::Ok as u8 && (whole_write || ty == msg::WRITE_MANY && a.first().is_some_and(|&d| d > 0)) {
                let (w, idx) = (p[0], u16::from_le_bytes([p[1], p[2]]));
                if let Ok(kept) = pc.read(w, idx) {
                    let (s, _) = pc.write(w, idx, &kept, 200);
                    assert_eq!(s, Status::Ok as u8, "the radio refused its own copy of {w}/{idx}: {kept:02x?}");
                    assert_eq!(pc.read(w, idx).ok().as_deref(), Some(&kept[..]), "{w}/{idx} changed when its own copy was written back");
                }
            }
        }
    }
}

#[test]
fn fuzz_link_frames() {
    // the wire format alone: anything decodes or fails cleanly, and what `build` makes parses back
    Fuzz::new("link_wire", 0x11E0, 100_000).run(
        |r| {
            let n = r.len(MAX_PAYLOAD + 2);
            let p = r.bytes(n);
            let mut w = [0u8; MAX_WIRE + 8];
            let ty = r.byte();
            match wire::build(ty, r.byte(), &p, &mut w) {
                Some(k) if !r.one_in(3) => {
                    let mut v = std::vec![1u8];
                    v.extend_from_slice(&w[..k - 1]);
                    mutate(r, &mut v, MAX_WIRE + 8, &[]);
                    v
                }
                _ => {
                    let n = r.len(MAX_WIRE + 8);
                    let mut v = std::vec![0u8];
                    v.extend(r.bytes(n));
                    v
                }
            }
        },
        |d| {
            let mut b = d.get(1..).unwrap_or_default().to_vec();
            if let Ok(f) = wire::parse(&mut b) {
                let (ty, seq, pl) = (f.ty, f.seq, f.payload.to_vec());
                assert!(pl.len() <= MAX_PAYLOAD);
                let mut w = [0u8; MAX_WIRE];
                let k = wire::build(ty, seq, &pl, &mut w).expect("a parsed frame builds again");
                let back = wire::parse(&mut w[..k - 1]).expect("and parses"); // less the delimiter
                assert_eq!((back.ty, back.seq, back.payload), (ty, seq, &pl[..]));
            }
        },
    );
}

#[test]
fn fuzz_link_session() {
    assert!(seeds().iter().any(|s| s.0 == what::SETTINGS) && seeds().iter().any(|s| s.0 == what::CHANNEL), "{:?}", seeds().iter().map(|s| (s.0, s.1)).collect::<Vec<_>>());
    Fuzz::new("link_session", 0x11E1, 20_000).run(case, run);
}

/// Raw bytes on the port, no framing at all (line noise, another protocol, a port left open): never a panic,
/// and the session still answers a good frame afterwards.
#[test]
fn fuzz_link_noise() {
    Fuzz::new("link_noise", 0x11E2, 5_000).run(
        |r| {
            let n = r.len(4 * MAX_WIRE);
            let mut b = r.bytes(n);
            if r.one_in(2) {
                b.iter_mut().for_each(|c| *c |= 1); // no delimiters: an overlong frame
            }
            b
        },
        |noise| {
            let mut pc = Pc::new(true);
            let _ = pc.exchange(noise);
            let _ = pc.exchange(&[0]);
            let (s, _) = pc.request(msg::HELLO, &[PROTOCOL]).expect("HELLO answered after the noise");
            assert_eq!(s, Status::Ok as u8);
        },
    );
}

/// LINK-51's compressed requests (the PC-to-radio window): requests coded as a PC codes them, sometimes damaged
/// (a coded byte changed, a count skipped, a frame cut), through the shared block into plain frames. Until the
/// first damage every frame comes out as it went in; after it, nothing panics.
#[test]
fn fuzz_link_compressed() {
    use crate::link::z::{Zip, Z};
    use ferrite_core::linkz::{LzEnc, UP};
    Fuzz::new("link_compressed", 0x11E3, 30_000).run(
        |r| {
            let mut v = Vec::new();
            for _ in 0..r.range(1, 10) {
                let (ty, p) = request(r);
                let p = &p[..p.len().min(200)];
                v.push(ty);
                v.push(p.len() as u8);
                v.extend_from_slice(p);
                v.push(if r.one_in(8) { r.range(1, 255) as u8 } else { 0 });
            }
            v
        },
        |d| {
            let mut zip = std::boxed::Box::new(Zip::new());
            let mut enc = std::boxed::Box::new(LzEnc::<UP>::new());
            let (mut up_n, mut seq, mut in_step) = (0u8, 0u8, true);
            let mut rest = d;
            while rest.len() >= 2 {
                let (ty, n) = (rest[0], rest[1] as usize);
                let Some(p) = rest.get(2..2 + n) else { break };
                let damage = rest.get(2 + n).copied().unwrap_or(0);
                rest = rest.get(3 + n..).unwrap_or_default();
                seq = seq.wrapping_add(1);
                let mut c = [0u8; 300];
                let k = enc.encode(p, &mut c[2..]);
                (c[0], c[1]) = (ty, up_n);
                up_n = up_n.wrapping_add(1);
                let mut w = [0u8; MAX_WIRE];
                let Some(len) = (match k.filter(|&k| k + 2 < p.len()) {
                    Some(k) => wire::build(Z, seq, &c[..k + 2], &mut w),
                    None => wire::build(ty, seq, p, &mut w),
                }) else {
                    continue;
                };
                let mut bytes = w[..len].to_vec();
                if damage != 0 {
                    in_step = false;
                    match damage % 3 {
                        0 => up_n = up_n.wrapping_add(1), // a count skipped: the next frames are out of step
                        1 => {
                            let i = damage as usize % (bytes.len() - 1);
                            bytes[i] = bytes[i].wrapping_add(damage) | 1;
                        }
                        _ => {
                            bytes.truncate(bytes.len() / 2);
                            bytes.push(0);
                        }
                    }
                }
                let mut out = Vec::new();
                for &b in &bytes {
                    zip.take(b, &mut |c| out.push(c));
                }
                if in_step && ty & 0x80 == 0 {
                    let f = out.split_mut(|&b| b == 0).find(|f| !f.is_empty()).expect("a frame out");
                    let f = wire::parse(f).expect("a plain frame");
                    assert_eq!((f.ty, f.seq, f.payload), (ty, seq, p), "a coded request came out changed");
                }
            }
        },
    );
}
