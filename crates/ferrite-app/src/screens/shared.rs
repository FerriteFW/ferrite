//! What the screens share for drawing: region dirty-tracking, the status bar, the hint bar and toast, list
//! rows, the edit grid's cells, the help box, and the slot texts several screens show. Each region of the
//! screen is hashed from what it shows; a screen draws a region only when its hash changed (HOME-7), so a meter
//! update never touches the frequency and a still screen costs nothing. Layouts follow the look review's
//! mock-ups (`docs/img/screens`).

use crate::chan::ChannelStore;
use crate::slot::band_name;
use crate::App;
use core::hash::{Hash, Hasher};
use ferrite_core::msg::{Device, LiveAll};
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets::{self as w, Row, StatusBar};

// Regions. Screens reuse the numbers; a screen change clears the content and forgets every hash.
/// A region's hash after its box was just filled with its background: [`App::dirty_shape`] draws it without a
/// blank (bit 1 clear, so no hash `dirty_shape` makes equals it).
pub(crate) const FRESH: u32 = 0xFFFF_FFF0;
pub(crate) const R_HINT: usize = 0;
pub(crate) const R_FRAME: usize = 1; // home: three cards' frames (1-3)
pub(crate) const R_HEAD: usize = 4; // card headers (4-6)
pub(crate) const R_FREQ: usize = 7; // frequencies (7-9)
pub(crate) const R_INFO: usize = 10; // chips, or the compact card's name (10-12)
pub(crate) const R_SUB: usize = 13; // S-meter text, or the compact card's second line (13-15)
pub(crate) const R_ROW: usize = 1; // lists and the edit grid: rows 1-13
pub(crate) const R_TOP: usize = 14; // menu header / Recent row / edit header
pub(crate) const R_HELP: usize = 15;
pub(crate) const R_NOTE: usize = 16;
pub(crate) const R_LINE: usize = 17; // edit: the other two slots (17-18)
pub(crate) const R_PAGE: usize = 19; // a screen's page: see App::new_page
pub(crate) const R_SCROLL: usize = 20; // the scroll cue (DISP-24)
pub(crate) const REGIONS: usize = 21;

/// What a screen draws from besides the App: the Radio task's snapshot, the radio-wide state, the channels.
pub struct Cx<'a, S: ChannelStore> {
    pub live: &'a LiveAll,
    pub dev: &'a Device,
    pub store: &'a S,
}

/// The hint bar's key + label pairs (KEY-9): at most three ([`w::HINT_MAX`]; four overflow 240 px at 12 px).
/// One rule: a screen lists its three most important, in the order they show; what it leaves out first is the
/// universal (BACK Back, which every screen has), then the least used of its own keys.
#[derive(Clone, Copy, Debug)]
pub struct Hints {
    items: [(&'static str, &'static str); w::HINT_MAX],
    n: u8,
}

impl Hints {
    pub const fn new(items: &[(&'static str, &'static str)]) -> Self {
        assert!(items.len() <= w::HINT_MAX, "at most three hints (KEY-9)");
        let mut h = Hints { items: [("", ""); w::HINT_MAX], n: 0 };
        while (h.n as usize) < items.len() && (h.n as usize) < w::HINT_MAX {
            h.items[h.n as usize] = items[h.n as usize];
            h.n += 1;
        }
        h
    }

    pub fn as_slice(&self) -> &[(&'static str, &'static str)] {
        &self.items[..self.n as usize]
    }
}

pub(crate) const ARROWS_LR: &str = "\x1b\x1a";
pub(crate) const ARROWS_UD: &str = "\x18\x19";
pub(crate) const ARROWS_4: &str = "\x1b\x18\x19\x1a";

/// FNV-1a, for region hashes.
struct Fnv(u32);
impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0 as u64
    }
    fn write(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ x as u32).wrapping_mul(16_777_619);
        }
    }
}

/// A region's hash mixed value by value (FNV-1a), one small path: `Mix::new().n(f).b(name).get()`. The generic
/// [`hash`] of a tuple is made again at each call site for its field types; a direct mix is two instructions and a
/// call a value. Numbers go in as u32 (`as u32` for bools, enums and small ints), text as bytes.
#[derive(Clone, Copy)]
pub(crate) struct Mix(u32);

impl Mix {
    pub(crate) const fn new() -> Mix {
        Mix(2_166_136_261)
    }
    #[inline(never)]
    pub(crate) fn b(self, s: &[u8]) -> Mix {
        let mut h = self.0;
        for &x in s {
            h = (h ^ x as u32).wrapping_mul(16_777_619);
        }
        Mix(h.rotate_left(5) ^ s.len() as u32)
    }
    #[inline(never)]
    pub(crate) fn n(self, v: u32) -> Mix {
        let mut h = self.0;
        for k in 0..4 {
            h = (h ^ (v >> (8 * k)) & 0xFF).wrapping_mul(16_777_619);
        }
        Mix(h)
    }
    /// Any value that knows how it mixes ([`MixIn`]): numbers as u32, text as bytes.
    #[inline(always)]
    pub(crate) fn h<T: MixIn + ?Sized>(self, t: &T) -> Mix {
        t.mix(self)
    }
    /// A value of a type with only `Hash` (made per type: for the few that aren't [`MixIn`]).
    #[inline(never)]
    pub(crate) fn any<T: Hash + ?Sized>(self, t: &T) -> Mix {
        self.n(hash(t))
    }
    /// Never 0, which means "not drawn".
    pub(crate) fn get(self) -> u32 {
        self.0 | 1
    }
}

/// How a value goes into a region's [`Mix`]: one call, inlined at the call site.
pub(crate) trait MixIn {
    fn mix(&self, m: Mix) -> Mix;
}

macro_rules! mix_num {
    ($($t:ty),*) => {$(
        impl MixIn for $t {
            #[inline(always)]
            fn mix(&self, m: Mix) -> Mix {
                m.n(*self as u32)
            }
        }
    )*};
}
mix_num!(u8, u16, u32, usize, i8, i16, i32, bool, char);

impl MixIn for u64 {
    #[inline(always)]
    fn mix(&self, m: Mix) -> Mix {
        m.n(*self as u32).n((*self >> 32) as u32)
    }
}
impl MixIn for i64 {
    #[inline(always)]
    fn mix(&self, m: Mix) -> Mix {
        (*self as u64).mix(m)
    }
}
impl MixIn for [u8] {
    #[inline(always)]
    fn mix(&self, m: Mix) -> Mix {
        m.b(self)
    }
}
impl MixIn for str {
    #[inline(always)]
    fn mix(&self, m: Mix) -> Mix {
        m.b(self.as_bytes())
    }
}
impl<const N: usize> MixIn for Buf<N> {
    #[inline(always)]
    fn mix(&self, m: Mix) -> Mix {
        m.b(self.as_bytes())
    }
}
impl<T: MixIn + ?Sized> MixIn for &T {
    #[inline(always)]
    fn mix(&self, m: Mix) -> Mix {
        (**self).mix(m)
    }
}
impl<const N: usize> MixIn for crate::settings::Text<N> {
    #[inline(always)]
    fn mix(&self, m: Mix) -> Mix {
        m.b(self.as_bytes())
    }
}
impl<T: MixIn, const N: usize> MixIn for [T; N] {
    #[inline(always)]
    fn mix(&self, m: Mix) -> Mix {
        self.iter().fold(m, |m, v| v.mix(m))
    }
}
macro_rules! mix_tuple {
    ($($n:ident $i:tt),*) => {
        impl<$($n: MixIn),*> MixIn for ($($n,)*) {
            #[inline(always)]
            fn mix(&self, m: Mix) -> Mix {
                m$(.h(&self.$i))*
            }
        }
    };
}
mix_tuple!(A 0, B 1);
mix_tuple!(A 0, B 1, C 2);
mix_tuple!(A 0, B 1, C 2, D 3);
impl<T: MixIn> MixIn for Option<T> {
    #[inline(always)]
    fn mix(&self, m: Mix) -> Mix {
        match self {
            Some(v) => v.mix(m.n(1)),
            None => m.n(0),
        }
    }
}

pub(crate) fn hash<T: Hash + ?Sized>(t: &T) -> u32 {
    let mut h = Fnv(2_166_136_261);
    t.hash(&mut h);
    h.0 | 1 // never 0, which means "not drawn"
}

/// Off, A+B, A+B+C.
pub(crate) const WATCH_TEXT: [&str; 3] = ["Off", "A+B", "A+B+C"];

/// HOME-13: how long the knob rests before home draws its heavier parts again.
pub const KNOB_REST_MS: u32 = 400;

impl App {
    /// HOME-13: the knob stepped within [`KNOB_REST_MS`]. Signed, since a step can carry a time just ahead of
    /// the last tick's.
    /// DISP-31: some regions wait on purpose now, and a fresh repaint may differ until they don't: the knob turning
    /// (HOME-13), home under a shortcut's popup (HOME-8) and under the quick menu (HOME-17): each waits until it goes.
    pub fn redraw_deferred(&self) -> bool {
        self.knob_busy()
            || self.screen == crate::Screen::Home && self.drawn.popup != 0 && self.toast.is_some_and(|t| t.center)
            || super::qmenu::frozen(self)
    }

    pub(crate) fn knob_busy(&self) -> bool {
        self.knob_at.is_some_and(|t| (self.now.wrapping_sub(t) as i32) < KNOB_REST_MS as i32)
    }

    /// HOME-13: region r is on the screen and waits while the knob turns; it redraws once the knob rests (its
    /// hash still differs then). A region not yet drawn is never held.
    pub(crate) fn held(&self, r: usize) -> bool {
        self.drawn.hash[r] != 0 && self.knob_busy()
    }

    /// Regions `r..r + n` count as not drawn: they repaint on their next check (a box drawn over them).
    pub(crate) fn forget(&mut self, r: usize, n: usize) {
        self.drawn.hash[r..r + n].iter_mut().for_each(|h| *h = 0);
    }

    /// True (and remembered) when region r's content differs from what is on screen.
    #[inline(never)] // one copy for every region check (size review #9)
    pub(crate) fn dirty(&mut self, r: usize, h: u32) -> bool {
        let d = self.drawn.hash[r] != h;
        self.drawn.hash[r] = h;
        d
    }

    /// As [`App::dirty`] for a box whose content can take two layouts (`shape`: e.g. a bare frequency or a name
    /// over a small one). Some(true): blank the box first (first draw, or the layout changed); Some(false): draw
    /// straight over it, since each part repaints its own rows and the rest is already background. Blanking on
    /// every change showed as tearing on the radio while the knob turned (Oct 12, 2026). The layout rides in the
    /// hash's low bit; bit 1 keeps it non-zero (0 = not drawn).
    pub(crate) fn dirty_shape(&mut self, r: usize, h: u32, shape: bool) -> Option<bool> {
        let prev = self.drawn.hash[r];
        let h = (h & !3) | 2 | shape as u32;
        if !self.dirty(r, h) {
            return None;
        }
        Some(prev != FRESH && (prev == 0 || prev & 1 != shape as u32))
    }

    /// DISP-24: the scroll cue beside rows `R_ROW + from ..+ count`, drawn from `y` for `h` when there are rows
    /// above or below the page (`top` of `n`, `shown` on it); redrawn whenever those rows redraw (a list's blank
    /// rows paint full width, over the cue) or the page moves, and cleared once a list no longer scrolls. A page
    /// whose every row draws its own part of the cue (a [`super::table::Table`]) passes no rows: a row's change
    /// then redraws that row only, not the whole cue as well (Oct 13, 2026).
    pub(crate) fn scroll_cue(&mut self, lcd: &mut impl Panel, rows: (usize, usize), y: i32, h: i32, more: bool, top: u32, shown: u32, n: u32) {
        let under = Mix::new().any(&self.drawn.hash[R_ROW + rows.0..(R_ROW + rows.0 + rows.1).min(R_PAGE)]).get();
        let prev = self.drawn.hash[R_SCROLL];
        // bit 0: the cue is on the screen
        if !self.dirty(R_SCROLL, (Mix::new().h(&under).h(&y).h(&h).h(&more).h(&top).h(&shown).h(&n).get() & !1) | more as u32) {
            return;
        }
        if more {
            self.drawn.cue = w::Cue::new(y, h, top, shown, n);
            w::scroll_cue(lcd, self.drawn.cue);
        } else if prev & 1 != 0 {
            lcd.fill(w::CUE_X, y, 3, h, BG);
        }
    }

    /// The scroll cue on the screen, for the rows beside it to draw with (none once the region is forgotten).
    pub(crate) fn cue(&self) -> w::Cue {
        if self.drawn.hash[R_SCROLL] & 1 != 0 { self.drawn.cue } else { w::Cue::default() }
    }

    /// A screen with pages (setup, About, a list and its detail): when the page changes, the content area is
    /// cleared and every region but the hint bar forgotten, as on a screen change. Returns true then.
    pub(crate) fn page<K: Hash>(&mut self, lcd: &mut impl Panel, key: K) -> bool {
        if !self.dirty(R_PAGE, Mix::new().any(&key).get() | 1) {
            return false;
        }
        lcd.fill(0, w::SBAR_H, 240, w::HINT_Y - w::SBAR_H, BG);
        self.drawn.hash[R_HINT + 1..R_PAGE].fill(0);
        true
    }

    /// [`App::page`] by number.
    pub(crate) fn new_page(&mut self, lcd: &mut impl Panel, page: u32) -> bool {
        self.page(lcd, page)
    }

    pub(crate) fn status_bar(&self, live: &LiveAll, dev: &Device) -> StatusBar {
        let mut name = Buf::<10>::new();
        self.screen.title(self, &mut name);
        // SBAR-16: TW with three slots watched, DW with two, counting the relay rule's drops.
        let m = self.watch_mask() & !self.relay_mask;
        // HF-5: in HF mode TW/DW and SCN are absent (everything pauses); SBAR-18: the alerts-paused bell.
        // TX-34: watching pauses while transmitting, so no TW/DW then either.
        let hf = super::hf::in_hf(self);
        // SBAR-16: only where watch matters (home, the Status page, Watch and scan settings); elsewhere it
        // cost the screen name its room.
        let here = matches!(self.screen, crate::Screen::Home | crate::Screen::Status(_) | crate::Screen::Group(super::groups::Group { g: 1, .. }));
        let n = if hf || self.radio.keyed.is_some() || !here { 0 } else { m.count_ones() as u8 };
        StatusBar {
            name,
            locked: self.locked(),
            lock_ptt: self.lock_mode() == Some(crate::settings::Lock::All),
            // SBAR-11: the bar shows what changes what a key does, so SCN is the active slot scanning; a
            // watched slot's scan shows on its card.
            scan: !hf && live.slots[self.active as usize].scanning,
            watch: if self.watch == 0 || n < 2 { 0 } else { n },
            vox: self.set.vox, // SBAR-25
            spk: dev.spkmic, // SBAR-26
            inbox: self.inbox,
            alert: self.alert & crate::ALERT_UNREAD != 0,
            clock: dev.clock_min,
            utc: dev.utc,
            h12: self.set.clock == 1, // SBAR-2.1: MENU 0 6 6's 12 h
            // SBAR-2: the callsign stands in for a clock with no time source.
            call: if dev.clock_min.is_none() { Buf::from(self.set.callsign.as_bytes()) } else { Buf::new() },
            gps: dev.gps,
            bt_on: self.bt_on(), // SBAR-8 (never in Low Power Mode, PWR-24)
            bt: dev.bt,
            battery: ferrite_core::battery::bars(dev.battery_pct), // SBAR-9: stock's bars, at the thresholds
            battery_low: dev.battery_low,
            battery_tier: ferrite_core::battery::tier(dev.battery_pct, dev.battery_low || dev.battery_cutoff),
            battery_cutoff: dev.battery_cutoff,
            blink_off: dev.battery_cutoff && (self.now / 500) % 2 == 1,
            alerts_paused: hf && self.set.same_on(),
            dry: self.dry_tx(), // SBAR-22, TX-48
            lpm: self.low_power(), // SBAR-24, PWR-24
            sat: self.sat_mark(), // SAT-20
        }
    }

    /// The hint bar, or the toast over it (HOME-9); a shortcut's popup leaves the hint bar alone.
    /// A toast is up over the hint bar.
    pub(crate) fn toast_shown(&self) -> bool {
        self.toast.is_some_and(|t| !t.center)
    }

    /// Low Power Mode's home is up (PWR-24.7): it draws the whole screen, bands included.
    pub(crate) fn lpm_home(&self) -> bool {
        self.screen == crate::Screen::Home && self.low_power()
    }

    pub(crate) fn render_hint(&mut self, lcd: &mut impl Panel) {
        if let Some(t) = self.toast.filter(|t| !t.center) {
            if self.dirty(R_HINT, Mix::new().h(&1u8).h(&t.head).h(&t.body).h(&t.icon).get()) {
                w::toast(lcd, t.head.as_bytes(), t.body.as_bytes(), t.icon);
            }
            return;
        }
        if self.lpm_home() {
            return; // PWR-24.7: Low Power Mode's home draws its own band
        }
        let h = self.screen.hints(self);
        if self.dirty(R_HINT, Mix::new().h(&0u8).any(&h.as_slice()).get()) {
            w::hint_bar(lcd, h.as_slice());
        }
    }

    /// List row `i` (region R_ROW + i) at y, drawn when what it shows changed (MENU-12).
    pub(crate) fn list_row(&mut self, lcd: &mut impl Panel, i: usize, y: i32, h: i32, r: &Row) {
        if self.dirty(R_ROW + i, Mix::new().h(&r.num).h(&r.name).h(&r.value).h(&r.dot).h(&r.selected).h(&r.grey).get()) {
            w::list_row(lcd, y, h, r, self.cue(), false);
        }
    }

    /// A menu's row (DISP-6, decision 372: MENU 0, Quick settings, the options pages): [`App::list_row`] in the
    /// menus' 14 px text.
    pub(crate) fn menu_row(&mut self, lcd: &mut impl Panel, i: usize, y: i32, h: i32, r: &Row) {
        if self.dirty(R_ROW + i, Mix::new().h(&r.num).h(&r.name).h(&r.value).h(&r.dot).h(&r.selected).h(&r.grey).get()) {
            w::list_row(lcd, y, h, r, self.cue(), true);
        }
    }

    /// Edit grid cell `i` (region R_ROW + i): label over value, focused or greyed (EDIT-3, EDIT-7).
    pub(crate) fn grid_cell(&mut self, lcd: &mut impl Panel, i: usize, x: i32, y: i32, w_: i32, h: i32, label: &[u8], value: &[u8], focused: bool, grey: bool) {
        if self.dirty(R_ROW + i, Mix::new().h(&label).h(&value).h(&focused).h(&grey).get()) {
            w::edit_cell(lcd, x, y, w_, h, label, value, focused, grey);
        }
    }

    /// The help box at the foot of a list: one line of help and the item's path (MENU-15).
    pub(crate) fn help_box(&mut self, lcd: &mut impl Panel, y: i32, h: i32, text: &[u8], path: &[u8]) {
        if self.dirty(R_HELP, Mix::new().h(&text).h(&path).get()) {
            w::help_box(lcd, y, h, text, path);
        }
    }

    /// The help box with a menu's [`Help`]: its region hashes the note's number, so the note is read from the text
    /// pack only when the box is drawn (UPD-22).
    pub(crate) fn help_note(&mut self, lcd: &mut impl Panel, y: i32, h: i32, help: crate::screens::groups::Help, path: &[u8]) {
        use crate::screens::groups::Help;
        let m = match help {
            Help::Note(t) => Mix::new().h(&1u8).h(&t.map_or(0xFFFF, |t| t.0)),
            Help::Why(w) => Mix::new().h(&2u8).h(w),
        };
        if self.dirty(R_HELP, m.h(&path).get()) {
            w::help_box(lcd, y, h, help.load().as_bytes(), path);
        }
    }

    /// The help box with a note from the text pack (UPD-22), read only when the box is drawn.
    pub(crate) fn help_txt(&mut self, lcd: &mut impl Panel, y: i32, h: i32, t: crate::text::Txt, path: &[u8]) {
        self.help_note(lcd, y, h, crate::screens::groups::Help::Note(Some(t)), path)
    }

    /// A message page (prompts, power off): a centred heading in `c`, an optional second line, wrapped text.
    pub(crate) fn message(&mut self, lcd: &mut impl Panel, head: &[u8], line: &[u8], text: &[u8], c: u16) {
        if self.dirty(R_TOP, Mix::new().h(&head).h(&line).h(&text).get()) {
            use ferrite_ui::assets::{UI_12, UI_15B};
            w::field(lcd, 10, 220, 60, 86, 80, head, &w::st(&UI_15B, c, BG, Align::Center));
            w::field(lcd, 10, 220, 92, 116, 110, line, const { &w::st(&UI_15B, TEXT, BG, Align::Center) });
            w::wrap(lcd, 16, 208, 124, 290, 140, 15, text, &UI_12, SUB, BG);
        }
    }

    // ---- slot texts (home cards, edit header) ----

    /// "VFO · VHF", "012 · Repeaters", "SCAN · Airband" (HOME-4.3): a channel names its zone where a VFO names its
    /// band (decision 178); a channel in no zone keeps the band.
    pub(crate) fn head_text(&self, s: u8, live: &LiveAll, store: &impl ChannelStore, b: &mut Buf<32>) {
        let sl = &self.slots[s as usize];
        let lv = &live.slots[s as usize];
        b.clear();
        if let Some(h) = self.aprs_holder().filter(|_| s == crate::aprs::APRS_SLOT) {
            // APRS-17 (decision 181): what holds C, "APRS Mode", "APRS KISS", "APRS Beacon"; APRS-17.5 (decision
            // 313): beacons beside APRS mode show on the beacon row, not in the header
            b.push(match h {
                crate::aprs::Holder::Mode => b"APRS Mode".as_slice(),
                crate::aprs::Holder::Kiss => b"APRS KISS",
                crate::aprs::Holder::Beacon => b"APRS Tracker",
            });
        } else if lv.scanning {
            b.push(b"SCAN");
        } else if sl.on_channel() {
            crate::slot::chan_num(sl.ch.map_or(0, |c| c.num), b);
        } else {
            b.push(b"VFO");
        }
        b.push(" \u{b7} ".as_bytes());
        // CHAN-34: a slot on Local names Local, where ◀▶ go on from
        let zone = sl.ch.filter(|_| sl.on_channel() && !lv.scanning && !self.aprs_slot(s)).and_then(|c| if sl.zone == crate::local::LOCAL { Some(crate::chan::Name::from(b"Local".as_slice())) } else { store.zone_name(c.zone) });
        if self.in_bcast_band(s) && !lv.scanning {
            // BCR-21: a slot in the broadcast band says so where the band goes; BCR-26.8: the programme type there
            // while it plays a station that sends one
            let pty = self.bcast_pty(live, s);
            b.push(if pty.is_empty() { b"Broadcast".as_slice() } else { pty.as_bytes() });
        } else if let Some(z) = zone {
            b.push(z.as_bytes());
        } else {
            b.push(band_name(self.shown_freq(s, live)).as_bytes());
        }
    }

    /// A channel slot's name, the cards' main line in channel mode (HOME-4); empty for a VFO, a scan or a
    /// channel without a name, which show their frequency large instead.
    pub(crate) fn chan_name(&self, s: u8, live: &LiveAll) -> Buf<16> {
        let sl = &self.slots[s as usize];
        let mut n = Buf::<16>::new();
        if !live.slots[s as usize].scanning && !self.aprs_slot(s) {
            if let Some(c) = sl.ch.filter(|_| sl.on_channel()) {
                n.set(c.name());
            }
        }
        n
    }

    /// The card's name: [`Self::chan_name`], or while a channel list scans the name of the channel it is on
    /// (HOME-4.1), read from the store as the scan screen's hits are.
    /// CHAN-33: 0 2 4 shows a channel by its name (the default), its number ("CH 012") or, empty here, by its
    /// frequency as a VFO is shown.
    pub(crate) fn card_name(&self, s: u8, live: &LiveAll, store: &impl ChannelStore) -> Buf<16> {
        let lv = &live.slots[s as usize];
        let scanned = if lv.scanning { self.scanned(s, lv.chan, store) } else { None };
        let name = if lv.scanning { scanned.map_or_else(Buf::new, |c| Buf::from(c.name())) } else { self.chan_name(s, live) };
        let sl = &self.slots[s as usize];
        match self.set.ch_display {
            1 if !name.is_empty() || sl.on_channel() && !lv.scanning && !self.aprs_slot(s) => {
                let num = if lv.scanning { scanned.map_or(0, |c| c.num) } else { sl.ch.map_or(0, |c| c.num) };
                let mut l = Buf::<32>::new();
                crate::slot::chan_label(num, &mut l);
                Buf::from(l.as_bytes())
            }
            2 => Buf::new(),
            _ => name,
        }
    }

    /// A channel scan's last hit by name (HOME-4.1); empty for a band scan or a channel without a name.
    pub(crate) fn hit_name(&self, s: u8, live: &LiveAll, store: &impl ChannelStore) -> Buf<16> {
        self.scanned(s, live.slots[s as usize].hit_chan, store).map_or_else(Buf::new, |c| Buf::from(c.name()))
    }

    /// The channel at a channel scan's position plus 1 as the Radio task reports it (0: none): the scan list is the
    /// UI's (SCAN-25), so it knows which channel that is.
    fn scanned(&self, s: u8, at: u16, store: &impl ChannelStore) -> Option<crate::chan::Channel> {
        let p = at.checked_sub(1)?;
        self.slot(s).on_channel().then(|| crate::chanscan::at(self, s, p, store)).flatten()
    }

    /// The slot's frequency, or the scan's current step while it scans.
    pub(crate) fn shown_freq(&self, s: u8, live: &LiveAll) -> u32 {
        let lv = &live.slots[s as usize];
        if lv.scanning { lv.freq } else { self.tuned_freq(s) }
    }

    /// The slot's state word and colour on a card: TX (a data key-up, HOME-18), Off (turned off with \*, HOME-15.1),
    /// Muted or RX muted (quick mute; drawn as "Muted" with the RX tag, HOME-15.3), RX, relay, off (not watched),
    /// heard (a tone hint), scanning, idle; on a broadcast slot FM (playing), muted (a slot heard has the speaker) or off, and "FM on B" on
    /// another slot in the band (BCR-22, BCR-27); "Watch off: relay" while broadcast plays (BCR-15).
    pub(crate) fn state(&self, s: u8, live: &LiveAll) -> (&'static str, u16) {
        let lv = &live.slots[s as usize];
        // HOME-18: a data key-up (APRS, KISS) has no Transmit screen; the card shows it
        if live.tx.keyed && live.tx.slot == s && self.radio.keyed.is_none() {
            return ("TX", RED);
        }
        // HOME-15.1: a slot turned off with \* says so whatever else (its card is the powered-down one)
        if self.slot_off(s) {
            return ("Off", SUB);
        }
        // WATCH-16.2: APRS isn't listening (another slot transmits, a relay cancel): say so
        if live.dropped & 1 << s != 0 && s == crate::aprs::APRS_SLOT && self.aprs_c() {
            return ("paused", AMBER);
        }
        // HOME-15: quick mute takes the state word, "RX muted" while it receives (the broadcast slot too); the cards
        // draw that as "Muted" with the RX tag beside it (HOME-15.3), the readback says both.
        if self.mute & 1 << s != 0 && !self.relay_off(s) && !lv.relay {
            return if lv.squelch_open { ("RX muted", AMBER) } else { ("Muted", AMBER) };
        }
        let watched = s == self.active || self.watch_mask() & 1 << s != 0;
        if self.in_bcast_band(s) && !lv.scanning {
            return match self.bcast_slot() {
                Some(b) if b != s => (["FM on A", "FM on B", "FM on C"][b as usize % 3], DIM),
                Some(_) if !self.radio.fm_on => ("off", DIM),
                Some(_) if self.bcast_heard(live) => ("FM", GREEN),
                Some(_) => ("muted", AMBER),
                None => ("off", DIM),
            };
        }
        if self.relay_off(s) {
            ("Watch off: relay", AMBER)
        } else if lv.relay {
            ("relay", AMBER)
        } else if self.monitor & 1 << s != 0 {
            ("MON", AMBER) // a latched monitor: squelch held open (decided Oct 6, 2026)
        } else if self.defers_to_c(s) && !lv.squelch_open {
            ("APRS on C", DIM) // APRS-21.2: C listens here; the card shows RX when C receives
        } else if !watched {
            ("off", DIM)
        } else if !lv.dtmf.is_empty() {
            ("DTMF", GREEN) // DEC-6.1: the card shows "DTMF 101#" ([`wide_state`])
        } else if lv.heard_tone != 0 {
            ("heard", AMBER) // TSRCH-13: the card shows "Heard 88.5" ([`heard_text`])
        } else if lv.squelch_open {
            ("RX", GREEN)
        } else if lv.scanning {
            ("scanning", SUB)
        } else if self.paused_until.is_some() && s != self.active && !(s == crate::aprs::APRS_SLOT && self.aprs_c()) {
            ("paused", SUB) // the APRS slot keeps listening through a pause (WATCH-16.1)
        } else {
            ("idle", SUB)
        }
    }
}

/// The tone hint on a card (TSRCH-13): "Heard 88.5", "Heard D023I". A DCS code read on air is also the other
/// polarity's code that sends the same bits (TONE-20.5: D023I is D047N), so both show, the one with the slot's
/// own code number (`own`, its receive tone) first, else the normal one: "Heard D023I/D047N" on a D023N slot. At
/// most 17 characters (120 px at UI_12), so a card's state field widens to fit it ([`HEARD_W`] at least).
/// A state shown as text in a wider field: the tone hint (TSRCH-13) and the DTMF heard (DEC-6.1, "DTMF 101#"); false
/// for the plain state words.
pub(crate) fn wide_state(state: &str, lv: &ferrite_core::msg::SlotLive, own: u16, b: &mut Buf<24>) -> bool {
    match state {
        "heard" => heard_text(lv.heard_tone, own, b),
        "DTMF" => {
            b.set(b"DTMF ");
            for i in 0..lv.dtmf.len() {
                b.push(&[lv.dtmf.char(i)]);
            }
        }
        _ => return false,
    }
    true
}

pub(crate) fn heard_text(t: u16, own: u16, b: &mut Buf<24>) {
    let mut tone = Buf::<16>::new();
    crate::slot::tone_text(t, &mut tone);
    let tb = tone.as_bytes();
    b.set(b"Heard ");
    let Some(other) = ferrite_core::dcs::twin(t, &crate::slot::DCS) else {
        b.push(tb.strip_prefix(b"T ").unwrap_or(tb));
        return;
    };
    let mut ob = Buf::<16>::new();
    crate::slot::tone_text(other, &mut ob);
    let mine = own & crate::slot::DCS_FLAG != 0 && other & 0x1FF == own & 0x1FF;
    let (first, second) = if mine { (ob.as_bytes(), tb) } else { (tb, ob.as_bytes()) };
    b.push(first);
    b.push(b"/");
    b.push(second);
}

/// The state field's width for a tone hint (TSRCH-13): 11 characters.
pub(crate) const HEARD_W: i32 = 80;
