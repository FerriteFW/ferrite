//! Key presses from key down/up and time (manual chapter 7):
//!
//! - **KEY-3** a short press fires on release, under 700 ms; a long press fires while held, at 700 ms, and the
//!   release after it does nothing. There is no gap between the two (decided Oct 7, 2026).
//! - **KEY-4** the four arrows repeat at 10 per second after 400 ms held; once one has repeated, its release does
//!   nothing. Arrows have no long press.
//!
//! The firmware's input sampler gives key down/up (uradio `keypad` with its own long press switched off); the
//! simulator gives the PC's key down/up. Both go through this one tracker.

use ferrite_core::keys::Key;

/// A release before this is a short press; the long press fires at the same moment (KEY-3: no gap).
pub const SHORT_MS: u32 = LONG_MS;
pub const LONG_MS: u32 = 700;
pub const REPEAT_AFTER_MS: u32 = 400;
pub const REPEAT_MS: u32 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Short,
    Long,
    Repeat,
    /// The release after a long press (not after a repeat): only the screen hears it, so the shortcut map can
    /// run its shortcut when the digit comes up (HOME-8).
    LongUp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub press: Press,
}

#[derive(Clone, Copy)]
struct Held {
    key: Key,
    since: u32,
    /// A long press or repeat has fired, or the key was used as a modifier: the release does nothing.
    fired: bool,
    /// The long press fired (not a modifier use).
    long: bool,
    /// When the next repeat is due.
    next: u32,
    /// Taken by the quick menu (HOME-17): no long press, repeat or release event at all.
    dead: bool,
    /// A screen's note on this hold, gone with its release (BCR-25.1: home's ◀▶ hold stepping bands).
    mark: u8,
}

/// Up to four keys held at once (SIDE2 + an arrow, # held while another key goes down, ...).
pub struct KeyTracker {
    held: [Option<Held>; 4],
}

fn repeats(k: Key) -> bool {
    matches!(k, Key::Up | Key::Down | Key::Left | Key::Right)
}

impl KeyTracker {
    pub const fn new() -> Self {
        KeyTracker { held: [None; 4] }
    }

    pub fn down(&mut self, key: Key, now: u32) {
        if self.is_held(key) {
            return; // the PC's auto-repeat, or a bounce: one press
        }
        if let Some(slot) = self.held.iter_mut().find(|h| h.is_none()) {
            *slot = Some(Held { key, since: now, fired: false, long: false, next: now.wrapping_add(REPEAT_AFTER_MS), dead: false, mark: 0 });
        }
    }

    /// The short press, if this release makes one.
    pub fn up(&mut self, key: Key, now: u32) -> Option<KeyEvent> {
        self.release(key, now).filter(|e| e.press == Press::Short)
    }

    /// The short press, or [`Press::LongUp`] after a long press (a modifier's release gives neither).
    pub fn release(&mut self, key: Key, now: u32) -> Option<KeyEvent> {
        let slot = self.held.iter_mut().find(|h| h.is_some_and(|h| h.key == key))?;
        let h = slot.take()?;
        if h.dead {
            return None;
        }
        if h.fired {
            (h.long && !repeats(key)).then_some(KeyEvent { key, press: Press::LongUp })
        } else {
            (now.wrapping_sub(h.since) < SHORT_MS).then_some(KeyEvent { key, press: Press::Short })
        }
    }

    /// Long presses and repeats that are due by `now`, oldest first.
    pub fn tick(&mut self, now: u32, mut out: impl FnMut(KeyEvent)) {
        for h in self.held.iter_mut().flatten() {
            if h.dead {
                continue;
            }
            if repeats(h.key) {
                while now.wrapping_sub(h.next) as i32 >= 0 {
                    h.fired = true;
                    h.next = h.next.wrapping_add(REPEAT_MS);
                    out(KeyEvent { key: h.key, press: Press::Repeat });
                }
            } else if !h.fired && now.wrapping_sub(h.since) >= LONG_MS {
                h.fired = true;
                h.long = true;
                out(KeyEvent { key: h.key, press: Press::Long });
            }
        }
    }

    /// The held key was used as a modifier (SIDE2 + knob): its release is neither a short press nor the release
    /// after a long press, even when the long press already fired (KEY-22: hold SIDE2 alone is the counter).
    pub fn used(&mut self, key: Key) {
        for h in self.held.iter_mut().flatten().filter(|h| h.key == key) {
            h.fired = true;
            h.long = false;
        }
    }

    /// The held key (`None`: every held key) was taken by the quick menu (HOME-17): it gives no long press, no
    /// repeat and nothing at its release. It still counts as held.
    pub fn kill(&mut self, key: Option<Key>) {
        for h in self.held.iter_mut().flatten() {
            if key.is_none_or(|k| h.key == k) {
                h.dead = true;
            }
        }
    }

    /// The screen's note on the held key's hold (0 when it has none, or the key isn't held).
    pub fn mark(&self, key: Key) -> u8 {
        self.held.iter().flatten().find(|h| h.key == key).map_or(0, |h| h.mark)
    }

    /// Notes `m` on the held key's hold; the next press of it starts at 0.
    pub fn set_mark(&mut self, key: Key, m: u8) {
        for h in self.held.iter_mut().flatten().filter(|h| h.key == key) {
            h.mark = m;
        }
    }

    pub fn is_held(&self, key: Key) -> bool {
        self.held.iter().flatten().any(|h| h.key == key)
    }

    /// Everything released (going home, the lock).
    pub fn clear(&mut self) {
        self.held = [None; 4];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec::Vec;

    fn run(t: &mut KeyTracker, from: u32, to: u32) -> Vec<KeyEvent> {
        let mut v = Vec::new();
        for ms in (from..=to).step_by(10) {
            t.tick(ms, |e| v.push(e));
        }
        v
    }

    #[test]
    fn short_fires_on_release_under_700() {
        let mut t = KeyTracker::new();
        t.down(Key::Menu, 0);
        assert!(run(&mut t, 0, 690).is_empty(), "nothing while held");
        assert_eq!(t.up(Key::Menu, 699), Some(KeyEvent { key: Key::Menu, press: Press::Short }));
        t.down(Key::Menu, 1000);
        run(&mut t, 1000, 1700);
        assert_eq!(t.up(Key::Menu, 1700), None, "700 ms is a long press, not short");
    }

    #[test]
    fn long_fires_while_held_at_700() {
        let mut t = KeyTracker::new();
        t.down(Key::Slot, 0);
        assert!(run(&mut t, 0, 690).is_empty());
        assert_eq!(run(&mut t, 700, 700), [KeyEvent { key: Key::Slot, press: Press::Long }]);
        assert!(run(&mut t, 710, 3000).is_empty(), "once");
        assert_eq!(t.up(Key::Slot, 3000), None, "the release after a long press does nothing");
    }

    #[test]
    fn release_after_long_is_long_up() {
        let mut t = KeyTracker::new();
        t.down(Key::Digit(5), 0);
        run(&mut t, 0, 800);
        assert_eq!(t.release(Key::Digit(5), 800), Some(KeyEvent { key: Key::Digit(5), press: Press::LongUp }));
        t.down(Key::Side(2), 1000);
        t.used(Key::Side(2));
        assert_eq!(t.release(Key::Side(2), 1100), None, "a modifier's release is nothing");
        t.down(Key::Up, 2000);
        run(&mut t, 2000, 2600);
        assert_eq!(t.release(Key::Up, 2600), None, "nor after repeats");
        // KEY-22: SIDE2 held past 700 ms and then used with the knob: its release is nothing either.
        t.down(Key::Side(2), 3000);
        run(&mut t, 3000, 3800);
        t.used(Key::Side(2));
        assert_eq!(t.release(Key::Side(2), 3900), None, "a modifier after its long press");
    }

    #[test]
    fn no_gap_between_short_and_long() {
        let mut t = KeyTracker::new();
        t.down(Key::Hash, 0);
        assert!(run(&mut t, 0, 600).is_empty());
        assert_eq!(t.up(Key::Hash, 600), Some(KeyEvent { key: Key::Hash, press: Press::Short }), "600 ms is short");
        t.down(Key::Hash, 1000);
        assert_eq!(run(&mut t, 1000, 1700), [KeyEvent { key: Key::Hash, press: Press::Long }]);
    }

    #[test]
    fn arrows_repeat_at_10_per_second_after_400() {
        let mut t = KeyTracker::new();
        t.down(Key::Up, 0);
        assert!(run(&mut t, 0, 390).is_empty());
        let v = run(&mut t, 400, 1390);
        assert_eq!(v.len(), 10, "400..1390 ms: 10 repeats");
        assert!(v.iter().all(|e| e.press == Press::Repeat && e.key == Key::Up));
        assert_eq!(t.up(Key::Up, 1400), None, "no extra step on release");
        t.down(Key::Down, 2000);
        assert_eq!(t.up(Key::Down, 2100), Some(KeyEvent { key: Key::Down, press: Press::Short }));
    }

    /// HOME-17: a key taken by the quick menu gives nothing more: no long press, no repeat, nothing at its release.
    #[test]
    fn killed_keys_give_nothing() {
        let mut t = KeyTracker::new();
        t.down(Key::Up, 0);
        t.down(Key::Star, 0);
        t.kill(Some(Key::Up));
        assert_eq!(run(&mut t, 0, 1000), [KeyEvent { key: Key::Star, press: Press::Long }], "no repeats for UP");
        assert!(t.is_held(Key::Up));
        assert_eq!(t.release(Key::Up, 1000), None);
        t.kill(None);
        assert_eq!(t.release(Key::Star, 1100), None, "not even the release after a long press");
        t.down(Key::Ptt(0), 2000);
        t.kill(Some(Key::Ptt(0)));
        assert_eq!(t.release(Key::Ptt(0), 2100), None, "no short press");
    }

    #[test]
    fn two_keys_held() {
        let mut t = KeyTracker::new();
        t.down(Key::Side(2), 0);
        t.down(Key::Up, 50);
        t.down(Key::Up, 60); // auto-repeat from the PC: ignored
        assert!(t.is_held(Key::Side(2)));
        assert_eq!(t.up(Key::Up, 100).map(|e| e.key), Some(Key::Up));
        assert!(!t.is_held(Key::Up));
        assert_eq!(run(&mut t, 700, 700), [KeyEvent { key: Key::Side(2), press: Press::Long }]);
    }
}
