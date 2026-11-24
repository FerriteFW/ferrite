//! The GPS module's sentences fuzzed (ferrite-fuzzkit): real GGA, RMC and GSV lines from several talkers with
//! good checksums, then mutated (fields cut, repeated, overlong, odd bytes) and re-checksummed half the time so the
//! parser behind the checksum is reached. No panic; the satellites stay inside their table.

use ferrite_fuzzkit::{mutate, Fuzz};
use nmea::Nmea;

const LINES: [&str; 9] = [
    "GNGGA,183412.00,4900.60000,N,12305.40000,W,1,09,0.9,12.3,M,-17.0,M,,",
    "GPRMC,183413.00,A,4900.60000,N,12305.40000,W,0.0,,290926,,,A",
    "GNRMC,183413.50,A,4900.60000,N,12305.40000,W,23.4,271.5,290926,,,A",
    "GPRMC,183414.00,V,,,,,,,290926,,,N",
    "GPGSV,3,1,12,01,40,083,46,02,17,308,41,12,07,344,39,14,22,228,45",
    "GLGSV,2,2,07,70,12,045,,71,33,110,22,72,75,200,31",
    "BDGSV,1,1,04,01,45,120,33,03,50,200,,06,10,330,18,08,60,010,40",
    "GNGGA,000000.00,8959.99999,S,17959.99999,E,2,12,99.9,-420.5,M,,M,,",
    "GAGSV,1,1,00",
];

fn sentence(body: &[u8], good_sum: bool) -> Vec<u8> {
    let cs = body.iter().fold(0u8, |a, b| a ^ b) ^ !good_sum as u8;
    let mut v = vec![b'$'];
    v.extend_from_slice(body);
    v.extend(format!("*{cs:02X}\r\n").bytes());
    v
}

#[test]
fn fuzz_nmea() {
    let corpus: Vec<Vec<u8>> = LINES.iter().map(|l| l.as_bytes().to_vec()).collect();
    Fuzz::new("nmea", 0x4E3A, 60_000).run(
        |r| {
            let mut out = Vec::new();
            for _ in 0..r.range(1, 6) {
                let mut body = r.pick(&corpus).clone();
                if r.one_in(2) {
                    // a field swapped for an edge: long digit runs, signs, dots, nothing
                    let mut fields: Vec<Vec<u8>> = body.split(|&c| c == b',').map(|f| f.to_vec()).collect();
                    let i = r.below(fields.len());
                    let n = r.range(1, 18);
                    fields[i] = match r.below(6) {
                        0 => (0..n).map(|_| b'9').collect(),
                        1 => (0..n).map(|_| *r.pick(b"0123456789")).collect(),
                        2 => format!("{}.{}", "9".repeat(n), "9".repeat(r.range(0, 9))).into_bytes(),
                        3 => b"-".to_vec(),
                        4 => format!("-{}", "9".repeat(n)).into_bytes(),
                        _ => vec![],
                    };
                    body = fields.join(&b',');
                }
                if !r.one_in(5) {
                    mutate(r, &mut body, 200, &corpus);
                }
                if r.one_in(3) {
                    out.extend_from_slice(&body); // raw: no '$', a bad or missing checksum
                } else {
                    out.extend(sentence(&body, !r.one_in(6)));
                }
            }
            out
        },
        |bytes| {
            let mut n = Nmea::new();
            for &c in bytes {
                n.feed(c);
                assert!(n.nsat as usize <= n.sat.len());
                let _ = n.has_fix();
            }
        },
    );
}
