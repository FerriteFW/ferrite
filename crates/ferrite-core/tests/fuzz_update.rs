//! What an update and a backup bring in from outside, fuzzed (ferrite-fuzzkit). The `.ffw`, `.btf` and `.fbin`
//! containers themselves are read by the tools and Ferrite Console, not by any Rust in this repository; what the
//! firmware reads of an update is here:
//!
//! - the SPI parts' headers and bodies (UPD-20 – UPD-23: the text pack, templates, logo, words), from a damaged or
//!   foreign chip: a part is used only when its header is its kind's and its CRC holds;
//! - the SSB patch's headers (bundle and stored copy);
//! - `STOCK_RESTORE`'s requests (BACKUP-7) and the link's raw-data ranges (LINK-59, LINK-60): whatever the PC sends,
//!   nothing in calibration or the update area is ever allowed;
//! - the backup reader's console (`ferrite-reader`, decision 264): its command line, its ranges, and the host's
//!   bytes during a dump (sync, ACK, NAK, CAN, noise, silence).
//!
//! Invariants: no panic, and the properties each says.

use ferrite_core::layout::{CALIBRATION, UPDATE_AREA, WRITABLE};
use ferrite_core::link::data as raw;
use ferrite_core::part::{self, Kind, HEADER, KINDS};
use ferrite_core::reader::{self, Command, Link, LineEdit, Out, BLOCK, ZCHUNK};
use ferrite_core::restore::{self, Op};
use ferrite_core::ssb;
use ferrite_fuzzkit::{mutate, Fuzz, Rng};

fn touches(a: u64, end: u64, r: &core::ops::Range<u32>) -> bool {
    a < r.end as u64 && (r.start as u64) < end
}

/// A part's slot as the chip holds it: a header and body, built as the packer builds them, then damaged.
#[test]
fn fuzz_part_headers() {
    let build = "0.9.5-fuzz";
    Fuzz::new("part_headers", 0x9A27, 20_000).run(
        |r| {
            let k = r.below(KINDS.len());
            let n = r.range(1, 600);
            let body = r.bytes(n);
            let mut v = vec![k as u8];
            v.extend_from_slice(&part::header(&KINDS[k], build, &body));
            v.extend_from_slice(&body);
            if !r.one_in(4) {
                mutate(r, &mut v, 700, &[]);
            }
            if r.one_in(5) && v.len() > 28 {
                // the length field lying: past the slot, zero, the edges
                let l = [0u32, 1, u32::MAX, 0x8000_0000, KINDS[k].slot.len() as u32, KINDS[k].slot.len() as u32 - HEADER as u32 + 1][r.below(6)];
                v[25..29].copy_from_slice(&l.to_le_bytes());
            }
            v
        },
        |d| {
            let Some((&k, slot)) = d.split_first() else { return };
            let kind: &Kind = &KINDS[k as usize % KINDS.len()];
            if slot.len() < HEADER {
                return;
            }
            let h: [u8; HEADER] = slot[..HEADER].try_into().unwrap();
            if let Some((len, crc)) = part::check_header(kind, &h, build) {
                assert!(len > 0 && HEADER as u64 + len as u64 <= kind.slot.len() as u64, "a part longer than its slot");
                let _ = crc;
            }
            // the whole check through a reader over the slot (erased past what we have)
            let reads_ok = true;
            let got = part::check(kind, build, &mut |a, buf| {
                assert!(a >= kind.slot.start && a as u64 + buf.len() as u64 <= kind.slot.end as u64, "a read outside the part's slot");
                let off = (a - kind.slot.start) as usize;
                for (i, b) in buf.iter_mut().enumerate() {
                    *b = slot.get(off + i).copied().unwrap_or(0xFF);
                }
                reads_ok
            });
            if let Some(len) = got {
                let body: Vec<u8> = (0..len as usize).map(|i| slot.get(HEADER + i).copied().unwrap_or(0xFF)).collect();
                assert_eq!(part::crc32(0, &body), u32::from_le_bytes(h[28..32].try_into().unwrap()), "a part used without its CRC");
            }
        },
    );
}

#[test]
fn fuzz_ssb_headers() {
    Fuzz::new("ssb_headers", 0x55B0, 20_000).run(
        |r| {
            let p = ssb::Patch { len: r.below(0x5000) as u32 & !7, sha: [r.byte(); 32] };
            let mut h = ssb::store_header(&p, *r.pick(&[ssb::SOURCE_FLASHER, ssb::SOURCE_LINK])).to_vec();
            if r.one_in(2) {
                h[..8].copy_from_slice(&ssb::BUNDLE_MAGIC);
            }
            mutate(r, &mut h, 80, &[]);
            h.resize(ssb::HEADER + r.below(64), 0);
            h
        },
        |d| {
            let h: [u8; ssb::HEADER] = d[..ssb::HEADER].try_into().unwrap();
            let slot = ferrite_core::layout::SSB_PATCH.end - ferrite_core::layout::SSB_PATCH.start;
            for p in [ssb::bundle(&h), ssb::stored(&h)].into_iter().flatten() {
                assert!(p.len.is_multiple_of(8) && ssb::HEADER as u64 + p.len as u64 <= slot as u64, "a patch longer than its slot");
            }
            let _ = ssb::records_ok(&d[ssb::HEADER..]);
        },
    );
}

/// An address at the protected ranges' edges, or anywhere.
fn addr(r: &mut Rng) -> u32 {
    match r.below(6) {
        0 => CALIBRATION.start - r.below(300) as u32,
        1 => CALIBRATION.end - r.below(300) as u32 + r.below(300) as u32,
        2 => UPDATE_AREA.start - r.below(300) as u32,
        3 => r.edge(4),
        _ => r.below(0x40_0000) as u32,
    }
}

/// `STOCK_RESTORE`'s requests through the parser and the check, against Ferrite's ranges and against ranges that
/// cover the whole address space (a PC lying about them): nothing that touches calibration or the update area, or
/// crosses a page, or leaves the chip, is ever allowed. The link's raw data range check likewise.
#[test]
fn fuzz_restore_requests() {
    Fuzz::new("restore_requests", 0x4E57, 100_000).run(
        |r| {
            let op = if r.one_in(10) { r.byte() } else { r.below(5) as u8 };
            let mut v = vec![r.below(2) as u8, op];
            match op {
                restore::ERASE => v.extend((addr(r) & if r.one_in(4) { u32::MAX } else { !0xFFF }).to_le_bytes()),
                restore::PROGRAM => {
                    v.extend(addr(r).to_le_bytes());
                    let n = r.range(1, restore::CHUNK + 2);
                    v.extend(r.bytes(n));
                }
                restore::CRC => {
                    v.extend(addr(r).to_le_bytes());
                    v.extend(r.edge(4).to_le_bytes());
                }
                _ => {}
            }
            if r.one_in(6) {
                mutate(r, &mut v, 200, &[]);
            }
            v
        },
        |d| {
            let Some((&all, p)) = d.split_first() else { return };
            let everything = core::slice::from_ref(&(0..u32::MAX));
            let ranges: &[core::ops::Range<u32>] = if all & 1 != 0 { everything } else { &WRITABLE };
            let chip = 4 << 20;
            if let Some(op) = restore::parse(p) {
                let ok = restore::check(&op, ranges, chip).is_ok();
                match op {
                    Op::Erase(a) if ok => {
                        let end = a as u64 + restore::SECTOR as u64;
                        assert!(a % restore::SECTOR == 0 && !touches(a as u64, end, &CALIBRATION) && !touches(a as u64, end, &UPDATE_AREA), "erase {a:#x} allowed");
                    }
                    Op::Program(a, b) if ok => {
                        let end = a as u64 + b.len() as u64;
                        assert!(!touches(a as u64, end, &CALIBRATION) && !touches(a as u64, end, &UPDATE_AREA), "program {a:#x}+{} allowed", b.len());
                        assert_eq!(a as u64 / restore::PAGE as u64, (end - 1) / restore::PAGE as u64, "a program across a page allowed");
                    }
                    Op::Crc(a, n) if ok => assert!(n > 0 && a as u64 + n as u64 <= chip as u64),
                    _ => {}
                }
            }
            // the link's raw range check (LINK-59, LINK-60) on the same numbers
            if p.len() >= 9 {
                let (a, n) = (u32::from_le_bytes(p[1..5].try_into().unwrap()), u32::from_le_bytes(p[5..9].try_into().unwrap()));
                if raw::allowed(a, n) {
                    let end = a as u64 + n as u64;
                    assert!(n > 0 && !touches(a as u64, end, &CALIBRATION) && end <= UPDATE_AREA.start as u64, "raw {a:#x}+{n:#x} allowed");
                }
            }
        },
    );
}

struct Sink(Vec<u8>);

impl Out for Sink {
    fn put(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
}

/// The reader's console: bytes through its line editor into commands; every range it would run is inside the chip
/// (`fdump`) or the MCU's main flash (`mdump`).
#[test]
fn fuzz_reader_console() {
    const WORDS: &[&str] = &["help", "?", "sys", "flash", "flashcrc", "reader", "tune", "reboot", "off", "mdump", "fdump", "fdumpz", "0x08000000", "0x080FFFFF", "0x400000", "0x3FFFFF", "0", "4194304", "115200", "921600", "0xFFFFFFFF", "0x", "2"];
    Fuzz::new("reader_console", 0x4EAD, 30_000).run(
        |r| {
            let mut v = r.words(WORDS, 5);
            if r.one_in(3) {
                v.extend(r.bytes(4));
            }
            v.push(b'\r');
            v
        },
        |bytes| {
            let mut e = LineEdit::default();
            for &c in bytes {
                if e.feed(c) == reader::Fed::Line {
                    let line = e.line().to_vec();
                    assert!(line.len() < 64);
                    let mut o = Sink(Vec::new());
                    match reader::parse(&line) {
                        Command::Fdump { addr, len, .. } | Command::FdumpZ { addr, len, .. } => {
                            for chip in [0, 1 << 20, 4 << 20, u32::MAX] {
                                if !reader::fdump_refusal(&mut o, addr, len, chip) {
                                    assert!(len > 0 && addr as u64 + len as u64 <= chip as u64, "fdump {addr:#x}+{len:#x} on {chip:#x}");
                                }
                            }
                        }
                        Command::Mdump { addr, len } => {
                            for kb in [0, 256, 1024, 4096, u32::MAX] {
                                if !reader::mdump_refusal(&mut o, addr, len, kb) {
                                    let end = 0x0800_0000u64 + kb.min(1024) as u64 * 1024;
                                    assert!(addr >= 0x0800_0000 && addr as u64 + len as u64 <= end, "mdump {addr:#x}+{len:#x} with {kb} KB");
                                }
                            }
                        }
                        _ => {}
                    }
                    e.clear();
                }
            }
        },
    );
}

/// A host on the other end of a dump: its bytes from the input (0xFF here meaning a moment's silence), a clock
/// that moves with every wait.
struct Host<'a> {
    bytes: &'a [u8],
    at: usize,
    ms: u32,
    sent: usize,
}

impl Link for Host<'_> {
    fn write(&mut self, b: &[u8]) {
        self.sent += b.len();
    }
    fn getc(&mut self, ms: u32) -> Option<u8> {
        let b = self.bytes.get(self.at).copied();
        self.at += 1;
        match b {
            Some(0xFF) | None => {
                self.ms = self.ms.wrapping_add(ms);
                None
            }
            Some(c) => {
                self.ms = self.ms.wrapping_add(1);
                Some(c)
            }
        }
    }
    fn clear_rx(&mut self) {}
    fn ms(&mut self) -> u32 {
        self.ms
    }
    fn pause(&mut self, ms: u32) {
        self.ms = self.ms.wrapping_add(ms);
    }
}

#[test]
fn fuzz_reader_dump() {
    Fuzz::new("reader_dump", 0x4EAE, 10_000).run(
        |r| {
            let mut v = vec![r.below(2) as u8, r.byte(), r.byte()];
            if !r.one_in(4) {
                v.extend_from_slice(b"FDS1");
            }
            for _ in 0..r.range(1, 40) {
                let any = r.byte();
                v.push(*r.pick(&[reader::ACK, reader::ACK, reader::ACK, reader::NAK, reader::CAN, 0xFF, b'F', any]));
            }
            v
        },
        |d| {
            let (z, a, l) = (d[0] & 1 != 0, d[1], d[2]);
            // a range of up to three blocks, anywhere in the chip (the reader refused anything else already)
            let chip = 4u32 << 20;
            let len = 1 + (l as u32 * 97) % (3 * BLOCK as u32);
            let addr = (a as u32 * 0x4_0F01) % (chip - len);
            let mut host = Host { bytes: &d[3..], at: 0, ms: 0, sent: 0 };
            let read = |at: u32, buf: &mut [u8]| {
                assert!(at >= addr && at as u64 + buf.len() as u64 <= addr as u64 + len as u64, "read {at:#x}+{} outside the dump", buf.len());
                buf.iter_mut().enumerate().for_each(|(i, b)| *b = (at as usize + i) as u8);
            };
            let r = if z {
                let mut buf = Box::new([0u8; ZCHUNK]);
                let mut zw = Box::new(reader::ZWork::new());
                reader::fdumpz_run(&mut host, &mut buf, &mut zw, addr, len, read, |_, _| true)
            } else {
                let mut buf = Box::new([0u8; BLOCK]);
                reader::fdump_run(&mut host, &mut buf, addr, len, read, |_, _| true)
            };
            if let Ok(done) = r {
                assert!(done.blocks >= 1);
            }
        },
    );
}
