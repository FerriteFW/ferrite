//! Satellite mode (manual chapter 28a): both chips while the satellite screen is open (SAT-1, SAT-14), the
//! downlink retuned as the UI sends it (once a second, Doppler-corrected, SAT-8), and in the `tx` build the uplink
//! keyed on the other chip in full duplex (the downlink left receiving, `Path::key_up_fd`) or on a plain key-up in
//! half duplex (SAT-11), with the access tone (the arming tone first, as the UI sends it, SAT-10). The relay never
//! moves: both ends are on 2 m and 70 cm (SAT-13, refused by the UI and here by `Cause::Auto`).

use super::*;

/// Satellite mode's own index in [`Radio::tuned`] (after the scope's).
pub(super) const SAT_TUNED: u8 = 6;
/// The UI's transmit slot for the uplink (`ferrite_app::sat::SAT_SLOT`).
#[cfg(feature = "tx")]
pub(super) const SAT_SLOT: u8 = 4;

/// Satellite mode running: the downlink, the uplink for the next key-up (and keyed, the one it is on), the
/// access or arming tone, half duplex.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct SatRun {
    pub down: Freq10,
    pub up: Freq10,
    pub tone: u16,
    pub hd: bool,
    /// The uplink's power (TX-9 levels: the satellite's choice, Low to start; SAT-22).
    pub power: u8,
}

impl Radio {
    /// SAT-1: on (or retuned) on the downlink, or off with 0: everything back as before (the next tick retunes the
    /// slots and the active slot's receive path).
    pub(super) async fn sat_rx(&mut self, f: Freq10) {
        if f == 0 {
            #[cfg(feature = "tx")]
            if self.path.keyed().is_some() && self.tx.slot == SAT_SLOT {
                self.sat_unkey().await;
            }
            if self.sat.take().is_some() {
                self.tuned = [None; 2];
                self.live.mode = ModeLive::Off;
                speaker(false);
                let a = self.slots[self.active as usize].freq;
                self.select_rx(a, Cause::Auto).await;
                log(b"sat off", 0);
            }
            return;
        }
        let was = self.sat.replace(SatRun { down: f, ..self.sat.unwrap_or(SatRun { down: 0, up: 0, tone: 0, hd: false, power: 0 }) });
        if was.is_none() {
            // SAT-14: Battery Save's receivers awake (the tick steps it as busy); watch and scans don't run.
            let a = self.save.activity(now_ms());
            self.apply(a).await;
            self.tuned = [None; 2];
            speaker(false);
            log(b"sat on, khz ", f / 100);
        }
        if self.path.keyed().is_none() || self.tx_chip() != Some(Sel::rx(f).chip()) {
            self.sat_tune().await;
        }
    }

    /// The chip that transmits now, if any.
    fn tx_chip(&self) -> Option<u8> {
        #[cfg(feature = "tx")]
        return self.tx.chip;
        #[cfg(not(feature = "tx"))]
        None
    }

    /// The downlink's chip on it, the other chip asleep unless keyed; the receive path to it.
    async fn sat_tune(&mut self) {
        let Some(r) = self.sat.filter(|r| r.down != ferrite_core::msg::SAT_IDLE) else {
            // the pass list: both chips taken, asleep
            for n in 0..2u8 {
                if self.tx_chip() != Some(n) {
                    self.tune_chip(n, None).await;
                }
            }
            return;
        };
        let n = Sel::rx(r.down).chip();
        if self.tx_chip() != Some(n) {
            self.tune_chip(n, Some((SAT_TUNED, r.down))).await;
        }
        if self.path.keyed().is_none() {
            if self.tx_chip() != Some(n ^ 1) {
                self.tune_chip(n ^ 1, None).await;
            }
            self.select_rx(r.down, Cause::Auto).await;
        }
    }

    /// The tick in satellite mode (instead of watch and scans): keyed, the sequencer's checks; the downlink's level
    /// and squelch to the UI, its audio on the speaker (in full duplex while keyed too, SAT-11).
    pub(super) async fn sat_tick(&mut self) {
        let Some(r) = self.sat else { return };
        let a = self.save.tick(now_ms(), true);
        self.apply(a).await;
        let now = now_ms() as u32;
        let keyed = self.path.keyed().is_some();
        #[cfg(feature = "tx")]
        if keyed {
            self.tx_tick(now).await;
        }
        let _ = now;
        if !keyed {
            self.sat_tune().await; // back after an unkey (the unkey cleared `tuned`)
        }
        let n = Sel::rx(r.down).chip();
        if r.down == ferrite_core::msg::SAT_IDLE || !self.has(n) || self.tx_chip() == Some(n) || self.tuned[n as usize] != Some((SAT_TUNED, r.down)) {
            self.live.mode = ModeLive::Sat { rssi: -127, open: false };
            return;
        }
        let mut bk = self.chip(n);
        let (rssi, st) = (bk.rssi_dbm(), bk.status());
        // SQL-9.4: a monitor on the active slot (SIDE1 on the pass page) opens the downlink whatever it hears.
        let monitored = self.monitor & !(self.user_mute | self.user_off) & 1 << self.active != 0;
        let open = st.squelch_open || monitored;
        let hear = open && !(keyed && r.hd) && !self.muted;
        self.chip(n).af_out(if hear { AfOut::Normal } else { AfOut::Mute });
        if keyed {
            AMP_EN[0].write(true);
            AMP_EN[1].write(hear);
        } else {
            speaker(hear);
        }
        self.live.mode = ModeLive::Sat { rssi, open };
    }

    /// SAT-8: the uplink for the next key-up; keyed, the transmitting chip retuned there (to confirm on the bench:
    /// a retune while keyed must not click or unlock, SAT-8's open point). SAT-8.1: a retune while keyed passes the
    /// key-up's frequency checks first, or the transmission ends there (a review of Nov 20, 2026: the uplink was
    /// retuned keyed with no band-plan, emergency or power-table check).
    pub(super) async fn sat_up(&mut self, f: Freq10) {
        let Some(r) = self.sat.as_mut() else { return };
        r.up = f;
        #[cfg(feature = "tx")]
        if let (Some(n), true) = (self.tx.chip, self.tx.slot == SAT_SLOT && self.path.keyed().is_some()) {
            if self.tuned[n as usize].is_some_and(|(_, was)| was != f) {
                if let Err(why) = self.retune_ok(f).and(if Sel::tx(f).chip() == n { Ok(()) } else { Err(Refusal::OutOfBand) }) {
                    log(b"sat: uplink retune refused ", why as u32);
                    self.transmit_cut().await;
                    return self.refuse(SAT_SLOT, why);
                }
                let w = self.tx.cal.map_or(f, |c| c.tx_word(f));
                self.chip(n).set_freq(w * 10);
                self.tuned[n as usize] = Some((SAT_SLOT, f));
            }
        }
    }

    /// SAT-8.1: may the keyed uplink move to `f`: the key-up's frequency checks (`TxCal::keyed_retune`) with the
    /// region and Extend TX of the last plan and the key-up's power level.
    #[cfg(feature = "tx")]
    fn retune_ok(&self, f: Freq10) -> Result<(), Refusal> {
        let (region, extend) = self.tx.region.ok_or(Refusal::Locked)?;
        let power = self.tx.data.unwrap_or(self.tx.power[self.tx.slot as usize % 4]).min(2);
        self.tx.cal.ok_or(Refusal::NotCalibrated)?.keyed_retune(f, region, extend, power)
    }

    /// SAT-10, SAT-11: the uplink's tone (keyed: on the transmitting chip now, the arming tone giving way to the
    /// access tone), half duplex and the power for the next key-up.
    pub(super) fn sat_tone(&mut self, tone: u16, hd: bool, power: u8) {
        let Some(r) = self.sat.as_mut() else { return };
        let changed = r.tone != tone;
        (r.tone, r.hd, r.power) = (tone, hd, power.min(2));
        #[cfg(feature = "tx")]
        if let (Some(n), true) = (self.tx.chip, changed && self.tx.slot == SAT_SLOT && self.path.keyed().is_some()) {
            self.chip(n).ctcss_tx(tone);
        }
        let _ = changed;
    }
}

#[cfg(feature = "tx")]
impl Radio {
    /// PTT on the pass page: the uplink keyed (SAT-11): full duplex on the other chip with the downlink left
    /// receiving, or half duplex (asked for, or both ends on one chip: a V/V transponder, an APRS digipeater) as
    /// a plain key-up; every normal check (region, calibration, battery, duty, PTT since boot: SAT-13).
    pub(super) async fn sat_key(&mut self) {
        let now = now_ms() as u32;
        if let Some(c) = crate::chan::TX_CAL.try_take() {
            (self.tx.cal, self.tx.gauge) = c;
        }
        let Some(r) = self.sat else { return self.refuse(SAT_SLOT, Refusal::Locked) };
        let Some((f, region, extend, _)) = self.tx.plan.take() else {
            return self.refuse(SAT_SLOT, Refusal::Locked);
        };
        if !self.ptt_seen_released() {
            return self.refuse(SAT_SLOT, Refusal::PttAtBoot);
        }
        let n = Sel::tx(f).chip();
        let fd = !r.hd && n != Sel::rx(r.down).chip();
        let req = Req {
            tx: f,
            rx: r.down,
            kind: TxKind::Voice,
            person: true,
            confirmed: false,
            am: false,
            rx_only: false,
            busy: false,
            hf_si: false,
            si_on: self.si.mode().is_some(),
            region,
            extend,
            cal_ok: self.tx.cal.is_some(),
            cal_window: self.tx.cal.is_some_and(|c| c.in_window(f)),
            batt_ok: self.battery_ok(),
            power: r.power,
        };
        if let Err(why) = self.tx.seq.check(&req, &self.path, now) {
            return self.refuse(SAT_SLOT, why);
        }
        // CAL-6.1: a band whose curves failed a check is locked; outside every row, not calibrated
        let pa = match self.tx.cal.map(|c| c.pa(f, r.power)) {
            Some(Ok(pa)) => pa,
            Some(Err(why)) => return self.refuse(SAT_SLOT, why),
            None => return self.refuse(SAT_SLOT, Refusal::NotCalibrated),
        };
        if !self.has(n) {
            return self.refuse(SAT_SLOT, Refusal::Locked);
        }
        crate::chan::RF_STATE.fetch_or(crate::chan::RF_BUSY_KEYED, core::sync::atomic::Ordering::Relaxed);
        crate::tasks::gps::pb10_low_now();
        if !fd {
            speaker(false);
        }
        self.ldo_on(n);
        self.tuned[n as usize] = Some((SAT_SLOT, f));
        self.chip(n).dtmf_rx(false);
        self.dtmf_key[n as usize] = None;
        let setup = self.tx_chip_setup(f, false, r.tone, false);
        radio_chip::tx_setup(&mut self.chip(n), &setup);
        self.tx.pa = pa;
        self.tx.fault = false;
        (self.tx.data, self.tx.slot, self.tx.dialled) = (Some(r.power), SAT_SLOT, false);
        self.live.tx = TxLive { keyed: false, slot: SAT_SLOT, mic: 0, refused: None, refusals: self.live.tx.refusals, dtmf_sent: self.live.tx.dtmf_sent, voice: self.live.tx.voice, ptt_stuck: self.live.tx.ptt_stuck };
        let mut st = Steps::default();
        let k = if fd { self.tx.seq.key_fd(&req, &mut self.path, now, &mut st) } else { self.tx.seq.key(&req, &mut self.path, now, &mut st) };
        if let Err(why) = k {
            self.unkeyed();
            return self.refuse(SAT_SLOT, why);
        }
        self.run(&st).await;
        if self.tx.fault {
            let mut st = Steps::default();
            self.tx.seq.safe(&mut self.path, now_ms() as u32, &mut st);
            self.run(&st).await;
            self.unkeyed();
            return self.refuse(SAT_SLOT, Refusal::Locked);
        }
        self.tx.chip = Some(n);
        self.live.tx.keyed = true;
        log(if fd { b"sat key fd, khz " } else { b"sat key hd, khz " }, f / 100);
    }

    /// The uplink unkeyed: no end tone or tail (a satellite repeats them to everyone), CW ID as set; the downlink
    /// back on the next tick.
    pub(super) async fn sat_unkey(&mut self) {
        let now = now_ms() as u32;
        let mut cfg = self.tx_cfg();
        (cfg.end_tone, cfg.tail) = (false, false);
        let mut st = Steps::default();
        self.tx.seq.release(&cfg, &mut self.path, now, &mut st);
        self.run(&st).await;
        self.tx.data = None;
        self.unkeyed();
        log(b"sat unkey", 0);
    }
}
