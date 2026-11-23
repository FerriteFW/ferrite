//! What survives a power cycle, as bytes: the radio-wide settings and the slots (frequencies, VFO settings, the
//! channel each is on, watch and Battery Save). Each type writes its fields in a fixed order, little-endian, so
//! the format doesn't depend on the compiler's struct layout; a reader that runs out of bytes leaves the rest at
//! their current values (the defaults), so a record from an older firmware that had fewer fields still loads
//! (a version change migrates what it can). Values are not trusted: everything that indexes a table is taken
//! modulo or checked where it is used, as for a fresh setting.
//!
//! The storage crate streams these through its own sink and source, so no buffer of the whole record exists.

use crate::chan::ChannelStore;
use crate::screens::hf::HfMem;
use crate::settings::{Settings, Text};
use crate::slot::SlotCfg;
use crate::{App, SLOTS};
use ferrite_core::msg::{RadioCmd, XbCfg};

/// Where bytes go (a CRC, a flash page buffer, a test vector).
pub trait Sink {
    fn put(&mut self, b: &[u8]);
}

/// Where bytes come from. `take` fills `out` and returns true, or returns false (and leaves `out`) when fewer
/// bytes are left.
pub trait Source {
    fn take(&mut self, out: &mut [u8]) -> bool;
}

/// A type that saves and loads itself, field by field.
pub trait Persist {
    fn save(&self, s: &mut dyn Sink);
    fn load(&mut self, r: &mut dyn Source);
}

macro_rules! int {
    ($($t:ty),*) => {$(
        impl Persist for $t {
            fn save(&self, s: &mut dyn Sink) {
                s.put(&self.to_le_bytes());
            }
            fn load(&mut self, r: &mut dyn Source) {
                let mut b = [0u8; core::mem::size_of::<$t>()];
                if r.take(&mut b) {
                    *self = <$t>::from_le_bytes(b);
                }
            }
        }
    )*};
}
int!(u8, i8, u16, i16, u32, i32);

impl Persist for bool {
    fn save(&self, s: &mut dyn Sink) {
        s.put(&[*self as u8]);
    }
    fn load(&mut self, r: &mut dyn Source) {
        let mut b = [0u8];
        if r.take(&mut b) {
            *self = b[0] != 0;
        }
    }
}

/// A channel number or none: 0xFFFF is none.
impl Persist for Option<u16> {
    fn save(&self, s: &mut dyn Sink) {
        self.unwrap_or(0xFFFF).save(s);
    }
    fn load(&mut self, r: &mut dyn Source) {
        let mut v = self.unwrap_or(0xFFFF);
        v.load(r);
        *self = (v != 0xFFFF).then_some(v);
    }
}

impl<T: Persist, const N: usize> Persist for [T; N] {
    fn save(&self, s: &mut dyn Sink) {
        self.iter().for_each(|x| x.save(s));
    }
    fn load(&mut self, r: &mut dyn Source) {
        self.iter_mut().for_each(|x| x.load(r));
    }
}

impl<const N: usize> Persist for Text<N> {
    fn save(&self, s: &mut dyn Sink) {
        s.put(&self.0);
    }
    fn load(&mut self, r: &mut dyn Source) {
        let mut b = [0u8; N];
        if r.take(&mut b) {
            self.0 = b;
        }
    }
}

/// `persist!(Type { field, ... })`: saves and loads the fields in this order. New fields go at the end only.
macro_rules! persist {
    // Booleans after the fields share one byte, bit 0 first, each stored as "differs from the type's DEFAULT": a
    // record from before a flag joined reads the old byte (one bool, 0 or 1) as the first flag, and the flags it
    // didn't have read 0, their defaults.
    // Fields in `then` come after the flag byte, so an older record's flag byte stays where it was.
    ($t:ty { $($f:ident),* $(,)? } bits { $($b:ident),* $(,)? } $(then { $($g:ident),* $(,)? })?) => {
        impl Persist for $t {
            fn save(&self, s: &mut dyn Sink) {
                $(self.$f.save(s);)*
                let d = <$t>::DEFAULT;
                let mut v = 0u8;
                let mut _i = 0;
                $(v |= ((self.$b != d.$b) as u8) << _i; _i += 1;)*
                v.save(s);
                $($(self.$g.save(s);)*)?
            }
            fn load(&mut self, r: &mut dyn Source) {
                $(self.$f.load(r);)*
                let d = <$t>::DEFAULT;
                let mut v = 0u8;
                let mut _i = 0;
                $(v |= ((self.$b != d.$b) as u8) << _i; _i += 1;)*
                v.load(r);
                let mut _i = 0;
                $(self.$b = d.$b ^ (v >> _i & 1 != 0); _i += 1;)*
                $($(self.$g.load(r);)*)?
            }
        }
    };
    ($t:ty { $($f:ident),* $(,)? }) => {
        impl Persist for $t {
            fn save(&self, s: &mut dyn Sink) {
                $(self.$f.save(s);)*
            }
            fn load(&mut self, r: &mut dyn Source) {
                $(self.$f.load(r);)*
            }
        }
    };
}

persist!(crate::data::Pos { lat, lon });
persist!(XbCfg { side1, side2, power, timeout_10s, hang_ds, flags });
persist!(HfMem { freq, mode, step, bw, agc, lna, bfo, recent, bands, band_bfo });
/// A slot's settings. Split rides in the reverse byte (bit 0 reverse, bit 1 split) so the record keeps its
/// length and older records (0 or 1 there) read as not split.
impl Persist for SlotCfg {
    fn save(&self, s: &mut dyn Sink) {
        self.mode.save(s);
        self.narrow.save(s);
        self.rx_tone.save(s);
        self.tx_tone.save(s);
        self.offset.save(s);
        self.power.save(s);
        self.step.save(s);
        self.squelch.save(s);
        self.busy.save(s);
        (self.reverse as u8 | (self.split as u8) << 1).save(s);
    }
    fn load(&mut self, r: &mut dyn Source) {
        self.mode.load(r);
        self.narrow.load(r);
        self.rx_tone.load(r);
        self.tx_tone.load(r);
        self.offset.load(r);
        self.power.load(r);
        self.step.load(r);
        self.squelch.load(r);
        self.busy.load(r);
        let mut f = self.reverse as u8 | (self.split as u8) << 1;
        f.load(r);
        (self.reverse, self.split) = (f & 1 != 0, f & 2 != 0);
        self.fold_bw(); // VFO-20: a saved FM + narrow reads as NFM
    }
}
persist!(Settings {
    callsign, owner, boot_card, region, extend_tx, extend_warned, tz, clock, clock_gps, units, layout, theme,
    pin_mode, pin, pin_len, pin_misses, pin_wait, bt, same, fm_freq, fm_step, fm_agc, fm_monitor, setup_done,
    home_ch, xb, hf, cable_port, coord_fmt, fixed_on, fixed, fixed_alt, same_codes, scope_bars, scope_step,
    scope_dwell, scope_trig, scope_line, scope_wf, scope_mode, term_colour,
    aprs_mode, predictive,
    lpm, lpm_below, spare_tokens, backlight, keypad_light, leds, auto_off, auto_lock, dtmf_sidetone, arrival_tone,
} bits { show_hidden, chan_only, cable_log, aprs_fixed, scan_tx } then { pon, pon_text, tone_level, scan_resume, scan_prio, tail_mode, tail_phase, end_tone, dtmf_ms, dtmf_delay, cw_id, gps, gps_rxtx, arrival, kiss_port, pocsag_on, pocsag_freq, pocsag_cap, popups, key_beep, beep_vol, alert_tones, vox, vox_level, vox_tail, fm_sleep, brightness, brightness_night, tx_light, wake_on, spare_temp, mic_gain, tx_timeout, menu_timeout, pause_time, spare_lock, busy_default, smeter_dbm, ch_display, scan_down, scan_return_hit, msg_vol, mcu_fixed, sq_chip_old, aprs_mute, dtmf_rx, aprs_share, local, sat_min_el, decode_log, sq_soft, mic_v2 });

/// The format of [`App::save_state`]; a reader of a different one keeps the defaults. 2 (decision 333): a slot's
/// zone is 2 bytes; version 1's one byte (255 no zone, 254 Local) still reads, so a settings record copied over
/// from layout 2 by Console's update keeps its slots.
pub const STATE_VERSION: u8 = 2;

impl App {
    /// The App's part of what survives a power cycle: the active slot, watch, Battery Save and each slot (VFO or
    /// channel mode, its VFO, the VFO's settings and band memories, its zone and position, the channel it is
    /// on). The radio-wide [`Settings`] are saved apart, as the boot screen needs them before the App exists.
    pub fn save_state(&self, s: &mut dyn Sink) {
        STATE_VERSION.save(s);
        (SLOTS as u8).save(s);
        self.active.save(s);
        self.watch.save(s);
        self.bsave.save(s);
        for sl in &self.slots {
            sl.vfo_mode.save(s);
            sl.vfo.save(s);
            sl.cfg.save(s);
            sl.mem.save(s);
            sl.zone.save(s);
            sl.pos.save(s);
            sl.ch.map(|c| c.num).save(s);
        }
    }

    /// The saved state back, at power on before the first draw. A slot on a channel reads that channel from the
    /// store again (edits not saved to it are gone, as on the radio); a channel that no longer exists leaves the
    /// slot on its VFO. Then the radio gets every slot again.
    pub fn restore_state(&mut self, r: &mut dyn Source, store: &impl ChannelStore) {
        let (mut ver, mut n) = (0u8, 0u8);
        ver.load(r);
        n.load(r);
        if !(1..=STATE_VERSION).contains(&ver) || n as usize != SLOTS {
            return;
        }
        // MODE-6.5: everything App::new queued for the Radio task goes again below; its copies would overflow the
        // queue at power-on and push out the settings that matter (the active slot, the watch set, the APRS slot).
        self.out.q = ferrite_core::cmdq::CmdRing::new();
        self.active.load(r);
        self.active %= SLOTS as u8;
        self.watch.load(r);
        self.watch %= 3;
        self.bsave.load(r);
        self.bsave %= 4;
        for sl in &mut self.slots {
            sl.vfo_mode.load(r);
            sl.vfo.load(r);
            sl.cfg.load(r);
            sl.mem.load(r);
            if ver == 1 {
                let mut z = 0u8;
                z.load(r);
                sl.zone = match z {
                    255 => crate::chan::NO_ZONE,
                    254 => crate::local::LOCAL,
                    z => z as crate::chan::ZoneId,
                };
            } else {
                sl.zone.load(r);
            }
            sl.pos.load(r);
            let mut num: Option<u16> = None;
            num.load(r);
            sl.ch = None;
            if let Some((c, pos)) = num.and_then(|n| store.by_num(n)) {
                (sl.ch, sl.zone, sl.pos) = (Some(c), c.zone, pos);
            } else {
                sl.vfo_mode = true;
            }
        }
        for s in 0..SLOTS as u8 {
            self.send_slot(s);
        }
        self.out.push(RadioCmd::Active(self.active));
        self.out.push(RadioCmd::Watch(self.watch_mask()));
        self.send_aprs_slot();
        if self.mute != 0 {
            self.send_mute(); // APRS-15.2: C muted in APRS mode
        }
        self.c_settled();
        self.send_radio_settings();
        self.out.push(RadioCmd::BatterySave(crate::level(self.bsave)));
    }
}

/// A byte slice as a [`Source`] (tests, the simulator).
pub struct SliceSource<'a>(pub &'a [u8]);

impl Source for SliceSource<'_> {
    fn take(&mut self, out: &mut [u8]) -> bool {
        if self.0.len() < out.len() {
            return false;
        }
        let (a, b) = self.0.split_at(out.len());
        out.copy_from_slice(a);
        self.0 = b;
        true
    }
}

/// Counts what is saved (sizing a record before writing it).
pub struct Count(pub usize);

impl Sink for Count {
    fn put(&mut self, b: &[u8]) {
        self.0 += b.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::Sample;

    struct Vec2(std::vec::Vec<u8>);
    impl Sink for Vec2 {
        fn put(&mut self, b: &[u8]) {
            self.0.extend_from_slice(b);
        }
    }

    #[test]
    fn settings_round_trip_and_short_records_keep_defaults() {
        let mut a = crate::sample::settings();
        a.set_callsign(b"ve3abc");
        a.home_ch = Some(11);
        a.hf.bfo = -120;
        let mut v = Vec2(std::vec::Vec::new());
        a.save(&mut v);
        let mut b = Settings::DEFAULT;
        b.load(&mut SliceSource(&v.0));
        assert_eq!(a, b);
        // An older, shorter record: what it has loads, the rest stays default.
        let mut c = Settings::DEFAULT;
        c.load(&mut SliceSource(&v.0[..10]));
        assert_eq!(c.callsign, a.callsign);
        assert_eq!(c.hf, Settings::DEFAULT.hf);
    }

    /// KEY-14.5: the byte that held 0 9 1 2's Lock covers (decision 302) stays where it was, so the record keeps
    /// its length; a stored choice loads into the spare and changes nothing (the mode is chosen at each lock).
    #[test]
    fn old_lock_covers_is_ignored() {
        let mut v = Vec2(std::vec::Vec::new());
        Settings::DEFAULT.save(&mut v);
        let at = v.0.len() - AFTER_SPARE + 4; // mic gain, TX timeout, menu timeout, pause time, then it
        for stored in [1, 2] {
            let mut w = v.0.clone();
            w[at] = stored;
            let mut b = Settings::DEFAULT;
            b.load(&mut SliceSource(&w));
            assert_eq!(Settings { spare_lock: 0, ..b }, Settings::DEFAULT);
            let mut x = Vec2(std::vec::Vec::new());
            b.save(&mut x);
            assert_eq!(x.0, w, "saved back as read, the same length");
        }
    }

    /// Decision 112: the byte that held 0 9 7 4's MCU temperature offset (SBAR-21, withdrawn) stays where it was,
    /// so the record keeps its layout (settings added since come after it: 0 8 3's mic gain); an old offset still
    /// in it loads into the spare and changes nothing.
    #[test]
    fn old_mcu_temp_offset_is_ignored() {
        let mut v = Vec2(std::vec::Vec::new());
        Settings::DEFAULT.save(&mut v);
        let n = v.0.len() - AFTER_SPARE;
        assert_eq!(v.0[n - 1], 0, "the spare");
        v.0[n - 1] = (-5i8) as u8; // an offset of -5 saved by the firmware that had it
        let mut b = Settings::DEFAULT;
        b.load(&mut SliceSource(&v.0));
        assert_eq!(Settings { spare_temp: 0, ..b }, Settings::DEFAULT);
    }

    /// The bytes saved after the spare: 0 8 3's mic gain, 0 8 1's timeout, then the eight settings wired in
    /// decision 243 (menu timeout to scan return), 0 7 5 6's message volume, 0 9 9 4's MCU clock (PWR-26.5), 0 9
    /// 5's squelch (SQL-18.3), 0 5 9 6's APRS mute, 0 4 3's DTMF decode, 0 5 9 7's shared VHF (WATCH-17.3), 0 2
    /// 5's Local zone (CHAN-34), the satellite list's minimum elevation (SAT-22) and 0 5 9 8's decode log
    /// (APRS-56), a byte each; then 0 9 5's squelch again at the end (SQL-18.3, SQL-18.7: the old byte is ignored),
    /// and 0.9.6's mark (VOX-15.1).
    const AFTER_SPARE: usize = 21;

    /// GPS-1.4: a radio keeps the setting it has. 0 6 1's byte is where the old on/off bool was, with Off and On
    /// its 0 and 1 and Auto 2: an old record set to Off reads Off, one set to On reads On, the record's length is
    /// unchanged, and a value out of range reads Auto.
    #[test]
    fn old_gps_on_off_is_kept() {
        use crate::settings::{GPS_AUTO, GPS_OFF, GPS_ON};
        let save = |gps: u8| {
            let mut v = Vec2(std::vec::Vec::new());
            Settings { gps, ..Settings::DEFAULT }.save(&mut v);
            v.0
        };
        let (off, on, auto) = (save(GPS_OFF), save(GPS_ON), save(GPS_AUTO));
        assert_eq!((on.len(), auto.len()), (off.len(), off.len()), "the record's length stays");
        let at = off.iter().zip(&on).position(|(a, b)| a != b).unwrap();
        assert_eq!((off[at], on[at], auto[at]), (false as u8, true as u8, 2), "as the old bool saved Off and On");
        for (rec, want) in [(&off, GPS_OFF), (&on, GPS_ON), (&auto, GPS_AUTO)] {
            let mut b = Settings::DEFAULT;
            b.load(&mut SliceSource(rec));
            assert_eq!(b.gps, want);
        }
        let mut b = Settings::DEFAULT;
        b.load(&mut SliceSource(&on[..at]));
        assert_eq!(b.gps, GPS_AUTO, "a record from before the byte: the default, Auto");
        let mut w = on.clone();
        w[at] = 7;
        b.load(&mut SliceSource(&w));
        b.sanitize();
        assert_eq!(b.gps, GPS_AUTO, "out of range: Auto");
    }

    /// VOX-15.1 (decision 436): a record from before 0.9.6 (no `mic_v2`, its last byte) with Mic gain at the old
    /// default 5 loads with the new default 9; another choice stays; a 0.9.6 record keeps a 5 chosen since.
    #[test]
    fn an_old_records_default_mic_gain_goes_to_9() {
        for (stored, want) in [(5, 9), (7, 7), (1, 1)] {
            let mut a = Settings::DEFAULT;
            a.mic_gain = stored;
            let mut v = Vec2(std::vec::Vec::new());
            a.save(&mut v);
            let mut b = Settings::DEFAULT;
            b.mic_v2 = false; // as the store's loader sets it
            b.load(&mut SliceSource(&v.0[..v.0.len() - 1]));
            b.mic_upgrade();
            assert_eq!((b.mic_gain, b.mic_v2), (want, true), "old record at {stored}");
        }
        let mut a = Settings::DEFAULT;
        a.mic_gain = 5;
        let mut v = Vec2(std::vec::Vec::new());
        a.save(&mut v);
        let mut b = Settings::DEFAULT;
        b.mic_v2 = false;
        b.load(&mut SliceSource(&v.0));
        b.mic_upgrade();
        assert_eq!(b.mic_gain, 5, "a 0.9.6 record's 5 is a choice");
    }

    /// VOX-15: a record from before 0 8 3 existed (it ends at the spare) loads with Mic gain at its default, 9.
    #[test]
    fn record_without_mic_gain_loads_the_default() {
        let mut a = Settings::DEFAULT;
        a.mic_gain = 8;
        let mut v = Vec2(std::vec::Vec::new());
        a.save(&mut v);
        let mut b = Settings::DEFAULT;
        b.load(&mut SliceSource(&v.0[..v.0.len() - AFTER_SPARE]));
        assert_eq!((b.mic_gain, b.tx_timeout_s()), (9, 120), "TX-30: an older record's timeout is the default");
        b.load(&mut SliceSource(&v.0));
        assert_eq!(b.mic_gain, 8);
    }

    /// The settings' length, the settings and the slots must fit the store's 1 KB record (ferrite-store
    /// `settings::MAX_PAYLOAD`).
    #[test]
    fn record_fits_the_store() {
        let leak = || -> &'static mut Settings { std::boxed::Box::leak(std::boxed::Box::new(Settings::DEFAULT)) };
        let alert = || std::boxed::Box::leak(std::boxed::Box::new(crate::screens::alert::AlertInfo::NONE));
        let a = App::new(leak(), alert());
        let mut c = Count(2);
        Settings::DEFAULT.save(&mut c);
        a.save_state(&mut c);
        assert!(c.0 <= 1024, "{} bytes", c.0);
    }

    /// The last Active, Watch and AprsSlot the Radio task gets from the power-on burst (None: lost).
    fn boot_cmds(a: &mut App) -> [Option<u8>; 3] {
        let mut got = [None; 3];
        while let Some(c) = a.pop_cmd() {
            match c {
                crate::RadioCmd::Active(x) => got[0] = Some(x),
                crate::RadioCmd::Watch(x) => got[1] = Some(x),
                crate::RadioCmd::AprsSlot(x) => got[2] = Some(x),
                _ => {}
            }
        }
        got
    }

    /// MODE-6.5: the same with APRS mode on and C muted (black, Nov 3, 2026: A stayed active underneath, its
    /// squelch held the watch hop on it and the speaker played it while the cards said C).
    #[test]
    fn app_state_round_trip_aprs_muted() {
        let store = Sample::new();
        let leak = |aprs| -> &'static mut Settings {
            let mut s = Settings::DEFAULT;
            s.aprs_mode = aprs;
            std::boxed::Box::leak(std::boxed::Box::new(s))
        };
        let alert = || std::boxed::Box::leak(std::boxed::Box::new(crate::screens::alert::AlertInfo::NONE));
        let mut a = App::new(leak(true), alert());
        a.set_watch(0);
        a.active = 2;
        a.mute = 0b100;
        let mut v = Vec2(std::vec::Vec::new());
        a.save_state(&mut v);
        let mut b = App::new(leak(true), alert());
        b.restore_state(&mut SliceSource(&v.0), &store);
        assert_eq!(b.active, 2);
        assert!(b.out.q.len() <= 24, "{} waiting: the queue (32) keeps room past the burst", b.out.q.len());
        assert_eq!(boot_cmds(&mut b)[0], Some(2), "MODE-6.5: the saved active slot reaches the Radio task");
    }

    /// MODE-6.5: the power-on burst, App::new and then the saved state, fits the UI's queue: the Radio task gets the
    /// saved active slot and watch set (until Oct 22, 2026 both were pushed out, and A stayed active underneath).
    #[test]
    fn app_state_round_trip() {
        let store = Sample::new();
        let leak = || -> &'static mut Settings { std::boxed::Box::leak(std::boxed::Box::new(Settings::DEFAULT)) };
        let alert = || std::boxed::Box::leak(std::boxed::Box::new(crate::screens::alert::AlertInfo::NONE));
        let mut a = App::new(leak(), alert());
        a.set_channel(1, 1, 1, &store);
        a.set_watch(2);
        a.slots[0].vfo = 14_652_000;
        a.active = 2;
        let mut v = Vec2(std::vec::Vec::new());
        a.save_state(&mut v);
        let mut b = App::new(leak(), alert());
        b.restore_state(&mut SliceSource(&v.0), &store);
        assert_eq!(b.active, 2);
        assert_eq!(b.watch, 2);
        assert_eq!(boot_cmds(&mut b), [Some(2), Some(0b111), Some(0)], "MODE-6.5: active, watch, APRS slot");
        assert_eq!(boot_cmds(&mut App::new(leak(), alert()))[0], Some(0), "first boot: App::new alone");

        assert_eq!(b.slots[0].vfo, 14_652_000);
        assert_eq!(b.slots[1].ch.map(|c| c.num), a.slots[1].ch.map(|c| c.num));
        assert!(!b.slots[1].vfo_mode);
        // A record of another format leaves everything as it was.
        let mut c = App::new(leak(), alert());
        v.0[0] = 99;
        c.restore_state(&mut SliceSource(&v.0), &store);
        assert_eq!(c.active, 0);
    }
}
