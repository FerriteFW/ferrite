//! SAFE-14: a PTT held at power-on is ignored until released: no transmission until every PTT input (PTT A, PTT B,
//! the connector's) has read released once after boot; one held longer than [`STUCK_MS`] from boot is reported
//! stuck, by name, until it is let go.

/// A PTT held this long from boot is stuck (SAFE-14).
pub const STUCK_MS: u32 = 1000;
pub use ferrite_core::stuck::{names, PTT_A, PTT_B, PTT_EXT};

/// The guard; starts closed at boot.
#[derive(Clone, Copy, Debug, Default)]
pub struct PttGuard {
    ok: bool,
    /// The first look happened; the inputs held since boot and never released.
    looked: bool,
    held: u8,
}

impl PttGuard {
    /// One look at the PTT inputs: `pressed` the [`PTT_A`], [`PTT_B`], [`PTT_EXT`] bits pressed now. True from the
    /// first look with all of them released, for the rest of the power-on.
    pub fn look(&mut self, pressed: u8) -> bool {
        self.held = if self.looked { self.held & pressed } else { pressed };
        self.looked = true;
        self.ok |= pressed == 0;
        self.ok
    }

    /// Transmit allowed (every input read released once since boot).
    pub fn open(&self) -> bool {
        self.ok
    }

    /// The inputs held since boot for [`STUCK_MS`] or more at `now` and not let go since: shown as "PTT stuck".
    pub fn stuck(&self, now: u32) -> u8 {
        if now >= STUCK_MS {
            self.held
        } else {
            0
        }
    }
}

/// SND-12.7 (decision 430): which mic a key-up uses, from the speaker mic's detect (`plugged`) and the PTT inputs
/// pressed now ([`PTT_A`], [`PTT_B`], [`PTT_EXT`]): the speaker mic's own PTT (the connector's) takes its mic; the
/// radio's own PTTs, with the connector's not pressed, take the radio's mic; a key-up with no PTT pressed (VOX, the
/// link, a data burst) keeps stock's choice, the speaker mic whenever one is plugged in. One of
/// `ferrite_core::msg::MIC_SRC_*`; the route (PE1) is high for [`ferrite_core::msg::MIC_SRC_SPK`] only.
pub fn mic_src(plugged: bool, pressed: u8) -> u8 {
    use ferrite_core::msg::{MIC_SRC_NONE, MIC_SRC_RADIO, MIC_SRC_SPK};
    if !plugged {
        MIC_SRC_NONE
    } else if pressed & PTT_EXT == 0 && pressed & (PTT_A | PTT_B) != 0 {
        MIC_SRC_RADIO
    } else {
        MIC_SRC_SPK
    }
}

#[cfg(test)]
mod mic_tests {
    use super::*;
    use ferrite_core::msg::{MIC_SRC_NONE, MIC_SRC_RADIO, MIC_SRC_SPK};

    /// SND-12.7: the PTT pressed picks the mic when a speaker mic is plugged in; without one, nothing changes.
    #[test]
    fn the_ptt_pressed_picks_the_mic() {
        assert_eq!(mic_src(false, PTT_A), MIC_SRC_NONE);
        assert_eq!(mic_src(false, PTT_EXT), MIC_SRC_NONE);
        assert_eq!(mic_src(true, PTT_EXT), MIC_SRC_SPK, "the speaker mic's PTT");
        assert_eq!(mic_src(true, PTT_A), MIC_SRC_RADIO, "the radio's PTT");
        assert_eq!(mic_src(true, PTT_B), MIC_SRC_RADIO, "the radio's PTT B");
        assert_eq!(mic_src(true, PTT_A | PTT_EXT), MIC_SRC_SPK, "the speaker mic's PTT held too");
        assert_eq!(mic_src(true, 0), MIC_SRC_SPK, "no PTT (VOX, the link, data): stock's, the speaker mic");
    }
}
