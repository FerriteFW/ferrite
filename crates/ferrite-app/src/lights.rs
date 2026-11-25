//! The backlight, keypad light and LEDs, the auto key lock and auto power off (manual chapters 7, 31 and 33:
//! KEY-20 and decision 63). The App decides; the firmware drives the pins (screen light PC6, keypad light PB3,
//! LEDs PC13/PC14) from [`App::dark`], [`App::keypad_lit`] and [`App::leds`], and switches off when
//! [`App::take_power_off`] says so.
//!
//! - **Dark:** no key, PTT or receiving for the backlight time (0 7 4 1). The key press that wakes a dark screen
//!   only wakes it; PTT acts at once (decision 63). Receive and transmit wake it and hold it lit only as 0 7 4 4
//!   says (DISP-15); with 0 7 4 3 on, a lit screen stays lit through a transmission, off it is dark while keyed
//!   (DISP-26).
//! - **Brightness:** 0 7 4 2's level for the theme (night red its own), as a PWM duty: [`App::backlight_duty`]
//!   (DISP-16, DISP-23).
//! - **LEDs:** red while transmitting, green while a slot receives; while the screen is dark, a heartbeat in the
//!   battery's colour (green above 80 %, yellow = both between the low warning and 80 %, red at or below it):
//!   one short flash every 5 s, or a double flash every 2 s while a message or alert is unread. A big link transfer
//!   (Console reading or writing records) flashes green while it runs and for a moment after (DISP-30).

use crate::settings::{AUTO_LOCK_TIMES, AUTO_OFF_TIMES, BACKLIGHT_TIMES};
use crate::App;

/// 0 7 4 2's levels 1-10 as PWM duty, thousandths, for the screen and the keypad light alike: a geometric curve
/// (each step about 1.41 times the last), so the steps look even to the eye. Level 1 is 4.5 %, the dimmest found
/// readable on the radio (Oct 12, 2026: the old 2 % was too dark); level 10 is full on.
pub const BRIGHTNESS: [u16; 10] = [45, 65, 90, 125, 180, 250, 355, 500, 710, 1000];

/// DISP-22.1: the keypad light's curve is the screen's scaled to this top (thousandths): half the power at every
/// level (Alex, Oct 22, 2026; on the launch-defaults list).
pub const KEYPAD_MAX: u16 = 500;

/// `a` is at or after `b` (wrapping ms).
fn after(a: u32, b: u32) -> bool {
    a.wrapping_sub(b) < 1 << 31
}
use ferrite_core::msg::{Device, LiveAll};

const DARK: u8 = 1;
const WARNED: u8 = 2;
const WANT_OFF: u8 = 4;
const SWALLOW: u8 = 8;
/// Keyed with 0 7 4 3 off: dark while keyed (DISP-26), the timeout left as it was.
const TX_DARK: u8 = 16;
/// A big link transfer was seen (DISP-30): [`App::xfer`] holds when.
const XFER: u8 = 32;

/// DISP-30: the transfer flash goes on this long after the last frame of a big transfer.
pub const XFER_LINGER_MS: u32 = 1000;
/// DISP-30: its period, half on, half off (twice the heartbeat's flash: quick, but not the steady green of receive).
pub const XFER_FLASH_MS: u32 = 2 * FLASH_MS;

/// DISP-30: whether the transfer flash is on at `now` (ms), and how long until it changes. Shared with the stock
/// restore's own loop, which runs without the App.
pub fn xfer_flash(now: u32) -> (bool, u32) {
    let t = now % XFER_FLASH_MS;
    if t < FLASH_MS { (true, FLASH_MS - t) } else { (false, XFER_FLASH_MS - t) }
}

/// A flash's length, and the two heartbeat periods.
const FLASH_MS: u32 = 120; // DISP-21.2: 80 ms looked dim (Alex, Oct 22, 2026)
const BEAT_MS: u32 = 5000;
const UNREAD_MS: u32 = 2000;
/// The unread pattern's flashes (red, green, red) start this far apart: a gap of a flash's length between them.
const NEXT_FLASH_MS: u32 = 240;
/// Auto power off warns this long before.
const OFF_WARN_MS: u32 = 10_000;
/// "On key press": the keypad stays lit this long after a key.
const KEY_LIGHT_MS: u32 = 5000;

/// The LEDs now, and when they next change (ms from now; 0 when they hold).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Leds {
    pub red: bool,
    pub green: bool,
    pub next_ms: u32,
}

impl App {
    /// The screen's backlight is off.
    pub fn dark(&self) -> bool {
        self.lit & DARK != 0
    }

    /// The backlight's PWM duty in thousandths (DISP-16, DISP-23): 0 while dark, else 0 7 4 2's level for the theme
    /// in use on a perceptual curve, 1000 at 10.
    pub fn backlight_duty(&self) -> u16 {
        if self.dark() || self.lit & TX_DARK != 0 {
            return 0;
        }
        let d = BRIGHTNESS[self.set.brightness() as usize - 1];
        if self.low_power() { d.min(crate::lpm::LPM_BACKLIGHT) } else { d } // PWR-24.5
    }

    /// The keypad light's PWM duty in thousandths (DISP-22): 0 while [`App::keypad_lit`] says off, else the same
    /// level as the screen's (the theme's 0 7 4 2), lit or not, so one level dims both, on its own lower curve
    /// (DISP-22.1, [`KEYPAD_MAX`]).
    pub fn keypad_duty(&self) -> u16 {
        if self.keypad_lit() && !self.low_power() {
            (BRIGHTNESS[self.set.brightness() as usize - 1] as u32 * KEYPAD_MAX as u32 / 1000) as u16
        } else {
            0
        }
    }

    /// The keypad light (0 7 4 5, DISP-22): with the screen, or for [`KEY_LIGHT_MS`] after any key or knob step
    /// (the one that woke the screen too).
    pub fn keypad_lit(&self) -> bool {
        let lit = !self.dark() && self.lit & TX_DARK == 0;
        match self.set.keypad_light {
            0 => false,
            1 => lit,
            2 => lit && self.now.wrapping_sub(self.pressed) < KEY_LIGHT_MS,
            _ => true,
        }
    }

    /// Auto power off has run out: the firmware switches off as the power switch does (PWR-17).
    pub fn take_power_off(&mut self) -> bool {
        let w = self.lit & WANT_OFF != 0;
        self.lit &= !WANT_OFF;
        w
    }

    /// DISP-30: a frame of a big link transfer (records read or written) came now: the LED flashes green while they
    /// come and for [`XFER_LINGER_MS`] after the last.
    pub(crate) fn link_transfer(&mut self, now: u32) {
        self.xfer = now;
        self.lit |= XFER;
    }

    /// DISP-30: a big link transfer is running (its last frame under [`XFER_LINGER_MS`] ago).
    pub fn transferring(&self) -> bool {
        self.lit & XFER != 0 && self.xfer_ago() < XFER_LINGER_MS
    }

    /// How long ago the last transfer frame came (0 when the link's clock ran ahead of the App's last tick).
    fn xfer_ago(&self) -> u32 {
        if after(self.xfer, self.now) { 0 } else { self.now.wrapping_sub(self.xfer) }
    }

    /// The LEDs (0 7 6).
    pub fn leds(&self, live: &LiveAll, dev: &Device) -> Leds {
        let mode = self.set.leds;
        if mode == 0 {
            return Leds::default();
        }
        if live.tx.keyed {
            return Leds { red: true, ..Leds::default() };
        }
        if live.slots.iter().any(|s| s.carrier) {
            // SQL-13.1: a carrier lights it, not level 0's always-open or monitor
            return Leds { green: true, ..Leds::default() };
        }
        // DISP-30: a big link transfer flashes green, lit screen or dark, over the heartbeat; the flash ends
        // XFER_LINGER_MS after the last frame (next_ms brings the tick back for it).
        if self.transferring() {
            let (green, next) = xfer_flash(self.now);
            let left = XFER_LINGER_MS - self.xfer_ago();
            return Leds { red: false, green, next_ms: next.min(left).max(1) };
        }
        if mode < 2 || !self.dark() {
            return Leds::default();
        }
        // The heartbeat: one flash in the battery's colour every 5 s, or, while something is unread (DISP-21.1), red,
        // green, red one after another every 2 s (never both at once: they don't blend into a clear yellow).
        // SBAR-9.1's levels: green, yellow (both), red.
        let (red, green) = match ferrite_core::battery::tier(dev.battery_pct, dev.battery_low || dev.battery_cutoff) {
            0 => (false, true),
            1 => (true, true),
            _ => (true, false),
        };
        let unread = [(true, false), (false, true), (true, false)];
        let (period, flashes): (u32, &[(bool, bool)]) = if self.badge() > 0 || self.alert & crate::ALERT_UNREAD != 0 { (UNREAD_MS, &unread) } else { (BEAT_MS, &[(red, green)][..]) };
        let t = self.now % period;
        let mut next_ms = period - t;
        for (i, &(r, g)) in flashes.iter().enumerate() {
            let start = i as u32 * NEXT_FLASH_MS;
            if t < start {
                next_ms = start - t;
                break;
            }
            if t < start + FLASH_MS {
                return Leds { red: r, green: g, next_ms: start + FLASH_MS - t };
            }
        }
        Leds { red: false, green: false, next_ms }
    }

    /// A key or the knob while dark: light up; true when this input only woke the screen.
    pub(crate) fn wake(&mut self, now: u32) -> bool {
        self.seen = now;
        self.pressed = now;
        self.lit &= !WARNED;
        if self.dark() {
            self.lit &= !DARK;
            return true;
        }
        false
    }

    /// The waking key's release is swallowed too (set when a key woke the screen).
    pub(crate) fn swallow(&mut self, on: bool) -> bool {
        let was = self.lit & SWALLOW != 0;
        if on {
            self.lit |= SWALLOW;
        } else {
            self.lit &= !SWALLOW;
        }
        was
    }

    /// A key woke the screen and its presses are being ignored until it is let go.
    pub(crate) fn swallowing(&self) -> bool {
        self.lit & SWALLOW != 0
    }

    /// The backlight time runs out now, as if no key had been pressed for it (the simulators' "Screen off"
    /// button, to see the dark-screen heartbeat). Nothing when the backlight is set to stay on.
    pub fn backlight_out(&mut self) {
        let bl = BACKLIGHT_TIMES[self.set.backlight as usize % BACKLIGHT_TIMES.len()] as u32 * 1000;
        if bl != 0 {
            self.seen = self.now.wrapping_sub(bl);
        }
    }

    /// Time passed: receiving keeps the radio awake (auto off); receive and transmit wake the screen as 0 7 4 4
    /// says (DISP-15), and 0 7 4 3 holds a lit screen through a transmission or darkens it (DISP-26); the
    /// backlight, auto lock and auto off.
    pub(crate) fn lights_tick(&mut self, now: u32, live: &LiveAll) {
        let rx = live.slots.iter().any(|s| s.carrier); // SQL-13.1
        let tx = live.tx.keyed;
        if !self.transferring() {
            self.lit &= !XFER; // DISP-30: over (so the clock wrapping round never brings it back)
        }
        // Auto off counts any activity, whatever wakes the screen.
        if rx || tx {
            self.awake = now;
            self.lit &= !WARNED;
        }
        // DISP-15.7: the APRS slot (and a slot deferring to it) never wakes the screen: packets come every few
        // seconds and would keep it lit; a message to us wakes it through its own popup.
        let voice = live.slots.iter().enumerate().any(|(i, s)| {
            s.squelch_open && !self.aprs_slot(i as u8) && !(i < crate::aprs::APRS_SLOT as usize && self.defers_to_c(i as u8))
        });
        // SAT-24: the satellite screen around a pass holds it lit (and wakes it as the window opens)
        if voice && self.set.wake_on >= 1 || tx && (self.set.wake_on >= 2 || self.set.tx_light && !self.dark()) || self.sat_awake() {
            self.seen = now;
        }
        if tx && !self.set.tx_light {
            self.lit |= TX_DARK;
        } else {
            self.lit &= !TX_DARK;
        }
        let idle = now.wrapping_sub(self.seen);
        let bl = BACKLIGHT_TIMES[self.set.backlight as usize % BACKLIGHT_TIMES.len()] as u32 * 1000;
        if bl != 0 && idle >= bl && self.lit & WARNED == 0 {
            self.lit |= DARK;
        } else if idle < bl || bl == 0 {
            self.lit &= !DARK;
        }
        // KEY-20: keys and the knob only (receiving doesn't keep the keys unlocked).
        let lock = AUTO_LOCK_TIMES[self.set.auto_lock as usize % AUTO_LOCK_TIMES.len()] as u32 * 1000;
        if lock != 0 && !self.locked() && now.wrapping_sub(self.last_input) >= lock {
            self.lock_keys(false); // always Keypad, with no toast
        }
        let off = AUTO_OFF_TIMES[self.set.auto_off as usize % AUTO_OFF_TIMES.len()] as u32 * 60_000;
        let idle = now.wrapping_sub(if after(self.awake, self.seen) { self.awake } else { self.seen });
        if off != 0 {
            if idle >= off {
                self.lit |= WANT_OFF;
            } else if idle + OFF_WARN_MS >= off && self.lit & WARNED == 0 {
                self.lit = (self.lit | WARNED) & !DARK;
                self.show_toast(b"Off in 10 s", b"press a key", false);
            }
        }
    }
}

