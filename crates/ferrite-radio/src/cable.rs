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
/// Bluetooth alike. Each port holds 576 bytes (the UART's 512-byte ring and the UI's 64), and a USB serial adapter
/// may send a few hundred more after an XOFF (issue #236: the host's driver has them queued already), so XOFF goes
/// out early, once [`Flow::XOFF_AT`] bytes wait (512 left to arrive in), and again after every
/// [`Flow::XOFF_AGAIN`] bytes that still come while above it (an XOFF lost, or a host that didn't stop); XON once
/// only [`Flow::XON_AT`] wait. Never while the link holds the port (LINK-23: a binary frame can't carry them), and
/// never once a 0x00 has come (TERM-2: a link is taking the port, and an XOFF would arrive in front of its first
/// answer, Console issue #93); a terminal sending text sends no 0x00.
#[derive(Clone, Copy, Debug, Default)]
pub struct Flow {
    held: bool,
    sync: bool,
    /// Bytes received since the last XOFF.
    since: u16,
}

impl Flow {
    pub const XOFF_AT: u16 = 64;
    pub const XOFF_AGAIN: u16 = 64;
    pub const XON_AT: u16 = 32;
    pub const XOFF: u8 = 0x13;
    pub const XON: u8 = 0x11;

    /// A byte received while the terminal holds the port.
    pub fn got(&mut self, b: u8) {
        if b == 0 {
            self.sync = true;
        }
        self.since = self.since.saturating_add(1);
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
        if !self.sync && waiting >= Self::XOFF_AT && (!self.held || self.since >= Self::XOFF_AGAIN) {
            (self.held, self.since) = (true, 0);
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

    /// TERM-13, TERM-13.1: a file sent too fast is paused with XOFF (64 bytes waiting) and let go with XON (32).
    #[test]
    fn xoff_then_xon() {
        let mut f = Flow::default();
        assert_eq!(f.step(63), None);
        assert_eq!(f.step(64), Some(Flow::XOFF));
        assert_eq!(f.step(300), None, "once while nothing more comes");
        assert_eq!(f.step(33), None);
        assert_eq!(f.step(32), Some(Flow::XON));
        assert_eq!(f.step(0), None);
    }

    /// TERM-13.1, LINK-23 (Console issue #93): once a 0x00 has come (a link taking the port), no XOFF goes ahead of its answers; the
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
        assert_eq!(f.step(64), Some(Flow::XOFF));
    }

    /// TERM-13.1 (issue #236): bytes that keep coming after an XOFF get another one every 64; none after a 0x00.
    #[test]
    fn xoff_again_while_bytes_still_come() {
        let mut f = Flow::default();
        assert_eq!(f.step(64), Some(Flow::XOFF));
        for _ in 0..63 {
            f.got(b'a');
        }
        assert_eq!(f.step(127), None);
        f.got(b'a');
        assert_eq!(f.step(128), Some(Flow::XOFF), "again");
        for _ in 0..64 {
            f.got(b'a');
        }
        assert_eq!(f.step(40), None, "below the mark: no XOFF, not yet XON");
        assert_eq!(f.step(32), Some(Flow::XON));
        // LINK-23: a link taking the port gets no XOFF, the first or a repeat
        f.got(0);
        for _ in 0..200 {
            f.got(1);
        }
        assert_eq!(f.step(201), None);
    }

    /// A host sending at full speed into a port of 576 bytes (the UART's ring and the UI's) that stops `late`
    /// bytes after each XOFF and starts again `late` bytes after an XON, while the terminal takes one 46-byte
    /// line and then waits `stall` byte times for the store. Returns the bytes lost and the XOFFs sent.
    fn feed(total: usize, late: usize, stall: usize, honours: bool) -> (usize, usize) {
        const ROOM: usize = 576;
        const LINE: usize = 46;
        let mut f = Flow::default();
        let (mut sent, mut waiting, mut lost, mut xoffs) = (0, 0usize, 0, 0);
        // the host: paused or not, and the byte count at which what the radio last said takes effect
        let (mut paused, mut change): (bool, Option<(usize, bool)>) = (false, None);
        let (mut line, mut wait) = (0usize, 0usize);
        let mut t = 0usize;
        while sent < total || waiting > 0 {
            t += 1;
            assert!(t < 10_000_000, "stuck: sent {sent}, waiting {waiting}, paused {paused}");
            // one byte time on the wire
            if let Some((at, p)) = change {
                if t >= at {
                    (paused, change) = (p, None);
                }
            }
            if sent < total && !paused {
                sent += 1;
                f.got(b'a');
                if waiting < ROOM {
                    waiting += 1;
                } else {
                    lost += 1;
                }
            }
            // the terminal: a line's bytes as fast as they come, then a wait for the store
            if wait > 0 {
                wait -= 1;
            } else if waiting > 0 {
                waiting -= 1;
                line += 1;
                if line == LINE {
                    (line, wait) = (0, stall);
                }
            }
            match f.step(waiting as u16) {
                Some(Flow::XOFF) => {
                    xoffs += 1;
                    // the first XOFF on its way stops the host; a repeat doesn't put that off
                    if honours && !matches!(change, Some((_, true))) {
                        change = Some((t + late, true));
                    }
                }
                Some(Flow::XON) => change = Some((t + late, false)),
                _ => {}
            }
        }
        (lost, xoffs)
    }

    /// TERM-13.1 (issue #236): a load at 115200 with the store taking 50 ms a line (575 byte times) loses nothing
    /// though the host stops up to 500 bytes after an XOFF; 160 bytes waiting (before) left room for 416.
    #[test]
    fn a_late_host_loses_nothing() {
        for late in [0, 100, 300, 500] {
            let (lost, xoffs) = feed(5_300, late, 575, true);
            assert_eq!(lost, 0, "{late} bytes late");
            assert!(xoffs > 1, "{late}: {xoffs}");
        }
        let (lost, _) = feed(5_300, 600, 575, true);
        assert!(lost > 0, "a host later than the room is beyond any threshold");
        // a host that never stops: XOFF keeps coming, every 64 bytes
        let (lost, xoffs) = feed(5_300, 0, 575, false);
        assert!(lost > 0 && xoffs > 50, "{lost} {xoffs}");
    }
}
