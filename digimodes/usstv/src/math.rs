//! Small f32 helpers (no libm): a sine table for the transmitter, sine and cosine for set-up, atan2 for the
//! receiver's discriminator.

use core::f32::consts::PI;

/// sin over one turn, 256 steps and the first again (for interpolation), full scale 32767.
pub static SIN: [i16; 257] = sin_table();

const fn sin_table() -> [i16; 257] {
    let mut t = [0i16; 257];
    let mut i = 0;
    while i < 257 {
        let x = (i % 256) as f64 * core::f64::consts::TAU / 256.0;
        t[i] = (sin64(x) * 32767.0) as i16;
        i += 1;
    }
    t
}

/// sin for any x, by reduction to [-pi, pi] and a series (const: the table above).
const fn sin64(mut x: f64) -> f64 {
    let tau = core::f64::consts::TAU;
    while x > core::f64::consts::PI {
        x -= tau;
    }
    while x < -core::f64::consts::PI {
        x += tau;
    }
    let x2 = x * x;
    let mut term = x;
    let mut sum = x;
    let mut n = 1;
    while n < 12 {
        term = -term * x2 / ((2 * n) as f64 * (2 * n + 1) as f64);
        sum += term;
        n += 1;
    }
    sum
}

pub fn sin(x: f32) -> f32 {
    sin64(x as f64) as f32
}

pub fn cos(x: f32) -> f32 {
    sin64(x as f64 + core::f64::consts::FRAC_PI_2) as f32
}

/// The table at a 32-bit phase, linearly interpolated.
#[inline]
pub fn sin_at(phase: u32) -> i32 {
    let i = (phase >> 24) as usize;
    let f = ((phase >> 8) & 0xFFFF) as i32;
    let (a, b) = (SIN[i] as i32, SIN[i + 1] as i32);
    a + (((b - a) * f) >> 16)
}

/// atan2 to about 1e-5 rad.
pub fn atan2(y: f32, x: f32) -> f32 {
    let (ax, ay) = (abs(x), abs(y));
    if ax == 0.0 && ay == 0.0 {
        return 0.0;
    }
    let swap = ay > ax;
    let z = if swap { ax / ay } else { ay / ax };
    let z2 = z * z;
    let mut a = z * (0.999_977_26 + z2 * (-0.332_623_47 + z2 * (0.193_543_46 + z2 * (-0.116_432_87 + z2 * (0.052_653_32 + z2 * -0.011_721_2)))));
    if swap {
        a = PI / 2.0 - a;
    }
    if x < 0.0 {
        a = PI - a;
    }
    if y < 0.0 { -a } else { a }
}

#[inline]
pub fn abs(x: f32) -> f32 {
    if x < 0.0 { -x } else { x }
}

#[inline]
pub fn clamp_u8(x: f32) -> u8 {
    if x <= 0.0 {
        0
    } else if x >= 255.0 {
        255
    } else {
        (x + 0.5) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atan2_matches() {
        for i in 0..360 {
            let a = (i as f32 - 179.5) * PI / 180.0;
            let got = atan2(sin(a) * 3.0, cos(a) * 3.0);
            assert!(abs(got - a) < 2e-5, "{a} {got}");
        }
    }

    #[test]
    fn table() {
        assert_eq!(SIN[0], 0);
        assert_eq!(SIN[64], 32767);
        assert!((SIN[192] + 32767).abs() <= 1);
        assert!((sin_at(1 << 29) - 23170).abs() < 40); // 45 degrees
    }
}
