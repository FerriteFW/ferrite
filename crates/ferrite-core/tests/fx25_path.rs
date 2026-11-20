//! FX.25 on the radio's APRS path (APRS-44, APRS-45): what the DSP task transmits for a frame with 0 5 8 5 set,
//! and what the decoder slot hands the APRS task for an FX.25 transmission with errors.

use ferrite_core::audio::{afsk_tx, tx_bits, uafsk, TxBits};
use ferrite_core::decoders::{Decoder, Found, RATE_HZ};
use uafsk::ufx25::{Encoder, Receiver, MODES};

/// An APRS position frame (AX.25 without its FCS).
fn frame() -> Vec<u8> {
    let mut f: Vec<u8> = b"APZFER".iter().map(|c| c << 1).collect();
    f.push(0x60);
    f.extend(b"VE3KPW".iter().map(|c| c << 1));
    f.push(0x60 | 9 << 1);
    f.extend(b"WIDE1 ".iter().map(|c| c << 1));
    f.push(0x60 | 1 << 1 | 1);
    f.extend([0x03, 0xF0]);
    f.extend_from_slice(b"!4527.15N/07541.41W>Ferrite FX.25 beacon");
    f
}

/// APRS-45: with 16, 32 or 64 check bytes the bits are the TX delay's flags, then the FX.25 tag of a code with that
/// many check bytes, and ufx25 gets the frame back from them; with 0 (off) or a frame too long for the code they
/// are plain AX.25.
#[test]
fn transmitted_bits_start_with_a_tag() {
    let f = frame();
    let cfg = afsk_tx(300);
    let pre = cfg.preamble as usize * 8;
    for check in [16u8, 32, 64] {
        let mut enc = None;
        let bits: Vec<u8> = tx_bits(&f, check, &cfg, &mut enc).collect();
        assert!(bits[..pre].chunks(8).all(|b| b == [0, 1, 1, 1, 1, 1, 1, 0]), "flags first");
        let tag = bits[pre..pre + 64].iter().enumerate().fold(0u64, |t, (i, &b)| t | (b as u64) << i);
        let mode = MODES.iter().find(|m| m.tag == tag).expect("an FX.25 tag after the flags");
        assert_eq!(mode.check(), check as usize);
        let mut rx = Receiver::new();
        let got: Vec<Vec<u8>> = bits.iter().filter_map(|&b| rx.push(b).map(|d| d.frame.to_vec())).collect();
        assert_eq!(got, std::slice::from_ref(&f), "{check}");
    }
    let mut enc = None;
    assert!(matches!(tx_bits(&f, 0, &cfg, &mut enc), TxBits::Plain(_)));
    let long = [f.as_slice(), &[b'x'; 130]].concat(); // fits 239 information bytes, not 191
    assert!(matches!(tx_bits(&long, 16, &cfg, &mut enc), TxBits::Fx25(_)));
    assert!(matches!(tx_bits(&long, 64, &cfg, &mut enc), TxBits::Plain(_)), "too long for 64: plain");
}

/// APRS-44: an FX.25 transmission with 8 wrong bytes in its block (16 check bytes) through the decoder slot: no
/// plain frame, one block for the APRS task, which corrects it to the frame (not a duplicate).
#[test]
fn receive_corrects_a_damaged_block() {
    let f = frame();
    let mut enc = Encoder::new(&f, 16).unwrap();
    for e in 0..8 {
        enc.block_mut()[3 + e * 9] ^= 0xA5; // inside the AX.25 frame: the plain copy is lost
    }
    let cfg = uafsk::TxConfig { rate_hz: RATE_HZ, ..afsk_tx(300) };
    let mut audio = vec![0i16; 1200];
    audio.extend(uafsk::Modulator::from_bits(enc.bits(cfg.preamble, cfg.postamble), cfg));
    audio.extend([0i16; 2400]);
    let mut slot = Decoder::afsk();
    let (mut plain, mut fixed) = (0, Vec::new());
    for half in audio.chunks(24) {
        slot.push(half, |x| match x {
            Found::Frame(_) => plain += 1,
            Found::Fx25(b) => {
                let mut b = *b;
                if let Some(d) = b.decode() {
                    fixed.push((d.frame.to_vec(), d.corrected, d.check, d.duplicate));
                }
            }
            _ => {}
        });
    }
    assert_eq!(plain, 0);
    assert_eq!(fixed, [(f, 8, 16, false)]);
}

/// APRS-45 against Dire Wolf (a black box: `atest -d x`): the DSP task's samples (48 kS/s, `afsk_tx(300)`) for
/// the beacon with 16, 32 and 64 check bytes, then plain, to `target/fx25_ferrite.wav`. Measured Oct 21, 2026
/// (Dire Wolf 1.7): all four decoded, the first three as FX.25 (tags 0x02, 0x06, 0x0A) with no errors.
/// `cargo test -p ferrite-core --test fx25_path -- --ignored`, then `atest -d x target/fx25_ferrite.wav`.
#[test]
#[ignore]
fn write_wav_for_dire_wolf() {
    use ferrite_core::audio::DAC_HZ;
    let f = frame();
    let cfg = afsk_tx(300);
    let mut s: Vec<i16> = Vec::new();
    for check in [16u8, 32, 64, 0] {
        s.extend(std::iter::repeat_n(0, DAC_HZ as usize / 2));
        let mut enc = None;
        s.extend(uafsk::Modulator::from_bits(tx_bits(&f, check, &cfg, &mut enc), cfg));
    }
    s.extend(std::iter::repeat_n(0, DAC_HZ as usize / 2));
    let n = (s.len() * 2) as u32;
    let mut d = Vec::new();
    d.extend_from_slice(b"RIFF");
    d.extend_from_slice(&(36 + n).to_le_bytes());
    d.extend_from_slice(b"WAVEfmt ");
    d.extend_from_slice(&16u32.to_le_bytes());
    d.extend_from_slice(&[1, 0, 1, 0]); // PCM, mono
    d.extend_from_slice(&DAC_HZ.to_le_bytes());
    d.extend_from_slice(&(DAC_HZ * 2).to_le_bytes());
    d.extend_from_slice(&[2, 0, 16, 0]); // 16-bit
    d.extend_from_slice(b"data");
    d.extend_from_slice(&n.to_le_bytes());
    s.iter().for_each(|x| d.extend_from_slice(&x.to_le_bytes()));
    std::fs::write(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fx25_ferrite.wav"), d).unwrap();
}
