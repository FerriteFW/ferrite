//! Live SSTV decoding for the bench: 16-bit little-endian mono PCM at RATE on stdin, the picture written as it
//! grows to OUT.bmp (320 x 256, re-written each line; a browser page reloading it watches it come), its mode and
//! line in OUT.bmp.txt. `live RATE OUT.bmp [VIS]` (VIS: start that mode at once, no header needed).
use std::io::{Read, Write};
use usstv::{Decoder, Mode, Sink};

struct Out {
    path: String,
    px: Vec<u16>,
    mode: &'static str,
    line: u16,
    state: &'static str,
}

impl Out {
    fn write(&self) {
        let (w, h) = (320usize, 256usize);
        let mut b = Vec::with_capacity(54 + w * h * 3);
        let size = (54 + w * h * 3) as u32;
        b.extend(b"BM");
        b.extend(size.to_le_bytes());
        b.extend([0u8; 4]);
        b.extend(54u32.to_le_bytes());
        b.extend(40u32.to_le_bytes());
        b.extend((w as i32).to_le_bytes());
        b.extend((-(h as i32)).to_le_bytes()); // top-down
        b.extend([1, 0, 24, 0]);
        b.extend([0u8; 24]);
        for &c in &self.px {
            let (r, g, bl) = usstv::color::from565(c);
            b.extend([bl, g, r]);
        }
        let tmp = format!("{}.tmp", self.path);
        std::fs::write(&tmp, &b).unwrap();
        std::fs::rename(&tmp, &self.path).unwrap();
        std::fs::write(format!("{}.txt", self.path), format!("{} {} line {}\n", self.mode, self.state, self.line)).unwrap();
    }
}

impl Sink for Out {
    fn start(&mut self, m: &'static Mode) {
        self.px.fill(0x8410);
        (self.mode, self.line, self.state) = (m.name, 0, "receiving");
        eprintln!("start: {}", m.name);
        self.write();
    }
    fn line(&mut self, y: u16, px: &[u16; 320]) {
        if (y as usize) < 256 {
            self.px[y as usize * 320..y as usize * 320 + 320].copy_from_slice(px);
        }
        self.line = y + 1;
        self.write();
    }
    fn end(&mut self, whole: bool) {
        self.state = if whole { "complete" } else { "signal lost" };
        eprintln!("end: {} at line {}", self.state, self.line);
        self.write();
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let rate: u32 = a[1].parse().unwrap();
    let mut out = Out { path: a[2].clone(), px: vec![0x8410; 320 * 256], mode: "waiting", line: 0, state: "" };
    out.write();
    let mut d = Decoder::new(rate);
    if let Some(v) = a.get(3) {
        d.force(Mode::by_vis(v.parse().unwrap()).unwrap(), &mut out);
    }
    let mut stdin = std::io::stdin().lock();
    let mut buf = vec![0u8; 4800];
    loop {
        let n = stdin.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        let s: Vec<i16> = buf[..n & !1].chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        d.feed(&s, &mut out);
    }
    std::io::stderr().flush().ok();
}
