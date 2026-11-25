//! A channel's fields and their rules, written once (decision 223): the radio's channel editor
//! ([`crate::screens::chedit`]), the terminal's ([`crate::term`]'s Page::Edit), the command line's `ch` and `load`
//! ([`crate::term::cmd`]) and the link's channel writes all step, type, reset and check a channel here, and each
//! keeps only its own layout and words (the grid and its notes, the terminal's rows, `key=value`).
//!
//! A change works on a copy and is kept only when the whole channel still passes [`check_edit`] (CHAN-22, CHAN-26,
//! TX-37); a refusal names its field, so each caller heads it its own way ("Power: ...", `power: ...`).

use crate::chan::{rx_ok, Channel, FLAG_APRS, FLAG_HIDDEN, FLAG_PRIORITY, FLAG_RX_ONLY, FLAG_SKIP, NO_ZONE, ZoneId};
use crate::slot::{band_near, band_step, Item, STEPS};
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::txplan::Region;
use ferrite_ui::text::Buf;

/// One field of a channel, as the editors and `ch` change it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChField {
    Name,
    /// Every zone, then none (CHAN-10); a new zone moves the channel (EDIT-5).
    Zone,
    Rx,
    /// Split's TX frequency (VFO-16): typed, or stepped while split.
    Tx,
    /// A slot setting the channel holds (EDIT-2): Mode, RX tone, Tone, Offset, Power, Squelch, Busy lock.
    It(Item),
    /// A flag on or off (CHAN-4): APRS, RX only, Hidden.
    Flag(u8),
    /// In scans, Skip or Priority (decision 63, CHAN-24): the two flags exclude each other.
    Scan,
    /// The one home channel (CHAN-17, CHAN-18).
    Home,
}

/// What a step needs besides the channel: how many zones exist, the region (a band's step, VFO-18), and which
/// zones have room for the channel (CHAN-35: a full zone is stepped over).
#[derive(Clone, Copy)]
pub struct Ctx<'a> {
    pub zones: ZoneId,
    pub region: Option<Region>,
    pub room: &'a dyn Fn(ZoneId) -> bool,
}

/// A [`Ctx::room`] for every zone (no store to ask).
pub fn any_room(_: ZoneId) -> bool {
    true
}

/// Why a change was refused: [`Refusal::field`] heads it, [`Refusal::why`] says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// CHAN-22: not a frequency a channel holds.
    RxRange,
    /// CHAN-26: an HF channel's mode is HF mode's.
    HfMode,
    /// CHAN-26: an HF channel is simplex.
    HfSimplex,
    /// CHAN-23: USB, LSB and CW only on HF channels.
    NotHf,
    /// TX-37: TX would need the band relay in the other state from RX.
    Relay,
    /// CHAN-22: a TX apart from RX (offset or split) stays within 64-1000 MHz.
    TxRange,
    /// CHAN-4: a receive-only channel has no power to set.
    RxOnly,
    /// CHAN-31: printable ASCII in (other characters map to '-').
    NameChars,
    /// CHAN-5: 16 characters.
    NameLong,
}

/// The scan choices' words (decision 63).
pub const SCAN_TEXT: [&str; 3] = ["In scans", "Skip", "Priority"];

impl Refusal {
    /// The field it is about, None when [`Refusal::why`] is a whole sentence.
    pub fn field(self) -> Option<ChField> {
        Some(match self {
            Refusal::RxRange => ChField::Rx,
            Refusal::HfMode | Refusal::NotHf => ChField::It(Item::Mode),
            Refusal::HfSimplex | Refusal::TxRange => ChField::It(Item::Offset),
            Refusal::Relay => return None,
            Refusal::RxOnly => ChField::It(Item::Power),
            Refusal::NameChars | Refusal::NameLong => ChField::Name,
        })
    }

    pub fn why(self) -> &'static str {
        match self {
            Refusal::RxRange => "0.15-64 MHz (HF), 64-630 or 756-1300 MHz",
            Refusal::HfMode => "an HF channel is AM, USB, LSB or CW (FM or NFM from 18 MHz)",
            Refusal::HfSimplex => "an HF channel is simplex",
            Refusal::NotHf => "USB, LSB and CW are for HF channels",
            Refusal::Relay => "Split TX needs the HF relay: not allowed with this RX frequency",
            Refusal::TxRange => "TX outside 64 to 1000 MHz",
            Refusal::RxOnly => "this channel is RX only",
            Refusal::NameChars => "letters, digits and punctuation only",
            Refusal::NameLong => "16 characters at most",
        }
    }

    /// "Head: why" with the field's label (the editors' "Power: this channel is RX only"), or the sentence alone.
    pub fn line<const N: usize>(self, out: &mut Buf<N>) {
        out.clear();
        if let Some(f) = self.field() {
            out.push(f.label(false).as_bytes());
            out.push(b": ");
        }
        out.push(self.why().as_bytes());
    }
}

/// A channel the radio can hold, as any edit leaves it (CHAN-22, CHAN-23, CHAN-26, TX-37). A TX apart from RX
/// (an offset or split) stays within 64-1000 MHz; a simplex channel above 1000 MHz is receive only (VFO-21).
pub fn check_edit(c: &Channel) -> Result<(), Refusal> {
    if !rx_ok(c.rx) {
        return Err(Refusal::RxRange);
    }
    let g = &c.cfg;
    if c.is_hf() {
        if !crate::screens::hf::channel_mode_ok(c.rx, g.mode) {
            return Err(Refusal::HfMode);
        }
        if g.offset != 0 || g.split {
            return Err(Refusal::HfSimplex);
        }
        return Ok(());
    }
    if !crate::slot::slot_mode_ok(g.mode) {
        return Err(Refusal::NotHf);
    }
    if !ferrite_core::txplan::relay_ok(g.rx_of(c.rx), g.tx_of(c.rx)) {
        return Err(Refusal::Relay);
    }
    if (g.split || g.offset != 0) && !(mhz(64)..mhz(1000)).contains(&g.tx_of(c.rx)) {
        return Err(Refusal::TxRange);
    }
    Ok(())
}

/// A change from `old` to `new` (an editor's step, # or typed value, `ch N key value`): [`check_edit`], and a power
/// changed on a receive-only channel (CHAN-4) refused.
pub fn check_change(old: &Channel, new: &Channel) -> Result<(), Refusal> {
    if new.cfg.power != old.cfg.power && new.flags & FLAG_RX_ONLY != 0 {
        return Err(Refusal::RxOnly);
    }
    check_edit(new)
}

/// A whole channel written at once (`ch N "name" ...`, `load`, the link: TERM-7, LINK-22): [`check_edit`].
pub fn check(c: &Channel) -> Result<(), Refusal> {
    check_edit(c)
}

/// `f` stepped `d` times by its band's step in the region (VFO-18), kept to the part of the range it is in: an HF
/// channel below 64 MHz, the others 64-1300 MHz stepping over 630-756 MHz, which no chip receives (CHAN-22).
fn freq_step(f: Freq10, d: i32, region: Option<Region>) -> Freq10 {
    let step = STEPS[band_step(band_near(f), region) as usize].0 as i32;
    let (lo, hi) = if f < crate::chan::HF_TOP { (crate::chan::CH_LO, crate::chan::HF_TOP - 1) } else { (mhz(64), mhz(1300)) };
    let n = (f as i32 + d * step).clamp(lo as i32, hi as i32) as Freq10;
    match n {
        _ if (mhz(630) + 1..mhz(756)).contains(&n) => if d > 0 { mhz(756) } else { mhz(630) },
        _ => n,
    }
}

/// An item's value `v` (its knob index) on a channel; Offset's 4th choice is Split on today's TX (VFO-16, as MENU
/// 5's knob), the others with the band's shift.
fn set_item(c: &mut Channel, it: Item, v: i32, region: Option<Region>) {
    if it == Item::Offset && v.rem_euclid(4) == 3 {
        let g = &mut c.cfg;
        let tx = if g.split { g.offset.max(0) } else { (c.rx as i64 + g.offset as i64).max(0) as i32 };
        (g.split, g.offset) = (true, tx);
    } else {
        it.set(&mut c.cfg, v, crate::slot::band_shift(band_near(c.rx), region));
    }
}

impl ChField {
    /// Its name: short in the radio's grid ("Power"), long in the terminal's rows ("TX power").
    pub fn label(self, long: bool) -> &'static str {
        match self {
            ChField::Name => "Name",
            ChField::Zone => "Zone",
            ChField::Rx => "RX",
            ChField::Tx => "TX",
            ChField::It(i) if long => i.name(),
            ChField::It(i) => i.label(),
            ChField::Flag(FLAG_APRS) => "APRS",
            ChField::Flag(FLAG_RX_ONLY) => "RX only",
            ChField::Flag(FLAG_HIDDEN) => "Hidden",
            ChField::Flag(FLAG_SKIP) => "Skip",
            ChField::Flag(FLAG_PRIORITY) => "Priority",
            ChField::Flag(_) => "Flag",
            ChField::Scan => "Scan",
            ChField::Home => "Home",
        }
    }

    /// The value as text: an item long or short ([`Item::text`]; power as set), a flag
    /// On/Off, the scan choice, Home Yes/No, the name. Zone, RX and TX are each caller's own (zone names, digits).
    pub fn text(self, c: &Channel, home: Option<u16>, long: bool, b: &mut Buf<16>) {
        b.clear();
        match self {
            ChField::It(it) => it.text(&c.cfg, long, b),
            ChField::Flag(bit) => b.set(if c.flags & bit != 0 { b"On" } else { b"Off" }),
            ChField::Scan => b.set(SCAN_TEXT[c.scan() as usize].as_bytes()),
            ChField::Home => b.set(if home == Some(c.num) { b"Yes" } else { b"No" }),
            ChField::Name => b.set(c.name()),
            ChField::Zone | ChField::Rx | ChField::Tx => {}
        }
    }

    /// A change to the field (kept only when it passes [`check_change`]; `home` is the radio's home channel):
    /// - [`Op::Step`], the knob or ◀▶ (EDIT-3): zones wrap through none, frequencies step by their band, items through
    ///   their values, a flag toggles, Scan cycles, Home toggles. The name, and TX while not split, are typed: unchanged.
    /// - [`Op::Reset`], # (MENU-18): an item's default (a channel's squelch is Global, SQL-6), a flag off, in scans, not
    ///   home. The zone, name and frequencies have none: unchanged.
    /// - [`Op::Typed`]: the name (printable ASCII, 16 at most; CHAN-31 maps the rest), RX in MHz, or the split's TX in
    ///   MHz (split on).
    pub fn change(self, c: &mut Channel, home: &mut Option<u16>, op: Op, cx: Ctx<'_>) -> Result<(), Refusal> {
        let mut n = *c;
        match (self, op) {
            (ChField::Home, Op::Step(_)) => {
                *home = if *home == Some(c.num) { None } else { Some(c.num) };
                return Ok(());
            }
            (ChField::Home, Op::Reset) => {
                *home = home.filter(|&h| h != c.num);
                return Ok(());
            }
            (ChField::Zone, Op::Step(d)) => {
                // CHAN-35: a zone with 256 channels already is stepped over; none (no limit) always takes it
                let z = cx.zones as i32 + 1;
                let i = if c.zone == NO_ZONE { z - 1 } else { c.zone as i32 };
                for k in 1..=z {
                    let j = (i + d * k).rem_euclid(z);
                    n.zone = if j == z - 1 { NO_ZONE } else { j as ZoneId };
                    if n.zone == NO_ZONE || n.zone == c.zone || (cx.room)(n.zone) {
                        break;
                    }
                }
            }
            (ChField::Rx, Op::Step(d)) => n.rx = freq_step(c.rx, d, cx.region),
            (ChField::Tx, Op::Step(d)) if c.cfg.split => {
                n.cfg.offset = freq_step(c.cfg.offset.max(0) as Freq10, d, cx.region).clamp(mhz(64), mhz(1000) - 1) as i32
            }
            (ChField::It(Item::Mode), Op::Step(d)) if d.abs() == 1 => {
                // CHAN-26.2 (ferrite#84): the step goes on past the modes the frequency doesn't have (an 18-64 MHz
                // channel: FM, NFM, AM), so the knob reaches every one it does; none: the first step's refusal
                let at = Item::Mode.get(&c.cfg).0 + d;
                let mut why = None;
                for k in 0..crate::slot::SLOT_MODES as i32 - 1 {
                    set_item(&mut n, Item::Mode, at + k * d, cx.region);
                    match check_change(c, &n) {
                        Ok(()) => {
                            *c = n;
                            return Ok(());
                        }
                        Err(r @ (Refusal::HfMode | Refusal::NotHf)) => why = why.or(Some(r)),
                        Err(r) => return Err(why.unwrap_or(r)),
                    }
                }
                return Err(why.unwrap_or(Refusal::HfMode));
            }
            (ChField::It(it), Op::Step(d)) => set_item(&mut n, it, it.get(&c.cfg).0 + d, cx.region),
            (ChField::It(Item::Squelch), Op::Reset) => set_item(&mut n, Item::Squelch, 0, cx.region),
            (ChField::It(it), Op::Reset) => set_item(&mut n, it, it.default_of(), cx.region),
            (ChField::Flag(bit), Op::Step(_)) => n.flags ^= bit,
            (ChField::Flag(bit), Op::Reset) => n.flags &= !bit,
            (ChField::Scan, Op::Step(d)) => n.set_scan((c.scan() as i32 + d).rem_euclid(3) as u8),
            (ChField::Scan, Op::Reset) => n.set_scan(0),
            (ChField::Name, Op::Typed(s)) if s.iter().any(|b| !(0x20..0x7F).contains(b)) => return Err(Refusal::NameChars),
            (ChField::Name, Op::Typed(s)) if s.len() > 16 => return Err(Refusal::NameLong),
            (ChField::Name, Op::Typed(s)) => n.set_name(s),
            (ChField::Rx, Op::Typed(s)) => n.rx = parse_mhz(s).ok_or(Refusal::RxRange)?,
            (ChField::Tx, Op::Typed(s)) => (n.cfg.split, n.cfg.offset) = (true, parse_mhz(s).ok_or(Refusal::TxRange)? as i32),
            _ => return Ok(()),
        }
        check_change(c, &n)?;
        *c = n;
        Ok(())
    }
}

/// What [`ChField::change`] does.
#[derive(Clone, Copy, Debug)]
pub enum Op<'a> {
    /// The knob or ◀▶, this many steps.
    Step(i32),
    /// #: back to the default.
    Reset,
    /// A value typed.
    Typed(&'a [u8]),
}

/// "146.52" or "146.520 00" typed: MHz to 10 Hz steps.
pub(crate) fn parse_mhz(s: &[u8]) -> Option<Freq10> {
    let (mut int, mut frac, mut places, mut dot, mut any) = (0u32, 0u32, 0u32, false, false);
    for &c in s {
        match c {
            b'0'..=b'9' if !dot => {
                int = int.checked_mul(10)?.checked_add((c - b'0') as u32)?;
                any = true;
            }
            b'0'..=b'9' if places < 5 => {
                frac = frac * 10 + (c - b'0') as u32;
                places += 1;
                any = true;
            }
            b'0'..=b'9' | b' ' => {}
            b'.' | b',' if !dot => dot = true,
            _ => return None,
        }
    }
    if !any || int > 9_999 {
        return None;
    }
    Some(int * 100_000 + frac * 10u32.pow(5 - places))
}

#[cfg(test)]
mod tests;
