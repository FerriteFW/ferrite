//! A picture as SSTV audio, for bench tests: `encode VIS IN.ppm|bars OUT.wav [RATE]` (a 320-wide binary PPM, as
//! tall as the mode's lines or taller; `bars` a test pattern). 16-bit mono WAV, 48 kHz unless RATE says.
use std::io::Write;
use usstv::{Encoder, Mode, Picture};

struct Img(Vec<u8>, usize, usize);
impl Picture for Img {
    fn rgb(&mut self, x: usize, y: usize) -> (u8, u8, u8) {
        let (x, y) = (x.min(self.1 - 1), y.min(self.2 - 1));
        let i = 3 * (y * self.1 + x);
        (self.0[i], self.0[i + 1], self.0[i + 2])
    }
}

fn ppm(path: &str) -> Img {
    let b = std::fs::read(path).expect("read the PPM");
    let mut parts = Vec::new();
    let mut i = 0;
    while parts.len() < 4 {
        while b[i].is_ascii_whitespace() {
            i += 1;
        }
        if b[i] == b'#' {
            while b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        let s = i;
        while !b[i].is_ascii_whitespace() {
            i += 1;
        }
        parts.push(String::from_utf8_lossy(&b[s..i]).to_string());
    }
    assert_eq!(parts[0], "P6", "a binary PPM");
    let (w, h): (usize, usize) = (parts[1].parse().unwrap(), parts[2].parse().unwrap());
    Img(b[i + 1..i + 1 + 3 * w * h].to_vec(), w, h)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vis: u8 = a[1].parse().unwrap();
    let mode = Mode::by_vis(vis).expect("a VIS code: 8, 44, 60, 99, 95");
    let mut img = if a[2] == "bars" {
        let mut v = Vec::new();
        for y in 0..256 {
            for x in 0..320 {
                let bars = [(255, 255, 255), (255, 255, 0), (0, 255, 255), (0, 255, 0), (255, 0, 255), (255, 0, 0), (0, 0, 255), (0, 0, 0)];
                let p = if y < 128 { bars[x * 8 / 320] } else { ((x * 255 / 319) as u8, (y * 255 / 255) as u8, 128) };
                v.extend([p.0, p.1, p.2]);
            }
        }
        Img(v, 320, 256)
    } else {
        ppm(&a[2])
    };
    let rate: u32 = a.get(4).map_or(48_000, |r| r.parse().unwrap());
    let mut e = Encoder::new(mode, rate, 20_000);
    let mut pcm = Vec::new();
    while let Some(s) = e.next(&mut img) {
        pcm.extend(s.to_le_bytes());
    }
    let mut f = std::fs::File::create(&a[3]).unwrap();
    let n = pcm.len() as u32;
    f.write_all(b"RIFF").unwrap();
    f.write_all(&(36 + n).to_le_bytes()).unwrap();
    f.write_all(b"WAVEfmt ").unwrap();
    f.write_all(&[16, 0, 0, 0, 1, 0, 1, 0]).unwrap();
    f.write_all(&rate.to_le_bytes()).unwrap();
    f.write_all(&(rate * 2).to_le_bytes()).unwrap();
    f.write_all(&[2, 0, 16, 0]).unwrap();
    f.write_all(b"data").unwrap();
    f.write_all(&n.to_le_bytes()).unwrap();
    f.write_all(&pcm).unwrap();
    eprintln!("{}: {} {:.1} s", a[3], mode.name, n as f32 / 2.0 / rate as f32);
}
