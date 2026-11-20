//! The link protocol over the simulator's cable, as a PC tool sees it (manual chapter 29, TERM-2).

use ferrite_core::link::{self as wire, msg, what, MAX_WIRE, PROTOCOL};
use ferrite_sim::simcore::SimCore;

fn ask(sim: &mut SimCore, t: &mut u32, ty: u8, seq: u8, payload: &[u8]) -> Vec<u8> {
    let mut w = [0u8; MAX_WIRE];
    let n = wire::build(ty, seq, payload, &mut w).unwrap();
    let mut bytes = vec![0u8];
    bytes.extend_from_slice(&w[..n]);
    sim.term_feed(&bytes);
    let mut out = Vec::new();
    for _ in 0..50 {
        *t += 20;
        sim.step(*t);
        out.extend(sim.term_take());
        if out.contains(&0) {
            break;
        }
    }
    let end = out.iter().position(|&b| b == 0).expect("an answer");
    let f = wire::parse(&mut out[..end]).expect("a good frame");
    assert_eq!((f.ty, f.seq), (ty | wire::ANSWER, seq));
    f.payload.to_vec()
}

/// HELLO, a channel read and written back over the cable; then the terminal still opens on Enter once the link
/// has been quiet (TERM-2).
#[test]
fn link_over_the_cable() {
    let mut sim = SimCore::new(1, false);
    let mut t = 0;
    while t < 6000 {
        t += 20;
        sim.step(t);
    }
    let a = ask(&mut sim, &mut t, msg::HELLO, 1, &[PROTOCOL, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&a[..2], &[0, PROTOCOL]);
    let r = ask(&mut sim, &mut t, msg::READ_MANY, 2, &[what::CHANNEL, 0, 0, 1, 0]);
    assert_eq!(r[0], 0);
    assert_eq!(&r[3..5], &[0, 0], "channel 1 exists in the sample");
    let total = r[5] as usize;
    let mut rec = r[6..6 + total].to_vec();
    rec[8] = b'Q';
    let mut p = vec![what::CHANNEL, 0, 0, total as u8];
    p.extend_from_slice(&rec);
    assert_eq!(ask(&mut sim, &mut t, msg::WRITE_MANY, 3, &p)[..2], [0, 1]);
    let r = ask(&mut sim, &mut t, msg::READ_MANY, 4, &[what::CHANNEL, 0, 0, 1, 0]);
    assert_eq!(r[6 + 8], b'Q');
    // quiet past the link's hold: Enter is the terminal's again
    t += 11_000;
    sim.step(t);
    sim.term_feed(b"\r");
    t += 100;
    sim.step(t);
    assert!(!sim.term_take().is_empty(), "the terminal answered");
}

/// LINK-24.1 in the simulator (what `tools/uitest.py`'s restarts use): REBOOT is answered, then the simulated
/// radio restarts from what its flash kept: the boot screen, then home with the settings as they were; the link
/// starts over (HELLO again).
#[test]
fn reboot_restarts_the_simulator() {
    let mut sim = SimCore::new(1, false);
    let mut t = 0;
    while t < 6000 {
        t += 20;
        sim.step(t);
    }
    ask(&mut sim, &mut t, msg::HELLO, 1, &[PROTOCOL, 0, 0, 0, 0, 0, 0, 0, 0]);
    sim.app_mut().settings_mut().brightness = 3;
    assert_eq!(ask(&mut sim, &mut t, msg::REBOOT, 2, &[])[0], 0, "answered first");
    assert!(sim.booting(), "restarting");
    while t < 12_000 {
        t += 20;
        sim.step(t);
    }
    assert!(!sim.booting());
    assert_eq!(sim.screen_name(), "Home");
    assert_eq!(sim.app().settings().brightness, 3, "the settings kept");
    let a = ask(&mut sim, &mut t, msg::HELLO, 3, &[PROTOCOL, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&a[..2], &[0, PROTOCOL]);
}

/// UPD-16, LINK-59 – LINK-62 (decision 333): a chip with layout 2's data (its header at 0x02C000) is left alone and
/// says so (D25, and `HELLO` ending with the layouts 3 and 2); Console reads it raw, never calibration or the update
/// area; erases and programs the new layout over the cable (not over Bluetooth); stamps the header last, and the
/// radio restarts on layout 3 with what was written. UPD-16.1: the page says "Updating..." while the writes come.
#[test]
fn a_layout_2_chip_waits_for_console() {
    use ferrite_core::link::data::{CRC, ERASE, PROGRAM, READ, STAMP};
    use ferrite_sim::simcore::Stored;
    use ferrite_store::sim::SimFlash;
    let mut f = SimFlash::new();
    let mut h = [0xFFu8; 20];
    h[..4].copy_from_slice(&ferrite_store::store::HEADER_MAGIC.to_le_bytes());
    h[4..6].copy_from_slice(&2u16.to_le_bytes());
    let mut c = ferrite_store::Crc32::new();
    c.update(&h[..16]);
    h[16..20].copy_from_slice(&c.get().to_le_bytes());
    f.mem[0x2_C000..0x2_C014].copy_from_slice(&h);
    f.mem[0x2_F000..0x2_F040].fill(0x42); // a layout 2 channel record's place
    let before = f.image();
    let mut sim = SimCore::with_flash(1, false, Stored::Image(before.clone()));
    let mut t = 0;
    while t < 6000 {
        t += 20;
        sim.step(t);
    }
    assert_eq!(sim.screen_name(), "Update", "D25");
    let a = ask(&mut sim, &mut t, msg::HELLO, 1, &[PROTOCOL, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&a[a.len() - 6..a.len() - 4], &[3, 2], "this firmware's layout, the chip's");
    let rd = |addr: u32, n: u8| {
        let mut p = vec![READ];
        p.extend_from_slice(&addr.to_le_bytes());
        p.push(n);
        p
    };
    let r = ask(&mut sim, &mut t, msg::DATA_READ, 2, &rd(0x2_C000, 20));
    assert_eq!((r[0], &r[1..]), (0, &h[..]), "layout 2's header, read raw");
    assert_eq!(ask(&mut sim, &mut t, msg::DATA_READ, 3, &rd(0xF000, 16))[0], 4, "calibration: refused");
    assert_eq!(ask(&mut sim, &mut t, msg::DATA_READ, 4, &rd(0x2F_FFF0, 32))[0], 4, "into the update area: refused");
    assert_eq!(ask(&mut sim, &mut t, msg::DATA_READ, 5, &rd(0, 233))[0], 3, "more than an answer holds");
    assert_eq!(sim.store().nor().image(), before, "nothing written by the radio itself");
    // the new layout's first channel sector: erased, then a record programmed, read back by its CRC
    let mut e = vec![ERASE];
    e.extend_from_slice(&0x1_0000u32.to_le_bytes());
    assert_eq!(ask(&mut sim, &mut t, msg::DATA_WRITE, 6, &e)[0], 0);
    let mut cal = vec![ERASE];
    cal.extend_from_slice(&0xF000u32.to_le_bytes());
    assert_eq!(ask(&mut sim, &mut t, msg::DATA_WRITE, 7, &cal)[0], 4, "calibration is never erased");
    let rec: Vec<u8> = (0..64u8).collect();
    let mut p = vec![PROGRAM];
    p.extend_from_slice(&0x1_0000u32.to_le_bytes());
    p.extend_from_slice(&rec);
    assert_eq!(ask(&mut sim, &mut t, msg::DATA_WRITE, 8, &p)[0], 0);
    // UPD-16.1: while the writes come, the page says the update is being written (a moving bar under it)
    sim.step(t + 40);
    assert!(sim.screen_text().contains("Updating..."), "{}", sim.screen_text());
    let mut across = vec![PROGRAM];
    across.extend_from_slice(&0x1_00F0u32.to_le_bytes());
    across.extend_from_slice(&[0; 32]);
    assert_eq!(ask(&mut sim, &mut t, msg::DATA_WRITE, 9, &across)[0], 3, "across a page");
    // Decision 339: the header and the migration mark are the store's own; the radio says the chip is half moved
    for at in [0u32, 0xE000] {
        let mut e = vec![ERASE];
        e.extend_from_slice(&at.to_le_bytes());
        assert_eq!(ask(&mut sim, &mut t, msg::DATA_WRITE, 9, &e)[0], 4, "{at:#x}: refused");
    }
    let a = ask(&mut sim, &mut t, msg::HELLO, 9, &[PROTOCOL, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&a[a.len() - 6..a.len() - 4], &[3, 255], "LINK-61: half moved until the stamp");
    let mut crc = vec![CRC];
    crc.extend_from_slice(&0x1_0000u32.to_le_bytes());
    crc.extend_from_slice(&64u32.to_le_bytes());
    let mut want = ferrite_store::Crc32::new();
    want.update(&rec);
    assert_eq!(ask(&mut sim, &mut t, msg::DATA_READ, 10, &crc), [&[0u8][..], &want.get().to_le_bytes()].concat());
    // layout 2's header sector goes last before the stamp (an interrupted update keeps the radio waiting)
    let mut e = vec![ERASE];
    e.extend_from_slice(&0x2_C000u32.to_le_bytes());
    assert_eq!(ask(&mut sim, &mut t, msg::DATA_WRITE, 11, &e)[0], 0);
    assert_eq!(ask(&mut sim, &mut t, msg::DATA_WRITE, 12, &[STAMP, 2, 0])[0], 9, "only this firmware's layout");
    assert_eq!(ask(&mut sim, &mut t, msg::DATA_WRITE, 13, &[STAMP, 3, 0])[0], 0);
    assert!(sim.booting(), "the radio restarts on the stamped layout");
    while t < 12_000 {
        t += 20;
        sim.step(t);
    }
    assert_eq!(sim.store().layout_found(), None);
    let a = ask(&mut sim, &mut t, msg::HELLO, 14, &[PROTOCOL, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&a[a.len() - 6..a.len() - 4], &[3, 3]);
    let r = ask(&mut sim, &mut t, msg::DATA_READ, 15, &rd(0x1_0000, 64));
    assert_eq!(&r[1..], &rec[..], "what Console wrote is there");
}

/// Decision 339, LINK-62: a power cut right after the stamp's answer (`--cut-restart`: the store's writes queued as
/// on the radio, the stamp not yet written when the power goes) loses nothing: the radio comes back waiting for
/// Console (D25, the layouts 3 and 255), never formatted, what was written still there; the stamp sent again
/// finishes the move.
#[test]
fn a_power_cut_right_after_the_stamp_loses_nothing() {
    use ferrite_core::link::data::{ERASE, PROGRAM, READ, STAMP};
    use ferrite_sim::simcore::Stored;
    use ferrite_store::sim::SimFlash;
    let mut f = SimFlash::new();
    let mut h = [0xFFu8; 20];
    h[..4].copy_from_slice(&ferrite_store::store::HEADER_MAGIC.to_le_bytes());
    h[4..6].copy_from_slice(&2u16.to_le_bytes());
    let mut c = ferrite_store::Crc32::new();
    c.update(&h[..16]);
    h[16..20].copy_from_slice(&c.get().to_le_bytes());
    f.mem[0x2_C000..0x2_C014].copy_from_slice(&h);
    let mut sim = SimCore::with_flash(1, false, Stored::Image(f.image()));
    sim.cut_next_restart();
    let mut t = 0;
    while t < 6000 {
        t += 20;
        sim.step(t);
    }
    let a = ask(&mut sim, &mut t, msg::HELLO, 1, &[PROTOCOL, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&a[a.len() - 6..a.len() - 4], &[3, 2]);
    let w = |op: u8, at: u32, rest: &[u8]| {
        let mut p = vec![op];
        p.extend_from_slice(&at.to_le_bytes());
        p.extend_from_slice(rest);
        p
    };
    let rec: Vec<u8> = (0..64u8).collect();
    assert_eq!(ask(&mut sim, &mut t, msg::DATA_WRITE, 2, &w(ERASE, 0x1_0000, &[]))[0], 0);
    assert_eq!(ask(&mut sim, &mut t, msg::DATA_WRITE, 3, &w(PROGRAM, 0x1_0000, &rec))[0], 0);
    assert_eq!(ask(&mut sim, &mut t, msg::DATA_WRITE, 4, &w(ERASE, 0x2_C000, &[]))[0], 0);
    assert_eq!(ask(&mut sim, &mut t, msg::DATA_WRITE, 5, &[STAMP, 3, 0])[0], 0);
    assert!(sim.booting(), "the restart: a power cut before the Storage task wrote the stamp");
    while t < 14_000 {
        t += 20;
        sim.step(t);
    }
    assert_eq!(sim.screen_name(), "Update", "D25: waiting for Console, not a first boot");
    let a = ask(&mut sim, &mut t, msg::HELLO, 6, &[PROTOCOL, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&a[a.len() - 6..a.len() - 4], &[3, 255], "half moved: only Console's copy finishes it");
    let r = ask(&mut sim, &mut t, msg::DATA_READ, 7, &w(READ, 0x1_0000, &[64]));
    assert_eq!(&r[1..], &rec[..], "nothing formatted");
    assert_eq!(ask(&mut sim, &mut t, msg::DATA_WRITE, 8, &[STAMP, 3, 0])[0], 0);
    while t < 22_000 {
        t += 20;
        sim.step(t);
    }
    let a = ask(&mut sim, &mut t, msg::HELLO, 9, &[PROTOCOL, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&a[a.len() - 6..a.len() - 4], &[3, 3], "stamped");
    let r = ask(&mut sim, &mut t, msg::DATA_READ, 10, &w(READ, 0x1_0000, &[64]));
    assert_eq!(&r[1..], &rec[..]);
}

/// LINK-41.1: a bulk channel write with two frames in flight (Console's and the link tool's window), in sessions
/// of 64 as the limits test sends them: every frame is answered, each in a few steps. A frame left unanswered
/// stalled the limits test on the simulator (Nov 19, 2026).
#[test]
fn windowed_bulk_writes_answer_every_frame() {
    let mut sim = SimCore::new(1, false);
    let mut t = 0;
    while t < 6000 {
        t += 20;
        sim.step(t);
    }
    assert_eq!(ask(&mut sim, &mut t, msg::HELLO, 1, &[PROTOCOL, 0, 0, 0, 0, 0, 0, 0, 0])[0], 0);
    let r = ask(&mut sim, &mut t, msg::READ_MANY, 2, &[what::CHANNEL, 0, 0, 1, 0]);
    let total = r[5] as usize;
    let tpl = r[6..6 + total].to_vec();
    let rec = |k: u16| {
        let mut b = tpl.clone();
        b[0..2].copy_from_slice(&k.to_le_bytes());
        b[2..4].copy_from_slice(&(k / 8).to_le_bytes()); // 8 a zone, as the limits test
        b[4..8].copy_from_slice(&(43_000_000 + k as u32 * 250).to_le_bytes());
        b
    };
    // 64 zones first, as the limits test makes them (8 a frame), over the sample's
    let r = ask(&mut sim, &mut t, msg::READ_MANY, 3, &[what::ZONE, 0, 0, 1, 0]);
    let zt = r[6..6 + r[5] as usize].to_vec();
    let mut seq = 10u8;
    for s in (0..64u16).step_by(8) {
        assert_eq!(ask(&mut sim, &mut t, msg::BEGIN, seq, &8u16.to_le_bytes())[0], 0);
        let mut p = vec![what::ZONE];
        for z in s..s + 8 {
            let mut b = zt.clone();
            b[..16].copy_from_slice(format!("{:<16}", format!("LIMZONE {z:03}")).as_bytes());
            p.extend_from_slice(&z.to_le_bytes());
            p.push(b.len() as u8);
            p.extend_from_slice(&b);
        }
        assert_eq!(ask(&mut sim, &mut t, msg::WRITE_MANY, seq.wrapping_add(1), &p)[0], 0, "zones from {s}");
        assert_eq!(ask(&mut sim, &mut t, msg::END, seq.wrapping_add(2), &[])[0], 0);
        seq = seq.wrapping_add(3);
    }
    for session in 0..5u16 {
        assert_eq!(ask(&mut sim, &mut t, msg::BEGIN, seq, &64u16.to_le_bytes())[0], 0);
        seq = seq.wrapping_add(1);
        // 13 frames of 5, two in flight
        let frames: Vec<(u8, Vec<u8>)> = (0..13u16)
            .map(|f| {
                let mut p = vec![what::CHANNEL];
                for k in (session * 64 + f * 5)..(session * 64 + (f * 5 + 5).min(64)) {
                    let b = rec(k);
                    p.extend_from_slice(&k.to_le_bytes());
                    p.push(b.len() as u8);
                    p.extend_from_slice(&b);
                }
                let s = seq.wrapping_add(f as u8);
                (s, p)
            })
            .collect();
        seq = seq.wrapping_add(13);
        let mut sent = 0;
        let mut answered = 0;
        let mut out = Vec::new();
        let mut steps = 0;
        while answered < frames.len() {
            while sent < frames.len() && sent < answered + 2 {
                let mut w = [0u8; MAX_WIRE];
                let n = wire::build(msg::WRITE_MANY, frames[sent].0, &frames[sent].1, &mut w).unwrap();
                let mut bytes = vec![0u8];
                bytes.extend_from_slice(&w[..n]);
                sim.term_feed(&bytes);
                sent += 1;
            }
            t += 5;
            sim.step(t);
            steps += 1;
            out.extend(sim.term_take());
            while let Some(end) = out.iter().position(|&b| b == 0) {
                let mut fr: Vec<u8> = out.drain(..=end).collect();
                fr.pop();
                if fr.is_empty() {
                    continue;
                }
                let f = wire::parse(&mut fr).expect("a good frame");
                assert_eq!(f.ty, msg::WRITE_MANY | wire::ANSWER);
                assert_eq!(f.seq, frames[answered].0, "session {session}: answers in order");
                assert_eq!(f.payload[0], 0, "session {session} frame {answered}: status");
                answered += 1;
            }
            assert!(steps < 2000, "session {session}: frame {answered} of 13 unanswered after {steps} steps");
        }
        assert_eq!(ask(&mut sim, &mut t, msg::END, seq, &[])[0], 0);
        seq = seq.wrapping_add(1);
        // quiet long enough for the zone counts' save (CHAN-36: 2 s after the last channel write)
        for _ in 0..50 {
            t += 50;
            sim.step(t);
        }
        sim.term_take();
    }
}
