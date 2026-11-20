//! Text formatting without `core::fmt`: a fixed buffer that `ufmt::uwrite!` writes into, and the few
//! printf conversions ufmt lacks (decimal widths, `%+d`, `%-5s`). Hex with zero padding is ufmt's `{:04X}`.
use core::convert::Infallible;
use core::mem::MaybeUninit;
use ufmt::{uDisplay, uWrite, Formatter};

/// A fixed buffer for formatted text: what does not fit is cut off, as `snprintf` does.
///
/// `new` leaves the bytes unwritten (zeroing them at every construction cost about 4 KB of flash over the UI's
/// buffers): the invariant is `n <= N` and `b[..n]` written, and only those bytes are ever read.
#[derive(Clone, Copy)]
pub struct Buf<const N: usize> {
    b: [MaybeUninit<u8>; N],
    n: usize,
}

impl<const N: usize> Buf<N> {
    pub const fn new() -> Self {
        Buf { b: [MaybeUninit::uninit(); N], n: 0 }
    }

    pub fn clear(&mut self) {
        self.n = 0;
    }

    pub fn as_bytes(&self) -> &[u8] {
        // SAFETY: n <= N and b[..n] are written (push keeps both), so this is in bounds and initialised.
        unsafe { core::slice::from_raw_parts(self.b.as_ptr().cast::<u8>(), self.n) }
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn push(&mut self, s: &[u8]) {
        push(&mut self.b, &mut self.n, s);
    }

    pub fn set(&mut self, s: &[u8]) {
        self.n = 0;
        self.push(s);
    }
}

/// The one copy of `push` for every size: appends what of s fits after b[..n], keeping n <= b.len().
#[inline(never)]
fn push(b: &mut [MaybeUninit<u8>], n: &mut usize, s: &[u8]) {
    let at = (*n).min(b.len());
    let take = s.len().min(b.len() - at);
    // SAFETY: at + take <= b.len() and take <= s.len(); u8 and MaybeUninit<u8> share a layout; s cannot overlap b
    // (s is shared, b is borrowed mutably).
    unsafe { core::ptr::copy_nonoverlapping(s.as_ptr(), b.as_mut_ptr().add(at).cast::<u8>(), take) };
    *n = at + take;
}

impl<const N: usize> PartialEq for Buf<N> {
    fn eq(&self, o: &Self) -> bool {
        self.as_bytes() == o.as_bytes()
    }
}

impl<const N: usize> Eq for Buf<N> {}

/// The text, for tests and asserts (never formatted on the radio).
impl<const N: usize> core::fmt::Debug for Buf<N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(core::str::from_utf8(self.as_bytes()).unwrap_or("?"))
    }
}

impl<const N: usize> core::hash::Hash for Buf<N> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.as_bytes().hash(h);
    }
}

impl<const N: usize> Default for Buf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Buf<N> {
    /// A buffer holding s (cut off at N).
    pub fn from(s: &[u8]) -> Self {
        let mut b = Self::new();
        b.push(s);
        b
    }
}

impl<const N: usize> uWrite for Buf<N> {
    type Error = Infallible;
    fn write_str(&mut self, s: &str) -> Result<(), Infallible> {
        self.push(s.as_bytes());
        Ok(())
    }
}

/// Formats into a buffer (cleared first) and gives back its bytes: `v!(b, "{} ms", t)`.
#[macro_export]
macro_rules! v {
    ($buf:expr, $($fmt:tt)*) => {{
        $buf.clear();
        let _ = ufmt::uwrite!($buf, $($fmt)*);
        $buf.as_bytes()
    }};
}

fn pad<W: uWrite + ?Sized>(f: &mut Formatter<'_, W>, n: usize, c: char) -> Result<(), W::Error> {
    for _ in 0..n {
        f.write_char(c)?;
    }
    Ok(())
}

fn digits(mut v: u32) -> usize {
    let mut n = 1;
    while v >= 10 {
        v /= 10;
        n += 1;
    }
    n
}

/// `%0Nu`
pub struct Zero(pub u32, pub usize);
impl uDisplay for Zero {
    fn fmt<W: uWrite + ?Sized>(&self, f: &mut Formatter<'_, W>) -> Result<(), W::Error> {
        pad(f, self.1.saturating_sub(digits(self.0)), '0')?;
        self.0.fmt(f)
    }
}

/// A time of day from minutes since midnight (SBAR-2.1): `14:32`, or with 12 h (MENU 0 6 6) `2:32p`:
/// the hour without its zero and `a` or `p`, as long as `14:32` at most (`12:32p`), so it fits where 24 h does.
pub struct Clock(pub u16, pub bool);
impl uDisplay for Clock {
    fn fmt<W: uWrite + ?Sized>(&self, f: &mut Formatter<'_, W>) -> Result<(), W::Error> {
        let (h, m) = ((self.0 / 60 % 24) as u32, (self.0 % 60) as u32);
        if self.1 {
            ((h + 11) % 12 + 1).fmt(f)?;
            f.write_str(":")?;
            Zero(m, 2).fmt(f)?;
            f.write_str(if h < 12 { "a" } else { "p" })
        } else {
            Zero(h, 2).fmt(f)?;
            f.write_str(":")?;
            Zero(m, 2).fmt(f)
        }
    }
}

/// `%Nu`: right-aligned with spaces.
pub struct Wide(pub u32, pub usize);
impl uDisplay for Wide {
    fn fmt<W: uWrite + ?Sized>(&self, f: &mut Formatter<'_, W>) -> Result<(), W::Error> {
        pad(f, self.1.saturating_sub(digits(self.0)), ' ')?;
        self.0.fmt(f)
    }
}

/// `%+d`
pub struct Plus(pub i32);
impl uDisplay for Plus {
    fn fmt<W: uWrite + ?Sized>(&self, f: &mut Formatter<'_, W>) -> Result<(), W::Error> {
        if self.0 >= 0 {
            f.write_char('+')?;
        }
        self.0.fmt(f)
    }
}

/// Bytes that are text in C (`%s` of a char array): printable ASCII as is, anything else as '?'.
pub struct Bytes<'a>(pub &'a [u8]);
impl uDisplay for Bytes<'_> {
    fn fmt<W: uWrite + ?Sized>(&self, f: &mut Formatter<'_, W>) -> Result<(), W::Error> {
        for &c in self.0 {
            f.write_char(if (0x20..0x7F).contains(&c) { c as char } else { '?' })?;
        }
        Ok(())
    }
}

/// `%-Ns`: left-aligned, padded with spaces.
pub struct Left<'a>(pub &'a str, pub usize);
impl uDisplay for Left<'_> {
    fn fmt<W: uWrite + ?Sized>(&self, f: &mut Formatter<'_, W>) -> Result<(), W::Error> {
        f.write_str(self.0)?;
        pad(f, self.1.saturating_sub(self.0.len()), ' ')
    }
}

/// `%Ns`: right-aligned, padded with spaces.
pub struct Right<'a>(pub &'a str, pub usize);
impl uDisplay for Right<'_> {
    fn fmt<W: uWrite + ?Sized>(&self, f: &mut Formatter<'_, W>) -> Result<(), W::Error> {
        pad(f, self.1.saturating_sub(self.0.len()), ' ')?;
        f.write_str(self.0)
    }
}

/// `%c` of a byte (port letters, SI4732 revision characters).
pub struct Ch(pub u8);
impl uDisplay for Ch {
    fn fmt<W: uWrite + ?Sized>(&self, f: &mut Formatter<'_, W>) -> Result<(), W::Error> {
        Bytes(&[self.0]).fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The cases of fw/tests/host/test_fmt.c that the firmware uses, through ufmt.
    #[test]
    fn conversions() {
        let mut b = Buf::<63>::new();
        assert_eq!(v!(b, "key {} {} {}", Wide(7, 2), "matrix", "down"), b"key  7 matrix down");
        assert_eq!(v!(b, "{}:{}:{}", Zero(0, 2), Zero(4, 2), Zero(9, 2)), b"00:04:09");
        // SBAR-2.1: the clock, 24 h and 12 h; 12 h never longer than 24 h's five characters plus one.
        assert_eq!(v!(b, "{}", Clock(14 * 60 + 32, false)), b"14:32");
        assert_eq!(v!(b, "{}", Clock(14 * 60 + 32, true)), b"2:32p");
        assert_eq!(v!(b, "{}", Clock(5, true)), b"12:05a");
        assert_eq!(v!(b, "{}", Clock(12 * 60, true)), b"12:00p");
        assert_eq!(v!(b, "{}", Clock(9 * 60 + 7, true)), b"9:07a");
        assert_eq!(v!(b, "{}", Clock(5, false)), b"00:05");
        assert_eq!(v!(b, "{} {} {}", Plus(12), Plus(-12), Plus(0)), b"+12 -12 +0");
        assert_eq!(v!(b, "{}", i32::MIN), b"-2147483648");
        assert_eq!(v!(b, "{:x} {:04x} {:06X}", 255u32, 255u32, 0xEF4015u32), b"ff 00ff EF4015");
        assert_eq!(v!(b, "{}|{}|", Left("ab", 4), Right("ab", 4)), b"ab  |  ab|");
        assert_eq!(v!(b, "{:08X}", 0x1234_ABCDu32), b"1234ABCD");
        assert_eq!(v!(b, "{}{}", Bytes(b"P\xFFx"), Ch(b'C')), b"P?xC");
        let mut small = Buf::<4>::new();
        assert_eq!(v!(small, "{}", "abcdefgh"), b"abcd"); // truncation
        assert_eq!(v!(small, "{}", 123456), b"1234");
    }

    #[test]
    fn buf_basics() {
        let mut b = Buf::<5>::new();
        assert!(b.is_empty());
        b.push(b"abc");
        b.push(b"defg"); // cut off at 5
        assert_eq!((b.as_bytes(), b.len()), (&b"abcde"[..], 5));
        b.push(b"x"); // full: nothing more
        assert_eq!(b.as_bytes(), b"abcde");
        let c = b; // Copy
        b.set(b"ab");
        assert!(b == Buf::from(b"ab"));
        assert!(b != c);
        assert_eq!(c.as_bytes(), b"abcde");
        b.clear();
        assert!(b == Buf::default());
        assert_eq!(Buf::<0>::from(b"a").as_bytes(), b"");
    }

    /// In test output a buffer shows as its text ('?' when it isn't UTF-8); equal buffers hash alike, whatever
    /// their capacity left unwritten.
    #[test]
    fn debug_and_hash() {
        use core::hash::{Hash, Hasher};
        assert_eq!(std::format!("{:?} {:?}", Buf::<8>::from(b"VA7FER"), Buf::<4>::from(b"\xFFa")), "VA7FER ?");
        let h = |b: &Buf<8>| {
            let mut s = std::collections::hash_map::DefaultHasher::new();
            b.hash(&mut s);
            s.finish()
        };
        let mut a = Buf::<8>::from(b"abcdefgh");
        a.set(b"xy");
        assert_eq!(h(&a), h(&Buf::from(b"xy")));
    }

    /// A writer that fails (a full link buffer, say) stops each conversion at its first write.
    #[test]
    fn a_failing_writer_stops_each_conversion() {
        struct Refuse(usize);
        impl uWrite for Refuse {
            type Error = ();
            fn write_str(&mut self, _: &str) -> Result<(), ()> {
                self.0 += 1;
                Err(())
            }
        }
        let mut w = Refuse(0);
        assert!(ufmt::uwrite!(w, "{}", Zero(5, 3)).is_err());
        assert!(ufmt::uwrite!(w, "{}", Wide(5, 3)).is_err());
        assert!(ufmt::uwrite!(w, "{}", Plus(5)).is_err());
        assert!(ufmt::uwrite!(w, "{}", Bytes(b"ab")).is_err());
        assert!(ufmt::uwrite!(w, "{}", Left("ab", 4)).is_err());
        assert!(ufmt::uwrite!(w, "{}", Right("ab", 4)).is_err());
        assert_eq!(w.0, 6, "one write each, nothing after");
    }
}
