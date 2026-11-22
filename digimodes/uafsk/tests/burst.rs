//! One KISS key-up carries a window. The first frame has the TX delay; the next starts on the shared closing
//! flag, not another delay; the tail is the usual three flags. Two frames sent apart each pay the delay.

use uafsk::hdlc::{Deframer, FlagBits, Framer};

fn bits(frame: &[u8], pre: u16, post: u16) -> Vec<u8> {
    Framer::new(frame, pre, post).collect()
}

fn leading_flags(bits: &[u8]) -> usize {
    let flag = [0u8, 1, 1, 1, 1, 1, 1, 0];
    let mut n = 0;
    while bits.len() >= n * 8 + 8 && bits[n * 8..n * 8 + 8] == flag {
        n += 1;
    }
    n
}

fn decoded(bits: &[u8]) -> Vec<Vec<u8>> {
    let mut d = Deframer::new();
    let mut out = Vec::new();
    for &b in bits {
        if let Some(f) = d.push(b) {
            out.push(f.to_vec());
        }
    }
    out
}

#[test]
fn a_window_shares_one_tx_delay() {
    // Fifteen bytes: the shortest AX.25 frame the deframer keeps (two addresses and a control byte).
    let a = vec![0x11u8; 15];
    let b = (0x20..0x2F).collect::<Vec<u8>>();
    // The last frame's one closing flag plus two more is the modem's three-flag tail.
    let solo = bits(&a, 45, 3);
    let split: Vec<u8> = bits(&a, 45, 1).into_iter().chain(FlagBits::new(2)).collect();
    assert_eq!(split, solo);
    assert_eq!(leading_flags(&solo), 45);

    let burst: Vec<u8> = Framer::new(&a, 45, 1).chain(Framer::new(&b, 0, 1)).chain(FlagBits::new(2)).collect();
    let apart: Vec<u8> = bits(&a, 45, 3).into_iter().chain(bits(&b, 45, 3)).collect();
    assert_eq!(leading_flags(&burst), 45, "the second frame does not repeat the TX delay");
    assert!(burst.len() < apart.len(), "one key-up is shorter than two");
    assert_eq!(decoded(&burst), vec![a.clone(), b]);
    assert_eq!(decoded(&solo), vec![a]);
}
