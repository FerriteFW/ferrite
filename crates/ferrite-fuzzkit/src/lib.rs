//! Deterministic fuzzing in the normal host tests (`cargo nextest run`): coverage-guided fuzzing needs its own
//! runner (`tools/fuzz.sh`, cargo-fuzz), so these tests instead build inputs from the code's own encoders and real
//! recordings, then mutate them (bit flips, truncations, extensions, length fields that lie, boundary values,
//! repeated fields). Every case comes from a fixed seed, so a run is the same every time.
//!
//! - `FERRITE_FUZZ_SCALE=N` (default 1) runs N times the cases, for a long CI run.
//! - `FERRITE_FUZZ_CASE=SEED` runs one case alone (the seed a failure printed).
//! - A failure prints the target, the case's seed and the input as hex, and writes the input to
//!   `target/fuzz-found/TARGET/SEED.hex`. Copy that file into `fuzz/regress/TARGET/` to keep it: every file there
//!   runs first, every time.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A small PRNG (SplitMix64): std only, the same numbers on every machine.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 0 to n - 1 (0 when n is 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }

    /// lo to hi, both included.
    pub fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi.saturating_sub(lo) + 1)
    }

    /// True one time in `n`.
    pub fn one_in(&mut self, n: usize) -> bool {
        self.below(n.max(1)) == 0
    }

    pub fn byte(&mut self) -> u8 {
        self.next() as u8
    }

    pub fn u16(&mut self) -> u16 {
        self.next() as u16
    }

    pub fn u32(&mut self) -> u32 {
        self.next() as u32
    }

    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.byte()).collect()
    }

    pub fn pick<'a, T>(&mut self, s: &'a [T]) -> &'a T {
        &s[self.below(s.len())]
    }

    /// A length up to `max`, mostly short, sometimes right at the limit or around it.
    pub fn len(&mut self, max: usize) -> usize {
        match self.below(8) {
            0 => max,
            1 => max.saturating_sub(1),
            2 => self.below(4).min(max),
            3 | 4 => self.below(max / 4 + 1),
            _ => self.below(max + 1),
        }
    }

    /// A boundary value for a field of `bytes` bytes (1, 2 or 4): 0, 1, the top, the sign edges and their
    /// neighbours, or a power of two.
    pub fn edge(&mut self, bytes: usize) -> u32 {
        let top: u32 = if bytes >= 4 { u32::MAX } else { (1u32 << (bytes * 8)) - 1 };
        let half = top / 2;
        let v = match self.below(9) {
            0 => 0,
            1 => 1,
            2 => top,
            3 => top - 1,
            4 => half,
            5 => half + 1,
            6 => 1u32 << self.below(bytes * 8),
            7 => (1u32 << self.below(bytes * 8)).wrapping_sub(1),
            _ => self.below(16) as u32,
        };
        v & top
    }

    /// Text made of `words`, random tokens, numbers and odd bytes: what a person or a script might type.
    pub fn words(&mut self, words: &[&str], max: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for i in 0..self.range(1, max.max(1)) {
            if i > 0 {
                out.push(*self.pick(b"     \t,=;:"));
            }
            match self.below(10) {
                0..=4 if !words.is_empty() => out.extend_from_slice(self.pick(words).as_bytes()),
                5 => out.extend_from_slice(self.edge(4).to_string().as_bytes()),
                6 => {
                    let n = self.range(1, 6);
                    out.extend((0..n).map(|_| *self.pick(b"0123456789.-+")));
                }
                7 => {
                    let n = self.range(1, 8);
                    out.extend((0..n).map(|_| self.below(95) as u8 + 32));
                }
                8 => out.push(b'"'),
                _ => {
                    let n = self.range(1, 4);
                    out.extend(self.bytes(n));
                }
            }
        }
        out
    }
}

/// How many times the cases (`FERRITE_FUZZ_SCALE`, default 1).
pub fn scale() -> usize {
    std::env::var("FERRITE_FUZZ_SCALE").ok().and_then(|s| s.trim().parse().ok()).unwrap_or(1).max(1)
}

/// A mutation of `data`: one to four of flips, stores, boundary values, length fields that lie, cuts, extensions,
/// repeats, deletions and splices from `corpus`; kept to `max` bytes.
pub fn mutate(r: &mut Rng, data: &mut Vec<u8>, max: usize, corpus: &[Vec<u8>]) {
    for _ in 0..r.range(1, 4) {
        let n = data.len();
        match r.below(14) {
            0 | 1 if n > 0 => {
                let i = r.below(n);
                data[i] ^= 1 << r.below(8);
            }
            2 if n > 0 => {
                let i = r.below(n);
                data[i] = r.byte();
            }
            3 if n > 0 => {
                let i = r.below(n);
                data[i] = *r.pick(&[0, 1, 0x7F, 0x80, 0xFF, 0xC0, 0xDB, 0x7E, 0x0D, 0x0A, b'$', b'*', b',', b' ']);
            }
            4 if n > 1 => {
                // a 16-bit length or index field lying: a boundary, or the data's own length give or take one
                let i = r.below(n - 1);
                let v = match r.below(3) {
                    0 => (n as u32).wrapping_add(r.below(3) as u32).wrapping_sub(1),
                    _ => r.edge(2),
                } as u16;
                data[i..i + 2].copy_from_slice(&v.to_le_bytes());
            }
            5 if n > 3 => {
                let i = r.below(n - 3);
                let v = r.edge(4);
                data[i..i + 4].copy_from_slice(&v.to_le_bytes());
            }
            6 => data.truncate(r.below(n + 1)),
            7 => {
                let k = r.len(64);
                data.extend(r.bytes(k));
            }
            8 if n > 0 => {
                // a field repeated: a range copied in again
                let a = r.below(n);
                let b = (a + r.range(1, 32)).min(n);
                let at = r.below(n + 1);
                let piece = data[a..b].to_vec();
                for _ in 0..r.range(1, 3) {
                    data.splice(at..at, piece.iter().copied());
                }
            }
            9 if n > 0 => {
                let a = r.below(n);
                let b = (a + r.range(1, 16)).min(n);
                data.drain(a..b);
            }
            10 => {
                let at = r.below(n + 1);
                let k = r.range(1, 16);
                let ins = r.bytes(k);
                data.splice(at..at, ins);
            }
            11 if !corpus.is_empty() => {
                let other = r.pick(corpus);
                if !other.is_empty() {
                    let a = r.below(other.len());
                    let b = (a + r.range(1, 64)).min(other.len());
                    let at = r.below(n + 1);
                    let cut = (at + r.below(n - at + 1)).min(n);
                    data.splice(at..cut, other[a..b].iter().copied());
                }
            }
            12 if n > 1 => {
                let (i, j) = (r.below(n), r.below(n));
                data.swap(i, j);
            }
            _ if n > 0 => {
                let i = r.below(n);
                data[i] = if r.one_in(2) { data[i].wrapping_add(1) } else { data[i].wrapping_sub(1) };
            }
            _ => data.push(r.byte()),
        }
    }
    data.truncate(max);
}

/// One fuzz target: its name (the file names of its failures and regressions), base seed and number of cases
/// (times [`scale`]).
pub struct Fuzz {
    name: String,
    seed: u64,
    cases: usize,
    slow: Duration,
}

impl Fuzz {
    pub fn new(name: &str, seed: u64, cases: usize) -> Self {
        Fuzz { name: name.into(), seed, cases: cases * scale(), slow: Duration::from_secs(2) }
    }

    /// A case taking longer than this fails as a hang (default 2 s; the tests run at opt-level 1).
    pub fn slow_ms(mut self, ms: u64) -> Self {
        self.slow = Duration::from_millis(ms);
        self
    }

    /// The regression inputs first (`fuzz/regress/NAME/*.hex`), then the cases: `make` builds an input from the
    /// case's own PRNG, `check` runs it (it panics on a broken invariant). Panics at the end if any failed.
    pub fn run(self, mut make: impl FnMut(&mut Rng) -> Vec<u8>, mut check: impl FnMut(&[u8])) {
        let started = Instant::now();
        let mut failed = Vec::new();
        for (file, input) in regressions(&self.name) {
            if let Err(why) = self.one(&input, &mut check) {
                eprintln!("fuzz {}: regression {} fails again: {why}\n  input ({} bytes): {}", self.name, file, input.len(), hex(&input));
                failed.push(file);
            }
        }
        let only: Option<u64> = std::env::var("FERRITE_FUZZ_CASE").ok().and_then(|s| {
            let s = s.trim();
            s.strip_prefix("0x").map_or_else(|| s.parse().ok(), |h| u64::from_str_radix(h, 16).ok())
        });
        let mut base = Rng::new(self.seed);
        for i in 0..self.cases {
            let case = base.next();
            if only.is_some_and(|o| o != case) {
                continue;
            }
            let mut r = Rng::new(case);
            let input = make(&mut r);
            if let Err(why) = self.one(&input, &mut check) {
                let saved = save(&self.name, case, &input);
                eprintln!(
                    "fuzz {}: case {i} failed: {why}\n  seed {:#x}, case {case:#x} (FERRITE_FUZZ_CASE={case:#x} runs it alone)\n  input ({} bytes): {}\n  saved: {} (copy into fuzz/regress/{}/ to keep it)",
                    self.name,
                    self.seed,
                    input.len(),
                    hex(&input),
                    saved.map_or_else(|| "not saved".into(), |p| p.display().to_string()),
                    self.name
                );
                failed.push(format!("{case:#x}"));
                if failed.len() >= 3 {
                    break;
                }
            }
        }
        eprintln!("fuzz {}: {} cases in {} ms", self.name, self.cases, started.elapsed().as_millis());
        assert!(failed.is_empty(), "fuzz {}: {} failed: {failed:?} (inputs above)", self.name, failed.len());
    }

    fn one(&self, input: &[u8], check: &mut impl FnMut(&[u8])) -> Result<(), String> {
        let t = Instant::now();
        let r = catch_unwind(AssertUnwindSafe(|| check(input)));
        let took = t.elapsed();
        match r {
            Err(e) => Err(e
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| e.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".into())),
            Ok(()) if took > self.slow => Err(format!("took {} ms (limit {} ms)", took.as_millis(), self.slow.as_millis())),
            Ok(()) => Ok(()),
        }
    }
}

/// The workspace (`ferrite/`).
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The decoders' recordings (`../testdata`, the private testdata repo), when it is there: the seeds that need it
/// are skipped without it (CI).
pub fn testdata() -> Option<PathBuf> {
    let p = root().join("../testdata");
    p.is_dir().then_some(p)
}

fn regressions(name: &str) -> Vec<(String, Vec<u8>)> {
    let dir = root().join("fuzz/regress").join(name);
    let mut files: Vec<_> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "hex")).collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            let body: String = text.lines().filter(|l| !l.trim_start().starts_with('#')).collect();
            let input = unhex(&body).unwrap_or_else(|| panic!("{}: not hex", p.display()));
            (p.file_name().unwrap().to_string_lossy().into_owned(), input)
        })
        .collect()
}

fn save(name: &str, case: u64, input: &[u8]) -> Option<PathBuf> {
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root().join("target"), PathBuf::from);
    let dir = target.join("fuzz-found").join(name);
    std::fs::create_dir_all(&dir).ok()?;
    let p = dir.join(format!("{case:016x}.hex"));
    std::fs::write(&p, format!("# fuzz {name}, case {case:#x}\n{}\n", hex(input))).ok()?;
    Some(p)
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Hex digits to bytes (whitespace ignored); None if it isn't hex.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    let s: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    if s.len() % 2 != 0 {
        return None;
    }
    s.chunks(2).map(|c| u8::from_str_radix(std::str::from_utf8(c).ok()?, 16).ok()).collect()
}

/// One hex-encoded line a line (`#` comments and blank lines skipped), as the testdata's `.hex` files.
pub fn hex_lines(path: &Path) -> Vec<Vec<u8>> {
    std::fs::read_to_string(path).map(|t| t.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')).filter_map(unhex).collect()).unwrap_or_default()
}

/// A 16-bit PCM WAV file's samples (the first channel) and rate.
pub fn read_wav(path: &Path) -> Option<(Vec<i16>, u32)> {
    let d = std::fs::read(path).ok()?;
    let (mut rate, mut chans, mut data) = (0, 1usize, Vec::new());
    let mut i = 12;
    while i + 8 <= d.len() {
        let len = u32::from_le_bytes(d[i + 4..i + 8].try_into().ok()?) as usize;
        let body = &d[i + 8..(i + 8 + len).min(d.len())];
        match &d[i..i + 4] {
            b"fmt " if body.len() >= 16 => {
                chans = u16::from_le_bytes([body[2], body[3]]).max(1) as usize;
                rate = u32::from_le_bytes(body[4..8].try_into().ok()?);
            }
            b"data" => data = body.chunks_exact(2 * chans).map(|c| i16::from_le_bytes([c[0], c[1]])).collect(),
            _ => {}
        }
        i += 8 + len + (len & 1);
    }
    (rate > 0).then_some((data, rate))
}

/// Samples as little-endian bytes (an audio target's input) and back.
pub fn samples_to_bytes(s: &[i16]) -> Vec<u8> {
    s.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn bytes_to_samples(b: &[u8]) -> Vec<i16> {
    b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_numbers_every_time() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        assert!((0..100).all(|_| a.next() == b.next()));
        assert_eq!(unhex(&hex(&[0, 1, 0xAB, 0xFF])), Some(vec![0, 1, 0xAB, 0xFF]));
    }

    #[test]
    fn mutations_stay_in_bounds() {
        let mut r = Rng::new(1);
        let corpus = vec![vec![1u8, 2, 3], vec![]];
        for _ in 0..10_000 {
            let n = r.below(40);
            let mut d = r.bytes(n);
            mutate(&mut r, &mut d, 50, &corpus);
            assert!(d.len() <= 50);
        }
    }

    #[test]
    fn a_failure_is_reported() {
        let r = catch_unwind(|| Fuzz::new("fuzzkit_self", 1, 20).run(|r| r.bytes(4), |b| assert!(b[0] != 0 || b[1] != 0 || b[2] < 255)));
        // 1 in 65536 cases fails at most; with 20 the run passes: the point is that a panic is caught, not fatal
        assert!(r.is_ok());
        let r = catch_unwind(|| Fuzz::new("fuzzkit_self", 1, 5).run(|r| r.bytes(1), |_| panic!("always")));
        assert!(r.is_err());
    }
}
