//! The programming cable (UART4): when it may send.
//!
//! - **Keyed (TX-43, TX audit F9):** nothing is sent, so a bias resistor from pin 11 never carries data into the
//!   transmitted audio. The link task asks before every byte; the Radio task marks the radio keyed just before the
//!   key-up plan runs, so a byte in flight is the most that can overlap.
//! - **Speaker mic (SND-14, decision 146):** with one in the side connector nothing is sent at all and PC10 (UART4
//!   TX, the connector's pin 11) is held high as a GPIO output, as stock holds it outside programming mode: Radtel
//!   speaker mics take their mic bias from that pin, so serial data on it (or a UART idling in AF) leaves the mic
//!   without a steady bias.

/// Whether the cable may send a byte now. `dry`: the PA drive held at 0 (the `tx-dry` build, or Dry TX on: TX-48),
/// where the bench reads the chips over the cable while keyed (`research/tx-vs-stock.md` section 6), so keying holds nothing; `spkmic`: a
/// speaker mic is connected (`ferrite_core::acc::ON`): never, in any build.
pub fn may_send(keyed: bool, dry: bool, spkmic: bool) -> bool {
    !spkmic && (dry || !keyed)
}

/// Software flow control for a terminal program sending a file (the terminal's `load`, TERM-13), on the cable and
/// Bluetooth alike: XOFF once [`Flow::XOFF_AT`] bytes wait (each UART's receive ring holds 512, so a slow sender
/// still fits), XON again at [`Flow::XON_AT`]. Never while the link holds the port (LINK-23: a binary frame can't
/// carry them), and never once a 0x00 has come (TERM-2: a link is taking the port, and an XOFF would arrive in front
/// of its first answer, Console issue #93); a terminal sending text sends no 0x00.
#[derive(Clone, Copy, Debug, Default)]
pub struct Flow {
    held: bool,
    sync: bool,
}

impl Flow {
    pub const XOFF_AT: u16 = 160;
    pub const XON_AT: u16 = 32;
    pub const XOFF: u8 = 0x13;
    pub const XON: u8 = 0x11;

    /// A byte received while the terminal holds the port.
    pub fn got(&mut self, b: u8) {
        if b == 0 {
            self.sync = true;
        }
    }

    /// The link holds the port: no flow control (what was held is let go without an XON: the link's sender
    /// doesn't use it).
    pub fn link(&mut self) {
        *self = Self::default();
    }

    /// The byte to send now, if any, with `waiting` bytes received and not yet taken.
    pub fn step(&mut self, waiting: u16) -> Option<u8> {
        if waiting == 0 {
            self.sync = false;
        }
        if !self.held && !self.sync && waiting >= Self::XOFF_AT {
            self.held = true;
            Some(Self::XOFF)
        } else if self.held && waiting <= Self::XON_AT {
            self.held = false;
            Some(Self::XON)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TERM-13: a file sent too fast is paused with XOFF and let go with XON.
    #[test]
    fn xoff_then_xon() {
        let mut f = Flow::default();
        assert_eq!(f.step(100), None);
        assert_eq!(f.step(160), Some(Flow::XOFF));
        assert_eq!(f.step(300), None, "once");
        assert_eq!(f.step(33), None);
        assert_eq!(f.step(32), Some(Flow::XON));
        assert_eq!(f.step(0), None);
    }

    /// Console issue #93: once a 0x00 has come (a link taking the port), no XOFF goes ahead of its answers; the
    /// link then holds the port and nothing is sent at all.
    #[test]
    fn no_xoff_after_a_sync() {
        let mut f = Flow::default();
        f.got(0);
        for b in [1u8; 200] {
            f.got(b);
        }
        assert_eq!(f.step(201), None);
        f.link();
        assert_eq!(f.step(0), None);
        // back with the terminal, a text sender is paused again
        assert_eq!(f.step(160), Some(Flow::XOFF));
    }
}
