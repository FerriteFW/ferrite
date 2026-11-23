//! SSTV pictures in the SPI flash (SSTV-4, decisions 403 and 404): four slots past the bootloader's fonts, one to
//! send (Console writes it) and three received; each a header sector, then 320 x 256 RGB565 rows. Shared by the radio, the simulator and the host
//! tests, as Console's reader and writer follow the same layout.

use crate::layout::SSTV;

/// Slots, and the first received one (decision 404: one to send, three received, past the bootloader's fonts).
pub const SLOTS: u8 = 4;
pub const FIRST_RX: u8 = 1;
/// One slot: a header sector and 40 of pixels.
pub const SLOT_LEN: u32 = 41 * 4096;
pub const WIDTH: usize = 320;
pub const LINES: usize = 256;
pub const ROW_LEN: usize = WIDTH * 2;
/// The CRC-32 (ISO-HDLC, as the link's `DATA_READ` CRC and the layout header) of an erased slot: 41 sectors of 0xFF.
pub const ERASED_CRC: u32 = 0x56FC_74B1;
/// Where a slot's pixels start, from its header.
pub const PIXELS: u32 = 4096;
pub const HEADER_LEN: usize = 64;
const MAGIC: [u8; 4] = *b"SSTV";
const FORMAT: u8 = 1;

pub const KIND_RX: u8 = 1;
pub const KIND_SEND: u8 = 2;

const _: () = assert!(SSTV.start + SLOTS as u32 * SLOT_LEN <= SSTV.end);

pub const fn slot_addr(slot: u8) -> u32 {
    SSTV.start + slot as u32 * SLOT_LEN
}

/// Row `y` of a slot's picture.
pub const fn row_addr(slot: u8, y: u16) -> u32 {
    slot_addr(slot) + PIXELS + y as u32 * ROW_LEN as u32
}

/// A picture's header (SSTV-4.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub kind: u8,
    /// The VIS code it came with (0: not received).
    pub vis: u8,
    pub complete: bool,
    /// Lines in it; None while it's being received.
    pub lines: Option<u16>,
    pub utc: u32,
    /// 10 Hz units; 0 none.
    pub freq: u32,
    pub name: [u8; 16],
    pub seq: u32,
}

impl Header {
    /// A received picture's header as it starts (lines and flags programmed at the end: [`Header::end`]).
    pub fn receiving(vis: u8, utc: u32, freq: u32, seq: u32) -> Header {
        Header { kind: KIND_RX, vis, complete: false, lines: None, utc, freq, name: [0; 16], seq }
    }

    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut b = [0xFFu8; HEADER_LEN];
        b[0..4].copy_from_slice(&MAGIC);
        b[4] = FORMAT;
        b[5] = self.kind;
        b[6] = self.vis;
        b[7] = if self.complete { 0xFE } else { 0xFF };
        b[8..10].copy_from_slice(&(WIDTH as u16).to_le_bytes());
        b[10..12].copy_from_slice(&self.lines.unwrap_or(0xFFFF).to_le_bytes());
        b[12..16].copy_from_slice(&self.utc.to_le_bytes());
        b[16..20].copy_from_slice(&self.freq.to_le_bytes());
        b[20..36].copy_from_slice(&self.name);
        b[36..40].copy_from_slice(&self.seq.to_le_bytes());
        b
    }

    /// None for an empty slot (no magic) or another format.
    pub fn decode(b: &[u8; HEADER_LEN]) -> Option<Header> {
        if b[0..4] != MAGIC || b[4] != FORMAT {
            return None;
        }
        let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let lines = match u16_at(10) {
            0xFFFF => None,
            n => Some(n.min(LINES as u16)),
        };
        let mut name = [0u8; 16];
        name.copy_from_slice(&b[20..36]);
        for c in &mut name {
            if *c == 0xFF {
                *c = 0;
            }
        }
        Some(Header { kind: b[5], vis: b[6], complete: b[7] & 1 == 0, lines, utc: u32_at(12), freq: u32_at(16), name, seq: u32_at(36) })
    }

    /// The bytes programmed into the header when the picture ends (SSTV-4.4): its lines (offset 10) and, if it
    /// came whole, the complete flag (offset 7). NOR: they only clear bits of what was written at the start.
    pub fn end(lines: u16, complete: bool) -> [(u32, u8); 3] {
        let l = lines.to_le_bytes();
        [(7, if complete { 0xFE } else { 0xFF }), (10, l[0]), (11, l[1])]
    }
}

/// The slot the next received picture goes into: an empty received slot, else the oldest (SSTV-4.1); and the
/// sequence number it gets.
pub fn next_rx(headers: &[Option<Header>; SLOTS as usize]) -> (u8, u32) {
    let seq = headers.iter().flatten().filter(|h| h.kind == KIND_RX).map(|h| h.seq).max().map_or(1, |s| s.wrapping_add(1));
    let slot = (FIRST_RX..SLOTS)
        .find(|&s| headers[s as usize].is_none())
        .or_else(|| (FIRST_RX..SLOTS).min_by_key(|&s| headers[s as usize].map_or(0, |h| h.seq)))
        .unwrap_or(FIRST_RX);
    (slot, seq)
}

/// What the DSP task's decoder says, for the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A picture started (its VIS code).
    Start(u8),
    /// Line `y` is in the line ring at [`Event::Line::1`]: (y, ring index).
    Line(u16, u8),
    /// The picture ended: whole, or the signal went.
    End(bool),
}

/// The mode names by VIS code, for the screen.
pub fn mode_name(vis: u8) -> &'static str {
    match vis {
        8 => "Robot 36",
        44 => "Martin M1",
        60 => "Scottie S1",
        99 => "PD90",
        95 => "PD120",
        _ => "",
    }
}

/// A picture to send (SSTV-5): the mode's VIS code, the slot (or [`TEST_CARD`]), the callsign for the test card,
/// and the audio's peak.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Send {
    pub vis: u8,
    pub src: u8,
    pub call: [u8; 10],
    pub amp: i16,
}

/// [`Send::src`] for the test card (SSTV-6).
pub const TEST_CARD: u8 = 0xFF;

/// 5 x 7 glyphs for the test card: digits, capitals, '/' and '-' (a row a byte, bit 4 the left column).
static GLYPHS: [(u8, [u8; 7]); 39] = [
    (b'0', [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E]),
    (b'1', [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E]),
    (b'2', [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F]),
    (b'3', [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E]),
    (b'4', [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02]),
    (b'5', [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E]),
    (b'6', [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E]),
    (b'7', [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08]),
    (b'8', [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E]),
    (b'9', [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C]),
    (b'A', [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11]),
    (b'B', [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E]),
    (b'C', [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E]),
    (b'D', [0x1C, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1C]),
    (b'E', [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F]),
    (b'F', [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10]),
    (b'G', [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F]),
    (b'H', [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11]),
    (b'I', [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E]),
    (b'J', [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C]),
    (b'K', [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11]),
    (b'L', [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F]),
    (b'M', [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11]),
    (b'N', [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11]),
    (b'O', [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E]),
    (b'P', [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10]),
    (b'Q', [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D]),
    (b'R', [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11]),
    (b'S', [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E]),
    (b'T', [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04]),
    (b'U', [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E]),
    (b'V', [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04]),
    (b'W', [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A]),
    (b'X', [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11]),
    (b'Y', [0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04]),
    (b'Z', [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F]),
    (b'/', [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10]),
    (b'-', [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00]),
    (b' ', [0; 7]),
];

fn glyph(c: u8) -> [u8; 7] {
    let c = c.to_ascii_uppercase();
    GLYPHS.iter().find(|g| g.0 == c).map_or([0; 7], |g| g.1)
}

/// Whether text at (x0, y0), `scale` pixels a dot (6 dots a character), covers (x, y).
fn text_at(text: &[u8], x0: i32, y0: i32, scale: i32, x: i32, y: i32) -> bool {
    let (dx, dy) = (x - x0, y - y0);
    if dx < 0 || dy < 0 || dy >= 7 * scale {
        return false;
    }
    let (ci, col) = ((dx / scale) / 6, (dx / scale) % 6);
    if ci as usize >= text.len() || col == 5 {
        return false;
    }
    glyph(text[ci as usize])[(dy / scale) as usize] >> (4 - col) & 1 != 0
}

/// The test card (SSTV-6), 320 x 256: colour bars over the top, the callsign large in white on black, "FERRITE"
/// and a grey ramp under it. `call` is 0-padded ("NO CALL" when empty).
/// The test card's tables: named statics, as the DSP task reads them for every sample (slowcheck).
static BARS: [(u8, u8, u8); 8] = [(255, 255, 255), (255, 255, 0), (0, 255, 255), (0, 255, 0), (255, 0, 255), (255, 0, 0), (0, 0, 255), (0, 0, 0)];
static NO_CALL: [u8; 7] = *b"NO CALL";
static FERRITE: [u8; 7] = *b"FERRITE";

pub fn test_card(x: usize, y: usize, call: &[u8]) -> (u8, u8, u8) {
    let (xi, yi) = (x as i32, y as i32);
    match y {
        0..=63 => BARS[x * 8 / WIDTH],
        64..=191 => {
            let n = call.iter().position(|&c| c == 0).unwrap_or(call.len());
            let call: &[u8] = if n == 0 { &NO_CALL } else { &call[..n] };
            // as big as fits: 6 dots a character at up to 6 pixels a dot
            let scale = (WIDTH as i32 - 16) / (6 * call.len() as i32).max(1);
            let scale = scale.clamp(2, 6);
            let w = call.len() as i32 * 6 * scale - scale;
            if text_at(call, (WIDTH as i32 - w) / 2, 80, scale, xi, yi) {
                return (255, 255, 255);
            }
            let f = &FERRITE;
            if text_at(f, (WIDTH as i32 - (f.len() as i32 * 18 - 3)) / 2, 160, 3, xi, yi) {
                return (255, 140, 0);
            }
            (0, 0, 0)
        }
        _ => {
            let v = (x * 255 / (WIDTH - 1)) as u8;
            (v, v, v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sstv_4_3_header_round_trip() {
        let mut h = Header::receiving(44, 1_700_000_000, 14_580_000, 7);
        h.name[..4].copy_from_slice(b"ISS!");
        let b = h.encode();
        assert_eq!(&b[0..4], b"SSTV");
        assert_eq!(b[7], 0xFF);
        assert_eq!(&b[10..12], &[0xFF, 0xFF]);
        assert_eq!(Header::decode(&b), Some(h));
        // the end, programmed over it
        let mut e = b;
        for (at, v) in Header::end(256, true) {
            e[at as usize] &= v;
        }
        let d = Header::decode(&e).unwrap();
        assert_eq!((d.lines, d.complete), (Some(256), true));
        assert_eq!(Header::decode(&[0xFF; HEADER_LEN]), None, "an erased slot is empty");
    }

    #[test]
    fn sstv_4_1_next_slot() {
        let mut hs = [None; SLOTS as usize];
        hs[0] = Some(Header { kind: KIND_SEND, ..Header::receiving(0, 0, 0, 0) });
        assert_eq!(next_rx(&hs), (1, 1));
        for (s, q) in [(1, 5), (2, 9), (3, 3)] {
            hs[s] = Some(Header::receiving(8, 0, 0, q));
        }
        assert_eq!(next_rx(&hs), (3, 10), "the oldest, the next number");
        hs[2] = None;
        assert_eq!(next_rx(&hs).0, 2, "an empty one first");
    }

    #[test]
    fn sstv_6_test_card() {
        let call = *b"VA7XYZ\0\0\0\0";
        assert_eq!(test_card(0, 0, &call), (255, 255, 255));
        assert_eq!(test_card(319, 10, &call), (0, 0, 0));
        // some of the callsign is white in the middle band
        let lit = (64..192).flat_map(|y| (0..WIDTH).map(move |x| (x, y))).filter(|&(x, y)| test_card(x, y, &call) == (255, 255, 255)).count();
        assert!(lit > 2000, "{lit}");
        assert_eq!(test_card(0, 255, &call), (0, 0, 0));
        assert_eq!(test_card(319, 255, &call), (255, 255, 255));
        let empty = [0u8; 10];
        assert!((64..192).any(|y| (0..WIDTH).any(|x| test_card(x, y, &empty) == (255, 255, 255))), "NO CALL");
    }

    #[test]
    fn sstv_4_4_erased_crc() {
        let mut c = 0xFFFF_FFFFu32;
        for _ in 0..SLOT_LEN {
            c ^= 0xFF;
            for _ in 0..8 {
                c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
            }
        }
        assert_eq!(!c, ERASED_CRC);
    }

    #[test]
    fn sstv_4_slots_fit() {
        // decision 404: past the bootloader's fonts, inside the SSTV region
        assert_eq!(slot_addr(0), crate::layout::BOOT_FONTS.end);
        assert!(slot_addr(SLOTS - 1) + SLOT_LEN <= SSTV.end);
        assert_eq!(row_addr(1, 255) + ROW_LEN as u32, slot_addr(2));
        for s in 0..SLOTS {
            assert!(crate::layout::writable(slot_addr(s), SLOT_LEN), "slot {s}");
        }
    }
}
