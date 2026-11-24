//! SAFE-8.3 (decision 448): the restart record kept across a power-off, in its own sector
//! ([`ferrite_core::layout::CRASH_LOG`]). A safety stop (SAFE-8.1) is left only by a power-off, which loses the
//! record in RAM, so each restart's start and each stop also write it here, and the next start reports the newest one
//! until Ferrite Console has read it.
//!
//! When: never in the fault path (a fault may come from the flash code itself, or leave the SPI bus half way through a
//! command). The fault path only notes the fault in RAM and resets; the record is written at the start that follows,
//! before the tasks run (a restart), or at the stop, once the radio is in its safe state (SAFE-8.1). Both are plain
//! busy-waited writes on a freshly started chip, so these functions block (`spin` waits a little between status
//! reads) and don't go through the store's queue, which isn't made yet then.
//!
//! A record is 64 bytes, a new one in the next erased place (64 a sector); the sector is erased only when all are
//! used: one erase per 64 faults or hangs. Two marks programmed later (bytes turned to zero, no erase): shown (the
//! start's notice came up) and read (Console read the readback's `restart` line, LINK-19.6). Only the newest valid
//! record counts; a cut write fails its CRC and reads as none.

use crate::io::Flash;
use crate::{Crc32, Nor, NorError, Result, SECTOR};
use ferrite_core::layout::CRASH_LOG;
use ferrite_core::msg::CrashRec;

pub const REC_LEN: usize = 64;
/// "FRCR".
pub const MAGIC: u32 = 0x5243_5246;
/// The CRC-32 of bytes 0..48.
const CRC_AT: usize = 48;
/// Programmed to zero once the start's notice has shown.
pub const SHOWN_AT: usize = 56;
/// Programmed to zero once Console has read the record.
pub const READ_AT: usize = 60;
/// Status reads [`wait`] allows (with `spin` 100 us on the radio: a second, past a sector erase's longest).
const WAIT_MAX: u32 = 10_000;

/// The record's bytes: magic, the three addresses, the count, the uptime, the flags (bit 0: stopped), the build;
/// the CRC; the marks erased, or `shown` already programmed (a restart's start shows it straight away).
pub fn encode(r: &CrashRec, shown: bool) -> [u8; REC_LEN] {
    let mut b = [0xFFu8; REC_LEN];
    b[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    for (i, pc) in r.pcs.iter().enumerate() {
        b[4 + 4 * i..8 + 4 * i].copy_from_slice(&pc.to_le_bytes());
    }
    b[16..20].copy_from_slice(&r.n.to_le_bytes());
    b[20..24].copy_from_slice(&r.up_s.to_le_bytes());
    b[24..28].copy_from_slice(&(r.stopped as u32).to_le_bytes());
    b[28..36].copy_from_slice(&r.build);
    let mut c = Crc32::new();
    c.update(&b[..CRC_AT]);
    b[CRC_AT..CRC_AT + 4].copy_from_slice(&c.get().to_le_bytes());
    if shown {
        b[SHOWN_AT..SHOWN_AT + 4].fill(0);
    }
    b
}

/// A record's contents and its marks (shown, read), or None (erased, cut, or not a record).
pub fn decode(b: &[u8; REC_LEN]) -> Option<(CrashRec, bool, bool)> {
    let mut c = Crc32::new();
    c.update(&b[..CRC_AT]);
    if crate::u32_at(b, 0) != MAGIC || crate::u32_at(b, CRC_AT) != c.get() {
        return None;
    }
    let mut build = [0u8; 8];
    build.copy_from_slice(&b[28..36]);
    let r = CrashRec {
        pcs: [crate::u32_at(b, 4), crate::u32_at(b, 8), crate::u32_at(b, 12)],
        n: crate::u32_at(b, 16),
        up_s: crate::u32_at(b, 20),
        stopped: crate::u32_at(b, 24) & 1 != 0,
        build,
    };
    Some((r, crate::u32_at(b, SHOWN_AT) != u32::MAX, crate::u32_at(b, READ_AT) != u32::MAX))
}

/// Until the chip's program or erase ends, `spin` between status reads.
fn wait<N: Nor>(f: &Flash<N>, spin: &mut dyn FnMut()) -> Result {
    for _ in 0..WAIT_MAX {
        if !f.nor().busy()? {
            return Ok(());
        }
        spin();
    }
    Err(NorError)
}

fn program<N: Nor>(f: &Flash<N>, addr: u32, data: &[u8], spin: &mut dyn FnMut()) -> Result {
    f.nor().start_program(addr, data)?;
    wait(f, spin)?;
    let mut got = [0u8; REC_LEN];
    let got = &mut got[..data.len()];
    f.raw_read(addr, got)?;
    if got != data {
        return Err(NorError);
    }
    Ok(())
}

/// The newest record: its address, contents and marks (shown, read). Read up to the first erased place.
pub fn newest<N: Nor>(f: &Flash<N>) -> Result<Option<(u32, CrashRec, bool, bool)>> {
    let mut last = None;
    let mut b = [0u8; REC_LEN];
    for a in CRASH_LOG.step_by(REC_LEN) {
        f.raw_read(a, &mut b)?;
        if b == [0xFF; REC_LEN] {
            break;
        }
        if let Some((r, shown, read)) = decode(&b) {
            last = Some((a, r, shown, read));
        }
    }
    Ok(last)
}

/// Writes `r` in the next erased place (none left: the sector erased first); its address. Only on a chip in this
/// layout with no move under way (UPD-16, LINK-62): another layout's data at this address is left alone.
pub fn append<N: Nor>(f: &Flash<N>, r: &CrashRec, shown: bool, spin: &mut dyn FnMut()) -> Result<u32> {
    if !crate::store::layout_ours(f)? {
        return Err(NorError);
    }
    let mut b = [0u8; REC_LEN];
    let mut at = None;
    for a in CRASH_LOG.step_by(REC_LEN) {
        f.raw_read(a, &mut b)?;
        if b == [0xFF; REC_LEN] {
            at = Some(a);
            break;
        }
    }
    let at = match at {
        Some(a) => a,
        None => {
            const _: () = assert!(CRASH_LOG.end - CRASH_LOG.start == SECTOR);
            f.nor().start_erase(CRASH_LOG.start)?;
            wait(f, spin)?;
            CRASH_LOG.start
        }
    };
    program(f, at, &encode(r, shown), spin)?;
    Ok(at)
}

/// The record at `at` marked shown (blocking, at start-up).
pub fn mark_shown<N: Nor>(f: &Flash<N>, at: u32, spin: &mut dyn FnMut()) -> Result {
    program(f, at + SHOWN_AT as u32, &[0; 4], spin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::SimFlash;
    use crate::{block, FlashStore};

    fn rec(pc: u32, n: u32) -> CrashRec {
        CrashRec { pcs: [pc, 1, 0], n, up_s: 42, stopped: n >= 3, build: *b"abc1234+" }
    }

    /// A formatted chip (this layout's header), as the radio has.
    fn chip() -> Flash<SimFlash> {
        let s = FlashStore::new(SimFlash::new(), ferrite_app::data::NoData);
        block(s.mount_async());
        Flash::new(s.into_parts().0)
    }

    /// SAFE-8.3: a record reads back as written, its marks erased until programmed; a damaged one reads as none.
    #[test]
    fn record_round_trip() {
        let r = rec(0x0800_1234, 2);
        let b = encode(&r, false);
        assert_eq!(decode(&b), Some((r, false, false)));
        assert_eq!(decode(&encode(&r, true)), Some((r, true, false)));
        let mut m = b;
        m[READ_AT..].fill(0);
        assert_eq!(decode(&m), Some((r, false, true)), "the marks lie outside the CRC");
        let mut bad = b;
        bad[5] ^= 1;
        assert_eq!(decode(&bad), None);
        assert_eq!(decode(&[0xFF; REC_LEN]), None);
        assert_eq!(r.build(), b"abc1234+");
    }

    /// SAFE-8.3: each record goes in the next erased place and the newest counts; the sector is erased once all 64
    /// places are used (one erase per 64 faults); the shown mark is a program, no erase.
    #[test]
    fn appends_then_wraps() {
        let f = chip();
        let mut spin = || {};
        assert_eq!(newest(&f).unwrap(), None);
        let at = append(&f, &rec(0x100, 1), true, &mut spin).unwrap();
        assert_eq!(at, CRASH_LOG.start);
        assert_eq!(newest(&f).unwrap(), Some((at, rec(0x100, 1), true, false)));
        let at2 = append(&f, &rec(0x200, 3), false, &mut spin).unwrap();
        assert_eq!(at2, at + REC_LEN as u32);
        assert_eq!(newest(&f).unwrap(), Some((at2, rec(0x200, 3), false, false)));
        mark_shown(&f, at2, &mut spin).unwrap();
        assert_eq!(newest(&f).unwrap().map(|n| (n.2, n.3)), Some((true, false)));
        let sector = (CRASH_LOG.start / SECTOR) as usize;
        let erases = f.nor().erases[sector];
        for i in 2..64 {
            append(&f, &rec(i, 1), true, &mut spin).unwrap();
        }
        assert_eq!(f.nor().erases[sector], erases, "64 records before an erase");
        assert_eq!(append(&f, &rec(0x999, 2), true, &mut spin).unwrap(), CRASH_LOG.start);
        assert_eq!(f.nor().erases[sector], erases + 1);
        assert_eq!(newest(&f).unwrap().map(|n| n.1), Some(rec(0x999, 2)));
    }

    /// SAFE-8.3: Console's read (LINK-19.6) marks the record through the store's queue: the next start finds it read.
    #[test]
    fn console_read_marks_it() {
        let f = chip();
        let at = append(&f, &rec(0x100, 3), false, &mut || {}).unwrap();
        let s = FlashStore::new(f.into_inner(), ferrite_app::data::NoData);
        block(s.mount_async());
        assert!(s.crash_read(at));
        let f = Flash::new(s.into_parts().0);
        assert_eq!(newest(&f).unwrap().map(|n| (n.0, n.2, n.3)), Some((at, false, true)));
    }

    /// SAFE-8.3, UPD-16: a chip without this layout's header (stock's data, or another layout's waiting for Console)
    /// gets no record.
    #[test]
    fn foreign_chip_left_alone() {
        let f = Flash::new(SimFlash::new());
        assert_eq!(append(&f, &rec(0x100, 1), true, &mut || {}), Err(NorError));
        assert_eq!(newest(&f).unwrap(), None);
    }

    /// SAFE-8.3: a write cut part way (the power gone) leaves no record, never a wrong one.
    #[test]
    fn cut_write_reads_as_none() {
        let f = chip();
        let ops = f.nor().ops;
        f.nor().cut_at = Some(ops + 20);
        assert!(append(&f, &rec(0x100, 1), true, &mut || {}).is_err());
        f.nor().power_on();
        assert_eq!(newest(&f).unwrap(), None);
    }
}
