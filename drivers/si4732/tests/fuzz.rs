//! RDS from the air fuzzed (ferrite-fuzzkit): groups of every type and version with any block error levels, and
//! the station name and radio text groups mutated. No panic; the text stays inside its buffer and printable.

use ferrite_fuzzkit::{mutate, Fuzz};
use si4732::rds::Rds;

#[test]
fn fuzz_rds_groups() {
    // a station sending its name (0A) and radio text (2A, 2B), as 10-byte groups: four blocks, then the errors
    let mut seed = Vec::new();
    for (b, c, d) in [(0x0000u16, 0xE0CDu16, 0x4342u16), (0x0001, 0, 0x4320), (0x0002, 0, 0x3838), (0x0003, 0, 0x2E31), (0x2000, 0x4865, 0x6C6C), (0x2001, 0x6F20, 0x5254), (0x2002, 0x0D20, 0x2020), (0x2810, 0, 0x4869), (0x2811, 0, 0x0D00)] {
        for w in [0xC201, b, c, d] {
            seed.extend_from_slice(&u16::to_be_bytes(w));
        }
        seed.push(0);
    }
    let corpus = vec![seed];
    Fuzz::new("rds_groups", 0x4D5, 30_000).run(
        |r| {
            let mut d = corpus[0].clone();
            mutate(r, &mut d, 2000, &corpus);
            if r.one_in(3) {
                let n = r.len(200);
                d.extend(r.bytes(n * 9));
            }
            d
        },
        |data| {
            let mut rds = Rds::init();
            for g in data.chunks_exact(9) {
                let w = |i: usize| u16::from_be_bytes([g[2 * i], g[2 * i + 1]]);
                rds.feed(w(0), w(1), w(2), w(3), g[8]);
                assert!(rds.text().len() <= rds.rt.len());
                assert!(rds.ps.iter().all(|&c| c == 0 || (0x20..0x7F).contains(&c)), "{:?}", rds.ps);
                let _ = (rds.ps_complete(), rds.rt_complete());
                if rds.groups % 97 == 0 {
                    rds = Rds::init(); // a retune
                }
            }
        },
    );
}
