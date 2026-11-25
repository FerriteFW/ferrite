//! Remote PTT over the link (manual chapter 29, LINK-20; Appendix G's `KEY`). A remote key-up is a PTT press: it
//! goes through [`App::key_down`] as VOX does (VOX-13), so every check a person's PTT meets is met here too (the
//! region and band plan, calibration, busy lockout, the receive-only channel, the build), and the Radio task checks
//! again when it keys. On top of what VOX already skips:
//!
//! - **Never an emergency frequency** (TX-35): refused with a toast, never the second-PTT prompt, and a remote
//!   press never answers a prompt a person opened ([`crate::screens::transmit::Emergency`]).
//! - **No keepalive, no transmit:** none for [`key::KEEPALIVE_MS`] and the radio unkeys as a cut (TX-47: no end
//!   tone, CW ID or carrier hold), as it does when the session closes or moves port.
//! - **The radio's own operator wins:** a local PTT pressed during a remote transmission ends it and does nothing
//!   else until it is let go (pressing again then transmits as usual); a remote key-up is refused while a local PTT
//!   is held. Ending it, rather than handing the transmission over, is the safest choice: a press is how a person
//!   on the radio stops a transmission they didn't start, and a transmission never changes hands while keyed.
//! - **It ends with its session:** `KEY` off, the session moving port or closing, and every stop of the
//!   transmission itself (the timeout, the Radio task's refusals and the sequencer's stops) end it; the PC hears why
//!   at its next keepalive.

use crate::chan::ChannelStore;
use crate::screens::transmit::{self, short};
use crate::{App, Screen};
use ferrite_core::keys::{Key, PTT_A, PTT_ACTIVE, PTT_B};
use ferrite_core::link::key;
use ferrite_core::msg::SLOTS;
use ferrite_core::txplan::{self, Refusal};

/// Who presses remote keys or keys remotely (LINK-28: a link session on each port at once): the cable's session,
/// Bluetooth's, or the terminal's `key` command.
pub const BY_CABLE: u8 = 0;
pub const BY_BT: u8 = 1;
pub const BY_TERM: u8 = 2;

/// A remote transmission holding the key: the PTT it pressed (`Key::Ptt` code), the slot keyed, the last keepalive,
/// and who keyed it ([`BY_CABLE`], [`BY_BT`]): the only one whose keepalives count (LINK-20, LINK-28).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Remote {
    pub ptt: u8,
    pub slot: u8,
    pub last: u32,
    pub by: u8,
}

/// Why a remote transmission ended, told to the PC at its next keepalive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteEnd {
    /// `KEY` off.
    Off,
    /// No keepalive for 2 s.
    Keepalive,
    /// The session closed or moved to the other port (LINK-28).
    Session,
    /// A person pressed PTT on the radio.
    LocalPtt,
    /// The transmit timeout (TX-30).
    Timeout,
    /// The Radio task refused or stopped it (calibration, battery under load, the duty limit).
    Radio(Refusal),
    /// It ended on the radio some other way.
    Stopped,
}

impl RemoteEnd {
    /// For the PC: `invalid` and this.
    pub fn text(self) -> &'static str {
        match self {
            RemoteEnd::Off => "not keyed",
            RemoteEnd::Keepalive => "ended: no keepalive for 2 s",
            RemoteEnd::Session => "ended: the link session closed",
            RemoteEnd::LocalPtt => "ended: PTT pressed on the radio",
            RemoteEnd::Timeout => "ended: transmit timeout",
            RemoteEnd::Radio(r) => short(r),
            RemoteEnd::Stopped => "ended on the radio",
        }
    }
}

/// The answer to a remote key-up or keepalive while the other port's session holds the key (LINK-28).
pub const OTHER_PORT: &str = "keyed from the other port";

/// A local PTT's bit in [`App::local_ptt`] and [`App::ptt_cut`]: A, B, the side connector.
pub(crate) fn bit(k: u8) -> u8 {
    match k {
        PTT_A => 1,
        PTT_B => 2,
        _ => 4,
    }
}

impl App {
    /// A remote transmission holds the key (LINK-20): the Transmit screen says "Remote".
    pub fn remote_keyed(&self) -> bool {
        self.remote.is_some()
    }

    /// `KEY` on from `by`: key `slot` (`key::ACTIVE` or 0-2) as its PTT would, with every check. Ok: the slot keyed;
    /// Err: why not, for the PC (the radio shows a refusal as a PTT's: the refused screen, or the toast VOX gets).
    /// One keyer at a time (LINK-28): refused while the other port's session holds the key.
    pub fn remote_key(&mut self, slot: u8, by: u8, now: u32, store: &impl ChannelStore) -> Result<u8, &'static str> {
        self.now = now;
        // The PTT that keys it (KEY-7): PTT B keys B; PTT A and the side connector the active slot. A or C keys only
        // while it is the active slot, since the radio has no PTT for it otherwise.
        let (ptt, s) = match slot {
            key::ACTIVE => (PTT_ACTIVE, self.active),
            1 => (PTT_B, 1),
            s if s == self.active => (PTT_ACTIVE, s),
            s if (s as usize) < SLOTS => return Err("that slot isn't active: the active slot or B"),
            _ => return Err("no such slot"),
        };
        if let Some(r) = self.remote {
            if r.by != by {
                return Err(OTHER_PORT);
            }
            if r.slot == s {
                self.remote = Some(Remote { last: now, ..r }); // a repeated key-up is a keepalive
                return Ok(s);
            }
            return Err(short(Refusal::InProgress));
        }
        if self.local_ptt | self.ptt_cut != 0 {
            return Err("PTT held on the radio");
        }
        if self.radio.keyed.is_some() {
            return Err(short(Refusal::InProgress));
        }
        // TX-35: not a person pressing PTT, so never an emergency frequency, and never the prompt's second press.
        if matches!(self.screen, Screen::Emergency(_)) || txplan::emergency(transmit::tx_freq(self, s)) {
            self.show_toast(b"No remote TX", b"emergency frequency", false);
            return Err("emergency frequency: only PTT on the radio, twice");
        }
        // APRS-21.3: the APRS frequency on A or B is data only while C is the APRS slot: PTT on the radio, twice.
        if self.aprs_voice_gate(s) {
            self.show_toast(b"No remote TX", b"APRS frequency", false);
            return Err("APRS frequency: data only: PTT on the radio, twice");
        }
        // APRS-18: a PTT there sends or opens a message; remote PTT is voice only.
        if matches!(self.screen, Screen::Compose(_)) || self.aprs_slot(s) {
            return Err(if self.pager_on() { "Pager: no voice there" } else { "APRS: no voice there" });
        }
        self.remote = Some(Remote { ptt, slot: s, last: now, by });
        self.remote_ended = None;
        self.press(Key::Ptt(ptt), true, now, store);
        if self.radio.keyed == Some(s) {
            return Ok(s);
        }
        // Refused on the way (the refused screen is up, as for PTT): let go, and say why.
        let why = match self.screen {
            Screen::TxRefused(t) => short(t.why),
            _ => "not keyed",
        };
        self.remote = None;
        self.press(Key::Ptt(ptt), false, now, store);
        Err(why)
    }

    /// `KEY` keepalive from `by`: Ok while its remote transmission holds; Err with why it ended once it has. The
    /// other port's keepalives don't hold it (LINK-28).
    pub fn remote_keepalive(&mut self, by: u8, now: u32) -> Result<(), &'static str> {
        match &mut self.remote {
            Some(r) if r.by != by => Err(OTHER_PORT),
            Some(r) => {
                r.last = now;
                Ok(())
            }
            None => Err(self.remote_ended.unwrap_or(RemoteEnd::Off).text()),
        }
    }

    /// The remote transmission ends (`KEY` off, a missing keepalive, the session, a person's PTT, a stop on the
    /// radio): its PTT comes up, which unkeys (TX-25).
    pub fn remote_unkey(&mut self, why: RemoteEnd, now: u32, store: &impl ChannelStore) {
        let Some(r) = self.remote.take() else { return };
        self.remote_ended = Some(why);
        // LINK-20, TX-47: a lost keepalive or session is a cut (no end tone, ID or carrier hold), as the timeout;
        // `KEY` off and the other ends unkey as a PTT let go.
        self.tx_cut = matches!(why, RemoteEnd::Keepalive | RemoteEnd::Session);
        self.press(Key::Ptt(r.ptt), false, now, store);
        self.tx_cut = false;
        let body: &[u8] = match why {
            RemoteEnd::Keepalive => b"no keepalive",
            RemoteEnd::Session => b"link closed",
            RemoteEnd::LocalPtt => b"PTT on the radio",
            _ => return,
        };
        self.show_toast(b"Remote PTT ended", body, false);
    }

    /// `by`'s session closed or started over (LINK-20, LINK-28): the remote transmission it keyed ends as a cut, and
    /// the keys it holds are let go. The other port's are left alone.
    pub fn remote_session_closed(&mut self, by: u8, now: u32, store: &impl ChannelStore) {
        if self.remote.is_some_and(|r| r.by == by) {
            self.remote_unkey(RemoteEnd::Session, now, store);
        }
        if self.rkeys.by == by {
            self.remote_keys_release(now, store);
        }
    }

    /// Every tick: no keepalive for 2 s ends it; so does the transmission ending on the radio.
    pub(crate) fn remote_tick(&mut self, now: u32, store: &impl ChannelStore) {
        let Some(r) = self.remote else { return };
        if now.wrapping_sub(r.last) >= key::KEEPALIVE_MS {
            return self.remote_unkey(RemoteEnd::Keepalive, now, store);
        }
        if self.radio.keyed != Some(r.slot) {
            let why = match self.screen {
                Screen::TxRefused(t) => RemoteEnd::Radio(t.why),
                _ if now.wrapping_sub(self.radio.t0) >= self.set.tx_timeout_s() * 1000 => RemoteEnd::Timeout,
                _ => RemoteEnd::Stopped,
            };
            self.remote_unkey(why, now, store);
        }
    }

    /// A PTT press or release that isn't a person's: through the same path as one.
    fn press(&mut self, k: Key, down: bool, now: u32, store: &impl ChannelStore) {
        self.virtual_ptt = true;
        if down {
            self.key_down(k, now, store);
        } else {
            self.key_up(k, now, store);
        }
        self.virtual_ptt = false;
    }

    /// A person's PTT down ([`App::key_down`]): noted; during a remote transmission it ends it and is taken (true).
    pub(crate) fn local_ptt_down(&mut self, k: Key, now: u32, store: &impl ChannelStore) -> bool {
        let Key::Ptt(p) = k else { return false };
        if self.virtual_ptt {
            return false;
        }
        let b = bit(p);
        self.local_ptt |= b;
        if self.ptt_cut & b != 0 {
            return true; // still the press that ended one (a bounce)
        }
        if self.remote.is_some() {
            self.ptt_cut |= b;
            self.remote_unkey(RemoteEnd::LocalPtt, now, store);
            return true;
        }
        false
    }

    /// A person's PTT up ([`App::key_up`]): true if it was the press that ended a remote transmission (taken).
    pub(crate) fn local_ptt_up(&mut self, k: Key) -> bool {
        let Key::Ptt(p) = k else { return false };
        if self.virtual_ptt {
            return false;
        }
        let b = bit(p);
        self.local_ptt &= !b;
        let cut = self.ptt_cut & b != 0;
        self.ptt_cut &= !b;
        cut
    }
}

// ---- Remote keys (LINK-19) ----

/// A remote keypad press (LINK-19, Appendix G's `KEY` actions [`key::UP`] to [`key::LONG`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Up,
    Down,
    Short,
    Long,
}

impl Press {
    /// From `KEY`'s action byte.
    pub fn of(action: u8) -> Option<Press> {
        Some(match action {
            key::UP => Press::Up,
            key::DOWN => Press::Down,
            key::SHORT => Press::Short,
            key::LONG => Press::Long,
            _ => return None,
        })
    }
}

/// Why a remote key isn't pressed: the link answers `busy` or `invalid` with the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyRefusal {
    /// A remote long press is still held: try again shortly (`busy`).
    Busy,
    /// `invalid`, and why.
    Invalid(&'static str),
}

impl KeyRefusal {
    pub fn text(self) -> &'static str {
        match self {
            KeyRefusal::Busy => "busy: a long press is still held",
            KeyRefusal::Invalid(t) => t,
        }
    }
}

/// A keypad or side key's stock code (0-21), the way back from [`Key`]. None for the PTTs.
pub fn code_of(k: Key) -> Option<u8> {
    (0..=key::LAST_CODE).find(|&c| ferrite_core::keys::key(c) == Some(k))
}

/// The remote keys held now and the long press in progress (LINK-19). Kept by the App, so the long press is let go
/// on time by its own tick and every key event is known as remote or local.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemoteKeys {
    /// Codes held by the link, a bit each.
    held: u32,
    /// The long press: its code, and when it is let go.
    long: Option<(u8, u32)>,
    /// When the last remote key went down (a key held [`key::HOLD_MAX_MS`] is let go).
    since: u32,
    /// Who holds them ([`BY_CABLE`], [`BY_BT`], [`BY_TERM`]): another may press only once they are let go.
    by: u8,
}

impl App {
    /// LINK-19: a keypad or side key from the link (stock's `code`), through the same path as the keypad
    /// ([`App::key_down`], [`App::key_up`], the long press, the lights, the wake and the lock). A remote key never
    /// transmits: it is refused while the radio transmits (DTMF and the tone burst are keys then), and any
    /// transmission it would start (PTT's paths, the dialer, a message, a beacon) is refused while it is handled; the
    /// crossband repeater's # starts it (XB-20.2) ([`App::remote_tx_refused`]). The PTTs are not keys here (LINK-20 is remote PTT).
    ///
    /// `by` is who presses: while another holds keys, a press is `busy` (LINK-28).
    pub fn remote_press(&mut self, code: u8, how: Press, by: u8, now: u32, store: &impl ChannelStore) -> Result<(), KeyRefusal> {
        let k = match ferrite_core::keys::key(code) {
            Some(k) if code <= key::LAST_CODE && !matches!(k, Key::Ptt(_)) => k,
            _ => return Err(KeyRefusal::Invalid("no such key: stock key codes 0-21")),
        };
        let bit = 1u32 << code;
        if self.rkeys.held != 0 && self.rkeys.by != by {
            return Err(KeyRefusal::Busy);
        }
        if how == Press::Up {
            if self.rkeys.held & bit != 0 {
                self.remote_up(code, now, store);
            }
            return Ok(()); // a key the link doesn't hold: nothing (never a person's key)
        }
        if self.rkeys.long.is_some() {
            return Err(KeyRefusal::Busy);
        }
        if self.radio.keyed.is_some() {
            return Err(KeyRefusal::Invalid("transmitting: remote keys wait for the unkey"));
        }
        if self.rkeys.held & bit != 0 {
            return match how {
                Press::Down => Ok(()),
                _ => Err(KeyRefusal::Invalid("held: send up first")),
            };
        }
        if self.keys.is_held(k) {
            return Err(KeyRefusal::Invalid("held on the radio"));
        }
        self.rkeys.held |= bit;
        (self.rkeys.since, self.rkeys.by) = (now, by);
        self.remote_in = true;
        self.key_down(k, now, store);
        self.remote_in = false;
        match how {
            Press::Short => self.remote_up(code, now, store),
            Press::Long => self.rkeys.long = Some((code, now.wrapping_add(key::LONG_HOLD_MS))),
            _ => {}
        }
        Ok(())
    }

    /// LINK-19: knob steps from the link, one click at a time as the encoder gives them ([`App::knob`]).
    pub fn remote_knob(&mut self, steps: i8, now: u32, store: &impl ChannelStore) -> Result<(), KeyRefusal> {
        if steps == 0 || steps.unsigned_abs() > key::KNOB_MAX as u8 {
            return Err(KeyRefusal::Invalid("knob: 1 to 20 steps either way"));
        }
        if self.rkeys.long.is_some() {
            return Err(KeyRefusal::Busy);
        }
        if self.radio.keyed.is_some() {
            return Err(KeyRefusal::Invalid("transmitting: remote keys wait for the unkey"));
        }
        self.remote_in = true;
        for _ in 0..steps.unsigned_abs() {
            self.knob(steps.signum(), now, store);
        }
        self.remote_in = false;
        Ok(())
    }

    /// The keys the link holds now, a bit per stock code.
    pub fn remote_keys_held(&self) -> u32 {
        self.rkeys.held
    }

    /// A remote long press still held: the link answers `busy` to keys until it is let go.
    pub fn remote_key_busy(&self) -> bool {
        self.rkeys.long.is_some()
    }

    /// Every key the link holds, let go (the session closed or moved port).
    pub fn remote_keys_release(&mut self, now: u32, store: &impl ChannelStore) {
        while self.rkeys.held != 0 {
            let code = self.rkeys.held.trailing_zeros() as u8;
            self.remote_up(code, now, store);
        }
    }

    /// Every tick: a long press let go on time, and a key held too long let go.
    pub(crate) fn remote_keys_tick(&mut self, now: u32, store: &impl ChannelStore) {
        if let Some((code, at)) = self.rkeys.long {
            if now.wrapping_sub(at) as i32 >= 0 {
                self.remote_up(code, now, store);
            }
        }
        if self.rkeys.held != 0 && now.wrapping_sub(self.rkeys.since) >= key::HOLD_MAX_MS {
            self.remote_keys_release(now, store);
        }
    }

    fn remote_up(&mut self, code: u8, now: u32, store: &impl ChannelStore) {
        self.rkeys.held &= !(1 << code);
        if self.rkeys.long.is_some_and(|(c, _)| c == code) {
            self.rkeys.long = None;
        }
        if let Some(k) = ferrite_core::keys::key(code) {
            self.remote_in = true;
            self.key_up(k, now, store);
            self.remote_in = false;
        }
    }

    /// Who presses the remote keys held now (or last): [`BY_CABLE`], [`BY_BT`], [`BY_TERM`].
    pub(crate) fn rkeys_by(&self) -> u8 {
        self.rkeys.by
    }

    /// A key event from the tracker (a long press, a repeat) is a remote key's.
    pub(crate) fn remote_held(&self, k: Key) -> bool {
        code_of(k).is_some_and(|c| self.rkeys.held & 1 << c != 0)
    }

    /// LINK-19: a remote key is being handled and would start a transmission: refused, with a toast. Every
    /// key-up a key can make (PTT's, the dialer's, a message sent, a beacon now) asks this first. The crossband
    /// repeater is the exception: remote keys start it as the radio's do (XB-20.2, decision 161).
    pub(crate) fn remote_tx_refused(&mut self) -> bool {
        if self.remote_in {
            self.show_toast(b"Remote keys", b"never transmit", false);
        }
        self.remote_in
    }
}
