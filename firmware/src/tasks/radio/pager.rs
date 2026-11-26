//! POCSAG through the BK4829's own FSK modem (DEC-23, decision 453): on the chip that has slot C in Pager mode.

use super::*;
use ferrite_core::pagerrx::PagerRx;

/// The pager receiver: the speed asked (0: off), the chip and what it was set up for, the frame being read.
pub(super) struct PagerState {
    pub(super) baud: u16,
    chip: Option<u8>,
    at: Option<(u8, Freq10)>,
    armed: bool,
    /// Words of this frame still to read, and when the last came (ms).
    left: u16,
    last: u32,
    /// When the frame's sync was seen (ms) and the words read since: the words are read as they fall due (16 bits
    /// each at the speed), not on the FIFO's almost-full flag, which the modem doesn't always raise (bench, Nov 24,
    /// 2026).
    at_sync: u32,
    read: u16,
    rx: PagerRx,
}

impl PagerState {
    pub(super) const fn new() -> Self {
        PagerState { baud: 0, chip: None, at: None, armed: false, left: 0, last: 0, at_sync: 0, read: 0, rx: PagerRx::new(1200) }
    }
}

/// No sync for this long after a frame: the transmission ended (a batch is 453 ms at 1200 baud).
const GAP_MS: u32 = 1200;

impl Radio {
    /// DEC-23: each tick, the modem set up on slot C's chip after any retune or wake, then its flags: a sync starts
    /// a frame (POCSAG's batches, each behind its own sync, read on in one frame), its words read as they fall due,
    /// and the modem set up again where the transmission ended. Pages go to the UI task as the DSP task's did
    /// ([`crate::tasks::dsp::PAGES`]).
    pub(super) fn pager_tick(&mut self, now: u32) {
        #[cfg(feature = "bench")]
        if self.fsk.is_some() {
            return; // TERM-43: the bench's modem test has the modem meanwhile
        }
        let baud = self.pager.baud;
        // C, held as the APRS slot is (AprsSlot: Pager mode holds C the same way, DEC-20)
        let (tuned, c) = (self.tuned, self.aprs_slots);
        let chip = if baud != 0 { (0..2u8).find(|&n| tuned[n as usize].is_some_and(|(s, f)| c & 1 << s != 0 && f != 0)) } else { None };
        let at = chip.and_then(|n| tuned[n as usize]);
        if chip != self.pager.chip || at != self.pager.at || baud != self.pager.rx.baud() && chip.is_some() {
            if let Some(old) = self.pager.chip.filter(|&o| self.tuned[o as usize].is_some()) {
                self.chip(old).fsk_off();
            }
            self.pager_end();
            (self.pager.chip, self.pager.at, self.pager.armed, self.pager.left) = (chip, at, false, 0);
            self.pager.rx = PagerRx::new(baud.max(1200));
        }
        let Some(n) = chip else { return };
        if !self.pager.armed {
            radio_chip::pager_arm(&mut self.chip(n), baud);
            self.pager.armed = true;
            return;
        }
        let flags = self.chip(n).interrupts();
        if flags & int::FSK_SYNC != 0 {
            // polarity: from the batch's idle codewords (PagerRx), not REG_0B (bench, Nov 24, 2026)
            self.pager.rx.sync(|p| {
                let _ = crate::tasks::dsp::PAGES.try_send(p);
            });
            (self.pager.left, self.pager.last, self.pager.at_sync, self.pager.read) = (ferrite_core::pagerrx::FRAME_BYTES / 2, now, now, 0);
        }
        // the words due by now: a word is 16 bits; one more word's time in hand, so a word is whole in the FIFO
        let due = if self.pager.left > 0 { (now.wrapping_sub(self.pager.at_sync) * baud as u32 / 16_000).saturating_sub(1) as u16 } else { 0 };
        for _ in 0..due.saturating_sub(self.pager.read).min(8).min(self.pager.left) {
            self.pager.read += 1;
            self.pager.left -= 1;
            self.pager.last = now;
            let w = self.chip(n).fsk_read();
            self.pager.rx.word(w, |p| {
                let _ = crate::tasks::dsp::PAGES.try_send(p);
            });
        }
        if self.pager.left > 0 && self.pager.rx.over() {
            // the transmission ended inside the frame (no sync where the next batch's belongs): receive off and on
            // doesn't end a frame (bench, Nov 24, 2026), the modem off and set up again does
            self.pager.left = 0;
            let mut bk = self.chip(n);
            bk.fsk_off();
            radio_chip::pager_arm(&mut bk, baud);
        } else if flags & int::FSK_RX_DONE != 0 && self.pager.left == 0 {
            radio_chip::pager_rearm(&mut self.chip(n), baud);
        } else if self.pager.left == 0 {
            // no frame: keep the FIFO empty for the next one's sync
            radio_chip::pager_clear(&mut self.chip(n), baud);
            if now.wrapping_sub(self.pager.last) > GAP_MS {
                self.pager_end();
                self.pager.last = now;
            }
        }
    }

    /// TERM-43's `fsk` run ended (`fsk_stop`): its set-up and its `fsk off` left the modem as the pager didn't, so
    /// the pager sets it up again next tick (without this it stayed deaf until slot C retuned).
    #[cfg(feature = "bench")]
    pub(super) fn pager_after_fsk(&mut self) {
        (self.pager.armed, self.pager.left) = (false, 0);
    }

    /// The transmission ended: the page in progress to the UI task.
    fn pager_end(&mut self) {
        self.pager.rx.end(|p| {
            let _ = crate::tasks::dsp::PAGES.try_send(p);
        });
    }
}
