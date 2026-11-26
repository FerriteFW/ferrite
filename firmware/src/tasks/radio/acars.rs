//! ACARS (manual chapter 27b, ACARS-11): while the ACARS screen is open the VHF chip receives the chosen frequency in
//! narrow AM and its audio goes to the decoder (EARO, PA2: the DSP task's ACARS receiver, `ferrite_core::acarsrx`);
//! the speaker stays off unless the monitor is latched; the UHF chip sleeps; watch, scans, Battery Save's sleeps
//! and the other decoders wait, as in satellite mode. The relay never moves (`Cause::Auto`: airband is on the VHF
//! path either way).

use super::*;

/// ACARS's own index in [`Radio::tuned`] (after satellite mode's).
pub(super) const ACARS_TUNED: u8 = 7;

impl Radio {
    /// ACARS-11: on (or retuned) at `f`, or off with 0: everything back as before (the next tick retunes the slots
    /// and the active slot's receive path).
    pub(super) async fn acars_rx(&mut self, f: Freq10) {
        if f == 0 {
            if self.acars.take().is_some() {
                self.tuned = [None; 2];
                self.live.mode = ModeLive::Off;
                speaker(false);
                let a = self.slots[self.active as usize].freq;
                self.select_rx(a, Cause::Auto).await;
                log(b"acars off", 0);
            }
            return;
        }
        if self.acars.replace(f).is_none() {
            // Battery Save's receivers awake (the tick steps it as busy); watch and scans don't run
            let a = self.save.activity(now_ms());
            self.apply(a).await;
            self.tuned = [None; 2];
            speaker(false);
            log(b"acars on, khz ", f / 100);
        }
        if self.path.keyed().is_none() {
            self.acars_tune(f).await;
        }
    }

    /// The VHF chip on `f`, the other asleep; the receive path to it.
    async fn acars_tune(&mut self, f: Freq10) {
        let n = Sel::rx(f).chip();
        self.tune_chip(n, Some((ACARS_TUNED, f))).await;
        self.tune_chip(n ^ 1, None).await;
        self.select_rx(f, Cause::Auto).await;
    }

    /// The tick while ACARS listens (instead of watch and scans): keyed (PTT on another slot's frequency is still a
    /// key-up), the sequencer's checks; else the chip kept tuned, its audio always to the decoder (a burst's start
    /// must not wait for the squelch), the speaker only with the monitor latched.
    pub(super) async fn acars_tick(&mut self) {
        let Some(f) = self.acars else { return };
        let a = self.save.tick(now_ms(), true);
        self.apply(a).await;
        #[cfg(feature = "tx")]
        if self.path.keyed().is_some() {
            return self.tx_tick(now_ms() as u32).await;
        }
        self.acars_tune(f).await; // only what changed (back after an unkey, which cleared `tuned`)
        let n = Sel::rx(f).chip();
        if !self.has(n) || self.tuned[n as usize] != Some((ACARS_TUNED, f)) {
            self.live.mode = ModeLive::Acars { freq: 0, rssi: -127, open: false };
            return;
        }
        let mut bk = self.chip(n);
        let (rssi, st) = (bk.rssi_dbm(), bk.status());
        bk.af_out(AfOut::Normal);
        let monitored = self.monitor & !(self.user_mute | self.user_off) & 1 << self.active != 0;
        speaker(monitored && !self.muted);
        self.live.mode = ModeLive::Acars { freq: f, rssi, open: st.squelch_open };
    }
}
