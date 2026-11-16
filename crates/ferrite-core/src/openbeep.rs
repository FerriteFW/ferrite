//! The open beep (SQL-20, decision 402): a slot with one set holds its audio back until the squelch has stayed open
//! a little longer, then plays one of the end tones' sounds over the audio as it opens. Only the speaker waits: the
//! squelch, the watch, the scans and the decoders see the open as before.

use crate::tonegen::{Step, END_TONES};

/// How long the audio waits after the squelch opens (SQL-20.2): with the software squelch's 30 ms (SQL-18.1), the
/// signal has held about 100 ms when the audio and the tone start; shorter opens stay silent.
pub const HOLD_MS: u32 = 70;

const NONE: u8 = 0xFF;

/// The audio's gate for the slot the speaker would play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gate {
    slot: u8,
    since: u32,
    passed: bool,
}

impl Gate {
    pub const DEFAULT: Gate = Gate { slot: NONE, since: 0, passed: false };

    /// One tick: `slot` is the slot whose audio would play now (None: nothing), `beep` its open beep (0 off).
    /// Returns whether its audio plays, and the beep to start (an index into `END_TONES`) on the tick it opens.
    pub fn step(&mut self, now: u32, slot: Option<u8>, beep: u8) -> (bool, Option<u8>) {
        let Some(s) = slot else {
            *self = Gate::DEFAULT;
            return (false, None);
        };
        if s != self.slot {
            *self = Gate { slot: s, since: now, passed: false };
        }
        if self.passed || beep == 0 {
            self.passed = true; // no beep: the audio as before; a beep set later waits for the next open
            return (true, None);
        }
        if now.wrapping_sub(self.since) < HOLD_MS {
            return (false, None);
        }
        self.passed = true;
        (true, Some(beep).filter(|&b| (b as usize) < END_TONES.len()))
    }
}

/// Plays a tone sequence step by step on the receive tick (the Radio task doesn't wait through it, SQL-20.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Player {
    seq: &'static [Step],
    i: u8,
    at: u32,
}

impl Player {
    /// `END_TONES[beep]` from `now`, and its first step's pitch (0: nothing to play).
    pub fn new(beep: u8, now: u32) -> (Player, u16) {
        let seq = END_TONES.get(beep as usize).copied().unwrap_or(&[]);
        (Player { seq, i: 0, at: now }, seq.first().map_or(0, |s| s.0))
    }

    /// The tick at `now`: Some(pitch) when the step changes (0: the sequence is over), None while it holds.
    pub fn tick(&mut self, now: u32) -> Option<u16> {
        let (_, ms) = *self.seq.get(self.i as usize)?;
        if now.wrapping_sub(self.at) < ms as u32 {
            return None;
        }
        self.at = self.at.wrapping_add(ms as u32);
        self.i += 1;
        Some(self.seq.get(self.i as usize).map_or(0, |s| s.0))
    }

    pub fn done(&self) -> bool {
        self.i as usize >= self.seq.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SQL-20.2: opens shorter than the wait stay silent; a held one gets its audio and the beep together.
    #[test]
    fn sql_20_2_short_opens_stay_silent() {
        let mut g = Gate::DEFAULT;
        // open for 60 ms, then closed: no audio, no beep
        for t in (0..60).step_by(10) {
            assert_eq!(g.step(t, Some(0), 1), (false, None));
        }
        assert_eq!(g.step(60, None, 1), (false, None));
        // held: the audio and the beep together at HOLD_MS, then audio only
        for t in (100..170).step_by(10) {
            assert_eq!(g.step(t, Some(0), 1), (false, None), "{t}");
        }
        assert_eq!(g.step(170, Some(0), 1), (true, Some(1)));
        assert_eq!(g.step(180, Some(0), 1), (true, None));
    }

    /// SQL-20.2, SQL-20.4: Off is as before; the audio turning to another slot waits that slot's own hold.
    #[test]
    fn sql_20_off_is_as_before_and_another_slot_starts_again() {
        let mut g = Gate::DEFAULT;
        assert_eq!(g.step(0, Some(1), 0), (true, None));
        // B's audio follows A's at once: B waits its own hold
        assert_eq!(g.step(10, Some(0), 2), (false, None));
        assert_eq!(g.step(80, Some(0), 2), (true, Some(2)));
        // a beep set while open waits for the next open
        let mut g = Gate::DEFAULT;
        g.step(0, Some(0), 0);
        assert_eq!(g.step(100, Some(0), 3), (true, None));
    }

    /// SQL-20.3: the tone's steps play in order on the tick, then it ends.
    #[test]
    fn sql_20_3_steps_play_in_order() {
        let (mut p, hz) = Player::new(4, 1000); // Chirp: 1000, 1250, 1500 Hz, 60 ms each
        assert_eq!(hz, 1000);
        assert_eq!(p.tick(1050), None);
        assert_eq!(p.tick(1060), Some(1250));
        assert_eq!(p.tick(1120), Some(1500));
        assert!(!p.done());
        assert_eq!(p.tick(1180), Some(0));
        assert!(p.done());
        assert_eq!(p.tick(1300), None);
        let (p, hz) = Player::new(0, 0);
        assert!(p.done() && hz == 0);
    }
}
