//! The transmit sequencer (TX-17): every transmission, from PTT to a KISS frame, goes through [`TxSeq::key`]
//! and comes out of [`TxSeq::release`] or [`TxSeq::tick`], with the same checks and the same orders. It keeps
//! the timeout (TX-30, with PTT released before the next key-up), the duty limit (TX-31), the battery cut-off
//! under load (TX-32) and the CW ID's ten minutes (VOX-12); the pins are the path machine's ([`Path`]).
//!
//! In the firmware it is built only with the `tx` feature; the UI makes the same checks first, so a refusal
//! from here (calibration, read-back) is rare.

use crate::freq::Freq10;
use crate::path::{Act, Blocked, Path, Sel, Steps};
use crate::txplan::{self, Refusal, Region};

/// The PA drive actually written for a wanted value `v` (TX-23): 0 when dry (Dry TX
/// on in a `tx` build (TX-48). The firmware's one PA choke point (`hal::pa::set`) goes through it, on every
/// key-up and every ramp step.
pub const fn pa_drive(v: u8, dry: bool) -> u8 {
    if dry { 0 } else { v }
}

/// What is being sent: the end tone follows voice only (TONE-11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// PTT, VOX, remote KEY.
    Voice,
    /// APRS, KISS, packet frames, a tone burst on its own, DTMF dialling.
    Data,
}

/// A key-up request: the slot's frequencies and what the Radio task knows about it.
#[derive(Clone, Copy, Debug)]
pub struct Req {
    pub tx: Freq10,
    /// The slot's RX frequency (TX-37's pair).
    pub rx: Freq10,
    pub kind: Kind,
    /// A person pressed PTT (not a beacon, KISS, VOX or remote KEY).
    pub person: bool,
    /// The emergency prompt's second PTT was given (TX-3).
    pub confirmed: bool,
    /// The slot is in AM (HF mode's AM on chip 1 too).
    pub am: bool,
    /// The slot is on a receive-only channel (CHAN-4; TX audit F11: the Radio task's own copy of the flag).
    pub rx_only: bool,
    /// Busy lockout is on for the slot and its squelch is open (TX-21).
    pub busy: bool,
    /// HF mode on an SI4732 frequency or mode (HF-24): below 18 MHz, AM up to 30 MHz, USB, LSB, CW.
    pub hf_si: bool,
    /// The SI4732 is playing a broadcast (TX-24: the UI stops it before key-up, BCR-29).
    pub si_on: bool,
    pub region: Region,
    pub extend: bool,
    /// The calibration sector and the MCU check passed (TX-13 to TX-15).
    pub cal_ok: bool,
    /// The calibration's TX windows hold `tx` (CAL-11, `TxCal::in_window`; 18-64 MHz as stock's HF mode, TX-41).
    pub cal_window: bool,
    /// The battery is above the cut-off.
    pub batt_ok: bool,
    /// The power level: 0 Low, 1 Mid, 2 High; it sets how fast the duty bucket fills (TX-38).
    pub power: u8,
}

/// The sequencer's settings, from the slot and the Transmit group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cfg {
    /// TX-30: 30-600 s.
    pub timeout_s: u16,
    /// The pre-alert this long before the timeout (0 = none).
    pub prealert_s: u8,
    pub end_tone: bool,
    /// CW ID every 10 min (VOX-12).
    pub cw_id: bool,
    /// CW ID at every unkey instead (0 8 6 2's "on unkey"; VOX-12).
    pub cw_each: bool,
    /// Tail elimination: a DCS turn-off code or the 55 Hz phase (TONE-9).
    pub tail: bool,
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg { timeout_s: 180, prealert_s: 10, end_tone: false, cw_id: false, cw_each: false, tail: false }
    }
}

/// VOX-12: the ID goes at the next unkey once this long has passed since the last one.
pub const CW_ID_MS: u32 = 10 * 60_000;
/// TX-31's duty limit (TX-38), a leaky bucket (to confirm on the bench): keyed time fills it, weighted by the power
/// level (High heats the PA most), idle time drains it at half of High's rate, and a full bucket unkeys and
/// refuses until it is half empty. Doubled from 120 s with the drain from a quarter (Alex, Nov 24, 2026, ferrite#228:
/// 2 min on High cut longer QSOs off).
pub const DUTY_FULL_MS: u32 = 240_000;
pub const DUTY_DRAIN_DIV: u32 = 2;
/// How fast keyed time fills the bucket per power level, in quarters: Low 1/4, Mid 1/2, High 1 (decided Oct 7,
/// 2026; weights to confirm on the bench). Continuous transmit from cold: High 4 min, Mid 8 min, Low 16 min.
/// A `static`: read at key-up by the Radio task, and only named statics stay in the firmware's fast flash.
pub static DUTY_WEIGHT: [u32; 3] = [1, 2, 4];

/// TX-32's hysteresis (TX audit F13): the battery must read under the cut-off this long without a break before a
/// transmission is cut; one low sample (a PA current spike, ADC noise) doesn't unkey. At the 10-50 ms keyed tick,
/// that is 10 or more readings.
pub const BATT_LOW_MS: u32 = 500;

/// Why the sequencer unkeyed by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The timeout's pre-alert (TX-30).
    PreAlert,
    Timeout,
    /// The duty limit (TX-31).
    Hot,
    /// The battery fell below the under-load threshold (TX-32).
    Battery,
}

/// The sequencer (36 bytes).
#[derive(Clone, Copy, Debug, Default)]
pub struct TxSeq {
    /// Keyed since (ms), and what.
    since: Option<(u32, Kind)>,
    alerted: bool,
    /// Timed out: PTT must be released before the next key-up.
    held: bool,
    /// The last CW ID (ms).
    last_id: Option<u32>,
    /// The bucket, in quarter milliseconds: a keyed ms adds its weight, an idle ms drains one, so no tick's
    /// rounding changes the rates (in ms, at the 10 ms keyed tick Low filled 2 a tick for 2.5: 10 min to full, not 8).
    duty: u32,
    duty_at: u32,
    hot: bool,
    /// The keyed power level's fill weight (DUTY_WEIGHT).
    weight: u8,
    /// Since when the battery has read under the cut-off without a break while keyed (TX-32, [`BATT_LOW_MS`]).
    batt_low: Option<u32>,
}

impl TxSeq {
    pub const fn new() -> Self {
        TxSeq { since: None, alerted: false, held: false, last_id: None, duty: 0, duty_at: 0, hot: false, weight: 4, batt_low: None }
    }

    pub fn keyed(&self) -> bool {
        self.since.is_some()
    }

    /// The checks, in TX-20's order, as the Radio task applies them (TX-17).
    pub fn check(&mut self, r: &Req, path: &Path, now: u32) -> Result<(), Refusal> {
        self.heat(now);
        if self.since.is_some() || path.keyed().is_some() {
            return Err(Refusal::InProgress);
        }
        if self.held {
            return Err(Refusal::ReleasePtt);
        }
        // TX audit F11: the UI's refusals, again here, so no path to the Radio task skips them.
        if r.rx_only {
            return Err(Refusal::RxOnlyChannel);
        }
        if r.hf_si {
            return Err(Refusal::ReceiveOnlyHere);
        }
        if r.am {
            return Err(Refusal::AmRxOnly);
        }
        if r.si_on {
            return Err(Refusal::Broadcast);
        }
        txplan::check(r.tx, r.region, r.extend)?;
        if !r.cal_ok {
            return Err(Refusal::Locked);
        }
        // TX-1, CAL-11: the region plan (or Extend TX's hardware range) ∩ this radio's calibration windows.
        if !r.cal_window {
            return Err(Refusal::NotCalibrated);
        }
        if !txplan::relay_ok(r.rx, r.tx) {
            return Err(Refusal::RelayMismatch);
        }
        if r.busy {
            return Err(Refusal::Busy);
        }
        if txplan::emergency(r.tx) && !(r.person && r.confirmed) {
            return Err(Refusal::Emergency);
        }
        if !r.batt_ok {
            return Err(Refusal::Battery);
        }
        if self.hot {
            return Err(Refusal::Hot);
        }
        Ok(())
    }

    /// Key up: the checks, then the path machine's key-up into `out`.
    pub fn key(&mut self, r: &Req, path: &mut Path, now: u32, out: &mut Steps) -> Result<(), Refusal> {
        self.check(r, path, now)?;
        path.key_up(Sel::tx(r.tx), now, out).map_err(|b| match b {
            Blocked::Keyed => Refusal::InProgress,
            Blocked::NeedsRelay | Blocked::TooOften => Refusal::RelayMismatch,
        })?;
        // TX audit F6: a data burst (APRS, KISS: AFSK from DAC1) with the audio stage on, as stock sets PB8 before
        // its AFSK (0x080091BC); it stays on to the unkey.
        if r.kind == Kind::Data {
            out.push(Act::Stage);
        }
        self.since = Some((now, r.kind));
        self.weight = DUTY_WEIGHT[r.power.min(2) as usize] as u8;
        (self.alerted, self.batt_low) = (false, None);
        Ok(())
    }

    /// The crossband repeater's key-up (XB-6, XB-10): the same checks (region, calibration, relay, battery, the duty
    /// limit weighted by power), then the path machine's crossband key-up, the receiving chip left in receive:
    /// `r.rx` is the receiving side's frequency, its receiver selected first (XB-5, decision 163).
    /// Send it as voice that no person keyed (`person: false`): it relays voice, gets the CW ID when the
    /// repeater asks for it, and is refused on an emergency frequency.
    pub fn key_xb(&mut self, r: &Req, path: &mut Path, now: u32, out: &mut Steps) -> Result<(), Refusal> {
        self.check(r, path, now)?;
        path.key_up_xb(Sel::rx(r.rx), Sel::tx(r.tx), now, out).map_err(|b| match b {
            Blocked::Keyed => Refusal::InProgress,
            Blocked::NeedsRelay | Blocked::TooOften => Refusal::RelayMismatch,
        })?;
        self.since = Some((now, r.kind));
        self.weight = DUTY_WEIGHT[r.power.min(2) as usize] as u8;
        (self.alerted, self.batt_low) = (false, None);
        Ok(())
    }

    /// Satellite mode's full-duplex key-up (SAT-11): the same checks, then the path machine's full-duplex key-up,
    /// the downlink's chip left receiving (`r.rx`, its receiver selected first). A person's voice key-up.
    pub fn key_fd(&mut self, r: &Req, path: &mut Path, now: u32, out: &mut Steps) -> Result<(), Refusal> {
        self.check(r, path, now)?;
        path.key_up_fd(Sel::rx(r.rx), Sel::tx(r.tx), now, out).map_err(|b| match b {
            Blocked::Keyed => Refusal::InProgress,
            Blocked::NeedsRelay | Blocked::TooOften => Refusal::RelayMismatch,
        })?;
        self.since = Some((now, r.kind));
        self.weight = DUTY_WEIGHT[r.power.min(2) as usize] as u8;
        (self.alerted, self.batt_low) = (false, None);
        Ok(())
    }

    /// Every PTT input released (TX-25): unkey if keyed, and a timed-out PTT may key again.
    pub fn release(&mut self, cfg: &Cfg, path: &mut Path, now: u32, out: &mut Steps) {
        self.held = false;
        self.unkey(cfg, true, path, now, out);
    }

    /// A remote transmission lost (LINK-20: no keepalive, the session gone): its PTT is up, and it unkeys as a cut
    /// (no end tone, CW ID or carrier hold, TX-47), the carrier dropped at once like the timeout's.
    pub fn cut(&mut self, cfg: &Cfg, path: &mut Path, now: u32, out: &mut Steps) {
        self.held = false;
        self.unkey(cfg, false, path, now, out);
    }

    /// Unkey (TX-26, TX-47, TONE-8): end tone (voice, unless cut short) → CW ID when due (voice only; never at a
    /// cut) → the audio stage on (stock's PB8 at 0x08024D80) → tail elimination inside stock's 400 ms carrier hold,
    /// or the hold alone (every unkey but a cut: timeout, duty, battery, a lost remote key-up) → the path machine's
    /// unkey.
    fn unkey(&mut self, cfg: &Cfg, normal: bool, path: &mut Path, now: u32, out: &mut Steps) {
        self.heat(now);
        let Some((_, kind)) = self.since.take() else { return };
        let voice = kind == Kind::Voice && normal;
        // Crossband (decision 95): stock's RX end first, while still keyed: the mic back and the receive select
        // off, then the roger and tail, then the carrier held (path::UNKEY_HOLD_MS) before the PA goes to 0.
        let xb = path.keyed_xb();
        if xb {
            path.xb_rx_end(out);
        }
        if voice && cfg.end_tone {
            out.push(Act::EndTone);
        }
        if voice && cfg.cw_id && (cfg.cw_each || self.last_id.is_none_or(|t| now.wrapping_sub(t) >= CW_ID_MS)) {
            out.push(Act::CwId);
            self.last_id = Some(now);
        }
        out.push(Act::Stage);
        // TX-47 (TX audit F5): stock holds the carrier 400 ms at every unkey (0x08024DD8), the tail inside it. A cut
        // (timeout, duty limit, battery under load) drops at once: Ferrite's own protections, no carrier added.
        // TX-47.6 (decision 296): a data frame drops at once too, so we hear the digipeat or ack that follows it.
        if kind == Kind::Data {
        } else if cfg.tail {
            out.push(Act::TailElim); // inside stock's 400 ms hold (tonegen::TAIL_MS)
        } else if normal {
            for _ in 0..crate::path::UNKEY_HOLD_MS / 200 {
                out.push(Act::Wait(200));
            }
        }
        path.unkey(now, out);
    }

    /// The safe state from anything (a fault, power off, a failed read-back): unkeyed at once, no tones.
    pub fn safe(&mut self, path: &mut Path, now: u32, out: &mut Steps) {
        self.heat(now);
        self.since = None;
        path.safe(now, out);
    }

    /// The duty bucket up to `now`.
    fn heat(&mut self, now: u32) {
        let dt = now.wrapping_sub(self.duty_at);
        self.duty_at = now;
        // Quarter ms: High's weight 4 is a ms a ms, and the drain a half of that (DUTY_DRAIN_DIV) is two.
        self.duty = if self.since.is_some() {
            self.duty.saturating_add(dt.saturating_mul(self.weight as u32))
        } else {
            self.duty.saturating_sub(dt.saturating_mul(4 / DUTY_DRAIN_DIV))
        };
        if self.duty >= DUTY_FULL_MS * 4 {
            self.hot = true;
        } else if self.duty <= DUTY_FULL_MS * 4 / 2 {
            self.hot = false;
        }
    }

    /// Time passing while keyed: the pre-alert, then the timeout (PTT must be released after), the duty limit and
    /// the battery under load (low for [`BATT_LOW_MS`] without a break, TX-32) each unkey without the end tone
    /// (TONE-11).
    pub fn tick(&mut self, cfg: &Cfg, batt_ok: bool, path: &mut Path, now: u32, out: &mut Steps) -> Option<Event> {
        self.heat(now);
        let (t0, _) = self.since?;
        self.batt_low = if batt_ok { None } else { Some(self.batt_low.unwrap_or(now)) };
        let batt_cut = self.batt_low.is_some_and(|t| now.wrapping_sub(t) >= BATT_LOW_MS);
        let on = now.wrapping_sub(t0);
        let limit = cfg.timeout_s.clamp(30, 600) as u32 * 1000;
        let ev = if on >= limit {
            self.held = true;
            Event::Timeout
        } else if self.hot {
            Event::Hot
        } else if batt_cut {
            Event::Battery
        } else {
            if !self.alerted && cfg.prealert_s > 0 && on + cfg.prealert_s as u32 * 1000 >= limit {
                self.alerted = true;
                return Some(Event::PreAlert);
            }
            return None;
        };
        self.unkey(cfg, false, path, now, out);
        Some(ev)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::freq::mhz;
    use crate::path::{Cause, Rx};

    /// TX-48: with Dry TX on every PA drive written is 0, whatever the ramp asks for; off,
    /// the wanted value goes through.
    #[test]
    fn dry_holds_the_pa_at_zero() {
        for v in [0u8, 1, 80, 200, 255] {
            assert_eq!(pa_drive(v, true), 0, "dry, {v}");
            assert_eq!(pa_drive(v, false), v, "not dry, {v}");
        }
    }

    fn req(tx: Freq10) -> Req {
        Req {
            tx,
            rx: tx,
            kind: Kind::Voice,
            person: true,
            confirmed: false,
            am: false,
            rx_only: false,
            busy: false,
            hf_si: false,
            si_on: false,
            region: Region::Us,
            extend: false,
            cal_ok: true,
            cal_window: true,
            batt_ok: true,
            power: 2,
        }
    }

    fn rx_on(f: Freq10) -> Path {
        let mut p = Path::new();
        p.rx(Rx::Bk(Sel::rx(f)), Cause::ActiveSlot, 0, &mut Steps::default()).unwrap();
        p
    }

    #[test]
    fn refusals() {
        let mut t = TxSeq::new();
        let p = rx_on(mhz(146));
        let ok = req(mhz(146) + 52_000);
        assert_eq!(t.check(&ok, &p, 0), Ok(()));
        assert_eq!(t.check(&Req { am: true, ..ok }, &p, 0), Err(Refusal::AmRxOnly));
        // TX audit F11: the UI's refusals made again in the Radio task (TX-20.1, TX-21, TX-24, HF-24, CHAN-4).
        assert_eq!(t.check(&Req { rx_only: true, ..ok }, &p, 0), Err(Refusal::RxOnlyChannel));
        assert_eq!(t.check(&Req { busy: true, ..ok }, &p, 0), Err(Refusal::Busy));
        assert_eq!(t.check(&Req { hf_si: true, ..req(mhz(28) + 40_000) }, &p, 0), Err(Refusal::ReceiveOnlyHere));
        assert_eq!(t.check(&Req { si_on: true, ..ok }, &p, 0), Err(Refusal::Broadcast));
        assert_eq!(t.check(&Req { cal_ok: false, ..ok }, &p, 0), Err(Refusal::Locked));
        assert_eq!(t.check(&Req { cal_window: false, ..ok }, &p, 0), Err(Refusal::NotCalibrated), "CAL-11");
        assert_eq!(t.check(&Req { cal_window: false, ..req(mhz(162) + 55_000) }, &p, 0), Err(Refusal::OutOfBand), "the plan first");
        assert_eq!(t.check(&Req { batt_ok: false, ..ok }, &p, 0), Err(Refusal::Battery));
        assert_eq!(t.check(&req(mhz(162) + 55_000), &p, 0), Err(Refusal::OutOfBand));
        assert_eq!(t.check(&Req { extend: true, ..req(mhz(162) + 55_000) }, &p, 0), Ok(()));
        assert_eq!(t.check(&req(mhz(223) + 50_000), &p, 0), Ok(()), "TX-10.4: 1.25 m as any band");
        assert_eq!(t.check(&req(mhz(52) + 52_500), &p, 0), Ok(()), "TX-4: 6 m");
        assert_eq!(t.check(&Req { extend: true, ..req(mhz(27) + 18_500) }, &p, 0), Ok(()), "TX-41: CB with Extend TX");
        assert_eq!(t.check(&Req { rx: mhz(28) + 40_000, ..ok }, &p, 0), Err(Refusal::RelayMismatch));
        let sos = Req { extend: true, ..req(mhz(121) + 50_000) };
        assert_eq!(t.check(&sos, &p, 0), Err(Refusal::NotCalibrated), "airband is outside the TX hardware");
        let sos = Req { extend: true, ..req(mhz(406) + 2_500) };
        assert_eq!(t.check(&sos, &p, 0), Err(Refusal::Emergency));
        assert_eq!(t.check(&Req { confirmed: true, ..sos }, &p, 0), Ok(()));
        assert_eq!(t.check(&Req { confirmed: true, person: false, ..sos }, &p, 0), Err(Refusal::Emergency), "TX-35");
        // TX-3, TX-10.5: 243.0 MHz on the open 200-300 MHz path: the double PTT, as the other emergency frequencies.
        let sos = Req { extend: true, ..req(mhz(243)) };
        assert_eq!(t.check(&sos, &p, 0), Err(Refusal::Emergency));
        assert_eq!(t.check(&Req { confirmed: true, ..sos }, &p, 0), Ok(()));
    }

    #[test]
    fn unkey_order_and_cw_id() {
        let mut t = TxSeq::new();
        let mut p = rx_on(mhz(146));
        let cfg = Cfg { end_tone: true, cw_id: true, tail: true, ..Cfg::default() };
        let r = req(mhz(146) + 52_000);
        t.key(&r, &mut p, 0, &mut Steps::default()).unwrap();
        assert_eq!(t.key(&r, &mut p, 0, &mut Steps::default()), Err(Refusal::InProgress));
        let mut s = Steps::default();
        t.release(&cfg, &mut p, 5_000, &mut s);
        assert_eq!(s.as_slice()[..5], [Act::EndTone, Act::CwId, Act::Stage, Act::TailElim, Act::PaZero]);
        // Again within ten minutes: no ID.
        t.key(&r, &mut p, 60_000, &mut Steps::default()).unwrap();
        let mut s = Steps::default();
        t.release(&cfg, &mut p, 65_000, &mut s);
        assert_eq!(s.as_slice()[..4], [Act::EndTone, Act::Stage, Act::TailElim, Act::PaZero]);
        // "On unkey": the ID every time.
        let each = Cfg { cw_each: true, ..cfg };
        t.key(&r, &mut p, 70_000, &mut Steps::default()).unwrap();
        let mut s = Steps::default();
        t.release(&each, &mut p, 75_000, &mut s);
        assert_eq!(s.as_slice()[..3], [Act::EndTone, Act::CwId, Act::Stage]);
        // Ten minutes after the last ID, data: no end tone, no ID; the audio stage on from the key-up (F6).
        let mut s = Steps::default();
        t.key(&Req { kind: Kind::Data, ..r }, &mut p, 700_000, &mut s).unwrap();
        assert_eq!(s.as_slice().last(), Some(&Act::Stage), "after the PA ramp");
        let mut s = Steps::default();
        t.release(&cfg, &mut p, 700_500, &mut s);
        assert_eq!(s.as_slice()[..2], [Act::Stage, Act::PaZero], "no tail or hold after a data frame (TX-47.6)");
        // Not keyed: release does nothing.
        let mut s = Steps::default();
        t.release(&cfg, &mut p, 701_000, &mut s);
        assert!(s.is_empty());
    }

    /// TX-47.1, TX-47.4 (TX audit F5): stock's 400 ms carrier hold at every voice unkey, tail or not (the tail plays
    /// inside it); a cut drops at once. TX-47.6 (decision 296): a data unkey holds nothing, tail on or off.
    #[test]
    fn carrier_held_400_ms_at_every_unkey() {
        let mut t = TxSeq::new();
        let mut p = rx_on(mhz(146));
        let r = req(mhz(146) + 52_000);
        for (kind, tail) in [(Kind::Voice, false), (Kind::Data, false), (Kind::Data, true)] {
            t.key(&Req { kind, ..r }, &mut p, 0, &mut Steps::default()).unwrap();
            let mut s = Steps::default();
            t.release(&Cfg { tail, ..Cfg::default() }, &mut p, 5_000, &mut s);
            let a = s.as_slice();
            let zero = a.iter().position(|x| *x == Act::PaZero).unwrap();
            let held: u32 = a[..zero].iter().map(|x| if let Act::Wait(ms) = x { *ms as u32 } else { 0 }).sum();
            let want = if kind == Kind::Data { 0 } else { crate::path::UNKEY_HOLD_MS as u32 };
            assert_eq!(held, want, "{kind:?}");
            assert!(kind == Kind::Voice || !a.contains(&Act::TailElim), "no tail after a data frame");
            assert_eq!(a[zero + 1], Act::Wait(crate::path::UNKEY_WAIT_MS), "then the unkey wait (F4)");
            assert_eq!(a[zero + 2], Act::ChipRx(0), "then the chip out of transmit");
        }
        t.key(&r, &mut p, 0, &mut Steps::default()).unwrap();
        let mut s = Steps::default();
        t.tick(&Cfg::default(), false, &mut p, 1_000, &mut s);
        assert!(!s.as_slice().iter().take_while(|x| **x != Act::PaZero).any(|x| matches!(x, Act::Wait(_))), "a cut: no hold");
    }

    /// LINK-20.4, LINK-20.5, TX-47.3, TX-47.4: a remote key-up lost (no keepalive, the session gone) is a cut: no end
    /// tone, CW ID or carrier hold before the PA goes to 0 (the bench measured 2.72 s from the last keepalive with the
    /// hold); its PTT counts as up, so the next key-up is allowed.
    #[test]
    fn remote_loss_is_a_cut() {
        let mut t = TxSeq::new();
        let mut p = rx_on(mhz(146));
        let cfg = Cfg { end_tone: true, cw_id: true, ..Cfg::default() };
        let r = req(mhz(146));
        t.key(&r, &mut p, 0, &mut Steps::default()).unwrap();
        let mut s = Steps::default();
        t.cut(&cfg, &mut p, 5_000, &mut s);
        assert_eq!(s.as_slice()[..2], [Act::Stage, Act::PaZero], "no end tone, ID or hold");
        assert!(!t.keyed() && p.keyed().is_none());
        assert_eq!(t.key(&r, &mut p, 6_000, &mut Steps::default()), Ok(()), "not held like a timeout");
        let mut s = Steps::default();
        t.cut(&cfg, &mut p, 7_000, &mut s);
        t.cut(&cfg, &mut p, 7_100, &mut s);
        assert!(!t.keyed());
    }

    #[test]
    fn timeout_then_release_before_rekey() {
        let mut t = TxSeq::new();
        let mut p = rx_on(mhz(446));
        let cfg = Cfg { timeout_s: 30, prealert_s: 5, end_tone: true, ..Cfg::default() };
        let r = req(mhz(446));
        t.key(&r, &mut p, 0, &mut Steps::default()).unwrap();
        assert_eq!(t.tick(&cfg, true, &mut p, 20_000, &mut Steps::default()), None);
        assert_eq!(t.tick(&cfg, true, &mut p, 25_000, &mut Steps::default()), Some(Event::PreAlert));
        assert_eq!(t.tick(&cfg, true, &mut p, 26_000, &mut Steps::default()), None);
        let mut s = Steps::default();
        assert_eq!(t.tick(&cfg, true, &mut p, 30_000, &mut s), Some(Event::Timeout));
        assert_eq!(s.as_slice()[..2], [Act::Stage, Act::PaZero], "no end tone at a timeout (TONE-11), no hold (TX-47)");
        assert!(!t.keyed() && p.keyed().is_none());
        assert_eq!(t.key(&r, &mut p, 31_000, &mut Steps::default()), Err(Refusal::ReleasePtt));
        t.release(&cfg, &mut p, 32_000, &mut Steps::default());
        assert_eq!(t.key(&r, &mut p, 33_000, &mut Steps::default()), Ok(()));
    }

    /// TX-32 (TX audit F13): the battery under load cuts only after reading low for BATT_LOW_MS without a break;
    /// one low sample, or low readings broken by a good one, don't.
    #[test]
    fn battery_under_load() {
        let mut t = TxSeq::new();
        let mut p = rx_on(mhz(446));
        let cfg = Cfg::default();
        t.key(&req(mhz(446)), &mut p, 0, &mut Steps::default()).unwrap();
        assert_eq!(t.tick(&cfg, false, &mut p, 1_000, &mut Steps::default()), None, "one low sample");
        assert_eq!(t.tick(&cfg, true, &mut p, 1_020, &mut Steps::default()), None);
        assert_eq!(t.tick(&cfg, false, &mut p, 1_040, &mut Steps::default()), None, "the run starts again");
        assert_eq!(t.tick(&cfg, false, &mut p, 1_040 + BATT_LOW_MS - 1, &mut Steps::default()), None);
        assert!(t.keyed());
        assert_eq!(t.tick(&cfg, false, &mut p, 1_040 + BATT_LOW_MS, &mut Steps::default()), Some(Event::Battery));
        assert!(!t.keyed());
        // A new key-up starts with no low run.
        t.key(&req(mhz(446)), &mut p, 10_000, &mut Steps::default()).unwrap();
        assert_eq!(t.tick(&cfg, false, &mut p, 10_010, &mut Steps::default()), None);
    }

    #[test]
    fn duty_limit() {
        let mut t = TxSeq::new();
        let mut p = rx_on(mhz(446));
        let cfg = Cfg { timeout_s: 600, prealert_s: 0, ..Cfg::default() };
        let r = req(mhz(446));
        t.key(&r, &mut p, 0, &mut Steps::default()).unwrap();
        assert_eq!(t.tick(&cfg, true, &mut p, DUTY_FULL_MS - 1, &mut Steps::default()), None);
        assert_eq!(t.tick(&cfg, true, &mut p, DUTY_FULL_MS, &mut Steps::default()), Some(Event::Hot));
        assert_eq!(t.key(&r, &mut p, DUTY_FULL_MS + 1000, &mut Steps::default()), Err(Refusal::Hot));
        // Half empty: 60 s drained at a quarter rate is 240 s.
        let cool = DUTY_FULL_MS + DUTY_FULL_MS / 2 * DUTY_DRAIN_DIV;
        assert_eq!(t.key(&r, &mut p, cool, &mut Steps::default()), Ok(()));
    }

    /// TX-38: the power level weights the duty bucket: Low lasts four times as long as High from cold.
    #[test]
    fn duty_by_power() {
        let cfg = Cfg { timeout_s: 1200, prealert_s: 0, ..Cfg::default() };
        for (power, full) in [(2u8, DUTY_FULL_MS), (1, DUTY_FULL_MS * 2)] {
            let mut t = TxSeq::new();
            let mut p = rx_on(mhz(446));
            let r = Req { power, ..req(mhz(446)) };
            t.key(&r, &mut p, 0, &mut Steps::default()).unwrap();
            assert_eq!(t.tick(&cfg, true, &mut p, full - 4, &mut Steps::default()), None, "power {power}");
            assert_eq!(t.tick(&cfg, true, &mut p, full, &mut Steps::default()), Some(Event::Hot), "power {power}");
        }
        // Low's 16 min is past the longest timeout (10 min, TX-30), so it is reached over key-ups: one of 590 s
        // leaves the bucket at a quarter of High's fill, not hot.
        let mut t = TxSeq::new();
        let mut p = rx_on(mhz(446));
        t.key(&Req { power: 0, ..req(mhz(446)) }, &mut p, 0, &mut Steps::default()).unwrap();
        assert_eq!(t.tick(&cfg, true, &mut p, 590_000, &mut Steps::default()), None);
        assert_eq!(t.duty, 590_000);
        const _: () = assert!(DUTY_FULL_MS * 4 > 600_000, "Low outlasts the longest timeout");
    }

    /// TX-38 at the Radio task's keyed tick (10 ms, or any odd interval): the weights hold whatever the tick, so Low
    /// fills at exactly its quarter rate (it once filled 2 ms a 10 ms tick for 2.5), and the bucket drains
    /// at half rate over any number of short calls.
    #[test]
    fn duty_rates_hold_at_any_tick() {
        let cfg = Cfg { timeout_s: 1200, prealert_s: 0, ..Cfg::default() };
        for (power, full, tick) in [(1u8, DUTY_FULL_MS * 2, 10u32), (1, DUTY_FULL_MS * 2, 13), (2, DUTY_FULL_MS, 7)] {
            let mut t = TxSeq::new();
            let mut p = rx_on(mhz(446));
            let r = Req { power, ..req(mhz(446)) };
            t.key(&r, &mut p, 0, &mut Steps::default()).unwrap();
            let mut now = 0;
            let hot = loop {
                now += tick;
                if let Some(e) = t.tick(&cfg, true, &mut p, now, &mut Steps::default()) {
                    break (e, now);
                }
            };
            assert_eq!(hot.0, Event::Hot, "power {power}, every {tick} ms");
            assert!(hot.1 >= full && hot.1 < full + tick, "power {power}, every {tick} ms: hot at {} ms, want {full}", hot.1);
        }
        // Low at odd ticks: exactly a quarter ms a ms keyed, whatever the tick.
        for tick in [10u32, 13] {
            let mut t = TxSeq::new();
            let mut p = rx_on(mhz(446));
            t.key(&Req { power: 0, ..req(mhz(446)) }, &mut p, 0, &mut Steps::default()).unwrap();
            let mut now = 0;
            while now + tick < 590_000 {
                now += tick;
                assert_eq!(t.tick(&cfg, true, &mut p, now, &mut Steps::default()), None);
            }
            assert_eq!(t.duty, now, "Low every {tick} ms");
        }
    }

    /// XB-5, decision 163, the radio's failing order (black radio, Oct 15, 2026, cable log "xb refused 12"): an
    /// open heard on side A (145.495), the receive path hopped to side B (446.475) before the engine's detect
    /// time ran out, then the key: it keys B's chip with A's receiver selected, never RelayMismatch. The pass
    /// listens on A until its squelch ends it, and the receive path hops again after.
    #[test]
    fn crossband_keys_after_the_hop_moved_away() {
        use crate::msg::XbCfg;
        use crate::xb::{Cmd, Repeater, DETECT_MS};
        let f = [mhz(145) + 49_500, mhz(446) + 47_500];
        let mut path = rx_on(f[0]);
        let mut t = TxSeq::new();
        let mut rep = Repeater::new(XbCfg { hang_ds: 0, flags: 0, ..XbCfg::DEFAULT }, 0);
        assert_eq!(rep.tick(0, [Some(true), None]), None, "side A opens");
        path.rx(Rx::Bk(Sel::rx(f[1])), Cause::Auto, 10, &mut Steps::default()).unwrap(); // the hop to B
        assert_eq!(rep.tick(10, [None, Some(false)]), None);
        assert_eq!(rep.tick(DETECT_MS + 10, [None, Some(false)]), Some(Cmd::Key { from: 0 }), "A's open, remembered");
        assert_eq!(path.rx_now(), Rx::Bk(Sel::UhfRx), "listening to B, whose chip transmits");
        let r = Req { tx: f[1], rx: f[0], person: false, power: 0, ..req(f[1]) };
        let mut s = Steps::default();
        assert_eq!(t.key_xb(&r, &mut path, 40, &mut s), Ok(()));
        assert_eq!((path.rx_now(), path.keyed(), path.keyed_xb()), (Rx::Bk(Sel::VhfRx), Some(Sel::UhfTx), true));
        // The pass follows A's squelch (XB-7): it closes, the pass ends, and the hop to B works again.
        let mut now = 50;
        while !matches!(rep.tick(now, [Some(false), None]), Some(Cmd::Unkey { .. })) {
            assert!(now < 1_000, "the pass never ended");
            now += 10;
        }
        let cfg = Cfg { timeout_s: 0, prealert_s: 0, end_tone: false, cw_id: false, cw_each: true, tail: false };
        t.release(&cfg, &mut path, now, &mut Steps::default());
        assert!(path.keyed().is_none() && rep.from().is_none());
        assert_eq!(path.rx(Rx::Bk(Sel::rx(f[1])), Cause::Auto, now, &mut Steps::default()), Ok(()), "hopping resumes");
    }

    /// XB-6: a crossband key-up with both sides on one chip passes the checks but not the path: refused as
    /// RelayMismatch, nothing sent to a pin, nothing counted as keyed.
    #[test]
    fn crossband_on_one_chip_refused_by_the_path() {
        let f = mhz(446) + 47_500;
        let mut path = rx_on(f);
        let mut t = TxSeq::new();
        let r = Req { rx: mhz(440), person: false, power: 0, ..req(f) };
        let mut s = Steps::default();
        assert_eq!(t.key_xb(&r, &mut path, 0, &mut s), Err(Refusal::RelayMismatch));
        assert!(s.is_empty() && path.keyed().is_none());
        assert_eq!(t.check(&r, &path, 10), Ok(()), "not left keyed: the next key-up is checked afresh");
    }
}
