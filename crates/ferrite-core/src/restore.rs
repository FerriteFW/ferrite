//! `STOCK_RESTORE` (BACKUP-7, LINK-4, LINK-8, decision 261): the way back to stock writes the owner's backup over
//! the cable into what Ferrite may have changed, then the PC flashes the stock `.BTF` (BACKUP-8).
//!
//! - Only Ferrite's own writable ranges ([`crate::layout::WRITABLE`]) are restored: since layout 3 (decision 333)
//!   that is the whole chip below the update area but calibration, which nothing ever writes.
//! - Calibration ([`CALIBRATION`]) and the update area ([`UPDATE_AREA`]) are refused by [`allowed`] whatever the
//!   ranges say, on top of the driver's own guard (SAFE-1, SAFE-2).
//! - The PC's requests, each a `STOCK_RESTORE` frame whose payload starts with an op: [`START`] (the radio stops
//!   and answers the chip size, the calibration sector's CRC-32, the ranges and the MCU's unique ID: decision
//!   263, appended so an older PC still reads what it knew), [`ERASE`] a 4 KB sector,
//!   [`PROGRAM`] up to [`CHUNK`] bytes within one 256-byte page, [`CRC`] a range read back, [`FINISH`] (the radio
//!   resets into the bootloader, saving nothing). Little-endian throughout.

use crate::layout::{CALIBRATION, UPDATE_AREA};
use crate::link::Status;
use core::ops::Range;

pub const START: u8 = 0;
pub const ERASE: u8 = 1;
pub const PROGRAM: u8 = 2;
pub const CRC: u8 = 3;
pub const FINISH: u8 = 4;

/// The most bytes one `PROGRAM` carries: half a page, so an aligned chunk never crosses a page.
pub const CHUNK: usize = 128;
pub const SECTOR: u32 = 4096;
pub const PAGE: u32 = 256;
/// The most ranges a `START` answer lists (9 bytes header, 8 a range, 12 the ID, in one 240-byte payload).
pub const MAX_RANGES: usize = 16;
/// The MCU unique ID's bytes at the end of a `START` answer (decision 263).
pub const UID_LEN: usize = 12;

/// A request, its shape checked (not yet where it writes: [`check`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op<'a> {
    Start,
    Erase(u32),
    Program(u32, &'a [u8]),
    Crc(u32, u32),
    Finish,
}

fn le32(b: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(..4)?.try_into().ok()?))
}

/// The payload as a request, or None when it is malformed.
pub fn parse(p: &[u8]) -> Option<Op<'_>> {
    let (&op, rest) = p.split_first()?;
    Some(match op {
        START if rest.is_empty() => Op::Start,
        ERASE if rest.len() == 4 => Op::Erase(le32(rest)?),
        PROGRAM if rest.len() > 4 && rest.len() <= 4 + CHUNK => Op::Program(le32(rest)?, &rest[4..]),
        CRC if rest.len() == 8 => Op::Crc(le32(rest)?, le32(&rest[4..])?),
        FINISH if rest.is_empty() => Op::Finish,
        _ => return None,
    })
}

fn overlaps(a: u32, end: u64, r: &Range<u32>) -> bool {
    (a as u64) < r.end as u64 && (r.start as u64) < end
}

/// Whether `len` bytes at `addr` may be erased or programmed: wholly inside one of `ranges`, and never touching
/// calibration or the update area, whatever `ranges` holds.
pub fn allowed(ranges: &[Range<u32>], addr: u32, len: u32) -> bool {
    let end = addr as u64 + len as u64;
    len > 0
        && !overlaps(addr, end, &CALIBRATION)
        && !overlaps(addr, end, &UPDATE_AREA)
        && ranges.iter().any(|r| addr >= r.start && end <= r.end as u64)
}

/// Where a request may go: `BadArg` for a misaligned or out-of-chip one, `Refused` outside the ranges (or into
/// calibration or the update area).
pub fn check(op: &Op, ranges: &[Range<u32>], chip: u32) -> Result<(), Status> {
    match *op {
        Op::Erase(a) if a % SECTOR != 0 => Err(Status::BadArg),
        Op::Erase(a) if allowed(ranges, a, SECTOR) => Ok(()),
        // the last byte's address checked: near the top of the address space it overflowed (found by fuzzing)
        Op::Program(a, d) if a.checked_add(d.len() as u32 - 1).is_none_or(|end| a / PAGE != end / PAGE) => Err(Status::BadArg),
        Op::Program(a, d) if allowed(ranges, a, d.len() as u32) => Ok(()),
        Op::Erase(_) | Op::Program(..) => Err(Status::Refused),
        Op::Crc(a, n) if n == 0 || a as u64 + n as u64 > chip as u64 => Err(Status::BadArg),
        _ => Ok(()),
    }
}

/// The `START` answer after its status byte: chip size, calibration CRC-32, the range count, each range's start
/// and end, then the MCU's unique ID ([`crate::uid::bytes`]: the PC tells the backup is this radio's by it, as
/// calibration can be the same on two radios, decision 263). Its length.
pub fn start_answer(chip: u32, cal_crc: u32, ranges: &[Range<u32>], uid: [u32; 3], out: &mut [u8]) -> usize {
    let n = ranges.len().min(MAX_RANGES);
    out[..4].copy_from_slice(&chip.to_le_bytes());
    out[4..8].copy_from_slice(&cal_crc.to_le_bytes());
    out[8] = n as u8;
    for (i, r) in ranges[..n].iter().enumerate() {
        out[9 + 8 * i..13 + 8 * i].copy_from_slice(&r.start.to_le_bytes());
        out[13 + 8 * i..17 + 8 * i].copy_from_slice(&r.end.to_le_bytes());
    }
    let at = 9 + 8 * n;
    out[at..at + UID_LEN].copy_from_slice(&crate::uid::bytes(uid));
    at + UID_LEN
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::WRITABLE;

    /// A program whose last byte is past the top of the address space is a bad argument, not an overflow (found
    /// by fuzzing).
    #[test]
    fn a_program_past_the_top_is_a_bad_argument() {
        let all = [Range { start: 0, end: u32::MAX }];
        assert_eq!(check(&Op::Program(u32::MAX, &[1, 2]), &all, 4 << 20), Err(Status::BadArg));
        assert_eq!(check(&Op::Program(0xFFFF_FFF0, &[0; 32]), &all, 4 << 20), Err(Status::BadArg));
        assert_eq!(check(&Op::Program(u32::MAX, &[1]), &all, 4 << 20), Err(Status::Refused), "one byte at the very top: past the ranges");
    }

    /// BACKUP-7, LINK-4: nothing in calibration or the update area is ever allowed, even when the ranges given
    /// cover the whole chip; inside Ferrite's own ranges everything is.
    #[test]
    fn calibration_and_the_update_area_are_never_allowed() {
        let all = [Range { start: 0, end: u32::MAX }];
        for a in (0..0x40_0000u32).step_by(SECTOR as usize) {
            let protected = CALIBRATION.contains(&a) || UPDATE_AREA.contains(&a);
            assert_eq!(allowed(&all, a, SECTOR), !protected, "{a:#x}");
            assert_eq!(check(&Op::Erase(a), &all, 4 << 20).is_ok(), !protected, "{a:#x}");
        }
        // A range reaching into either from below.
        assert!(!allowed(&all, CALIBRATION.start - 16, 32));
        assert!(!allowed(&all, UPDATE_AREA.start - 16, 32));
        assert!(!allowed(&all, u32::MAX - 3, 8), "no wrap");
        for r in &WRITABLE {
            assert!(allowed(&WRITABLE, r.start, SECTOR) && allowed(&WRITABLE, r.end - SECTOR, SECTOR));
            assert!(!allowed(&WRITABLE, r.end - 8, 16), "not past its end");
        }
        assert!(allowed(&WRITABLE, 0, SECTOR), "layout 3: from the layout header at 0x000000");
        assert!(!allowed(&WRITABLE, CALIBRATION.start, SECTOR) && !allowed(&WRITABLE, UPDATE_AREA.start, SECTOR));
    }

    /// BACKUP-7: requests parse by shape, then go only where they may.
    #[test]
    fn requests_parse_and_check() {
        let w = &WRITABLE[..];
        let at = WRITABLE[0].start;
        assert_eq!(parse(&[START]), Some(Op::Start));
        assert_eq!(parse(&[START, 1]), None);
        assert_eq!(parse(&[ERASE, 0, 0xC0, 2, 0]), Some(Op::Erase(0x02_C000)));
        assert_eq!(parse(&[ERASE, 0, 0]), None);
        let mut p = [0u8; 5 + CHUNK + 1];
        p[0] = PROGRAM;
        p[1..5].copy_from_slice(&at.to_le_bytes());
        assert_eq!(parse(&p[..5 + CHUNK]), Some(Op::Program(at, &p[5..5 + CHUNK])));
        assert_eq!(parse(&p), None, "over a chunk");
        assert_eq!(parse(&p[..5]), None, "no bytes");
        assert_eq!(parse(&[CRC, 0, 0, 0, 0, 0, 0x10, 0, 0]), Some(Op::Crc(0, 0x1000)));
        assert_eq!(parse(&[FINISH]), Some(Op::Finish));
        assert_eq!(parse(&[9]), None);
        assert_eq!(parse(&[]), None);

        assert_eq!(check(&Op::Erase(at + 1), w, 4 << 20), Err(Status::BadArg));
        assert_eq!(check(&Op::Erase(CALIBRATION.start), w, 4 << 20), Err(Status::Refused));
        assert_eq!(check(&Op::Program(at + 200, &[0; 100]), w, 4 << 20), Err(Status::BadArg), "crosses a page");
        assert_eq!(check(&Op::Program(at + 128, &[0; 128]), w, 4 << 20), Ok(()));
        assert_eq!(check(&Op::Program(UPDATE_AREA.start, &[0; 16]), w, 4 << 20), Err(Status::Refused));
        assert_eq!(check(&Op::Crc(0, 4 << 20), w, 4 << 20), Ok(()), "reading is anywhere");
        assert_eq!(check(&Op::Crc(1, 4 << 20), w, 4 << 20), Err(Status::BadArg));
        assert_eq!(check(&Op::Crc(0, 0), w, 4 << 20), Err(Status::BadArg));
    }

    /// BACKUP-7, LINK-8 (decision 263): the ranges, then the MCU's unique ID last, where an older PC stops reading.
    #[test]
    fn start_answer_layout() {
        let mut out = [0u8; 240];
        let uid = [0x3F2A_91C0, 0x7D44_0123, 0x89AB_CDEF];
        let n = start_answer(4 << 20, 0x1234_5678, &WRITABLE, uid, &mut out);
        assert_eq!(n, 9 + 8 * WRITABLE.len() + UID_LEN);
        assert_eq!(&out[n - UID_LEN..n], &crate::uid::bytes(uid));
        assert_eq!(&out[..9], &[0, 0, 0x40, 0, 0x78, 0x56, 0x34, 0x12, WRITABLE.len() as u8]);
        assert_eq!(le32(&out[9..]), Some(WRITABLE[0].start));
        assert_eq!(le32(&out[13..]), Some(WRITABLE[0].end));
        const { assert!(1 + 9 + 8 * MAX_RANGES + UID_LEN <= crate::link::MAX_PAYLOAD) };
    }
}
