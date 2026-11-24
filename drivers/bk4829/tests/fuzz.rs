//! What the BK4829 reads back, fuzzed (ferrite-fuzzkit): every reading the drivers decode from a register (RSSI,
//! noise, status, interrupts, the sub-tone scan, DTMF, FSK, the frequency scan) on any register values, as a
//! glitch on the 3-wire bus could give. No panic, every result inside its range.

use bk4829::{scan_dhz, Bk4829, Bus};
use ferrite_fuzzkit::Fuzz;

/// A chip whose registers read as the input's bytes, in turn.
struct Regs<'a> {
    data: &'a [u8],
    at: usize,
}

impl Bus for Regs<'_> {
    fn write(&mut self, _reg: u8, _value: u16) {}
    fn read(&mut self, _reg: u8) -> u16 {
        let b = |i: usize| self.data.get(i % self.data.len().max(1)).copied().unwrap_or(0);
        let v = u16::from_le_bytes([b(self.at), b(self.at + 1)]);
        self.at += 2;
        v
    }
}

#[test]
fn fuzz_bk4829_readings() {
    Fuzz::new("bk4829_readings", 0xB482, 20_000).run(
        |r| {
            let n = r.len(64);
            let mut d = r.bytes(n);
            // the edges a scan or a status bit turns on
            for _ in 0..r.below(4) {
                if d.len() > 1 {
                    let i = r.below(d.len() - 1);
                    d[i..i + 2].copy_from_slice(&(r.edge(2) as u16).to_le_bytes());
                }
            }
            d
        },
        |data| {
            let mut bk = Bk4829::new(Regs { data, at: 0 });
            for _ in 0..4 {
                assert!(bk.rssi() <= 0x1FF);
                assert!((-160..=95).contains(&bk.rssi_dbm()));
                assert!(bk.noise() <= 0x7F);
                let _ = (bk.glitch(), bk.status(), bk.interrupts(), bk.freq(), bk.id());
                if let (Some(dhz), _) = bk.subtone_scan() {
                    assert!(dhz <= scan_dhz(0x1FFF));
                }
                assert!("0123456789ABCD*#".contains(bk.dtmf_code()));
                assert!(bk.dtmf_index() < 16);
                let _ = (bk.fsk_read(), bk.fsk_crc_ok(), bk.freq_scan_result());
            }
        },
    );
}
