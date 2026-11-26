//! TERM-42 `bk rxaf` and TERM-43 `fsk` (decision 305): bench tests of the BK4829's receive filters and its own FSK
//! modem, a bench build's only (TERM-38). Nothing here runs unless the terminal asked: no override and no run at
//! boot, and RAM only.
//!
//! - `bk rxaf`: REG_2B bits 10:8 written after every FM tune (`tune_finish`), so the override outlives watch hops,
//!   Battery Save's wakes and every retune; a key-up's `flat`/`data` restore puts REG_2B back as it found it and the
//!   unkey's retune writes the override again.
//! - `fsk rx`: the modem set up on the chip that has the active slot, again whenever that chip retunes or wakes
//!   (as SAME's, which stands aside meanwhile, as does DTMF on that chip: the interrupt flags are the test's). Each
//!   tick reads the flags: a sync starts a frame, FIFO almost full gives 4 words, done the rest, the chip's CRC
//!   verdict, and receive on again.
//! - `fsk tx`: armed for the next PTT key-up (a data key-up, as `txtest`), only with Dry TX on; the keyed chip goes
//!   to tone transmit and sends the frames back to back, the next loaded at each transmit-done flag (the keyed
//!   tick), then holds the silent carrier until the release.

use super::*;
use ferrite_core::fskbench::{Ask, Cfg, Report, RxFrame, Stats};

/// A run of the FSK modem test.
pub(super) struct FskRun {
    cfg: Cfg,
    /// `fsk tx`: the frames to send (None: receiving), and whether a key-up has had them yet.
    tx: Option<u16>,
    spent: bool,
    /// The chip receiving (and the slot and frequency it was set up for), or sending while keyed.
    chip: Option<u8>,
    at: Option<(u8, Freq10)>,
    frame: RxFrame,
    stats: Stats,
}

impl FskRun {
    fn new(cfg: Cfg, tx: Option<u16>) -> Self {
        FskRun { cfg, tx, spent: false, chip: None, at: None, frame: RxFrame::default(), stats: Stats::default() }
    }

    /// `fsk tx`'s frames still wait for a key-up.
    pub(super) fn tx_armed(&self) -> bool {
        self.tx.is_some() && !self.spent
    }
}

impl Radio {
    /// TERM-42: the override set, ended or asked. A change retunes the slots' chips (not while keyed: the unkey's
    /// retune comes anyway), so it takes effect now.
    pub(super) fn rxaf_ask(&mut self, set: Option<Option<u16>>) -> DebugAns {
        if let Some(bits) = set {
            self.rxaf = bits.map(|b| b & 0xF700);
            if self.path.keyed().is_none() {
                for s in self.tuned.map(|t| t.map(|(s, _)| s)).into_iter().flatten().filter(|&s| (s as usize) < SLOTS) {
                    self.retune(s);
                }
            }
        }
        DebugAns::RxAf(self.rxaf)
    }

    /// TERM-42.2: the IF override set, ended or asked; a change retunes as `rxaf_ask`'s does.
    pub(super) fn rxif_ask(&mut self, set: Option<Option<u16>>) -> DebugAns {
        if let Some(v) = set {
            self.rxif = v;
            if self.path.keyed().is_none() {
                for s in self.tuned.map(|t| t.map(|(s, _)| s)).into_iter().flatten().filter(|&s| (s as usize) < SLOTS) {
                    self.retune(s);
                }
            }
        }
        DebugAns::RxIf(self.rxif)
    }

    /// TERM-42.3: a held register added or replaced, all ended, or asked; a change retunes as `rxaf_ask`'s does.
    pub(super) fn hold_ask(&mut self, set: Option<Option<(u8, u16)>>) -> DebugAns {
        if let Some(h) = set {
            match h {
                None => self.holds = [(0, 0); ferrite_core::msg::HOLDS],
                Some((r, v)) => {
                    let at = self.holds.iter().position(|x| x.0 == r).or_else(|| self.holds.iter().position(|x| x.0 == 0));
                    match at {
                        Some(i) => self.holds[i] = (r, v),
                        None => return DebugAns::Refused("four registers held already: bk hold off first"),
                    }
                }
            }
            if self.path.keyed().is_none() {
                for s in self.tuned.map(|t| t.map(|(s, _)| s)).into_iter().flatten().filter(|&s| (s as usize) < SLOTS) {
                    self.retune(s);
                }
            }
        }
        DebugAns::Hold(self.holds)
    }

    /// TERM-43: `fsk` asked, started or ended.
    pub(super) fn fsk_ask(&mut self, a: Ask) -> DebugAns {
        match a {
            Ask::Show => {}
            Ask::Off => self.fsk_stop(),
            Ask::Rx(cfg) => {
                if self.path.keyed().is_some() {
                    return DebugAns::Refused("transmitting: try again after the unkey");
                }
                self.fsk_stop();
                self.fsk = Some(FskRun::new(cfg, None));
            }
            #[cfg(not(feature = "tx"))]
            Ask::Tx(..) => return DebugAns::Refused("the receive-only build has no transmit: fsk tx needs the tx-bench build"),
            #[cfg(feature = "tx")]
            Ask::Tx(cfg, n) => {
                if !crate::chan::DRY_TX.load(core::sync::atomic::Ordering::SeqCst) {
                    return DebugAns::Refused("Dry TX is off: fsk tx keys only with Dry TX on (0 8 8 5)");
                }
                if self.path.keyed().is_some() {
                    return DebugAns::Refused("transmitting: try again after the unkey");
                }
                self.fsk_stop();
                self.tx.test = None; // one test signal per key-up: the last armed
                self.fsk = Some(FskRun::new(cfg, Some(n)));
            }
        }
        DebugAns::Fsk(self.fsk_report())
    }

    fn fsk_report(&self) -> Report {
        match &self.fsk {
            None => Report { cfg: None, tx: None, armed: false, chip: None, stats: Stats::default() },
            Some(r) => Report { cfg: Some(r.cfg), tx: r.tx, armed: r.tx_armed(), chip: r.chip, stats: r.stats },
        }
    }

    /// The run ended: a receiving chip's modem off and its slot retuned as normal (a sending one stops sending,
    /// keyed until the release); SAME and the pager set their modems up again where they want them.
    fn fsk_stop(&mut self) {
        let Some(run) = self.fsk.take() else { return };
        if let Some(n) = run.chip.filter(|&n| self.has(n)) {
            if run.tx.is_none() {
                self.chip(n).fsk_off();
                if let Some((s, _)) = run.at {
                    self.retune(s);
                }
            } else {
                #[cfg(feature = "tx")]
                if self.path.keyed().is_some() {
                    radio_chip::fsk_bench_tx_end(&mut self.chip(n), &run.cfg);
                }
            }
        }
        (self.same.chip, self.same.at, self.same.armed) = (None, None, false);
        self.pager_after_fsk();
    }

    /// TERM-43: the chip the receive test listens with (its interrupt flags are the test's), if any.
    pub(super) fn fsk_chip(&self) -> Option<u8> {
        self.fsk.as_ref().filter(|r| r.tx.is_none()).and_then(|r| r.chip)
    }

    /// TERM-43 `fsk rx`, each tick in SAME's place: set up on the active slot's chip after any retune, then its flags.
    pub(super) fn fsk_tick(&mut self) {
        let Some(mut run) = self.fsk.take() else { return };
        if run.tx.is_none() {
            self.fsk_rx_step(&mut run);
        }
        self.fsk = Some(run);
    }

    fn fsk_rx_step(&mut self, run: &mut FskRun) {
        let active = self.active;
        let chip = (0..2u8).find(|&n| self.has(n) && self.tuned[n as usize].is_some_and(|(s, f)| s == active && f != 0));
        let at = chip.and_then(|n| self.tuned[n as usize]);
        if chip != run.chip || at != run.at {
            if let Some(old) = run.chip.filter(|&o| Some(o) != chip && self.tuned[o as usize].is_some()) {
                self.chip(old).fsk_off();
            }
            (run.chip, run.at, run.frame) = (chip, at, RxFrame::default());
            if let Some(n) = chip {
                radio_chip::fsk_bench_rx(&mut self.chip(n), &run.cfg);
            }
            return;
        }
        let Some(n) = chip else { return };
        let mut bk = self.chip(n);
        let flags = bk.interrupts();
        if flags & int::FSK_SYNC != 0 {
            let neg = bk.read(bk4829::reg::DTMF_CODE) & 0x80 != 0; // REG_0B bit 7: the sync found inverted
            run.stats.sync(neg, &mut run.frame);
        }
        // Almost full is 4 words (REG_5E's default); done leaves the rest, at most the FIFO's 8.
        if flags & int::FSK_FIFO_FULL != 0 {
            for _ in 0..run.frame.left().min(4) {
                run.frame.push(bk.fsk_read());
            }
        }
        if flags & int::FSK_RX_DONE != 0 {
            for _ in 0..run.frame.left().min(8) {
                run.frame.push(bk.fsk_read());
            }
            let crc = run.cfg.crc.then(|| bk.fsk_crc_ok());
            run.stats.done(&mut run.frame, crc);
            radio_chip::fsk_bench_rearm(&mut bk, &run.cfg);
        }
    }

    /// TERM-43: whether this PTT key-up sends the armed frames. Armed with Dry TX since off: dropped, logged, and
    /// the key-up is an ordinary one.
    #[cfg(feature = "tx")]
    pub(super) fn fsk_tx_armed(&mut self) -> bool {
        let Some(run) = self.fsk.as_mut().filter(|r| r.tx_armed()) else { return false };
        if !crate::chan::DRY_TX.load(core::sync::atomic::Ordering::SeqCst) {
            run.spent = true;
            log(b"fsk tx dropped: Dry TX is off", 0);
            return false;
        }
        true
    }

    /// TERM-43: keyed on chip `n` with the frames armed: tone transmit, the modem set up, frame 0 sent.
    #[cfg(feature = "tx")]
    pub(super) fn fsk_key_up(&mut self, n: u8) {
        let narrow = self.tx.narrow;
        let Some(run) = self.fsk.as_mut() else { return };
        (run.spent, run.chip, run.stats.sent) = (true, Some(n), 0);
        let cfg = run.cfg;
        radio_chip::fsk_bench_tx_begin(&mut self.chip(n), &cfg, narrow, 0);
        log(b"fsk tx frames ", self.fsk.as_ref().and_then(|r| r.tx).unwrap_or(0) as u32);
    }

    /// TERM-43, the keyed tick: at each transmit-done flag, the next frame, or the end of the frames.
    #[cfg(feature = "tx")]
    pub(super) fn fsk_tx_poll(&mut self) {
        let Some(run) = self.fsk.as_ref() else { return };
        let (Some(total), Some(n), cfg, sent) = (run.tx, run.chip, run.cfg, run.stats.sent) else { return };
        if sent >= total || !self.has(n) || self.chip(n).interrupts() & int::FSK_TX_DONE == 0 {
            return;
        }
        let sent = sent + 1;
        if sent < total {
            radio_chip::fsk_bench_tx_frame(&mut self.chip(n), &cfg, sent);
        } else {
            radio_chip::fsk_bench_tx_end(&mut self.chip(n), &cfg);
            log(b"fsk tx sent ", sent as u32);
        }
        if let Some(r) = self.fsk.as_mut() {
            r.stats.sent = sent;
        }
    }

    /// TERM-43: the key-up ends (release, cut, any unkey): a frame still going out is stopped and the modem off.
    #[cfg(feature = "tx")]
    pub(super) fn fsk_tx_stop(&mut self) {
        let Some(run) = self.fsk.as_mut().filter(|r| r.tx.is_some()) else { return };
        let (Some(n), cfg, done) = (run.chip.take(), run.cfg, run.stats.sent >= run.tx.unwrap_or(0)) else { return };
        if !done && self.has(n) {
            radio_chip::fsk_bench_tx_end(&mut self.chip(n), &cfg);
            log(b"fsk tx cut at frame ", self.fsk.as_ref().map_or(0, |r| r.stats.sent) as u32);
        }
    }
}
