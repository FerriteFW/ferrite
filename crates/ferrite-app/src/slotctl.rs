//! A slot's working fields from the PC (manual chapter 37, TERM-35; chapter 29, LINK-33): the terminal's `slot`
//! command and the link's `SLOT` message both come here, so they share one set of rules. Each change takes the
//! keypad's own path: a frequency is the VFO entry's ([`crate::screens::entry::tune_vfo`]), mode and power the
//! quick settings' knob ([`App::change`]), a tone the Tones screen's ([`crate::screens::tones::set`]), `vfo` the V/M
//! key and `active` the SLOT key. So the checks are the keypad's, and so are the writes: a VFO's state is kept as the
//! keypad's is, and a change on a channel slot saves that channel, as a change made there on the radio does (EDIT-4).
//!
//! Refused as a whole, before anything changes: a slot on a channel given a frequency (its VFO is tuned only with
//! `vfo` in the same line, or after it: the channel is never retuned from here), a shift over 100 MHz, a split TX
//! frequency no chip tunes or above 1000 MHz, a shift or split that needs the other band relay from RX (TX-37), the
//! APRS slot (APRS-19),
//! channel-only mode (PIN-5: only `active`), a slot scanning (SIDE2 stops it), and anything while transmitting.
//!
//! The link's `TUNE` (LINK-68, decision 373) moves a slot the way home's keys do: a channel by number, ▲▼, ◀▶, a
//! zone, SIDE2's scan ([`App::slot_tune`]), through home's own code with the slot made active for it.

use crate::chan::ChannelStore;
use crate::screens::{entry, tones};
use crate::slot::Item;
use crate::App;
use ferrite_core::freq::Freq10;
use ferrite_core::msg::{RadioCmd, SLOTS};

/// What to change on a slot; None (false) leaves a field as it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlotChange {
    /// The VFO's frequency (10 Hz).
    pub freq: Option<Freq10>,
    /// FM, NFM, AM (0-2).
    pub mode: Option<u8>,
    /// Low, mid, high (0-2).
    pub power: Option<u8>,
    /// Both tones, sent and decoded (CHAN-3's tone word); or each alone.
    pub rx_tone: Option<u16>,
    pub tx_tone: Option<u16>,
    /// The repeater shift or split, as the slot's record keeps it: (signed shift in 10 Hz, 0 simplex; false) or
    /// (the TX frequency; true), as MENU 5 sets it ([`App::set_shift`], [`App::set_split`]).
    pub offset: Option<(i32, bool)>,
    /// To the slot's VFO, as V/M from a channel (the VFO keeps its own frequency unless `freq` is given).
    pub vfo: bool,
    /// Make it the active slot, as the SLOT key.
    pub active: bool,
}

impl SlotChange {
    /// Changes nothing: a read.
    pub fn is_empty(&self) -> bool {
        *self == SlotChange::default()
    }

    fn touches_slot(&self) -> bool {
        SlotChange { active: false, ..*self } != SlotChange::default()
    }
}

/// A `TUNE` (LINK-68): what to do to the slot, as a key on home would.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tune {
    /// The channel by number (0-based), as typed on a channel slot.
    Channel(u16),
    /// ▲▼ this many times.
    Step(i8),
    /// ◀▶ this many times: zones on a channel, bands on a VFO.
    Group(i8),
    /// The zone's first channel (0xFFFE: Local), as picked in the zone list.
    Zone(u16),
    /// SIDE2: start (true) or stop a scan.
    Scan(bool),
    /// \*'s quick mute and off (HOME-15, HOME-15.1): 0 on, 1 muted, 2 off.
    Quiet(u8),
}

/// A slot as the PC sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotState {
    pub active: bool,
    /// The channel it is on (0-based), or None on its VFO.
    pub channel: Option<u16>,
    pub freq: Freq10,
    pub mode: u8,
    pub power: u8,
    pub rx_tone: u16,
    pub tx_tone: u16,
    /// As the slot's record: signed shift, or the split's TX frequency.
    pub offset: i32,
    pub split: bool,
    pub aprs: bool,
    pub scanning: bool,
}

impl App {
    /// Slot `s` (0-2) as it is now.
    pub fn slot_state(&self, s: u8) -> SlotState {
        let sl = &self.slots[s as usize % SLOTS];
        let c = self.tuned_cfg(s);
        SlotState {
            active: self.active == s,
            channel: sl.ch.filter(|_| sl.on_channel()).map(|c| c.num),
            freq: self.tuned_freq(s),
            mode: c.mode,
            power: c.power,
            rx_tone: c.rx_tone,
            tx_tone: c.tx_tone,
            offset: c.offset,
            split: c.split,
            aprs: self.aprs_slot(s),
            scanning: self.scan_mask & 1 << s != 0,
        }
    }

    /// Changes slot `s` as the keypad would, or refuses the whole change with the reason (nothing changed).
    pub fn slot_apply(&mut self, s: u8, ch: &SlotChange, _store: &impl ChannelStore) -> Result<(), &'static str> {
        if s as usize >= SLOTS {
            return Err("no such slot: A, B or C");
        }
        if ch.mode.is_some_and(|m| !crate::slot::slot_mode_ok(m)) {
            return Err("mode: fm, nfm, am or wfm");
        }
        if ch.power.is_some_and(|p| p > 2) {
            return Err("power: low, mid or high");
        }
        if ch.touches_slot() {
            if self.radio.keyed.is_some() {
                return Err("transmitting: nothing changes while keyed (TX-22)");
            }
            if self.set.chan_only {
                return Err("channel-only mode: only the active slot can change (PIN-5)");
            }
            if self.aprs_slot(s) {
                return Err(self.aprs_held());
            }
            if self.scan_mask & 1 << s != 0 {
                return Err("the slot is scanning: SIDE2 stops it");
            }
            if let Some(f) = ch.freq {
                if self.slots[s as usize].on_channel() && !ch.vfo {
                    return Err("the slot is on a channel: add vfo to tune its VFO");
                }
                if let Some(why) = entry::refuse(f) {
                    return Err(why);
                }
            }
            if let Some((v, split)) = ch.offset {
                if split {
                    let tx = v.max(0) as Freq10;
                    if let Some(why) = entry::refuse(tx) {
                        return Err(why);
                    }
                    if tx > ferrite_core::freq::mhz(1000) {
                        return Err("split: never TX above 1000 MHz");
                    }
                } else if v.unsigned_abs() >= ferrite_core::freq::mhz(100) {
                    return Err("offset: under 100 MHz");
                }
                // TX-37 on the frequency the slot will have.
                let rx = ch.freq.unwrap_or_else(|| self.tuned_freq(s));
                let mut c = self.tuned_cfg(s);
                (c.offset, c.split) = (v, split);
                if !ferrite_core::txplan::relay_ok(c.rx_of(rx), c.tx_of(rx)) {
                    return Err("TX would need the other band relay from RX (TX-37)");
                }
            }
        }
        // Each field through the keypad's path, on `s` as the active slot (they all act on the active one).
        let was = self.active;
        self.active = s;
        if ch.vfo && !self.slots[s as usize].vfo_mode {
            self.slots[s as usize].vfo_mode = true; // V/M (the VFO keeps its frequency)
            self.send_slot(s);
        }
        if let Some(f) = ch.freq {
            entry::tune_vfo(self, s, f);
        }
        if let Some(m) = ch.mode {
            let d = m as i32 - self.slots[s as usize].cfg().mode as i32;
            self.change(Item::Mode, d);
        }
        if let Some(p) = ch.power {
            let d = p as i32 - self.slots[s as usize].cfg().power as i32;
            self.change(Item::Power, d);
        }
        match ch.offset {
            Some((tx, true)) => _ = self.set_split(tx.max(0) as Freq10),
            Some((shift, false)) => _ = self.set_shift(shift),
            None => {}
        }
        if let Some(t) = ch.tx_tone {
            tones::set(self, false, t);
        }
        if let Some(t) = ch.rx_tone {
            tones::set(self, true, t);
        }
        self.active = was;
        if ch.active && self.active != s {
            self.active = s; // NAV-5, the SLOT key
            self.out.push(RadioCmd::Active(s));
            self.monitor_off_leaving(was); // SQL-14.1
        }
        Ok(())
    }

    /// Why the APRS slot can't be changed (APRS-19).
    fn aprs_held(&self) -> &'static str {
        match self.aprs_holder() {
            Some(crate::aprs::Holder::Kiss) => "slot C held by KISS (KISS-9)",
            Some(crate::aprs::Holder::Beacon) => "slot C held by Tracker (APRS-29.2)",
            _ => "slot C held by APRS mode (APRS-19)",
        }
    }

    /// LINK-68: tunes slot `s` as home's keys would with it active, or refuses with the reason (nothing changed).
    /// The active slot stays as it was, except for a scan's start: the scanned slot becomes the active one.
    pub fn slot_tune(&mut self, s: u8, t: Tune, store: &impl ChannelStore) -> Result<(), &'static str> {
        use crate::screens::home;
        if s as usize >= SLOTS {
            return Err("no such slot: A, B or C");
        }
        if self.radio.keyed.is_some() {
            return Err("transmitting: nothing changes while keyed (TX-22)");
        }
        if self.set.chan_only && s != self.active {
            return Err("channel-only mode: only the active slot can change (PIN-5)");
        }
        // LINK-68: on, muted or off as \* sets them, on any slot; the APRS slot mutes (APRS-2), never off
        if let Tune::Quiet(q) = t {
            if q == 2 && self.aprs_slot(s) {
                return Err("the APRS slot is never off (HOME-15.1)");
            }
            if q == 2 && self.scan_mask & 1 << s != 0 {
                return Err("the slot is scanning: stop the scan first");
            }
            home::set_quiet(self, s, q);
            return Ok(());
        }
        if self.aprs_slot(s) {
            return Err(self.aprs_held());
        }
        let scanning = self.scan_mask & 1 << s != 0;
        if scanning && !matches!(t, Tune::Scan(_)) {
            return Err("the slot is scanning: stop the scan first");
        }
        let sl = &self.slots[s as usize];
        // what the keys would refuse, checked first
        let target = match t {
            Tune::Channel(num) => match store.by_num(num) {
                None => return Err("no channel by that number"),
                Some((c, _)) if c.hf_mode_only() => return Err("below 18 MHz: HF mode recalls it (CHAN-26)"),
                Some(found) => Some(found),
            },
            Tune::Step(_) if sl.on_channel() && self.zlen(store, sl.zone) == 0 => return Err("the slot's zone is empty"),
            Tune::Zone(z) => {
                let known = if z == crate::local::LOCAL { crate::local::on(self) } else { z < store.zones() };
                if !known {
                    return Err("no such zone");
                }
                match home::first_slot_channel(self, store, z) {
                    Some(p) => Some((self.zat(store, z, p).ok_or("no such zone")?, p)),
                    None => return Err("that zone has no channel this slot can take"),
                }
            }
            Tune::Scan(true) if !scanning => {
                if self.low_power() {
                    return Err("Low Power Mode: no scans (PWR-24.4)");
                }
                if self.slot_off(s) {
                    return Err("the slot is off: * turns it back on (HOME-15.4)");
                }
                if sl.on_channel() && sl.zone == crate::local::LOCAL && self.local.is_empty() {
                    return Err("Local has no channels to scan (CHAN-34)");
                }
                None
            }
            _ => None,
        };
        let was = self.active;
        self.active = s;
        match t {
            Tune::Channel(_) | Tune::Zone(_) => {
                let (c, pos) = target.expect("checked above");
                let zone = if let Tune::Zone(z) = t { z } else { c.zone };
                let sl = &mut self.slots[s as usize];
                (sl.zone, sl.pos, sl.ch, sl.vfo_mode) = (zone, pos, Some(c), false);
                self.send_slot(s);
            }
            Tune::Step(d) => home::step(self, d as i32, false, store),
            Tune::Group(d) => {
                // one press at a time: a press goes on to the next zone that has a channel
                for _ in 0..d.unsigned_abs() {
                    home::group(self, d.signum() as i32, store);
                }
            }
            Tune::Scan(false) if scanning => crate::screens::scan::side2(self, store),
            Tune::Scan(true) if !scanning => {
                // a scan runs on the slot the radio receives: SIDE2 scans the active one, so the slot stays active;
                // the scan screen opens only over home: another screen in use stays up
                let up = self.screen;
                crate::screens::scan::side2(self, store);
                if !matches!(up, crate::Screen::Home) {
                    self.screen = up;
                }
                if was != s {
                    self.out.push(RadioCmd::Active(s)); // NAV-5
                    self.monitor_off_leaving(was); // SQL-14.1
                }
                return Ok(());
            }
            Tune::Scan(_) | Tune::Quiet(_) => {}
        }
        self.active = was;
        Ok(())
    }
}
