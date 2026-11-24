//! A slot's settings and the values they take (manual chapters 9 and 11, Appendix A): the nine quick
//! settings, the edit-in-place grid, their names, help lines, value texts and defaults, matching the manual's
//! Appendix A.

use ferrite_ui::text::{Buf, Zero};
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::msg::Param;
use ufmt::uwrite;

/// One slot's settings: a VFO's, or a copy of the channel it is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SlotCfg {
    /// Index into [`MODES`]: FM (wide), NFM (narrow) or AM; the mode is the bandwidth too (VFO-20).
    pub mode: u8,
    /// Narrow filter: follows the mode ([`SlotCfg::fold_bw`]); kept for the record's byte and the radio.
    pub narrow: bool,
    /// Tone words (CHAN-3): CTCSS in 0.1 Hz; bit 15 DCS, bit 14 inverted, the code in octal; 0 = none.
    pub rx_tone: u16,
    pub tx_tone: u16,
    /// Transmit offset, 10 Hz units, signed; 0 = simplex. With [`SlotCfg::split`]: the TX frequency itself.
    pub offset: i32,
    /// 0 low, 1 mid, 2 high.
    pub power: u8,
    /// Index into [`STEPS`].
    pub step: u8,
    /// Level 0-9; on a channel also [`SQ_GLOBAL`]: the slot's own level (SQL-6).
    pub squelch: u8,
    /// Busy lockout: 0 off, 1 on (TX-21).
    pub busy: u8,
    pub reverse: bool,
    /// Split (MENU 5, the 4th offset choice): transmit on the absolute frequency in `offset`, whatever RX is
    /// tuned to. Fits the struct's padding: no bigger.
    pub split: bool,
}

impl SlotCfg {
    /// VFO-20: the mode carries the bandwidth. A record or saved state from before has FM with its narrow flag
    /// set for narrow FM: that reads as NFM. Afterwards narrow means NFM or NAM; AM keeps the wide filter.
    pub fn fold_bw(&mut self) {
        if self.mode == 0 && self.narrow {
            self.mode = 1;
        }
        self.narrow = mode_narrow(self.mode);
    }

    /// The frequency this config transmits on with RX at `rx`: the split TX, or RX plus the shift; reverse
    /// swaps the two (TX-6 checks this one).
    pub fn tx_of(&self, rx: Freq10) -> Freq10 {
        let tx = if self.split { self.offset.max(0) as i64 } else { rx as i64 + self.offset as i64 };
        (if self.reverse { rx as i64 } else { tx }).max(0) as Freq10
    }

    /// The tone it sends: reverse swaps the tones with the frequencies (VFO-7: listening on the input you hear
    /// the users' tone, and transmitting on the output you send the repeater's). The Radio task does the same
    /// swap with the raw tones and the Reverse parameter.
    pub fn sent_tone(&self) -> u16 {
        if self.reverse { self.rx_tone } else { self.tx_tone }
    }

    /// The tone it listens for (CTCSS/DCS decode), reverse swapping as above.
    pub fn heard_tone(&self) -> u16 {
        if self.reverse { self.tx_tone } else { self.rx_tone }
    }

    /// The frequency it listens on with RX tuned to `rx`: reverse listens on the TX side.
    pub fn rx_of(&self, rx: Freq10) -> Freq10 {
        if self.reverse {
            let tx = if self.split { self.offset.max(0) as i64 } else { rx as i64 + self.offset as i64 };
            tx.max(0) as Freq10
        } else {
            rx
        }
    }
}

pub const DEFAULT_CFG: SlotCfg = SlotCfg {
    mode: 1, // NFM (VFO-20)
    narrow: true,
    rx_tone: 0,
    tx_tone: 0,
    offset: 0,
    power: 0,
    step: STEP_12K5,
    squelch: 4,
    busy: 0,
    reverse: false,
    split: false,
};

impl Default for SlotCfg {
    fn default() -> Self {
        DEFAULT_CFG
    }
}

pub const MODES: [&str; 8] = ["FM", "NFM", "AM", "USB", "LSB", "CW", "WFM", "NAM"];
/// Channels and home VFOs use FM, NFM, AM, NAM and WFM (CHAN-23, VFO-4, BCR-33); USB, LSB and CW are HF mode's.
pub const SLOT_MODES: usize = 5;
/// BCR-33 (decision 353, as nicFW's tuner channels): wide FM on the SI4732, a broadcast slot anywhere 64-108 MHz.
pub const WFM: u8 = 6;
/// Narrow AM (VFO-4.2, decision 447): AM through the narrow receive filter (REG_43 0x4008, as NFM), for 8.33 kHz
/// airband channels; the airband's default mode. Stored as 7, after WFM; CHIRP calls it NAM too.
pub const NAM: u8 = 7;
/// The slot modes in the order the knob steps through them (their stored numbers: WFM after HF's three, NAM last).
pub const SLOT_MODE_IDS: [u8; SLOT_MODES] = [0, 1, 2, NAM, WFM];

/// A range preset's modes, in the order the editor steps them (CHAN-23).
pub const PRESET_MODES: [u8; 4] = [0, 1, 2, NAM];

/// A mode that receives through the narrow filter: NFM and NAM (VFO-20).
pub fn mode_narrow(m: u8) -> bool {
    m == 1 || m == NAM
}

/// A mode that demodulates AM: AM and NAM (receive only, TX-20).
pub fn mode_am(m: u8) -> bool {
    m == 2 || m == NAM
}

/// A mode a slot or channel may have (FM, NFM, AM, NAM, WFM).
pub fn slot_mode_ok(m: u8) -> bool {
    SLOT_MODE_IDS.contains(&m)
}

/// Steps in 10 Hz units and their names (MENU 6).
pub const STEPS: [(u32, &str); 14] = [
    (1, "10 Hz"),
    (5, "50 Hz"),
    (10, "100 Hz"),
    (50, "500 Hz"),
    (100, "1 kHz"),
    (250, "2.5 kHz"),
    (500, "5 kHz"),
    (625, "6.25 kHz"),
    (833, "8.33 kHz"),
    (1000, "10 kHz"),
    (1250, "12.5 kHz"),
    (2500, "25 kHz"),
    (5000, "50 kHz"),
    (10000, "100 kHz"),
];
pub const STEP_12K5: u8 = 10;
/// The 8.33 kHz step: 25 kHz / 3, not a flat 8.330 kHz (VFO-18.1).
pub const STEP_833: u8 = 8;

/// The 8.33 kHz channel grid (VFO-18.1): each 25 kHz block at +0, +8.333 and +16.667 kHz, in 10 Hz.
const GRID_833: [Freq10; 3] = [0, 833, 1667];

/// The 8.33 kHz channel nearest f.
pub fn on_833(f: Freq10) -> Freq10 {
    let (block, r) = (f / 2500 * 2500, f % 2500);
    match r {
        0..=416 => block,
        417..=1249 => block + 833,
        1250..=2083 => block + 1667,
        _ => block + 2500,
    }
}

/// f moved d 8.33 kHz channels from the channel nearest it, so steps land on the grid however f was reached.
pub fn step_833(f: Freq10, d: i32) -> Freq10 {
    let g = on_833(f);
    let n = (g / 2500) as i64 * 3 + GRID_833.iter().position(|&o| o == g % 2500).unwrap_or(0) as i64 + d as i64;
    let n = n.max(0);
    ((n / 3) as Freq10) * 2500 + GRID_833[(n % 3) as usize]
}
pub const STEP_5K: u8 = 6;
/// A channel's squelch "Global": it uses the slot's level (the VFO's), not its own (SQL-6, CHAN-14).
pub const SQ_GLOBAL: u8 = 10;
const STEP_25K: u8 = 11;

/// The 50 standard CTCSS tones, 0.1 Hz.
pub const CTCSS: [u16; 50] = [
    670, 693, 719, 744, 770, 797, 825, 854, 885, 915, 948, 974, 1000, 1035, 1072, 1109, 1148, 1188, 1230, 1273, 1318,
    1365, 1413, 1462, 1514, 1567, 1598, 1622, 1655, 1679, 1713, 1738, 1773, 1799, 1835, 1862, 1899, 1928, 1966, 1995,
    2035, 2065, 2107, 2181, 2257, 2291, 2336, 2418, 2503, 2541,
];

/// The 104 DCS codes (octal).
pub const DCS: [u16; 104] = [
    0o023, 0o025, 0o026, 0o031, 0o032, 0o036, 0o043, 0o047, 0o051, 0o053, 0o054, 0o065, 0o071, 0o072, 0o073, 0o074,
    0o114, 0o115, 0o116, 0o122, 0o125, 0o131, 0o132, 0o134, 0o143, 0o145, 0o152, 0o155, 0o156, 0o162, 0o165, 0o172,
    0o174, 0o205, 0o212, 0o223, 0o225, 0o226, 0o243, 0o244, 0o245, 0o246, 0o251, 0o252, 0o255, 0o261, 0o263, 0o265,
    0o266, 0o271, 0o274, 0o306, 0o311, 0o315, 0o325, 0o331, 0o332, 0o343, 0o346, 0o351, 0o356, 0o364, 0o365, 0o371,
    0o411, 0o412, 0o413, 0o423, 0o431, 0o432, 0o445, 0o446, 0o452, 0o454, 0o455, 0o462, 0o464, 0o465, 0o466, 0o503,
    0o506, 0o516, 0o523, 0o526, 0o532, 0o546, 0o565, 0o606, 0o612, 0o624, 0o627, 0o631, 0o632, 0o654, 0o662, 0o664,
    0o703, 0o712, 0o723, 0o731, 0o732, 0o734, 0o743, 0o754,
];
pub const DCS_FLAG: u16 = 1 << 15;
pub const DCS_INV: u16 = 1 << 14;
/// Off, 50 CTCSS, 104 DCS normal, 104 DCS inverted.
const TONES: i32 = 1 + 50 + 104 + 104;

/// Tone word to its place in the knob's order (Off, CTCSS, DCS N, DCS I). A tone typed off the lists (TONE-15)
/// sits after the last standard one below it: [`tone_at`] gives that one back, so the knob steps from there.
pub fn tone_index(t: u16) -> i32 {
    if t == 0 {
        0
    } else if t & DCS_FLAG == 0 {
        CTCSS.iter().position(|&c| c == t).map_or_else(|| CTCSS.iter().filter(|&&c| c < t).count().max(1) as i32, |i| 1 + i as i32)
    } else {
        let c = t & 0x1FF;
        let i = DCS.iter().position(|&d| d == c).map_or_else(|| DCS.iter().filter(|&&d| d < c).count().saturating_sub(1) as i32, |i| i as i32);
        51 + i + if t & DCS_INV != 0 { 104 } else { 0 }
    }
}

/// A tone word on neither list: typed (TONE-15), or read from a channel another radio wrote.
pub fn tone_custom(t: u16) -> bool {
    t != 0 && tone_at(tone_index(t)) != t
}

pub fn tone_at(i: i32) -> u16 {
    let i = i.rem_euclid(TONES);
    match i {
        0 => 0,
        1..=50 => CTCSS[i as usize - 1],
        51..=154 => DCS_FLAG | DCS[i as usize - 51],
        _ => DCS_FLAG | DCS_INV | DCS[i as usize - 155],
    }
}

/// "T 100.0", "D023N", "" for none.
pub fn tone_text(t: u16, b: &mut Buf<16>) {
    b.clear();
    if t == 0 {
    } else if t & DCS_FLAG == 0 {
        let _ = uwrite!(b, "T {}.{}", t / 10, t % 10);
    } else {
        let c = t & 0x1FF;
        let _ = uwrite!(b, "D{}{}{}{}", c >> 6, (c >> 3) & 7, c & 7, if t & DCS_INV != 0 { "I" } else { "N" });
    }
}

/// The tones as home's cards show them (HOME-4), at most 7 characters (the chip's width), reverse swapping sent and
/// heard ([`SlotCfg::sent_tone`]): the same both ways as [`tone_text`] ("T 100.0", "D023N"); heard only, "R " and the
/// tone ("R 88.5", "R D023N"); sent with a different heard tone, the sent one marked "*" ("T100.0*", "D023N*"); sent
/// only (the usual repeater access tone), as before.
pub fn card_tone(c: &SlotCfg, b: &mut Buf<16>) {
    let (sent, heard) = (c.sent_tone(), c.heard_tone());
    tone_text(if sent == 0 { heard } else { sent }, b);
    if sent == heard || heard == 0 || b.is_empty() {
        return;
    }
    let mut t = Buf::<16>::new();
    let v = b.as_bytes().strip_prefix(b"T ").unwrap_or(b.as_bytes());
    if sent == 0 {
        t.set(b"R ");
        t.push(v);
    } else {
        t.set(if sent & DCS_FLAG == 0 { b"T".as_slice() } else { b"" });
        t.push(v);
        t.push(b"*");
    }
    *b = t;
}

/// The shift as text: [`offset_text`], or for split the TX frequency itself: "TX 146.520" (short, the cards and
/// lists) / "Split 146.520" (long, the editors).
pub fn shift_text(c: &SlotCfg, long: bool, b: &mut Buf<16>) {
    if c.split {
        b.set(if long { b"Split " } else { b"TX " });
        b.push(mhz3(c.offset.max(0) as Freq10).as_bytes());
    } else {
        offset_text(c.offset, long, b);
    }
}

/// The shift for the big card's fixed shift slot (HOME-4): "+600k", "-5M", "SPLIT" (the card's header shows the
/// split's TX frequency), with "R" in front while reversed ("R+600k", "R SPLIT"); empty for simplex. At most 7
/// characters, the slot's width.
pub fn shift_chip(c: &SlotCfg, b: &mut Buf<16>) {
    let mut o = Buf::<16>::new();
    if c.split {
        o.set(b"SPLIT");
    } else {
        offset_text(c.offset, false, &mut o);
    }
    b.clear();
    if c.reverse && !o.is_empty() {
        b.push(if c.split { b"R " } else { b"R" });
    }
    b.push(o.as_bytes());
}

/// "146.520": MHz with three decimals.
pub fn mhz3(f: Freq10) -> Buf<12> {
    let mut b = Buf::<12>::new();
    let khz = f / 100;
    let _ = uwrite!(b, "{}.{}", khz / 1000, Zero(khz % 1000, 3));
    b
}

/// "-600k", "+5M", "+1.6M" (short), or "-600 kHz", "+5 MHz" (long); "" for simplex.
pub fn offset_text(o: i32, long: bool, b: &mut Buf<16>) {
    b.clear();
    if o == 0 {
        return;
    }
    let sign = if o < 0 { "-" } else { "+" };
    let a = o.unsigned_abs();
    if a >= 100_000 {
        let (m, frac) = (a / 100_000, a % 100_000 / 10_000);
        let _ = uwrite!(b, "{}{}", sign, m);
        if frac != 0 {
            let _ = uwrite!(b, ".{}", frac);
        }
        b.push(if long { b" MHz" } else { b"M" });
    } else {
        let _ = uwrite!(b, "{}{}", sign, a / 100);
        b.push(if long { b" kHz" } else { b"k" });
    }
}

/// A band for ◀▶ in VFO mode (NAV-2) and for the slot's label. The full band table (chip, path, relay, PA,
/// transmit allowed: VFO-2) belongs to the radio core; this is the part the UI needs.
pub struct BandDef {
    pub name: &'static str,
    pub lo: Freq10,
    pub hi: Freq10,
    /// Where the VFO starts on its first visit.
    pub start: Freq10,
    pub mode: u8,
    pub step: u8,
    /// The repeater offset amount on this band (10 Hz).
    pub shift: i32,
}

pub const BANDS: [BandDef; 7] = [
    BandDef { name: "HF", lo: mhz(1) + 80_000, hi: mhz(30), start: mhz(7) + 10_000, mode: 2, step: 6, shift: 0 },
    BandDef { name: "6 m", lo: mhz(50), hi: mhz(54), start: mhz(52) + 52_500, mode: 0, step: STEP_12K5, shift: 50_000 },
    BandDef { name: "Airband", lo: mhz(108), hi: mhz(137), start: mhz(118) + 95_000, mode: 2, step: STEP_25K, shift: 0 },
    BandDef { name: "VHF", lo: mhz(137), hi: mhz(156), start: mhz(146) + 52_000, mode: 0, step: STEP_12K5, shift: 60_000 },
    BandDef { name: "Marine", lo: mhz(156), hi: mhz(163), start: mhz(156) + 80_000, mode: 0, step: STEP_25K, shift: 0 },
    BandDef { name: "1.25 m", lo: mhz(220), hi: mhz(225), start: mhz(223) + 50_000, mode: 0, step: STEP_12K5, shift: 160_000 },
    BandDef { name: "UHF", lo: mhz(400), hi: mhz(520), start: mhz(446), mode: 0, step: STEP_12K5, shift: 500_000 },
];
pub const BAND_125M: usize = 5;
pub const BAND_AIR: usize = 2;
pub const BAND_VHF: usize = 3;
pub const BAND_UHF: usize = 6;

/// A band's repeater offset amount in a region (VFO-26): 70 cm in Europe takes IARU Region 1's 7.6 MHz; elsewhere
/// the band's own (600 kHz on 2 m, 5 MHz on 70 cm).
pub fn band_shift(b: usize, r: Option<ferrite_core::txplan::Region>) -> i32 {
    match b {
        BAND_UHF if r == Some(ferrite_core::txplan::Region::Europe) => 760_000,
        _ => BANDS[b].shift,
    }
}

/// The usual repeater shift for a frequency in a region (VFO-26): its band's amount, down on 2 m below 147 MHz
/// and on 70 cm in Europe (Region 1's inputs sit 7.6 MHz below), else up.
pub fn usual_shift(f: Freq10, r: Option<ferrite_core::txplan::Region>) -> i32 {
    let b = band_near(f);
    let s = band_shift(b, r);
    let down = f >= mhz(144) && f < mhz(147) || b == BAND_UHF && r == Some(ferrite_core::txplan::Region::Europe);
    if down { -s } else { s }
}

/// A band's default mode in a region (SETUP-7): its FM is wide FM in the Americas, Australia and New Zealand,
/// NFM in the UK, Europe, Japan and Other; the airband is NAM everywhere (VFO-4.2), HF stays AM. No region
/// chosen yet: wide.
pub fn band_mode(b: usize, r: Option<ferrite_core::txplan::Region>) -> u8 {
    use ferrite_core::txplan::Region::{Europe, Japan, Other, Uk};
    match BANDS[b].mode {
        0 if matches!(r, Some(Uk | Europe | Japan | Other)) => 1,
        _ if b == BAND_AIR => NAM,
        m => m,
    }
}

/// A band's default step in a region (VFO-18): 2 m and 1.25 m in the US and Canada take 5 kHz, as their 15 and
/// 20 kHz channel plans (146.520, 146.940, 147.225) need; elsewhere the band's own (12.5 kHz on 2 m, IARU R1).
pub fn band_step(b: usize, r: Option<ferrite_core::txplan::Region>) -> u8 {
    use ferrite_core::txplan::Region::{Canada, Us};
    match b {
        BAND_VHF | BAND_125M if matches!(r, Some(Us | Canada)) => STEP_5K,
        _ => BANDS[b].step,
    }
}

/// The band a frequency is in, or None between bands.
pub fn band_of(f: Freq10) -> Option<usize> {
    BANDS.iter().position(|b| f >= b.lo && f < b.hi)
}

/// The band a frequency is in, or the nearest one below it (for ◀▶ from between bands).
pub fn band_near(f: Freq10) -> usize {
    band_of(f).unwrap_or_else(|| BANDS.iter().rposition(|b| b.lo <= f).unwrap_or(0))
}

/// The label on the slot: the band's name, or the chip range's.
pub fn band_name(f: Freq10) -> &'static str {
    match band_of(f) {
        Some(i) => BANDS[i].name,
        None if f < mhz(64) => "HF",
        None if f < mhz(300) => "VHF",
        None => "UHF",
    }
}

/// What the knob can change, wherever it shows: the quick settings rows (MENU 1-9, 0) and the edit grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    Squelch,
    Power,
    /// The tone that opens squelch (CTCSS or DCS decode); Tones is the one sent.
    RxTone,
    Tones,
    Offset,
    Step,
    Watch,
    Channel,
    BatterySave,
    Advanced,
    Mode,
    BusyLock,
    Reverse,
}

/// Quick settings, in order (MENU-1); row 9 is "0 More".
pub const QUICK: [Item; 10] = [
    Item::Squelch,
    Item::Power,
    Item::Mode,
    Item::Tones,
    Item::Offset,
    Item::Step,
    Item::Watch,
    Item::Channel,
    Item::BatterySave,
    Item::Advanced,
];

/// The edit-in-place grid for a VFO, row by row (EDIT-2, HomeEdit mock-up).
pub const GRID: [Item; 9] = [
    Item::Mode,
    Item::RxTone,
    Item::Tones,
    Item::Offset,
    Item::Power,
    Item::Step,
    Item::Squelch,
    Item::BusyLock,
    Item::Reverse,
];

impl Item {
    /// The one English name (MENU-7).
    pub fn name(self) -> &'static str {
        match self {
            Item::Squelch => "Squelch",
            Item::Power => "TX power",
            Item::RxTone => "RX tone",
            Item::Tones => "Tones",
            Item::Offset => "Offset",
            Item::Step => "Step",
            Item::Watch => "Watch",
            Item::Channel => "Channel",
            Item::BatterySave => "Battery Save",
            Item::Advanced => "More",
            Item::Mode => "Mode",
            Item::BusyLock => "Busy lock",
            Item::Reverse => "Reverse",
        }
    }

    /// The edit grid's shorter label.
    pub fn label(self) -> &'static str {
        match self {
            Item::Power => "Power",
            Item::Tones => "Tone",
            _ => self.name(),
        }
    }

    /// The help line (MENU-15), in the text pack (UPD-22).
    pub fn help(self, vfo: bool) -> crate::text::Txt {
        match self {
            Item::Squelch => txt!("Audio opens above this level. Home: SIDE1 + knob."),
            Item::Power => txt!("Transmit power on this slot. Home: hold 2."),
            Item::RxTone => txt!("The tone that opens squelch. Off: any carrier."),
            Item::Tones => txt!("CTCSS or DCS sent with TX. OK: RX and TX apart."),
            Item::Offset => txt!("Simplex, + or -, or Split: a TX freq. Hold 7."),
            Item::Step => txt!("How far a VFO tunes. Home: hold 6."),
            Item::Watch => txt!("Listen on two or three slots. Home: hold 4."),
            Item::Channel if vfo => txt!("Save this VFO as a channel. Home: hold 9."),
            Item::Channel => txt!("Edit this channel: name, zone, number, flags."),
            Item::BatterySave => txt!("Receiver sleep: deeper saves more, hears later."),
            Item::Advanced => txt!("Everything else, in nine groups."),
            Item::Mode => txt!("FM, NFM, AM, NAM (8.33 kHz). WFM: broadcast RX."),
            Item::BusyLock => txt!("Refuse transmit while the channel is busy."),
            Item::Reverse => txt!("Listen on the repeater's input. Hold *, PTT A."),
        }
    }

    /// The setting's own value on a slot's config, as an index the knob steps through, and how many there are.
    pub fn get(self, c: &SlotCfg) -> (i32, i32) {
        match self {
            // Global, then levels 0-9; a VFO skips Global ([`crate::App`]'s change).
            Item::Squelch => (if c.squelch >= SQ_GLOBAL { 0 } else { c.squelch as i32 + 1 }, 11),
            Item::Power => (c.power as i32, 3),
            Item::RxTone => (tone_index(c.rx_tone), TONES),
            Item::Tones => (tone_index(c.tx_tone), TONES),
            Item::Offset => (if c.split { 3 } else { (c.offset.signum() + 3) % 3 }, 4), // simplex, +, -, split
            Item::Step => (c.step as i32, STEPS.len() as i32),
            Item::Mode => (SLOT_MODE_IDS.iter().position(|&m| m == c.mode).unwrap_or(0) as i32, SLOT_MODES as i32),
            Item::BusyLock => (c.busy.min(1) as i32, 2),
            Item::Reverse => (c.reverse as i32, 2),
            _ => (0, 1),
        }
    }

    /// Sets the value (wrapping) on a slot's config; returns the radio parameter that changed.
    pub fn set(self, c: &mut SlotCfg, v: i32, shift: i32) -> Option<(Param, i32)> {
        let n = self.get(c).1;
        let v = v.rem_euclid(n);
        Some(match self {
            Item::Squelch => {
                c.squelch = if v == 0 { SQ_GLOBAL } else { v as u8 - 1 };
                (Param::Squelch, c.squelch as i32) // SQ_GLOBAL: the App sends the slot's level
            }
            Item::Power => {
                c.power = v as u8;
                (Param::Power, v)
            }
            Item::RxTone => {
                c.rx_tone = tone_at(v);
                (Param::RxTone, c.rx_tone as i32)
            }
            Item::Tones => {
                c.tx_tone = tone_at(v);
                (Param::TxTone, c.tx_tone as i32)
            }
            // Split needs a TX frequency: the App asks for it (frequency entry) and sets it with `set_split`.
            Item::Offset if v == 3 => return None,
            Item::Offset => {
                // VFO-26: + and - keep the amount the slot has (a custom one from hold 8 too); from simplex or
                // split, the band's
                let shift = if !c.split && c.offset != 0 { c.offset.abs() } else if shift == 0 { 60_000 } else { shift };
                (c.offset, c.split) = ([0, shift, -shift][v as usize], false);
                (Param::Offset, c.offset)
            }
            // VFO-4.2: the 8.33 kHz step on AM goes to NAM, the filter 8.33 kHz channels need
            Item::Step if v as u8 == STEP_833 && c.mode == 2 => {
                (c.step, c.mode, c.narrow) = (STEP_833, NAM, true);
                (Param::Mode, NAM as i32)
            }
            Item::Step => {
                c.step = v as u8;
                return None; // the UI's own: the radio only sees frequencies
            }
            Item::Mode => {
                c.mode = SLOT_MODE_IDS[v as usize % SLOT_MODES];
                c.narrow = mode_narrow(c.mode); // VFO-20: the App sends the bandwidth with it
                (Param::Mode, c.mode as i32)
            }
            Item::BusyLock => {
                c.busy = v as u8;
                (Param::BusyLock, v)
            }
            Item::Reverse => {
                c.reverse = v == 1;
                (Param::Reverse, v)
            }
            _ => return None,
        })
    }

    /// The default (MENU-18; the dot of MENU-12 marks anything else).
    pub fn default_of(self) -> i32 {
        self.get(&DEFAULT_CFG).0
    }

    /// The value as text: long in menus ("-600 kHz"), short in the grid and chips ("-600k").
    pub fn text(self, c: &SlotCfg, long: bool, b: &mut Buf<16>) {
        b.clear();
        match self {
            Item::Squelch if c.squelch >= SQ_GLOBAL => b.set(b"Global"),
            Item::Squelch if c.squelch == 0 && long => b.set(b"0 (open)"), // SQL: level 0 is squelch off
            Item::Squelch => {
                let _ = uwrite!(b, "{}", c.squelch);
            }
            Item::Power => b.set(["Low", "Mid", "High"][c.power as usize % 3].as_bytes()),
            Item::RxTone => {
                tone_text(c.rx_tone, b);
                if b.is_empty() {
                    b.set(b"Off");
                }
            }
            Item::Tones => {
                tone_text(c.tx_tone, b);
                if b.is_empty() {
                    b.set(b"Off");
                }
            }
            Item::Offset => {
                shift_text(c, long, b);
                if b.is_empty() {
                    b.set(b"Simplex");
                }
            }
            Item::Step => {
                let s = STEPS[c.step as usize % STEPS.len()].1;
                if long {
                    b.set(s.as_bytes());
                } else {
                    // "12.5 kHz" -> "12.5k", "500 Hz" -> "500 Hz"
                    match s.strip_suffix(" kHz") {
                        Some(k) => {
                            b.set(k.as_bytes());
                            b.push(b"k");
                        }
                        None => b.set(s.as_bytes()),
                    }
                }
            }
            Item::Mode => b.set(MODES[c.mode as usize % MODES.len()].as_bytes()),
            Item::BusyLock => b.set(if c.busy != 0 { b"On" } else { b"Off" }),
            Item::Reverse => b.set(if c.reverse { b"On" } else { b"Off" }),
            _ => {}
        }
    }
}

/// Every radio parameter of a config, for when a slot moves to another channel.
pub fn params(c: &SlotCfg) -> [(Param, i32); 10] {
    [
        (Param::Mode, c.mode as i32),
        (Param::Bandwidth, c.narrow as i32),
        (Param::Squelch, c.squelch as i32),
        (Param::RxTone, c.rx_tone as i32),
        (Param::TxTone, c.tx_tone as i32),
        (Param::Offset, c.offset),
        (Param::Power, c.power as i32),
        (Param::BusyLock, c.busy as i32),
        (Param::Reverse, c.reverse as i32),
        (Param::Split, if c.split { c.offset } else { 0 }),
    ]
}

/// The mode chip: the mode's name, as everywhere else (NFM, not the cards' old "FM-N": decision 417).
pub fn mode_chip(c: &SlotCfg, b: &mut Buf<16>) {
    b.set(MODES[c.mode as usize % MODES.len()].as_bytes());
}

/// The clarifier in 100 Hz as "CLR +0.3" (VFO-22).
pub fn clar_text(c: i8, b: &mut Buf<16>) {
    b.set(b"CLR ");
    b.push(if c < 0 { b"-" } else { b"+" });
    let _ = uwrite!(b, "{}.{}", c.unsigned_abs() / 10, c.unsigned_abs() % 10);
}

/// "CH 012": menus, lists and editors.
pub fn chan_label(num: u16, b: &mut Buf<32>) {
    let _ = uwrite!(b, "CH {}", Zero(num as u32 + 1, 3));
}

/// "012": the slot cards' header, where the name beside it already says it is a channel (HOME-4).
pub fn chan_num(num: u16, b: &mut Buf<32>) {
    let _ = uwrite!(b, "{}", Zero(num as u32 + 1, 3));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(b: &Buf<16>) -> &str {
        core::str::from_utf8(b.as_bytes()).unwrap()
    }

    #[test]
    fn tones_round_trip() {
        let mut b = Buf::new();
        for i in 0..TONES {
            assert_eq!(tone_index(tone_at(i)), i, "tone {i}");
        }
        tone_text(1000, &mut b);
        assert_eq!(s(&b), "T 100.0");
        tone_text(DCS_FLAG | 0o023, &mut b);
        assert_eq!(s(&b), "D023N");
        tone_text(DCS_FLAG | DCS_INV | 0o754, &mut b);
        assert_eq!(s(&b), "D754I");
        assert_eq!(tone_at(-1), DCS_FLAG | DCS_INV | 0o754, "wraps backwards");
    }

    #[test]
    fn texts() {
        let mut b = Buf::new();
        offset_text(-60_000, false, &mut b);
        assert_eq!(s(&b), "-600k");
        offset_text(500_000, false, &mut b);
        assert_eq!(s(&b), "+5M");
        offset_text(160_000, true, &mut b);
        assert_eq!(s(&b), "+1.6 MHz");
        let c = DEFAULT_CFG;
        Item::Step.text(&c, false, &mut b);
        assert_eq!(s(&b), "12.5k");
        Item::Step.text(&c, true, &mut b);
        assert_eq!(s(&b), "12.5 kHz");
        mode_chip(&c, &mut b);
        assert_eq!(s(&b), "NFM");
    }

    #[test]
    fn bands() {
        assert_eq!(band_name(mhz(146) + 52_000), "VHF");
        assert_eq!(band_name(mhz(443) + 47_500), "UHF");
        assert_eq!(band_name(mhz(118) + 95_000), "Airband");
        assert_eq!(band_of(mhz(223)), Some(BAND_125M));
        assert_eq!(band_near(mhz(300)), BAND_125M);
        for b in &BANDS {
            assert!(b.lo <= b.start && b.start < b.hi, "{}", b.name);
        }
    }

    #[test]
    fn help_fits_two_lines() {
        // The help box holds two lines of 29 characters, with the path after the text where it fits.
        for it in QUICK.iter().chain(&GRID) {
            for vfo in [true, false] {
                let h = crate::text::load(it.help(vfo));
                assert!(h.as_bytes().len() <= 50, "{:?}: {}", it, h.as_str());
            }
        }
    }

    #[test]
    fn items_set_and_wrap() {
        let mut c = DEFAULT_CFG;
        assert_eq!(Item::Offset.set(&mut c, 2, 60_000), Some((Param::Offset, -60_000)));
        assert_eq!(Item::Offset.get(&c).0, 2);
        Item::Squelch.set(&mut c, 11, 0);
        assert_eq!(c.squelch, SQ_GLOBAL, "wraps to Global");
        Item::Squelch.set(&mut c, 1, 0);
        assert_eq!(c.squelch, 0);
        for it in GRID {
            assert_eq!(it.get(&DEFAULT_CFG).0, it.default_of());
        }
    }
}
