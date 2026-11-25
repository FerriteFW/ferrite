//! The airline table (ACARS-6): a flight's two-letter IATA code to the airline's ICAO code and name, kept in the
//! SPI flash ([`crate::layout::AIRLINES`]) so it costs the MCU's flash nothing. Console writes it (the CSV from
//! data.ferritefw.dev, packed by `tools/mkairlines.py`); with none there the screens show the raw code.
//!
//! Little-endian, fixed width:
//!
//! ```text
//! header (16): magic "FAIR" | version u16 (1) | count u16 | CRC-32 of the rows (zlib's) u32 | 4 reserved (0)
//! row (29):    IATA [2] | ICAO [3] | name [24], ASCII, NUL-padded
//! ```
//!
//! Rows sorted by IATA code (bytewise), each code once. A lookup is a binary search: about ten 2-byte reads and one
//! row's, through the same raw reader the store gives everything else ([`check`] once, then [`lookup`]).

use crate::layout::AIRLINES;

pub const MAGIC: [u8; 4] = *b"FAIR";
pub const VERSION: u16 = 1;
pub const HEADER: usize = 16;
pub const ROW: usize = 29;
pub const NAME: usize = 24;
/// The rows the region holds.
pub const MAX: usize = (AIRLINES.end - AIRLINES.start) as usize - HEADER;
pub const MAX_ROWS: u16 = (MAX / ROW) as u16;

/// One airline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Airline {
    pub iata: [u8; 2],
    pub icao: [u8; 3],
    pub name: [u8; NAME],
}

impl Airline {
    pub fn name(&self) -> &[u8] {
        crate::acars::field(&self.name)
    }

    pub fn icao(&self) -> &[u8] {
        crate::acars::field(&self.icao)
    }
}

/// Reads `buf.len()` bytes of the SPI flash at an address (false: it can't), as [`crate::part::check`] takes it.
pub type Read<'a> = &'a mut dyn FnMut(u32, &mut [u8]) -> bool;

/// The table's rows when its header is good and the rows' CRC holds (None: empty, missing or damaged). Reads every
/// row once, 512 bytes at a time: call it when the table may have changed, not per lookup.
#[inline(never)]
pub fn check(read: Read) -> Option<u16> {
    let mut h = [0u8; HEADER];
    if !read(AIRLINES.start, &mut h) {
        return None;
    }
    let count = u16::from_le_bytes([h[6], h[7]]);
    if h[..4] != MAGIC || u16::from_le_bytes([h[4], h[5]]) != VERSION || count == 0 || count > MAX_ROWS {
        return None;
    }
    let want = u32::from_le_bytes([h[8], h[9], h[10], h[11]]);
    let (mut crc, mut at, end) = (0, AIRLINES.start + HEADER as u32, AIRLINES.start + HEADER as u32 + count as u32 * ROW as u32);
    let mut b = [0u8; 512];
    while at < end {
        let n = ((end - at) as usize).min(b.len());
        if !read(at, &mut b[..n]) {
            return None;
        }
        crc = crate::part::crc32(crc, &b[..n]);
        at += n as u32;
    }
    (crc == want).then_some(count)
}

/// Airline `iata` in a table of `count` rows ([`check`]'s answer).
#[inline(never)]
pub fn lookup(read: Read, count: u16, iata: [u8; 2]) -> Option<Airline> {
    let row = |i: u16| AIRLINES.start + HEADER as u32 + i as u32 * ROW as u32;
    let (mut lo, mut hi) = (0u16, count.min(MAX_ROWS));
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let mut k = [0u8; 2];
        if !read(row(mid), &mut k) {
            return None;
        }
        match k.cmp(&iata) {
            core::cmp::Ordering::Less => lo = mid + 1,
            core::cmp::Ordering::Greater => hi = mid,
            core::cmp::Ordering::Equal => {
                let mut r = [0u8; ROW];
                if !read(row(mid), &mut r) {
                    return None;
                }
                let mut a = Airline { iata, icao: [0; 3], name: [0; NAME] };
                a.icao.copy_from_slice(&r[2..5]);
                a.name.copy_from_slice(&r[5..]);
                return Some(a);
            }
        }
    }
    None
}

/// A table's header for `rows` (the packer's; `tools/mkairlines.py` writes the same).
pub fn header(rows: &[u8]) -> [u8; HEADER] {
    let mut h = [0u8; HEADER];
    h[..4].copy_from_slice(&MAGIC);
    h[4..6].copy_from_slice(&VERSION.to_le_bytes());
    h[6..8].copy_from_slice(&((rows.len() / ROW) as u16).to_le_bytes());
    h[8..12].copy_from_slice(&crate::part::crc32(0, rows).to_le_bytes());
    h
}

/// One row: the codes and the name cut to [`NAME`] bytes.
pub fn row(iata: &[u8; 2], icao: &[u8], name: &[u8]) -> [u8; ROW] {
    let mut r = [0u8; ROW];
    r[..2].copy_from_slice(iata);
    let n = icao.len().min(3);
    r[2..2 + n].copy_from_slice(&icao[..n]);
    let n = name.len().min(NAME);
    r[5..5 + n].copy_from_slice(&name[..n]);
    r
}

/// The test fixture's airlines (`tools/mkairlines-fixture.csv`), sorted by IATA code: the simulator writes them to
/// its flash, the tests look them up.
#[cfg(not(target_os = "none"))]
pub const FIXTURE: [(&[u8; 2], &str, &str); 10] = [
    (b"AA", "AAL", "American Airlines"),
    (b"AC", "ACA", "Air Canada"),
    (b"AS", "ASA", "Alaska Airlines"),
    (b"DL", "DAL", "Delta Air Lines"),
    (b"F8", "FLE", "Flair Airlines"),
    (b"PD", "POE", "Porter Airlines"),
    (b"QK", "JZA", "Jazz Aviation"),
    (b"TS", "TSC", "Air Transat"),
    (b"UA", "UAL", "United Airlines"),
    (b"WS", "WJA", "WestJet"),
];

/// [`FIXTURE`] as the table's bytes (header and rows).
#[cfg(not(target_os = "none"))]
pub fn fixture(out: &mut [u8; HEADER + FIXTURE.len() * ROW]) {
    for (k, (iata, icao, name)) in FIXTURE.iter().enumerate() {
        out[HEADER + k * ROW..HEADER + (k + 1) * ROW].copy_from_slice(&row(iata, icao.as_bytes(), name.as_bytes()));
    }
    let h = header(&out[HEADER..]);
    out[..HEADER].copy_from_slice(&h);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn reader(img: &[u8]) -> impl FnMut(u32, &mut [u8]) -> bool + '_ {
        move |a, b| {
            let o = (a - AIRLINES.start) as usize;
            match img.get(o..o + b.len()) {
                Some(s) => {
                    b.copy_from_slice(s);
                    true
                }
                None => {
                    b.fill(0xFF);
                    true
                }
            }
        }
    }

    fn table() -> Vec<u8> {
        let mut t = [0u8; HEADER + FIXTURE.len() * ROW];
        fixture(&mut t);
        t.to_vec()
    }

    /// ACARS-6: a binary search finds every airline in the table and nothing else.
    #[test]
    fn finds_every_fixture_airline_and_no_other() {
        let t = table();
        let mut r = reader(&t);
        let n = check(&mut r).unwrap();
        assert_eq!(n, 10);
        for (iata, icao, name) in FIXTURE {
            let a = lookup(&mut r, n, *iata).unwrap();
            assert_eq!((a.icao(), a.name()), (icao.as_bytes(), name.as_bytes()));
        }
        for miss in [*b"A0", *b"AB", *b"ZZ", *b"00", *b"WT"] {
            assert_eq!(lookup(&mut r, n, miss), None);
        }
    }

    /// ACARS-6: no table, another version or a changed row: none (the screen shows the code).
    #[test]
    fn erased_or_damaged_tables_are_refused() {
        let erased = [0xFFu8; 64];
        assert_eq!(check(&mut reader(&erased)), None);
        let mut t = table();
        t[HEADER + 7] ^= 1;
        assert_eq!(check(&mut reader(&t)), None, "a row changed: the CRC fails");
        let mut t = table();
        t[4] = 2;
        assert_eq!(check(&mut reader(&t)), None, "another version");
        assert_eq!(check(&mut |_, _| false), None, "no flash");
    }

    /// ACARS-6: 32 KB holds every airline with an IATA code.
    #[test]
    fn the_fixture_is_sorted_and_fits() {
        assert!(FIXTURE.windows(2).all(|w| w[0].0 < w[1].0));
        const { assert!(MAX_ROWS >= 1000, "room for every airline with an IATA code") };
    }

    /// ACARS-6: `tools/mkairlines.py` makes the same bytes from the fixture's CSV (its output is kept beside it).
    #[test]
    fn the_python_packer_agrees() {
        let csv = include_str!("../../../tools/mkairlines-fixture.csv");
        let bin = include_bytes!("../../../tools/mkairlines-fixture.bin");
        let mut rows = Vec::new();
        let mut lines: Vec<Vec<&str>> = csv.lines().skip(1).filter(|l| !l.trim().is_empty()).map(|l| l.split(',').collect()).collect();
        lines.sort_by(|a, b| a[0].cmp(b[0]));
        for f in &lines {
            let iata: [u8; 2] = f[0].as_bytes().try_into().unwrap();
            rows.extend_from_slice(&row(&iata, f[1].as_bytes(), f[2].as_bytes()));
        }
        let mut want = header(&rows).to_vec();
        want.extend(rows);
        assert_eq!(&bin[..], &want[..]);
        assert_eq!(&bin[..], &table()[..], "and FIXTURE is the CSV");
    }
}
