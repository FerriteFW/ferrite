//! The fixed-size records at fixed addresses (`docs/firmware-design.md`, "Records"): channel n, zone z and range
//! preset p each have one place, computed from the number. Channels are 64 B (63 per sector), zones 32 B (127
//! per sector), presets 224 B (16 in one sector); the last bytes of every sector stay free, for the scratch
//! copy's footer ([`crate::scratch`]). Each record carries a CRC; an erased or torn record reads as empty.
//! Fields the UI doesn't model yet (a channel's position, coverage, service type, PTT ID option; a preset's skip
//! list) are kept as they were when a record is rewritten.

use crate::{crc16, u16_at, u32_at, SECTOR};
use ferrite_app::chan::{Channel, Name, ZoneId, NO_ZONE};
use ferrite_app::data::{Dtmf, Pos, RangePreset, ZoneCfg, DTMF_MEMS};
use ferrite_app::slot::{band_near, SlotCfg, BANDS, DEFAULT_CFG, SQ_GLOBAL};
use ferrite_core::layout::{CHANNELS, PRESETS, ZONES};
use ferrite_ui::text::Buf;

// ---- channels ----

pub const CH_LEN: usize = 64;
/// The record's format, at offset 60. 2 (decision 333): the zone is 2 bytes, its low byte at 18 and its high byte
/// at 61 (0xFFFF no zone); version 1 had one byte at 18 (255 no zone) and 61 spare (0xFF).
pub const CH_VERSION: u8 = 2;
/// Where a channel record keeps its zone: the low byte, and the high byte.
pub const CH_ZONE_LO: usize = 18;
pub const CH_ZONE_HI: usize = 61;
/// Flag bits the UI's `Channel::flags` carries (0-2 skip, APRS, RX-only; 5-6 talkaround, hidden); 3-4 are
/// busy lockout (bit 3; a nonzero bit 4 from the old three-way setting reads as on), 7 "has position" (kept), 9 reverse, 10 split (TX at bytes 4-7 is the
/// transmit frequency itself, not RX plus a shift: CHAN-2); 8 and 11-15 are kept as they were. A record from
/// before split has bit 10 clear and reads as a shift, as it always did.
const UI_FLAGS: u16 = 0x0067;
/// The UI's priority flag (`FLAG_PRIORITY`, 0x80 in `Channel::flags`) lives in record bit 8.
const PRIORITY: u16 = 1 << 8;
const BUSY_SHIFT: u16 = 3;
const REVERSE: u16 = 1 << 9;
const SPLIT: u16 = 1 << 10;
const HAS_POS: u16 = 1 << 7;
pub const NO_POS: i32 = 0x7FFF_FFFF;

/// Channels in one sector.
pub const CH_PER_SECTOR: u32 = 63;

/// Channel `num`'s record: sector num / 63, slot num % 63.
pub fn ch_addr(num: u16) -> u32 {
    let n = num as u32;
    CHANNELS.start + n / CH_PER_SECTOR * SECTOR + n % CH_PER_SECTOR * CH_LEN as u32
}

/// LINK-71: the lists' flash sectors, hashed one by one: 64 of channels (63 a sector), 5 of zones (127 a sector),
/// then the range presets' (their 16 records, not the DTMF memories after them in that sector).
pub const CH_SECTORS: usize = 64;
pub const ZONE_SECTORS: usize = 5;
pub const LIST_SECTORS: usize = CH_SECTORS + ZONE_SECTORS + 1;
const _: () = assert!((CHANNELS.end - CHANNELS.start) / SECTOR == CH_SECTORS as u32);
const _: () = assert!((ZONES.end - ZONES.start) / SECTOR == ZONE_SECTORS as u32);

/// LINK-71: the list sector (as [`LIST_SECTORS`] counts them) a write at `addr` changes, if any.
pub fn list_sector(addr: u32) -> Option<usize> {
    if CHANNELS.contains(&addr) {
        Some(((addr - CHANNELS.start) / SECTOR) as usize)
    } else if ZONES.contains(&addr) {
        Some(CH_SECTORS + ((addr - ZONES.start) / SECTOR) as usize)
    } else if (PRESETS.start..PRESETS.start + MAX_PRESETS as u32 * PRESET_LEN as u32).contains(&addr) {
        Some(CH_SECTORS + ZONE_SECTORS)
    } else {
        None
    }
}

/// LINK-71: a held record into its sector's hash (FNV-1a over its index, then its bytes): the same records give
/// the same hash, whatever was written or restarted in between.
pub fn hash_mix(h: u32, idx: u16, rec: &[u8]) -> u32 {
    let mut h = h;
    for &b in idx.to_le_bytes().iter().chain(rec) {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    h
}
pub const HASH0: u32 = 0x811C_9DC5;

/// LINK-71: whether channel record `b` counts in its sector's hash: as the mount counts it (its byte 19 written,
/// then [`ch_valid`]), so the hash made at the mount and one made later agree.
pub fn ch_held(b: &[u8; CH_LEN]) -> bool {
    b[CH_ZONE_LO + 1] != 0xFF && ch_valid(b)
}

/// Whether a record holds a channel: a good CRC, this format, an RX frequency, and a zone that can be (one of the
/// zones, or none).
pub fn ch_valid(b: &[u8; CH_LEN]) -> bool {
    u32_at(b, 0) != 0xFFFF_FFFF && crc16(&b[..62]) == u16_at(b, 62) && b[60] == CH_VERSION && {
        let z = ch_zone(b);
        z < MAX_ZONES || z == NO_ZONE
    }
}

/// The channel in record `b` at number `num`, if it holds one.
pub fn ch_decode(num: u16, b: &[u8; CH_LEN]) -> Option<Channel> {
    if !ch_valid(b) {
        return None;
    }
    let rx = u32_at(b, 0);
    let tx = u32_at(b, 4);
    let flags = u16_at(b, 16);
    let mut cfg: SlotCfg = DEFAULT_CFG;
    cfg.mode = b[8];
    cfg.narrow = b[9] != 0;
    cfg.fold_bw(); // VFO-20: FM + narrow from before reads as NFM
    cfg.power = b[10].saturating_sub(1);
    cfg.step = match b[11] {
        0 => BANDS[band_near(rx)].step,
        s => s - 1,
    };
    cfg.rx_tone = u16_at(b, 12);
    cfg.tx_tone = u16_at(b, 14);
    cfg.split = flags & SPLIT != 0 && tx != 0;
    cfg.offset = if tx == 0 {
        0
    } else if cfg.split {
        tx as i32
    } else {
        (tx as i64 - rx as i64) as i32
    };
    cfg.busy = ((flags >> BUSY_SHIFT) & 3 != 0) as u8;
    cfg.reverse = flags & REVERSE != 0;
    // SQL-20.6: the open beep in byte 31's bits 5-7; 0 (every record before it) is off, as is a value out of range.
    cfg.open_beep = Some(b[31] >> 5).filter(|&v| (v as usize) < ferrite_app::chan::OPEN_BEEPS.len()).unwrap_or(0);
    // 0 = Global (the slot's level), 1-10 = levels 0-9 (CHAN-14). Records written before Global existed always
    // stored level + 1, so they read the same.
    cfg.squelch = match b[19] {
        0 => SQ_GLOBAL,
        s => (s - 1).min(9),
    };
    let ui = (flags & UI_FLAGS) as u8 | if flags & PRIORITY != 0 { ferrite_app::chan::FLAG_PRIORITY } else { 0 };
    // Byte 31 (decision 86): end tone bits 0-2, tail bits 3-4; 0 (every record before it) is Global for both,
    // and so is a value out of range.
    let mut c = Channel { num, zone: ch_zone(b), rx, name: [0; 16], cfg, flags: ui, tones: b[31] & 0x1F };
    // A name stored before CHAN-31 reads in the name set, as it would be written now.
    let name = &b[32..48];
    c.set_name(&name[..name.iter().position(|&x| x == 0).unwrap_or(16)]);
    c.tones = c.end_tone() | c.tail() << 3;
    Some(c)
}

/// The record for `ch`, keeping what the UI doesn't model from `old` (the record it replaces, or the one it
/// moves from); a new record has no position.
pub fn ch_encode(ch: &Channel, old: &[u8; CH_LEN]) -> [u8; CH_LEN] {
    let keep = ch_valid(old);
    let mut b = [0u8; CH_LEN];
    if keep {
        b[20..32].copy_from_slice(&old[20..32]);
        b[48..60].copy_from_slice(&old[48..60]);
    } else {
        b[20..24].copy_from_slice(&NO_POS.to_le_bytes());
        b[24..28].copy_from_slice(&NO_POS.to_le_bytes());
        b[48..60].fill(0xFF);
        b[48] = 0;
    }
    let c = &ch.cfg;
    b[0..4].copy_from_slice(&ch.rx.to_le_bytes());
    let tx = if c.split {
        c.offset.max(0) as u32
    } else if c.offset == 0 {
        0
    } else {
        (ch.rx as i64 + c.offset as i64) as u32
    };
    b[4..8].copy_from_slice(&tx.to_le_bytes());
    b[8] = c.mode;
    b[9] = c.narrow as u8;
    b[10] = c.power + 1;
    b[11] = c.step + 1;
    b[12..14].copy_from_slice(&c.rx_tone.to_le_bytes());
    b[14..16].copy_from_slice(&c.tx_tone.to_le_bytes());
    let old_flags = if keep { u16_at(old, 16) } else { 0 };
    let flags = (old_flags & !(UI_FLAGS | 3 << BUSY_SHIFT | REVERSE | SPLIT | PRIORITY))
        | (ch.flags as u16 & UI_FLAGS)
        | if ch.flags & ferrite_app::chan::FLAG_PRIORITY != 0 { PRIORITY } else { 0 }
        | ((c.busy.min(1) as u16) << BUSY_SHIFT)
        | if c.reverse { REVERSE } else { 0 }
        | if c.split { SPLIT } else { 0 };
    b[16..18].copy_from_slice(&flags.to_le_bytes());
    b[CH_ZONE_LO] = ch.zone as u8;
    b[19] = if c.squelch >= SQ_GLOBAL { 0 } else { c.squelch + 1 };
    b[31] = ch.end_tone() | ch.tail() << 3 | c.open_beep.min(5) << 5;
    b[32..48].copy_from_slice(&ch.name);
    b[60] = CH_VERSION;
    b[CH_ZONE_HI] = (ch.zone >> 8) as u8;
    let crc = crc16(&b[..62]);
    b[62..64].copy_from_slice(&crc.to_le_bytes());
    b
}

/// The zone a channel record names (whatever it holds: check [`ch_valid`] first).
pub fn ch_zone(b: &[u8; CH_LEN]) -> ZoneId {
    u16::from_le_bytes([b[CH_ZONE_LO], b[CH_ZONE_HI]])
}

/// A channel's position, if its record has one (CHAN-2, CHAN-15).
pub fn ch_position(b: &[u8; CH_LEN]) -> Option<Pos> {
    let (lat, lon) = (u32_at(b, 20) as i32, u32_at(b, 24) as i32);
    (ch_valid(b) && u16_at(b, 16) & HAS_POS != 0 && lat != NO_POS).then_some(Pos { lat, lon })
}

/// A record with a position set (the terminal's `pos=`, the link's positions: LINK-53; the simulator's sample).
pub fn ch_with_position(b: &[u8; CH_LEN], p: Pos) -> [u8; CH_LEN] {
    let mut r = *b;
    r[20..24].copy_from_slice(&p.lat.to_le_bytes());
    r[24..28].copy_from_slice(&p.lon.to_le_bytes());
    let f = u16_at(&r, 16) | HAS_POS;
    r[16..18].copy_from_slice(&f.to_le_bytes());
    let crc = crc16(&r[..62]);
    r[62..64].copy_from_slice(&crc.to_le_bytes());
    r
}

/// A record with no position (CHAN-2: the flag clear, latitude and longitude "none").
pub fn ch_without_position(b: &[u8; CH_LEN]) -> [u8; CH_LEN] {
    let mut r = *b;
    r[20..24].copy_from_slice(&NO_POS.to_le_bytes());
    r[24..28].copy_from_slice(&NO_POS.to_le_bytes());
    let f = u16_at(&r, 16) & !HAS_POS;
    r[16..18].copy_from_slice(&f.to_le_bytes());
    let crc = crc16(&r[..62]);
    r[62..64].copy_from_slice(&crc.to_le_bytes());
    r
}

/// LINK-53: record `b` with its position set or cleared (None), when it holds a channel.
pub fn pos_rec(b: &[u8; CH_LEN], pos: Option<Pos>) -> Option<[u8; CH_LEN]> {
    ch_valid(b).then(|| match pos {
        Some(p) => ch_with_position(b, p),
        None => ch_without_position(b),
    })
}

// ---- zones ----

pub const ZONE_LEN: usize = 32;
/// Zones in one sector.
pub const ZONES_PER_SECTOR: u32 = 127;
/// Zones 0 to `MAX_ZONES` - 1 (CHAN-6; 0xFFFF is "no zone").
pub use ferrite_app::chan::MAX_ZONES;
const _: () = assert!(MAX_ZONES < NO_ZONE && MAX_ZONES < ferrite_app::local::LOCAL);
const _: () = assert!(MAX_ZONES as u32 <= (ZONES.end - ZONES.start) / SECTOR * ZONES_PER_SECTOR);

/// Zone `z`'s record: sector z / 127, slot z % 127.
pub fn zone_addr(z: ZoneId) -> u32 {
    let z = z as u32;
    ZONES.start + z / ZONES_PER_SECTOR * SECTOR + z % ZONES_PER_SECTOR * ZONE_LEN as u32
}

/// Name (16) · flags: scan all, shown, by distance (1) · resume (1) · dwell (1) · reserved, 0 (1; it was smart scan,
/// now the range scans' hit list, SCAN-23) · priority channels (2 + 2, 0xFFFF none) · channel count (2: CHAN-36,
/// decision 333; 0xFFFF in a record from before it) · reserved (4) · CRC-16 (2).
pub fn zone_decode(b: &[u8; ZONE_LEN]) -> Option<(Name, ZoneCfg)> {
    if crc16(&b[..30]) != u16_at(b, 30) || b[..30] == [0xFF; 30] {
        return None;
    }
    let n = b[..16].iter().position(|&c| c == 0).unwrap_or(16);
    let pri = |i: usize| Some(u16_at(b, i)).filter(|&v| v != 0xFFFF);
    let cfg = ZoneCfg {
        scan_all: b[16] & 1 != 0,
        shown: b[16] & 2 != 0,
        by_distance: b[16] & 4 != 0,
        resume: b[17],
        dwell: b[18],
        pri: [pri(20), pri(22)],
    };
    Some((Name::from(&b[..n]), cfg))
}

/// The channel count a zone record holds (CHAN-36), None when it has none or isn't a record.
pub fn zone_count(b: &[u8; ZONE_LEN]) -> Option<u16> {
    zone_decode(b)?;
    Some(u16_at(b, ZONE_COUNT)).filter(|&n| n != 0xFFFF)
}

/// Where a zone record keeps its channel count.
pub const ZONE_COUNT: usize = 24;

/// Zone record `b` with its channel count set (and its CRC again).
pub fn set_zone_count(b: &mut [u8; ZONE_LEN], n: u16) {
    b[ZONE_COUNT..ZONE_COUNT + 2].copy_from_slice(&n.to_le_bytes());
    let crc = crc16(&b[..30]);
    b[30..32].copy_from_slice(&crc.to_le_bytes());
}

pub fn zone_encode(name: &[u8], cfg: &ZoneCfg, count: u16) -> [u8; ZONE_LEN] {
    let mut b = [0xFFu8; ZONE_LEN];
    b[..16].fill(0);
    let n = name.len().min(16);
    b[..n].copy_from_slice(&name[..n]);
    b[16] = cfg.scan_all as u8 | (cfg.shown as u8) << 1 | (cfg.by_distance as u8) << 2;
    b[17] = cfg.resume;
    b[18] = cfg.dwell;
    b[19] = 0;
    b[20..22].copy_from_slice(&cfg.pri[0].unwrap_or(0xFFFF).to_le_bytes());
    b[22..24].copy_from_slice(&cfg.pri[1].unwrap_or(0xFFFF).to_le_bytes());
    b[ZONE_COUNT..ZONE_COUNT + 2].copy_from_slice(&count.to_le_bytes());
    let crc = crc16(&b[..30]);
    b[30..32].copy_from_slice(&crc.to_le_bytes());
    b
}

/// A new zone's settings: in "scan all", shown in ◀▶, the radio's resume and dwell.
pub const NEW_ZONE: ZoneCfg = ZoneCfg { scan_all: true, shown: true, by_distance: false, resume: 0, dwell: 255, pri: [None, None] };

// ---- range presets ----

pub const PRESET_LEN: usize = 224;
pub const MAX_PRESETS: u8 = 16;

pub fn preset_addr(i: u8) -> u32 {
    PRESETS.start + i as u32 * PRESET_LEN as u32
}

/// Name (16) · start (4) · end (4) · step, mode, bandwidth, resume (1 each) · skips in use (1) · reserved (1) ·
/// CRC-16 of the 30 bytes before (2) · 48 skipped frequencies (192, kept as they are).
pub fn preset_decode(b: &[u8]) -> Option<RangePreset> {
    if crc16(&b[..30]) != u16_at(b, 30) || b[..30] == [0xFF; 30] {
        return None;
    }
    let n = b[..16].iter().position(|&c| c == 0).unwrap_or(16).min(12);
    let mut p = RangePreset {
        name: Buf::from(&b[..n]),
        start: u32_at(b, 16),
        end: u32_at(b, 20),
        step: b[24],
        mode: b[25],
        narrow: b[26] != 0,
        skips: b[28],
        resume: if b[27] <= 12 { b[27] } else { 0 }, // SCAN-24: 0 Global
    };
    p.fold_bw(); // VFO-20
    Some(p)
}

/// The first 32 bytes of a preset record (the skip list after them is left as it is).
pub fn preset_encode(p: &RangePreset) -> [u8; 32] {
    let mut b = [0u8; 32];
    let name = p.name.as_bytes();
    b[..name.len().min(16)].copy_from_slice(&name[..name.len().min(16)]);
    b[16..20].copy_from_slice(&p.start.to_le_bytes());
    b[20..24].copy_from_slice(&p.end.to_le_bytes());
    b[24] = p.step;
    b[25] = p.mode;
    b[26] = p.narrow as u8;
    b[27] = p.resume;
    b[28] = p.skips;
    b[29] = 0xFF;
    let crc = crc16(&b[..30]);
    b[30..32].copy_from_slice(&crc.to_le_bytes());
    b
}

// ---- DTMF memories ----

/// 16 records of 16 B after the range presets, in the same sector (0x041E00-0x041EFF).
pub const DTMF_LEN: usize = 16;

pub fn dtmf_addr(i: u8) -> u32 {
    PRESETS.start + MAX_PRESETS as u32 * PRESET_LEN as u32 + (i % DTMF_MEMS) as u32 * DTMF_LEN as u32
}

/// Length (1) · symbols, two to a byte (8) · reserved (5) · CRC-16 of the 14 bytes before (2).
pub fn dtmf_decode(b: &[u8]) -> Option<Dtmf> {
    if b[..14] == [0xFF; 14] || crc16(&b[..14]) != u16_at(b, 14) || b[0] > Dtmf::MAX {
        return None;
    }
    let mut d = [0u8; 8];
    d.copy_from_slice(&b[1..9]);
    Some(Dtmf { len: b[0], d })
}

/// The memories' names (DTMF-12): 16 records of 12 B after the memories (0x041F00-0x041FBF): name, zero-padded
/// (10) · CRC-16 (2).
pub const DTMF_NAME_LEN: usize = 12;

pub fn dtmf_name_addr(i: u8) -> u32 {
    dtmf_addr(0) + DTMF_MEMS as u32 * DTMF_LEN as u32 + (i % DTMF_MEMS) as u32 * DTMF_NAME_LEN as u32
}

pub fn dtmf_name_decode(b: &[u8]) -> Option<Buf<10>> {
    if b[..10] == [0xFF; 10] || crc16(&b[..10]) != u16_at(b, 10) {
        return None;
    }
    let n = b[..10].iter().position(|&c| c == 0).unwrap_or(10);
    Some(Buf::from(&b[..n]))
}

pub fn dtmf_name_encode(name: &[u8]) -> [u8; DTMF_NAME_LEN] {
    let mut b = [0u8; DTMF_NAME_LEN];
    let n = name.len().min(10);
    b[..n].copy_from_slice(&name[..n]);
    let crc = crc16(&b[..10]);
    b[10..12].copy_from_slice(&crc.to_le_bytes());
    b
}

pub fn dtmf_encode(m: &Dtmf) -> [u8; DTMF_LEN] {
    let mut b = [0xFFu8; DTMF_LEN];
    b[0] = m.len;
    b[1..9].copy_from_slice(&m.d);
    let crc = crc16(&b[..14]);
    b[14..16].copy_from_slice(&crc.to_le_bytes());
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrite_app::chan::channel;
    use ferrite_app::slot::DCS_FLAG;

    /// CHAN-1: 4,000 channels of 64 B from SPI 0x010000, 63 a sector, all inside the region; an erased record (RX
    /// frequency 0xFFFFFFFF) is empty. CHAN-6: 512 zones of 32 B from 0x050000, 127 a sector, inside theirs.
    #[test]
    fn channel_and_zone_regions() {
        assert_eq!(CH_LEN, 64);
        assert_eq!(CHANNELS.start, 0x01_0000);
        assert_eq!(ch_addr(63), 0x01_0000 + SECTOR);
        assert!(ch_addr(3999) + CH_LEN as u32 <= CHANNELS.end);
        assert!(!ch_valid(&[0xFF; CH_LEN]) && ch_decode(5, &[0xFF; CH_LEN]).is_none());
        assert_eq!((ZONE_LEN, ZONES_PER_SECTOR, MAX_ZONES), (32, 127, 512));
        assert_eq!(zone_addr(0), 0x05_0000);
        assert_eq!(zone_addr(127), 0x05_0000 + SECTOR);
        assert!(zone_addr(MAX_ZONES - 1) + ZONE_LEN as u32 <= ZONES.end);
    }

    /// CHAN-2, decision 333: the record's version is 2, its zone 2 bytes (low at 18, high at 61; 0xFFFF none); a
    /// version 1 record, or a zone that can't be (past the last, not none), reads as no channel.
    #[test]
    fn a_channel_record_holds_a_two_byte_zone() {
        let mut c = channel(7, 300, 14_600_000, "Club", ferrite_app::slot::DEFAULT_CFG);
        let b = ch_encode(&c, &[0xFF; CH_LEN]);
        assert_eq!((b[60], b[CH_ZONE_LO], b[CH_ZONE_HI], ch_zone(&b)), (2, 44, 1, 300));
        assert_eq!(ch_decode(7, &b).map(|c| c.zone), Some(300));
        c.zone = NO_ZONE;
        let b = ch_encode(&c, &[0xFF; CH_LEN]);
        assert_eq!((b[CH_ZONE_LO], b[CH_ZONE_HI]), (0xFF, 0xFF));
        assert_eq!(ch_decode(7, &b).map(|c| c.zone), Some(NO_ZONE));
        let fix = |mut b: [u8; CH_LEN]| {
            let k = crc16(&b[..62]);
            b[62..64].copy_from_slice(&k.to_le_bytes());
            b
        };
        let mut v1 = b;
        v1[60] = 1;
        assert!(ch_decode(7, &fix(v1)).is_none(), "version 1: Console translates it");
        let mut far = b;
        (far[CH_ZONE_LO], far[CH_ZONE_HI]) = (0x00, 0x02); // zone 512
        assert!(!ch_valid(&fix(far)));
    }

    /// CHAN-3: the tone word is CTCSS in 0.1 Hz, bit 15 DCS, bit 14 inverted, 0 none; each survives a round
    /// trip through the record as written.
    #[test]
    fn tone_words() {
        for (rx, tx) in [(885u16, 1000u16), (DCS_FLAG | 0o023, DCS_FLAG | 0x4000 | 0o754), (0, 1318)] {
            let mut cfg = DEFAULT_CFG;
            (cfg.rx_tone, cfg.tx_tone) = (rx, tx);
            let c = channel(3, 0, 14_652_000, "T", cfg);
            let b = ch_encode(&c, &[0xFF; CH_LEN]);
            assert_eq!((u16::from_le_bytes([b[12], b[13]]), u16::from_le_bytes([b[14], b[15]])), (rx, tx));
            assert_eq!(ch_decode(3, &b).map(|d| (d.cfg.rx_tone, d.cfg.tx_tone)), Some((rx, tx)));
        }
        assert_eq!(DCS_FLAG, 0x8000);
    }

    /// CHAN-7: each channel has a fixed place in flash: channel n at sector n / 63, slot n % 63.
    #[test]
    fn channel_places_are_fixed() {
        assert_eq!(ch_addr(0), CHANNELS.start);
        assert_eq!(ch_addr(62), CHANNELS.start + 62 * CH_LEN as u32);
        assert_eq!(ch_addr(63), CHANNELS.start + SECTOR);
        assert_eq!(ch_addr(999), CHANNELS.start + 999 / 63 * SECTOR + 999 % 63 * CH_LEN as u32);
    }

    #[test]
    fn channel_round_trip_keeps_unmodelled_fields() {
        let mut cfg = DEFAULT_CFG;
        cfg.tx_tone = DCS_FLAG | 0o023;
        cfg.offset = -60_000;
        cfg.power = 2;
        cfg.busy = 1;
        cfg.reverse = true;
        cfg.squelch = 0;
        let mut c = channel(11, 1, 14_694_000, "North hill", cfg);
        c.flags = ferrite_app::chan::FLAG_SKIP;
        let r = ch_encode(&c, &[0xFF; CH_LEN]);
        assert_eq!(ch_decode(11, &r), Some(c));
        // Split: TX is stored as itself and comes back the same (RX 446.000, TX 146.520).
        let mut sp = c;
        sp.rx = 44_600_000;
        sp.cfg.split = true;
        sp.cfg.offset = 14_652_000;
        let rs = ch_encode(&sp, &[0xFF; CH_LEN]);
        assert_eq!(u32_at(&rs, 4), 14_652_000);
        assert_eq!(ch_decode(11, &rs), Some(sp));
        // Global squelch is stored as 0 and reads back as Global (CHAN-14).
        let mut g = c;
        g.cfg.squelch = SQ_GLOBAL;
        let rg = ch_encode(&g, &[0xFF; CH_LEN]);
        assert_eq!(rg[19], 0);
        assert_eq!(ch_decode(11, &rg), Some(g));
        assert_eq!(ch_position(&r), None);
        let p = ch_with_position(&r, Pos { lat: 1, lon: -2 });
        assert_eq!(ch_position(&p), Some(Pos { lat: 1, lon: -2 }));
        // Rewritten (renamed): the position stays.
        c.set_name(b"Hill");
        let q = ch_encode(&c, &p);
        assert_eq!(ch_position(&q), Some(Pos { lat: 1, lon: -2 }));
        assert_eq!(ch_decode(11, &q), Some(c));
        // Torn: empty.
        let mut t = q;
        t[40] ^= 1;
        assert_eq!(ch_decode(11, &t), None);
        assert_eq!(ch_decode(0, &[0xFF; CH_LEN]), None);
    }

    /// Decision 63: Hidden (UI bit 6, record bit 6) and Priority (UI bit 7, record bit 8) survive a round trip;
    /// record bit 7 stays "has position".
    #[test]
    fn hidden_and_priority_round_trip() {
        use ferrite_app::chan::{FLAG_HIDDEN, FLAG_PRIORITY, FLAG_SKIP};
        let c = Channel { num: 3, zone: 0, rx: 14_652_000, name: [0; 16], cfg: DEFAULT_CFG, flags: FLAG_HIDDEN | FLAG_PRIORITY, tones: 0 };
        let r = ch_encode(&c, &[0xFF; CH_LEN]);
        assert_eq!(u16_at(&r, 16) & (1 << 8), 1 << 8);
        assert_eq!(u16_at(&r, 16) & (1 << 7), 0, "a new record has no position");
        assert_eq!(ch_decode(3, &r).unwrap().flags, FLAG_HIDDEN | FLAG_PRIORITY);
        let c2 = Channel { flags: FLAG_SKIP, ..c };
        assert_eq!(ch_decode(3, &ch_encode(&c2, &r)).unwrap().flags, FLAG_SKIP, "cleared again");
    }

    #[test]
    fn zones_and_presets_round_trip() {
        let cfg = ZoneCfg { scan_all: true, shown: false, by_distance: true, resume: 11, dwell: 3, pri: [Some(9), None] };
        let mut z = zone_encode(b"Repeaters", &cfg, 17);
        assert!(zone_decode(&z) == Some((Name::from(b"Repeaters"), cfg)));
        assert_eq!(zone_count(&z), Some(17), "CHAN-36: the count");
        set_zone_count(&mut z, 256);
        assert!(zone_decode(&z).is_some() && zone_count(&z) == Some(256));
        let mut old = z;
        old[ZONE_COUNT..ZONE_COUNT + 2].fill(0xFF);
        let c = crc16(&old[..30]);
        old[30..32].copy_from_slice(&c.to_le_bytes());
        assert_eq!(zone_count(&old), None, "a record from before counts has none");
        assert!(zone_decode(&[0xFF; ZONE_LEN]).is_none());
        let p = RangePreset { name: Buf::from(b"Airband"), start: 11_800_000, end: 13_700_000, step: 10, mode: 2, narrow: false, skips: 3, resume: 11 };
        let mut rec = [0xFFu8; PRESET_LEN];
        rec[..32].copy_from_slice(&preset_encode(&p));
        assert!(preset_decode(&rec) == Some(p));
        assert!(preset_decode(&[0xFF; PRESET_LEN]).is_none());
        // DTMF memories sit after the presets, inside the sector, clear of its free tail.
        let m = Dtmf::parse(b"*79#").unwrap();
        assert_eq!(dtmf_decode(&dtmf_encode(&m)), Some(m));
        assert!(dtmf_decode(&[0xFF; DTMF_LEN]).is_none());
        assert!(dtmf_addr(DTMF_MEMS - 1) + DTMF_LEN as u32 <= PRESETS.end - 32);
        assert!(dtmf_name_addr(DTMF_MEMS - 1) + DTMF_NAME_LEN as u32 <= PRESETS.end - 32);
        assert_eq!(dtmf_name_decode(&dtmf_name_encode(b"Link on")).unwrap().as_bytes(), b"Link on");
        assert!(dtmf_name_decode(&[0xFF; DTMF_NAME_LEN]).is_none());
    }

    /// Decision 86, TONE-17: end tone and tail in byte 31 round trip; an older record (0 there) and a value out
    /// of range read as Global.
    #[test]
    fn end_tone_and_tail_in_byte_31() {
        let mut c = Channel { num: 4, zone: 0, rx: 14_652_000, name: [0; 16], cfg: DEFAULT_CFG, flags: 0, tones: 0 };
        c.set_end_tone(6);
        c.set_tail(2);
        let b = ch_encode(&c, &[0xFF; CH_LEN]);
        let d = ch_decode(4, &b).unwrap();
        assert_eq!((d.end_tone(), d.tail()), (6, 2));
        let mut old = b;
        old[31] = 0;
        let crc = crc16(&old[..62]);
        old[62..64].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(ch_decode(4, &old).map(|c| (c.end_tone(), c.tail())), Some((0, 0)), "older record: Global");
        old[31] = 7 | 3 << 3;
        let crc = crc16(&old[..62]);
        old[62..64].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(ch_decode(4, &old).map(|c| (c.end_tone(), c.tail())), Some((0, 0)), "out of range: Global");
    }

    /// SQL-20.6: the open beep in byte 31's bits 5-7 beside the end tone and tail; an older record reads Off, as
    /// does a value out of range.
    #[test]
    fn open_beep_in_byte_31() {
        let mut c = Channel { num: 4, zone: 0, rx: 14_652_000, name: [0; 16], cfg: DEFAULT_CFG, flags: 0, tones: 0 };
        c.set_end_tone(6);
        c.set_tail(2);
        c.cfg.open_beep = 5;
        let b = ch_encode(&c, &[0xFF; CH_LEN]);
        let d = ch_decode(4, &b).unwrap();
        assert_eq!((d.end_tone(), d.tail(), d.cfg.open_beep), (6, 2, 5));
        let mut old = b;
        for (byte, want) in [(6 | 2 << 3, 0), (7 << 5, 0), (4 << 5, 4)] {
            old[31] = byte;
            let crc = crc16(&old[..62]);
            old[62..64].copy_from_slice(&crc.to_le_bytes());
            assert_eq!(ch_decode(4, &old).map(|c| c.cfg.open_beep), Some(want), "byte 31 = {byte:#x}");
        }
    }
}
