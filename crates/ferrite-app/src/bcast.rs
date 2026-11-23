//! Broadcast FM by frequency (manual chapter 20, BCR-21 to BCR-30; decision 107): a slot tuned into the broadcast
//! FM band plays on the SI4732 by itself, with no menu, and becomes a BK4829 slot again when tuned out. One slot
//! at a time: the slot that last came into the band has it, and another one in the band shows "FM on B" with no
//! audio until it is tuned out. The FM screen and the broadcast slot share one FM state: its frequency is the
//! slot's while there is one, and the presets recall into it.
//!
//! The broadcast is the exclusive mode [`Mode::Fm`](crate::screens::activity::Mode::Fm) like the FM screen's ([`App::wanted_mode`]): any other mode
//! screen, or PTT, stops it (TX-24), and it starts again after. Home's scope strip is the one mode it plays
//! beside, while the active slot is another, normal one (MODE-5, decision 115; [`crate::screens::activity::RadioUi::fm_on`]).

use crate::App;
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::msg::{BroadcastCmd, LiveAll, ModeLive, RadioCmd, SLOTS};
use ferrite_core::txplan::Region;

/// The broadcast FM band (BCR-21): 87.5-108.0 MHz, 76-95 MHz with region Japan.
pub fn band(r: Option<Region>) -> (Freq10, Freq10) {
    match r {
        Some(Region::Japan) => (mhz(76), mhz(95)),
        _ => (mhz(87) + 50_000, mhz(108)),
    }
}

/// A seek that hasn't landed by now is given up (the receiver found nothing new).
const SEEK_MS: u32 = 10_000;

/// BCR-33 (decision 353): a WFM slot plays broadcast anywhere the SI4732's FM covers, 64-108 MHz (10 Hz).
pub const WFM_BAND: (ferrite_core::freq::Freq10, ferrite_core::freq::Freq10) = (6_400_000, 10_800_000);

impl App {
    /// Slot `s` is tuned into the broadcast FM band (BCR-21); the APRS slot never is (it is held to its
    /// frequency, APRS-19).
    pub fn in_bcast_band(&self, s: u8) -> bool {
        let (lo, hi) = if self.tuned_cfg(s).mode == crate::slot::WFM { WFM_BAND } else { band(self.set.region()) };
        !self.aprs_slot(s) && (lo..=hi).contains(&self.tuned_freq(s))
    }

    /// The slot playing broadcast FM (BCR-22), if any.
    pub fn bcast_slot(&self) -> Option<u8> {
        self.radio.bcast
    }

    /// BCR-22: which slot has the broadcast, from where the slots are: a slot that came into the band takes it
    /// (the active one first when several came at once); the slot that has it keeps it while it stays in; when it
    /// leaves, another slot still in the band takes it back. A change goes to the Radio task (BCR-23).
    pub(crate) fn bcast_update(&mut self) {
        let inb = (0..SLOTS as u8).filter(|&s| self.in_bcast_band(s)).fold(0u8, |m, s| m | 1 << s);
        let came = inb & !self.radio.bcast_in;
        self.radio.bcast_in = inb;
        let a = self.active;
        let pick = |m: u8| match m {
            0 => None,
            m if m & 1 << a != 0 => Some(a),
            m => Some(m.trailing_zeros() as u8),
        };
        let next = pick(came).or(self.radio.bcast.filter(|&s| inb & 1 << s != 0)).or_else(|| pick(inb));
        if next != self.radio.bcast {
            self.radio.bcast = next;
            self.radio.bcast_since = self.now;
            self.out.push(RadioCmd::BcastSlot(next));
        }
        // BCR-5 (decision 352: the slot's, the FM screen's before): the sleep timer mutes the broadcast slot once
        // its time is up (quick mute, HOME-15: never saved, so it plays again after a restart or a \* on it)
        let min = crate::settings::SLEEP_TIMES[self.set.fm_sleep as usize % crate::settings::SLEEP_TIMES.len()] as u32;
        if let Some(s) = next.filter(|&s| min != 0 && self.mute & 1 << s == 0 && self.now.wrapping_sub(self.radio.bcast_since) >= min * 60_000) {
            self.mute |= 1 << s;
            self.send_mute();
            self.radio.bcast_since = self.now;
            self.show_toast(b"Sleep timer", b"broadcast muted", false);
        }
        // BCR-4: radio monitor off keeps the other slots quiet while the broadcast plays (it starts, stops, or the
        // setting changes: the mask goes again)
        if self.radio_mute() != self.radio.mute_sent {
            self.send_mute();
        }
    }

    /// The Radio task's mute mask: the quick mutes (HOME-15), and with radio monitor off (0 2 6 3, BCR-4) every
    /// slot but the broadcast one while it plays, so none of them takes the speaker from it (they still receive,
    /// light the LED and decode).
    pub(crate) fn radio_mute(&self) -> u8 {
        let monitor_off = match self.radio.bcast {
            Some(b) if self.radio.fm_on && !self.set.fm_monitor => ((1u8 << SLOTS) - 1) & !(1 << b),
            _ => 0,
        };
        self.mute | monitor_off
    }

    /// [`App::radio_mute`] to the Radio task.
    pub(crate) fn send_mute(&mut self) {
        let m = self.radio_mute();
        self.radio.mute_sent = m;
        self.out.push(RadioCmd::Mute(m));
    }

    /// The station the SI4732 plays (10 kHz): the broadcast slot's frequency, else the FM screen's last one.
    pub fn fm_freq(&self) -> u16 {
        match self.radio.bcast {
            Some(s) => (self.tuned_freq(s) / 1000) as u16,
            None => self.set.fm_freq,
        }
    }

    /// The FM tuning range (10 kHz): the region's broadcast band (BCR-21; decision 352 retired the FM screen's
    /// 64-108 MHz).
    pub(crate) fn fm_range(&self) -> (u16, u16) {
        let wfm = self.radio.bcast.is_some_and(|s| self.tuned_cfg(s).mode == crate::slot::WFM);
        let (lo, hi) = if wfm { WFM_BAND } else { band(self.set.region()) };
        ((lo / 1000) as u16, (hi / 1000) as u16)
    }

    /// A new FM station (tuned, sought or a preset, BCR-25): into the broadcast slot, on its VFO (a channel slot
    /// moves to its VFO there), and kept as the FM screen's last station. The reconciler retunes the receiver.
    pub(crate) fn fm_set(&mut self, f: u16) {
        self.set.fm_freq = f;
        let Some(s) = self.radio.bcast else { return };
        let f10 = f as Freq10 * 1000;
        let sl = &mut self.slots[s as usize % SLOTS];
        if sl.vfo_mode && sl.vfo == f10 {
            return;
        }
        (sl.vfo, sl.vfo_mode, sl.clar) = (f10, true, 0);
        sl.mem[crate::slot::band_near(f10)] = f10;
        self.send_slot(s);
    }

    /// ◀▶ seek from the station playing (BCR-25): the receiver's own; the station it stops on is taken as it lands
    /// ([`App::bcast_tick`]).
    pub(crate) fn fm_seek(&mut self, up: bool) {
        if self.radio.fm_on {
            self.out.push(RadioCmd::Broadcast(BroadcastCmd::Seek { up }));
            self.radio.fm_seek_at = Some(self.now);
        }
    }

    /// MODE-2 for the broadcast: while FM runs, a new station (the slot tuned, a preset) retunes the receiver.
    /// Not while a seek is landing: that one is the receiver's.
    pub(crate) fn fm_retune(&mut self) {
        let f = self.fm_freq();
        if self.radio.fm_on && self.radio.fm_seek_at.is_none() && f != self.radio.fm_sent {
            self.out.push(RadioCmd::Broadcast(BroadcastCmd::Fm(f)));
            self.radio.fm_sent = f;
            self.set.fm_freq = f;
        }
    }

    /// Every tick: a seek's station, once the receiver reports one other than where it began.
    pub(crate) fn bcast_tick(&mut self, live: &LiveAll) {
        let Some(t) = self.radio.fm_seek_at else { return };
        match live.mode {
            ModeLive::Fm { freq, .. } if self.radio.fm_on && freq != 0 && freq != self.radio.fm_sent => {
                self.radio.fm_sent = freq;
                self.radio.fm_seek_at = None;
                self.fm_set(freq);
            }
            _ if !self.radio.fm_on || self.now.wrapping_sub(t) >= SEEK_MS => self.radio.fm_seek_at = None,
            _ => {}
        }
    }

    /// The broadcast plays on the speaker now (no amateur slot heard, BCR-24).
    pub fn bcast_heard(&self, live: &LiveAll) -> bool {
        self.radio.fm_on && live.bcast_heard
    }

    /// Slot `s` isn't listened to while broadcast plays: the band relay is released (BCR-15), and it needs it.
    pub(crate) fn relay_off(&self, s: u8) -> bool {
        self.radio.fm_on && Some(s) != self.radio.bcast && ferrite_core::path::Sel::rx(self.tuned_freq(s)).relay()
    }

    /// HOME-14, MODE-5: home's strip can't sweep beside the broadcast: the active slot is in the broadcast band (it
    /// plays, or shows "FM on B"), or needs the band relay the broadcast leaves released (BCR-15).
    /// MODE-5.1 (ferrite#120): nor with the APRS slot active: a sweep is all the Radio task does while it runs (no
    /// watch turns, no turn for C, no squelch for the decoder's capture), so around C APRS would hear nothing. With
    /// A or B active the strip sweeps and APRS waits; # pauses the strip ([`App::strip_paused`]) to give it back.
    /// MODE-5.1: # on home pauses and resumes the strip: single + scope, where the strip can run here.
    pub(crate) fn strip_pausable(&self) -> bool {
        self.set.layout == 1 && !self.strip_blocked() && !self.low_power()
    }

    pub(crate) fn strip_blocked(&self) -> bool {
        if self.aprs_slot(self.active) {
            return true;
        }
        self.radio.bcast.is_some() && (self.in_bcast_band(self.active) || ferrite_core::path::Sel::rx(self.tuned_freq(self.active)).relay())
    }
}

/// The programme types (BCR-26.8): RBDS's North American list (NRSC-4) in the US and Canada, RDS's (EN 50067)
/// elsewhere; empty for 0 (none) and RBDS's unassigned 24-28. At most 15 characters: the card's header holds
/// "123 · " and the name beside "muted". Consts, so in slow flash (read twice a second at most).
const PTY_RBDS: [&str; 32] = [
    "", "News", "Information", "Sports", "Talk", "Rock", "Classic rock", "Adult hits", "Soft rock", "Top 40",
    "Country", "Oldies", "Soft", "Nostalgia", "Jazz", "Classical", "Rhythm & blues", "Soft R&B", "Language",
    "Religious music", "Religious talk", "Personality", "Public", "College", "", "", "", "", "", "Weather",
    "Emergency test", "Emergency",
];
const PTY_RDS: [&str; 32] = [
    "", "News", "Current affairs", "Information", "Sport", "Education", "Drama", "Culture", "Science", "Varied",
    "Pop music", "Rock music", "Easy listening", "Light classical", "Classical", "Other music", "Weather",
    "Finance", "Children's", "Social affairs", "Religion", "Phone-in", "Travel", "Leisure", "Jazz", "Country",
    "National music", "Oldies", "Folk music", "Documentary", "Alarm test", "Alarm",
];

pub fn pty_name(pty: u8, region: Option<Region>) -> &'static str {
    let rbds = matches!(region, Some(Region::Us | Region::Canada));
    (if rbds { &PTY_RBDS } else { &PTY_RDS })[pty as usize & 31]
}

impl App {
    /// BCR-26.8: the programme type slot `s` plays now ("" when it isn't the broadcast playing, or none is sent).
    pub(crate) fn bcast_pty(&self, live: &LiveAll, s: u8) -> &'static str {
        match live.mode {
            ModeLive::Fm { pty, .. } if self.radio.fm_on && self.bcast_slot() == Some(s) => pty_name(pty, self.set.region()),
            _ => "",
        }
    }
}

/// The RDS line (BCR-26): the radio text, else the station name, else nothing (padding trimmed).
pub fn rds_text<'a>(ps: &'a [u8], rt: &'a [u8]) -> &'a [u8] {
    let rt = trim(rt);
    if rt.is_empty() { trim(ps) } else { rt }
}

/// Text up to its padding (NULs, trailing spaces).
pub fn trim(b: &[u8]) -> &[u8] {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    let mut s = &b[..n];
    while let [rest @ .., b' '] = s {
        s = rest;
    }
    s
}
