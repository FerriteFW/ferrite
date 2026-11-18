//! Every mode through the encoder and back through the decoder: clean, with noise, with the sender's clock off
//! (slant), and with the mode forced (no VIS).

use usstv::color::from565;
use usstv::mode::{Mode, MODES, WIDTH};
use usstv::{Decoder, Encoder, Picture, Sink};

/// A test card: colour bars on top, a grey ramp, a diagonal and a colour gradient below.
struct Card {
    h: usize,
}

impl Card {
    fn px(&self, x: usize, y: usize) -> (u8, u8, u8) {
        let bars = [(255, 255, 255), (255, 255, 0), (0, 255, 255), (0, 255, 0), (255, 0, 255), (255, 0, 0), (0, 0, 255), (0, 0, 0)];
        if y < self.h / 3 {
            bars[x * 8 / WIDTH]
        } else if y < self.h / 2 {
            let v = (x * 255 / (WIDTH - 1)) as u8;
            (v, v, v)
        } else {
            ((x * 255 / WIDTH) as u8, (y * 255 / self.h) as u8, 128)
        }
    }
}

impl Picture for Card {
    fn rgb(&mut self, x: usize, y: usize) -> (u8, u8, u8) {
        self.px(x, y)
    }
}

#[derive(Default)]
struct Got {
    mode: Option<&'static str>,
    lines: Vec<(u16, Vec<u16>)>,
    end: Option<bool>,
}

impl Sink for Got {
    fn start(&mut self, mode: &'static Mode) {
        self.mode = Some(mode.name);
    }
    fn line(&mut self, y: u16, px: &[u16; WIDTH]) {
        self.lines.push((y, px.to_vec()));
    }
    fn end(&mut self, complete: bool) {
        self.end = Some(complete);
    }
}

/// Deterministic noise, roughly Gaussian (sum of four uniforms).
struct Noise(u32);
impl Noise {
    fn next(&mut self) -> f32 {
        let mut s = 0.0;
        for _ in 0..4 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            s += (self.0 >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
        }
        s * 1.73
    }
}

/// The picture's height as sent: PD120 sends 496 lines (two a frame), which the receiver averages into its 248.
fn sent_h(mode: &Mode) -> usize {
    if mode.layout == usstv::mode::Layout::Pd && mode.lines == mode.frames { 2 * mode.lines as usize } else { mode.lines as usize }
}

/// What received line `y` should be: the card's line, or for PD120 the average of the two sent for it.
fn expected(mode: &Mode, card: &Card, x: usize, y: usize) -> (u8, u8, u8) {
    if sent_h(mode) == mode.lines as usize {
        return card.px(x, y);
    }
    let (a, b) = (card.px(x, 2 * y), card.px(x, 2 * y + 1));
    let m = |p: u8, q: u8| ((p as u16 + q as u16) / 2) as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

/// The transmission at `tx_rate` (the sender's clock), heard at 24 kHz: noise of `noise` (rms, of 8000 peak).
fn air(mode: &'static Mode, tx_rate: u32, noise: f32, vis: bool) -> Vec<i16> {
    let mut e = Encoder::new(mode, tx_rate, 8000);
    let mut card = Card { h: sent_h(mode) };
    let mut out = Vec::new();
    let mut nz = Noise(7);
    let mut push = |s: f32, out: &mut Vec<i16>| out.push((s + noise * nz.next()).clamp(-32767.0, 32767.0) as i16);
    for _ in 0..12_000 {
        push(0.0, &mut out);
    }
    let skip = if vis { 0 } else { usstv::mode::VIS_US as u64 * tx_rate as u64 / 1_000_000 };
    let mut k = 0u64;
    while let Some(s) = e.next(&mut card) {
        if k >= skip {
            push(s as f32, &mut out);
        }
        k += 1;
    }
    for _ in 0..12_000 {
        push(0.0, &mut out);
    }
    out
}

/// Mean absolute error per channel against the card, the edge columns left out; and the worst line's.
#[allow(clippy::needless_range_loop)] // x is the card's coordinate too
fn score(mode: &'static Mode, got: &Got) -> (f32, f32) {
    let card = Card { h: sent_h(mode) };
    let edges = [card.h / 3 * mode.lines as usize / card.h, card.h / 2 * mode.lines as usize / card.h];
    let (mut sum, mut n, mut worst) = (0.0, 0.0, 0.0f32);
    for (y, px) in &got.lines {
        let (mut ls, mut ln) = (0.0, 0.0);
        for x in 6..WIDTH - 6 {
            // the boundaries between bars and the bands are a pixel or two soft: leave them out
            if (x * 8) % WIDTH < 8 || x * 8 % WIDTH > WIDTH - 8 {
                continue;
            }
            let yy = *y as usize;
            if edges.iter().any(|&b| yy + 2 >= b && yy < b + 2) {
                continue;
            }
            let (r, g, b) = from565(px[x]);
            let (r0, g0, b0) = expected(mode, &card, x, yy);
            let e = ((r as f32 - r0 as f32).abs() + (g as f32 - g0 as f32).abs() + (b as f32 - b0 as f32).abs()) / 3.0;
            ls += e;
            ln += 1.0;
        }
        if ln > 0.0 {
            worst = worst.max(ls / ln);
            sum += ls;
            n += ln;
        }
    }
    (sum / n, worst)
}

fn run(mode: &'static Mode, tx_rate: u32, noise: f32, vis: bool) -> (Got, f32, f32) {
    let audio = air(mode, tx_rate, noise, vis);
    let mut d = Decoder::new(24_000);
    let mut got = Got::default();
    if vis {
        d.feed(&audio, &mut got);
    } else {
        // forced at the start of the audio: it waits for the first sync
        d.force(mode, &mut got);
        d.feed(&audio, &mut got);
    }
    let (mean, worst) = score(mode, &got);
    (got, mean, worst)
}

#[test]
fn every_mode_clean() {
    for &m in MODES.iter() {
        let (got, mean, worst) = run(m, 24_000, 0.0, true);
        assert_eq!(got.mode, Some(m.name), "VIS");
        assert_eq!(got.end, Some(true), "{}: ended complete", m.name);
        assert_eq!(got.lines.len(), m.lines as usize, "{}: lines", m.name);
        let ys: Vec<u16> = got.lines.iter().map(|l| l.0).collect();
        assert!(ys.windows(2).all(|w| w[1] == w[0] + 1) && ys[0] == 0, "{}: lines in order", m.name);
        println!("{:10} clean: mean error {mean:.1}, worst line {worst:.1}", m.name);
        // Robot 36's colour pixels are 0.14 ms: the discriminator's filters soften them most
        let (mm, mw) = if m.vis == 8 { (9.0, 14.0) } else { (4.0, 7.0) };
        assert!(mean < mm && worst < mw, "{}: mean {mean}, worst {worst}", m.name);
    }
}

#[test]
fn every_mode_noisy() {
    // 8000 peak (5660 rms) against 2000 rms noise: about 9 dB in the 12 kHz band, worse than a fair FM signal
    for &m in MODES.iter() {
        let (got, mean, worst) = run(m, 24_000, 2000.0, true);
        assert_eq!(got.mode, Some(m.name), "VIS");
        assert_eq!(got.lines.len(), m.lines as usize, "{}", m.name);
        println!("{:10} noisy: mean error {mean:.1}, worst line {worst:.1}", m.name);
        let mm = if m.vis == 8 { 42.0 } else { 28.0 };
        assert!(mean < mm, "{}: mean {mean}", m.name);
    }
}

#[test]
fn slant_corrected() {
    // the sender's clock 300 ppm fast and slow: uncorrected, Martin M1's last line would be 34 ms (24 % of a
    // colour scan) out
    for &m in MODES.iter() {
        for ppm in [-300i32, 300] {
            let rate = (24_000i64 * (1_000_000 + ppm as i64) / 1_000_000) as u32;
            let (got, mean, worst) = run(m, rate, 500.0, true);
            assert_eq!(got.lines.len(), m.lines as usize, "{} {ppm}", m.name);
            println!("{:10} {ppm:+} ppm: mean error {mean:.1}, worst line {worst:.1}", m.name);
            let mm = if m.vis == 8 { 14.0 } else { 8.0 };
            assert!(mean < mm && worst < 30.0, "{} {ppm} ppm: mean {mean}, worst {worst}", m.name);
        }
    }
}

#[test]
fn forced_mode_without_vis() {
    for &m in MODES.iter() {
        let (got, mean, _) = run(m, 24_000, 300.0, false);
        // the first frame or two go to finding the sync
        assert!(got.lines.len() + 4 >= m.lines as usize, "{}: {} lines", m.name, got.lines.len());
        println!("{:10} forced: {} lines, mean error {mean:.1}", m.name, got.lines.len());
        assert!(mean < 12.0, "{}: mean {mean}", m.name);
    }
}

/// A receiver whose audio clips hard (black's ADC at 3 kHz deviation, Nov 16, 2026): square waves, their harmonics
/// beside the tones. Clipped at a fifth of the peak.
#[test]
fn clipped_audio() {
    for &m in MODES.iter() {
        let mut audio = air(m, 24_000, 300.0, true);
        for x in &mut audio {
            *x = (*x).clamp(-1600, 1600);
        }
        let mut d = Decoder::new(24_000);
        let mut got = Got::default();
        d.feed(&audio, &mut got);
        let (mean, worst) = score(m, &got);
        println!("{:10} clipped: {} lines, mean error {mean:.1}, worst line {worst:.1}", m.name, got.lines.len());
        assert_eq!(got.lines.len(), m.lines as usize, "{}", m.name);
        assert!(mean < 12.0, "{}: mean {mean}", m.name);
    }
}

#[test]
fn noise_alone_starts_nothing() {
    let mut nz = Noise(99);
    let audio: Vec<i16> = (0..24_000 * 60).map(|_| (3000.0 * nz.next()) as i16).collect();
    let mut d = Decoder::new(24_000);
    let mut got = Got::default();
    d.feed(&audio, &mut got);
    assert_eq!(got.mode, None);
}

#[test]
fn lengths() {
    let secs = |m: &Mode| m.image_us() as f32 / 1e6;
    assert!((secs(&usstv::mode::ROBOT36) - 36.0).abs() < 0.1);
    assert!((secs(&usstv::mode::MARTIN1) - 114.3).abs() < 0.1);
    assert!((secs(&usstv::mode::SCOTTIE1) - 109.6).abs() < 0.2);
    assert!((secs(&usstv::mode::PD90_MODE) - 89.99).abs() < 0.1);
    assert!((secs(&usstv::mode::PD120_MODE) - 126.1).abs() < 0.1);
}

#[test]
#[ignore]
fn survey() {
    for &m in MODES.iter() {
        for (what, rate, noise, vis) in [("clean", 24_000, 0.0, true), ("noisy", 24_000, 2000.0, true), ("+300", 24_007, 500.0, true), ("forced", 24_000, 300.0, false)] {
            let (got, mean, worst) = run(m, rate, noise, vis);
            println!("{:10} {what:6} lines {:3} end {:?} mean {mean:5.1} worst {worst:5.1}", m.name, got.lines.len(), got.end);
        }
    }
}

/// Writes decoded pictures to target/sstv/ (PPM) for a look.
#[test]
#[ignore]
fn pictures() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/sstv");
    std::fs::create_dir_all(&dir).unwrap();
    for (m, rate, noise, name) in [(&usstv::mode::ROBOT36, 24_000, 0.0, "robot36-clean"), (&usstv::mode::PD120_MODE, 24_007, 2000.0, "pd120-noisy-slant"), (&usstv::mode::MARTIN1, 24_000, 1000.0, "martin1")] {
        let (got, _, _) = run(m, rate, noise, true);
        let mut out = format!("P6 {} {} 255\n", WIDTH, got.lines.len()).into_bytes();
        for (_, px) in &got.lines {
            for &c in px {
                let (r, g, b) = from565(c);
                out.extend([r, g, b]);
            }
        }
        std::fs::write(dir.join(format!("{name}.ppm")), out).unwrap();
    }
}

/// SSTV-2.3: PD120's two lines a frame (496 sent) come out as their average (248): alternate black and white lines
/// give a mid grey, not one of the two.
#[test]
fn pd120_averages_its_line_pairs() {
    struct Stripes;
    impl Picture for Stripes {
        fn rgb(&mut self, _x: usize, y: usize) -> (u8, u8, u8) {
            if y.is_multiple_of(2) { (0, 0, 0) } else { (255, 255, 255) }
        }
    }
    let m = &usstv::mode::PD120_MODE;
    let mut e = Encoder::new(m, 24_000, 8000);
    let mut audio = vec![0i16; 12_000];
    while let Some(s) = e.next(&mut Stripes) {
        audio.push(s);
    }
    audio.extend([0i16; 12_000]);
    let mut d = Decoder::new(24_000);
    let mut got = Got::default();
    d.feed(&audio, &mut got);
    assert_eq!((got.mode, got.end, got.lines.len()), (Some("PD120"), Some(true), 248));
    let (_, px) = &got.lines[124];
    let (r, g, b) = from565(px[160]);
    assert!([r, g, b].iter().all(|&v| (100..=155).contains(&v)), "a mid grey: {r} {g} {b}");
}
