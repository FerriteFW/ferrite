//! POCSAG through the BK4829's own FSK modem (DEC-23, decision 453): on the chip that has slot C in Pager mode.

use super::*;
use ferrite_core::pagerrx::PagerRx;

/// The pager receiver: the speed asked (0: off), the chip and what it was set up for, the batch being read.
pub(super) struct PagerState {
    pub(super) baud: u16,
    chip: Option<u8>,
    at: Option<(u8, Freq10)>,
    armed: bool,
    /// Words of this frame still to read, and when the last came (ms).
    left: u16,
    last: u32,
    rx: PagerRx,
}

impl PagerState {
    pub(super) const fn new() -> Self {
        PagerState { baud: 0, chip: None, at: None, armed: false, left: 0, last: 0, rx: PagerRx::new(1200) }
    }
}

/// Bench counters (read with `mem32`): syncs, words, frames ended early, early re-arms, frames done.
#[unsafe(no_mangle)]
pub static PAGER_DBG: [core::sync::atomic::AtomicU32; 5] = [const { core::sync::atomic::AtomicU32::new(0) }; 5];
/// Bench: the words since the last sync (64 of them), and how many.
#[unsafe(no_mangle)]
pub static PAGER_WORDS: [core::sync::atomic::AtomicU32; 65] = [const { core::sync::atomic::AtomicU32::new(0) }; 65];
fn dbg_word(w: u16) {
    use core::sync::atomic::Ordering::Relaxed;
    let i = PAGER_WORDS[64].fetch_add(1, Relaxed) as usize % 64; // a ring: [64] counts every word
    PAGER_WORDS[i].store(w as u32, Relaxed);
}
fn dbg(i: usize) {
    PAGER_DBG[i].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

/// No sync for this long after a batch: the transmission ended (a batch is 453 ms at 1200 baud).
const GAP_MS: u32 = 1200;

impl Radio {
    /// DEC-23: each tick, the modem set up on slot C's chip after any retune or wake, then its flags: a sync starts
    /// a batch (either polarity: the modem corrects it), FIFO almost full gives 4 words, done the rest and listen again.
    /// Pages go to the UI task as the DSP task's did ([`crate::tasks::dsp::PAGES`]).
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
            (self.pager.chip, self.pager.at, self.pager.armed) = (chip, at, false);
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
            dbg(0);
            dbg_word(0xFFFF); // a sync's mark (with the words left and the batch position after it)
            dbg_word(self.pager.left);
            // polarity: from the batch's idle codewords (PagerRx), not REG_0B bit 7 (bench, Nov 24, 2026)
            self.pager.rx.sync(|p| {
                let _ = crate::tasks::dsp::PAGES.try_send(p);
            });
            (self.pager.left, self.pager.last) = (ferrite_core::pagerrx::FRAME_BYTES / 2, now);
        }
        let words: u16 = if flags & int::FSK_RX_DONE != 0 { 8 } else if flags & int::FSK_FIFO_FULL != 0 { 4 } else { 0 };
        for _ in 0..words.min(self.pager.left) {
            self.pager.last = now; // words still coming: the gap counts from the last
            let w = self.chip(n).fsk_read();
            dbg(1);
            dbg_word(w);
            self.pager.left -= 1;
            self.pager.rx.word(w, |p| {
                let _ = crate::tasks::dsp::PAGES.try_send(p);
            });
        }
        // the frame done, or the transmission over before it (no sync between batches): listen for the next
        if self.pager.left > 0 && self.pager.rx.over() {
            dbg(3);
            // mid-frame: receive off and on doesn't end a frame (bench, Nov 24, 2026), the modem off and set up does
            self.pager.left = 0;
            let mut bk = self.chip(n);
            bk.fsk_off();
            radio_chip::pager_arm(&mut bk, baud);
        } else if flags & int::FSK_RX_DONE != 0 {
            dbg(4);
            self.pager.left = 0;
            radio_chip::pager_rearm(&mut self.chip(n), baud);
        } else if self.pager.left == 0 && now.wrapping_sub(self.pager.last) > GAP_MS {
            self.pager_end();
            self.pager.last = now;
        }
    }

    /// The transmission ended: the page in progress to the UI task.
    fn pager_end(&mut self) {
        self.pager.rx.end(|p| {
            let _ = crate::tasks::dsp::PAGES.try_send(p);
        });
    }
}
