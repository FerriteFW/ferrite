//! The frequency counter and tone search (chapter 22).

use super::*;

impl Radio {
    /// The frequency counter on or off (TSRCH-10): an exclusive mode like HF mode; watch and scans pause. Off, the
    /// both-selects hold goes if the counter set it (TSRCH-10.3).
    pub(super) async fn counter(&mut self, on: bool) {
        for n in 0..2u8 {
            if self.has(n) {
                self.chip(n).freq_scan(false);
            }
        }
        if let Some(c) = self.counter.take() {
            if c.held {
                let mut s = Steps::default();
                let _ = self.path.set_both(false, &mut s);
                self.run(&s).await;
            }
        }
        self.tuned = [None; 2]; // the counter, or the engines afterwards, tune them again
        let first = (0..2u8).find(|&n| self.has(n)).unwrap_or(0);
        let both = self.has(0) && self.has(1);
        self.counter = on.then_some(Counter { both, held: false, chip: first, since: 0, last: None, pair: Default::default() });
        self.live.mode = if on { ModeLive::Probe { freq: 0, rssi: -127, tone: 0 } } else { ModeLive::Off };
        speaker(false);
        log(b"counter ", on as u32);
    }

    /// The counter's tick. With both chips: both measure at once, chip 0 on VHF and chip 1 on UHF, with both band
    /// receive selects on (TSRCH-10.3, SAFE-24: never the HF select or the relay; a key-up clears both first), so a
    /// strong carrier's junk in one chip can't keep the other from its turn. Each chip's path is asked for with
    /// `Cause::Auto`: the relay never moves (SAFE-20). If the hold is refused, or with one chip, they take turns.
    pub(super) async fn counter_tick(&mut self, now: u32) {
        let Some(mut c) = self.counter else { return };
        if c.both {
            if self.tuned[0].is_none() || self.tuned[1].is_none() {
                self.tune_chip(0, Some((COUNTER_SLOT, COUNTER_TUNE[0]))).await;
                self.tune_chip(1, Some((COUNTER_SLOT, COUNTER_TUNE[1]))).await;
                self.select_rx(COUNTER_TUNE[0], Cause::Auto).await;
                let was = self.path.both();
                let mut s = Steps::default();
                let ok = self.path.rx_now() == Rx::Bk(Sel::rx(COUNTER_TUNE[0])) && self.path.set_both(true, &mut s).is_ok();
                self.run(&s).await;
                if ok {
                    c.held |= !was;
                    for n in 0..2u8 {
                        self.chip(n).freq_scan(true);
                    }
                    log(b"counter: both chips ", 1);
                } else {
                    // Refused (the relay energised by a slot, say): one chip at a time, as before.
                    c.both = false;
                    self.tuned = [None; 2];
                    log(b"counter: both chips ", 0);
                }
                self.counter = Some(c);
                return;
            }
            for n in 0..2u8 {
                let mut bk = self.chip(n);
                if let Some(hz) = bk.freq_scan_result() {
                    bk.freq_scan(false);
                    let got = hz / 10;
                    let rssi = bk.rssi_dbm();
                    let kept = c.pair.reading(n, got, rssi, now);
                    // The cable log says what each chip read and whether it counted (a report's first question).
                    log(if kept.is_some() { if n == 0 { b"counter c0 kept khz " } else { b"counter c1 kept khz " } } else if n == 0 { b"counter c0 drop khz " } else { b"counter c1 drop khz " }, got / 100);
                    self.chip(n).freq_scan(true);
                }
            }
            self.live.mode = match c.pair.shown(now) {
                Some((freq, rssi)) => ModeLive::Probe { freq, rssi, tone: 0 },
                None => ModeLive::Probe { freq: 0, rssi: -127, tone: 0 },
            };
            self.counter = Some(c);
            return;
        }
        let n = c.chip;
        let f = COUNTER_TUNE[n as usize];
        if self.tuned[n as usize].is_none() {
            self.tune_chip(1 - n, None).await;
            self.tune_chip(n, Some((COUNTER_SLOT, f))).await;
            self.select_rx(f, Cause::Auto).await;
            if self.path.rx_now() == Rx::Bk(Sel::rx(f)) {
                self.chip(n).freq_scan(true);
            }
            (c.since, c.last) = (now, None);
        } else {
            let mut bk = self.chip(n);
            if let Some(hz) = bk.freq_scan_result() {
                bk.freq_scan(false);
                let got = hz / 10;
                let rssi = bk.rssi_dbm();
                if tonegen::counter_chip(got) == Some(n) {
                    if let Some(freq) = c.last.and_then(|l| tonegen::counter_agree(l, got)) {
                        self.live.mode = ModeLive::Probe { freq, rssi, tone: 0 };
                        log(b"counter khz ", freq / 100);
                        c.since = now; // TSRCH-10.3: only a kept reading holds the turn
                    }
                    c.last = Some(got);
                }
                self.chip(n).freq_scan(true);
            } else if now.wrapping_sub(c.since) > COUNTER_TURN_MS {
                bk.freq_scan(false);
                self.live.mode = ModeLive::Probe { freq: 0, rssi: -127, tone: 0 };
                let other = 1 - n;
                if self.has(other) {
                    self.tune_chip(n, None).await;
                    c.chip = other;
                }
                self.tuned[c.chip as usize] = None; // starts its scan on the next tick
            }
        }
        self.counter = Some(c);
    }

    /// Tone search on or off (TSRCH-1, TSRCH-3): an exclusive mode like the counter; watch and scans pause. Off,
    /// the engines tune the chips again, so the slot's own tone detector is back (`tune_finish`).
    pub(super) fn tone_search(&mut self, slot: u8, on: bool) {
        self.tuned = [None; 2]; // tuned below, or by the engines afterwards
        let slot = slot.min(SLOTS as u8 - 1);
        self.tsearch = on.then_some((slot, ferrite_core::tonesearch::Search::new()));
        let f = self.slots[slot as usize].rx_freq();
        self.live.mode = if on { ModeLive::Probe { freq: f, rssi: -127, tone: 0 } } else { ModeLive::Off };
        speaker(false);
        log(b"tone search ", on as u32);
    }

    /// Tone search's tick: the slot's own chip on its frequency (its band's, as the watch picks), the other asleep,
    /// the sub-tone detectors in scan (`tone_probe`, the tone hint's, TSRCH-12); each tick's reading goes to
    /// `tonesearch::Search`, which keeps one when the next agrees with the carrier there. The audio stays muted.
    /// The slot's card follows its squelch and signal (TSRCH-2).
    pub(super) async fn tsearch_tick(&mut self) {
        let Some((s, mut srch)) = self.tsearch else { return };
        let a = self.save.tick(now_ms(), true); // awake throughout, as the counter
        self.apply(a).await;
        let f = self.slots[s as usize].rx_freq();
        let n = ferrite_core::freq::band(f).1;
        if !self.has(n) {
            return;
        }
        if self.tuned[n as usize] != Some((s, f)) {
            self.tune_chip(1 - n, None).await;
            self.tune_chip(n, Some((s, f))).await;
            self.select_rx(f, Cause::ActiveSlot).await;
            self.chip(n).af_out(AfOut::Mute);
            radio_chip::tone_probe(&mut self.chip(n));
            return;
        }
        let mut bk = self.chip(n);
        let (rssi, st) = (bk.rssi_dbm(), bk.status());
        let (c, d) = bk.subtone_scan();
        let c = srch.average(st.squelch_open, c); // TSRCH-3.1: CTCSS snapped from the mean of four reads
        let word = radio_chip::heard_tone(c, d, &ferrite_app::slot::CTCSS, &ferrite_app::slot::DCS);
        if srch.step(st.squelch_open, word) {
            log(b"tone found ", srch.found as u32);
        }
        let sl = &mut self.live.slots[s as usize];
        (sl.rssi, sl.squelch_open) = (rssi, st.squelch_open); // the level, open or closed
        self.live.mode = ModeLive::Probe { freq: f, rssi, tone: srch.found };
        self.tsearch = Some((s, srch));
    }
}
