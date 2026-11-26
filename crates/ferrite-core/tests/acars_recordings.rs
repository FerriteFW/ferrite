//! ACARS-11 on recordings (ferrite#192): the decoders' audio from the radio (`audio --raw`, 12 or 16 kHz) brought to
//! the DSP task's 24 kHz and through [`ferrite_core::acarsrx::Demod`]; every frame must parse, and the files' names
//! say how many good frames each must give at least (`...-N.wav`). The test data's `acars/` (PROVENANCE.md); a
//! missing folder skips.

use ferrite_core::acars::parse;
use ferrite_core::acarsrx::Demod;
use ferrite_fuzzkit::{read_wav, testdata_private_file};

/// Linear interpolation to `to` Hz.
fn resample(x: &[i16], from: u32, to: u32) -> Vec<i16> {
    let n = x.len() as u64 * to as u64 / from as u64;
    (0..n)
        .map(|i| {
            let p = i as f64 * from as f64 / to as f64;
            let (k, f) = (p as usize, p.fract());
            let a = x[k] as f64;
            let b = *x.get(k + 1).unwrap_or(&x[k]) as f64;
            (a + (b - a) * f) as i16
        })
        .collect()
}

/// ACARS-11: each recording gives at least the good frames its name says, and nothing that doesn't parse.
#[test]
fn recordings_decode() {
    let Some(dir) = testdata_private_file("acars") else { return };
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "wav")).collect();
    files.sort();
    assert!(!files.is_empty());
    for p in files {
        let name = p.file_stem().unwrap().to_string_lossy().to_string();
        let want: usize = name.rsplit('-').next().and_then(|n| n.parse().ok()).expect("name ends in -N");
        let (x, rate) = read_wav(&p).unwrap();
        let x = resample(&x, rate, 24_000);
        let mut d = Demod::new(24_000);
        let (mut good, mut bad) = (0, 0);
        d.feed(&x, |f| match parse(f) {
            Ok(fr) if fr.crc_ok => good += 1,
            Ok(_) => bad += 1,
            Err(e) => panic!("{name}: a frame that doesn't parse: {e:?}"),
        });
        eprintln!("{name}: {good} good, {bad} bad CRC");
        assert!(good >= want, "{name}: {good} good frames, want {want}");
    }
}
