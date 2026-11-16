//! Colour: BT.601 studio swing (Y 16-235, Cr and Cb around 128), the conversion MMSSTV and QSSTV use for Robot
//! and PD; RGB565 for the screen and storage.

use crate::math::clamp_u8;

pub fn to_ycc(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let (r, g, b) = (r as f32, g as f32, b as f32);
    let y = 16.0 + (65.738 * r + 129.057 * g + 25.064 * b) / 256.0;
    let cr = 128.0 + (112.439 * r - 94.154 * g - 18.285 * b) / 256.0;
    let cb = 128.0 + (-37.945 * r - 74.494 * g + 112.439 * b) / 256.0;
    (clamp_u8(y), clamp_u8(cr), clamp_u8(cb))
}

pub fn to_rgb(y: u8, cr: u8, cb: u8) -> (u8, u8, u8) {
    let (y, cr, cb) = (y as f32 - 16.0, cr as f32 - 128.0, cb as f32 - 128.0);
    let r = (298.082 * y + 408.583 * cr) / 256.0;
    let g = (298.082 * y - 100.291 * cb - 208.120 * cr) / 256.0;
    let b = (298.082 * y + 516.411 * cb) / 256.0;
    (clamp_u8(r), clamp_u8(g), clamp_u8(b))
}

pub const fn rgb565(r: u8, g: u8, b: u8) -> u16 {
    (r as u16 >> 3) << 11 | (g as u16 >> 2) << 5 | b as u16 >> 3
}

pub const fn from565(c: u16) -> (u8, u8, u8) {
    let (r, g, b) = ((c >> 11) as u8, (c >> 5 & 0x3F) as u8, (c & 0x1F) as u8);
    (r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for &(r, g, b) in &[(0, 0, 0), (255, 255, 255), (255, 0, 0), (0, 255, 0), (0, 0, 255), (120, 60, 200)] {
            let (y, cr, cb) = to_ycc(r, g, b);
            let (r2, g2, b2) = to_rgb(y, cr, cb);
            for (a, b) in [(r, r2), (g, g2), (b, b2)] {
                assert!((a as i32 - b as i32).abs() <= 3, "{r} {g} {b} -> {r2} {g2} {b2}");
            }
        }
        assert_eq!(from565(rgb565(255, 255, 255)), (255, 255, 255));
    }
}
