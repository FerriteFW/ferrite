//! The watch engine (manual chapter 13): which slot each BK4829 listens to now, whose audio is heard, and when a
//! watched slot takes over. Plain decisions: the Radio task tunes the chips it names and reports each chip's
//! squelch back on the next [`Watch::tick`].
//!
//! - Two chips: slots on the same chip take turns on it (WATCH-2), [`ACTIVE_DWELL_MS`] on the active slot and
//!   [`WATCHED_DWELL_MS`] on each watched one; a slot with its squelch open keeps the chip, and for
//!   [`HOLD_MS`] after it closes (the reply after the squelch tail).
//! - Priority (APRS-21.1, decision 181): a slot in [`Cfg::prio`] (APRS mode's or KISS's C) has turns of
//!   [`PRIO_DWELL_MS`], counted only while its chip has the receive path; once its squelch opens it keeps the chip
//!   (and the path) until it closes, muted or not, with no hold after; another slot's hold after its squelch
//!   closes doesn't keep it from its turns; and a slot open on the priority slot's own frequency hands the chip
//!   to it. Another slot already open on a different frequency keeps the chip as before.
//! - Deferring (APRS-21.2): while the APRS slot ([`Cfg::aprs`]) is listened to, another slot on its frequency
//!   takes no turns of its own: the APRS slot already listens there. Its squelch follows the APRS slot's, and
//!   when it would be heard the APRS slot's chip plays ([`Out::defer`]).
//! - Audio: the active slot's first; a watched slot only while the active one is closed or muted (WATCH-3). A
//!   muted slot (quick mute, HOME-15) is never heard and never holds a shared chip past its dwell, so it blocks
//!   no other slot; it still listens, so its squelch and S-meter carry on.
//! - The active slot never changes here: a watched slot that is heard plays and shows RX, and only the user
//!   changes the active slot (WATCH-5; Follow was dropped, decision 81).
//! - Paused (WATCH-6, WATCH-16, TX-34): while keyed and until [`Watch::pause`]'s time, only the active slot.
//! - Cancelled (WATCH-11): when the watch set needs more than one relay state, only the active slot.
//! - Off (BCR-23, WATCH-2.1): slots no chip listens to at all, whatever the set says: a slot turned off with \*
//!   (HOME-15.1), so its turns go to the others; the broadcast slot (the SI4732 has it); and, while broadcast plays
//!   with the relay released, slots that need the relay (BCR-15).
//!
//! The timings marked "to confirm on the bench" in the manual (WATCH-17).

use crate::freq::{band, Freq10};
use core::sync::atomic::{AtomicBool, Ordering::Relaxed};

static VOICE_FIRST: AtomicBool = AtomicBool::new(false);

/// WATCH-17.3 (decision 295): 0 5 8 7 is APRS first (the default); the UI task passes the setting on each frame.
pub fn aprs_first() -> bool {
    !VOICE_FIRST.load(Relaxed)
}

pub fn set_aprs_first(on: bool) {
    VOICE_FIRST.store(!on, Relaxed);
}
use crate::msg::SLOTS;
use crate::path::Sel;

/// How long the active slot keeps a shared chip per turn (to confirm on the bench).
pub const ACTIVE_DWELL_MS: u32 = 150;
/// How long a watched slot keeps a shared chip per turn: enough for the squelch to open on a carrier (stock
/// hops about every 70 ms; to confirm on the bench).
pub const WATCHED_DWELL_MS: u32 = 70;
/// How long a priority slot (APRS-21.1: APRS mode's or KISS's C) keeps a shared chip per turn, counted while its
/// chip has the receive path: enough for its squelch, with the open delay at 0 (SQL-17), to read a carrier open
/// after the retune (some 40 ms on the radio, decision 166) with margin; stock's whole turn is 90-110 ms
/// (`analysis/watch.md`). With the active slot's 150 ms the cycle is 250 ms, inside an APRS frame's 300 ms of flags.
pub const PRIO_DWELL_MS: u32 = 100;
/// WATCH-17.3 (decision 295), APRS first: the priority slot's turn on a chip, or on the receive path, while the
/// watch also has other slots. Long enough that a frame starting while another slot looks is caught early in its
/// flags (a shared chip retunes for each look and settles some 40 ms, decision 166).
pub const APRS_FIRST_MS: u32 = 400;
/// WATCH-17.3: another slot's look between the priority slot's turns, APRS first: its settle (the software
/// squelch, about 70-80 ms after a retune) and a reading or two, so a call that starts is still caught and then
/// keeps the chip as any open slot does.
pub const LOOK_MS: u32 = 100;
/// SCAN-21 (decision 189): while a range scan runs, the receive path stays on its chip this long between the other
/// chip's watched turns (each [`WATCHED_DWELL_MS`] or a priority slot's), so the fast pass keeps 30 steps a second
/// and more while the other band is still watched.
pub const SCAN_TURN_MS: u32 = 1_000;
/// A slot that closes keeps its chip this long (the reply after the squelch tail).
pub const HOLD_MS: u32 = 2_000;
/// A chip given the receive path back (or a new slot) reads its squelch shut until it has woken, tuned and passed
/// its open delay: a closing read this soon after tells nothing about a slot that was open (HOME-15: the muted
/// active slot's card flickered to Muted on every hop back; to confirm on the bench).
pub const REACQUIRE_MS: u32 = 60;

/// APRS-21.4 (decision 300): after a data frame from the APRS slot, it keeps its chip and the receive path this
/// long, no looks at the other slots, for the digipeat or ack that follows.
pub const AFTER_TX_MS: u32 = 1_000;

/// What the engine is told each tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cfg {
    /// The watch set, a bit per slot (the active slot is always in).
    pub set: u8,
    pub active: u8,
    /// Each slot's frequency now (for its chip and relay state).
    pub freq: [Freq10; SLOTS],
    /// Slots not heard, the active one too: quick mute (HOME-15). They still listen.
    pub muted: u8,
    /// Slots not listened to at all, the active one too: turned off (HOME-15.1, WATCH-2.1), the broadcast slot
    /// (BCR-23).
    pub off: u8,
    /// Slots whose squelch is held open (a latched monitor, SQL-14).
    pub held: u8,
    /// Slots with receive priority on their chip (APRS-21.1, decision 181: C while APRS mode or KISS hold it).
    pub prio: u8,
    /// The APRS slot, a bit (C while APRS mode, KISS or beacons hold it): other slots on its frequency defer to it
    /// (APRS-21.2).
    pub aprs: u8,
    /// Slots running a range scan (SCAN-21): the active slot's keeps its chip from the other slots on it, and the
    /// receive path with it but for the other chip's short turns ([`SCAN_TURN_MS`]). A watched slot's scan takes
    /// its turns as before.
    pub scan: u8,
    /// WATCH-17.3 (decision 295): APRS first: a priority slot takes [`APRS_FIRST_MS`] turns and the others
    /// [`LOOK_MS`] looks, the priority slot's turn after each look on a chip they share. Off: Voice first
    /// (WATCH-17.2's turns).
    pub first: bool,
    /// TERM-29.4 (a bench test, the fox-hunt measurement): slots received on the other band's chip, a bit each.
    pub swap: u8,
}

/// What to do now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Out {
    /// The slot each chip listens to.
    pub chip: [Option<u8>; 2],
    /// Whose audio is on the speaker.
    pub audio: Option<u8>,
    /// WATCH-11: the set needs both relay states; only the active slot is listened to.
    pub cancelled: bool,
    /// Paused (keyed or [`Watch::pause`]).
    pub paused: bool,
    /// The priority slots in the turn (APRS-21.1), for [`Hop`]'s dwell.
    pub prio: u8,
    /// The chip a priority slot holds with its squelch open: the receive path stays on it ([`Hop`]).
    pub hold: Option<u8>,
    /// The chip on a slot running a range scan (SCAN-21), for [`Hop`]'s turns.
    pub scan: Option<u8>,
    /// Slots deferring to the APRS slot (APRS-21.2): no chip of their own, their squelch the APRS slot's.
    pub defer: u8,
    /// The slot they defer to.
    pub defer_to: Option<u8>,
    /// WATCH-17.3: APRS first ([`Cfg::first`]), for [`Hop`]'s dwell.
    pub first: bool,
    /// WATCH-17.5: each chip's squelch rising, as [`Watch::rising`] last set it (for [`Hop`]).
    pub rising: [bool; 2],
    /// WATCH-16.2: watched slots left out this tick by a key-up or a relay cancel (not by turns on a shared chip).
    pub dropped: u8,
}

/// A slot's turn on a shared chip, or of the receive path on its chip: the active slot's, a priority slot's
/// (APRS-21.1), or another watched one's. `beside`: a priority slot has the other chip, so the active slot's turn
/// is a watched one's (WATCH-17.2, decision 279) and the priority slot is never deaf longer than
/// [`WATCHED_DWELL_MS`]; that chip stays tuned between turns (APRS-16), so the shorter turn costs no retune. Sharing
/// the priority slot's chip, the active slot keeps [`ACTIVE_DWELL_MS`]: each turn there starts with a retune.
///
/// WATCH-17.3 (decision 295): APRS first (`first`), with a priority slot in the watch, it takes [`APRS_FIRST_MS`]
/// and every other slot [`LOOK_MS`], wherever they are.
pub fn dwell(slot: Option<u8>, active: u8, prio: u8, beside: bool, first: bool) -> u32 {
    match slot {
        Some(s) if first && prio & 1 << s != 0 => APRS_FIRST_MS,
        Some(_) if first && prio != 0 => LOOK_MS,
        Some(s) if prio & 1 << s != 0 => PRIO_DWELL_MS,
        Some(s) if s == active && !beside => ACTIVE_DWELL_MS,
        _ => WATCHED_DWELL_MS,
    }
}

/// The engine (56 bytes).
#[derive(Clone, Copy, Debug, Default)]
pub struct Watch {
    on: [Option<u8>; 2],
    since: [u32; 2],
    /// Squelch open, a bit per slot, as last heard.
    open: u8,
    closed_at: [u32; SLOTS],
    /// Closed within [`HOLD_MS`], a bit per slot.
    recent: u8,
    pause_until: Option<u32>,
    /// Each chip listened to at the last turn, and since when (its [`REACQUIRE_MS`]).
    listening: [bool; 2],
    got: [u32; 2],
    /// WATCH-17.3: the last slot other than the priority one that looked on each chip (APRS first goes on from it).
    last: [u8; 2],
    /// APRS-21.4: when the APRS slot's last data frame ended ([`Watch::after_tx`]).
    after_tx: Option<u32>,
    /// WATCH-17.5: each chip's squelch rising ([`Watch::rising`]).
    rising: [bool; 2],
}

fn after(now: u32, t: u32) -> bool {
    now.wrapping_sub(t) < 1 << 31
}

impl Watch {
    pub const fn new() -> Self {
        Watch { on: [None; 2], since: [0; 2], open: 0, closed_at: [0; SLOTS], recent: 0, pause_until: None, listening: [false; 2], got: [0; 2], last: [0; 2], after_tx: None, rising: [false; 2] }
    }

    /// Pause for `secs` (WATCH-6, WATCH-16); 0 resumes now.
    /// APRS-21.4: a data frame from the APRS slot just ended at `now`: it keeps the chip [`AFTER_TX_MS`].
    pub fn after_tx(&mut self, now: u32) {
        self.after_tx = Some(now);
    }

    pub fn pause(&mut self, now: u32, secs: u16) {
        self.pause_until = (secs > 0).then(|| now.wrapping_add(secs as u32 * 1000));
    }

    /// Squelch open on a slot, as last heard.
    pub fn open(&self, slot: u8) -> bool {
        self.open & 1 << slot != 0
    }

    /// WATCH-17.5: each chip's squelch is rising: its own bit reads open and the debounce before it opens
    /// (SQL-18.7) is still counting. Such a chip keeps its slot and the receive path past the dwell, as stock never
    /// hops while the bit reads open: the bit comes some 48 ms after a retune and the count needs 30 ms more, past
    /// a watched slot's 70 ms turn, so without this a watched slot never opened (black, Nov 21, 2026). It lasts at
    /// most the count, so it can't hold a chip for long. The Radio task sets it before each [`Watch::turn`].
    pub fn rising(&mut self, r: [bool; 2]) {
        self.rising = r;
    }

    /// A tick: `sq` is each chip's squelch since the last one (for the slot it was on), `keyed` any slot
    /// transmitting.
    pub fn tick(&mut self, now: u32, c: &Cfg, sq: [bool; 2], keyed: bool) -> Out {
        self.turn(now, c, sq, [true; 2], keyed)
    }

    /// [`Watch::tick`] where only the chips in `listened` were heard since the last turn (the Radio task's: the
    /// one whose receive path [`Hop`] selected). A chip not listened to tells nothing, so its slot keeps its last
    /// squelch state: a muted active slot (HOME-15) receiving on one band while the hop gives the other band's
    /// watched slot its dwell reads RX muted throughout, not Muted on every hop (the two-radio run, Oct 15, 2026).
    pub fn turn(&mut self, now: u32, c: &Cfg, sq: [bool; 2], listened: [bool; 2], keyed: bool) -> Out {
        for (n, s) in self.on.into_iter().enumerate() {
            let Some(s) = s else { continue };
            if !listened[n] {
                self.listening[n] = false;
                continue;
            }
            if !self.listening[n] {
                (self.listening[n], self.got[n]) = (true, now);
            }
            if self.open(s) && !sq[n] && now.wrapping_sub(self.got[n]) < REACQUIRE_MS {
                continue; // still reacquiring: no news yet
            }
            if self.open(s) && !sq[n] && c.prio & 1 << s == 0 {
                // A priority slot has no hold after it closes (APRS-21.1: the packet ended).
                self.closed_at[s as usize] = now;
                self.recent |= 1 << s;
            }
            self.open = self.open & !(1 << s) | (sq[n] as u8) << s;
        }
        for s in 0..SLOTS {
            if now.wrapping_sub(self.closed_at[s]) >= HOLD_MS {
                self.recent &= !(1 << s);
            }
        }
        let mut out = Out::default();
        let active = c.active % SLOTS as u8;
        let relay = |s: usize| Sel::rx(c.freq[s]).relay();
        let mut set = (c.set | 1 << active) & !c.off;
        let watched = set;
        // The relay state the set is held to: the active slot's, or released when the active slot is off (BCR-15).
        let base = set & 1 << active != 0 && relay(active as usize);
        if (0..SLOTS).any(|s| set & 1 << s != 0 && relay(s) != base) {
            (set, out.cancelled) = (1 << active & !c.off, true);
        }
        if self.pause_until.is_some_and(|t| after(now, t)) {
            self.pause_until = None;
        }
        if keyed {
            (set, out.paused) = (1 << active & !c.off, true);
        } else if self.pause_until.is_some() {
            // WATCH-16.1: the pause after a transmission (or #) keeps the APRS slot listening, so APRS isn't deaf
            // for its 30 s
            (set, out.paused) = ((1 << active | c.aprs & set) & !c.off, true);
        }
        out.dropped = watched & !set;
        self.open &= set;
        let prio = c.prio & set;
        let after = !keyed && self.after_tx.is_some_and(|t| now.wrapping_sub(t) < AFTER_TX_MS);
        if !after && !keyed {
            self.after_tx = None;
        }
        out.prio = prio;
        // APRS-21.2: slots on the listened APRS slot's frequency take no turns (a latched monitor keeps its own).
        let to = (0..SLOTS as u8).find(|&x| c.aprs & set & 1 << x != 0);
        let defer = to.map_or(0, |p| {
            (0..SLOTS as u8)
                .filter(|&s| s != p && set & !c.held & 1 << s != 0 && c.freq[s as usize] == c.freq[p as usize])
                .fold(0u8, |m, s| m | 1 << s)
        });
        (out.defer, out.defer_to) = (defer, to.filter(|_| defer != 0));
        let listen = set & !defer;
        for n in 0..2u8 {
            // SQL-14, WATCH-3.2: a latched monitor holds its slot open for the turn, as stock's squelch 0 never hops.
            let open = self.open | c.held & set;
            let on_chip = |s: u8| listen & 1 << s != 0 && band(c.freq[s as usize]).1 ^ (c.swap >> s & 1) == n;
            // SCAN-21 (decision 189): a chip with a scanning slot stays on it; only a priority slot (APRS-21.1)
            // still takes its turns there.
            // Only the active slot's scan: a watched slot's scan keeps taking turns, never the active slot's chip.
            let scanning = (0..SLOTS as u8).find(|&x| c.scan & 1 << active & 1 << x != 0 && on_chip(x));
            let mine = |s: u8| on_chip(s) && scanning.is_none_or(|x| x == s || prio & 1 << s != 0);
            let cur = self.on[n as usize].filter(|&s| mine(s));
            // APRS-21.1: the priority slot sharing this chip, if any.
            let p = (0..SLOTS as u8).find(|&x| prio & 1 << x != 0 && mine(x));
            if let Some(s) = cur.filter(|_| prio != 0 && !listened[n as usize]) {
                // APRS-21.1, WATCH-17.2: with a priority slot in the watch, turns on either chip count only while
                // the chip has the receive path, so each is listened to whole (the Hop goes round this chip's
                // slots, then to the other chip); a turn on the other chip doesn't run out while C has the path.
                if open & 1 << s == 0 {
                    self.since[n as usize] = now;
                }
            }
            let next = match cur {
                // APRS-21.4: just after our own frame the priority slot has the chip, whatever was on it
                _ if after && p.is_some() => p,
                // APRS-21.1: an open priority slot keeps its chip until it closes, muted or not (its decoder).
                Some(s) if p == Some(s) && open & 1 << s != 0 => Some(s),
                // APRS-21.1: a slot open on the priority slot's frequency hands it the carrier.
                Some(s) if open & 1 << s != 0 && p.is_some_and(|p| p != s && c.freq[p as usize] == c.freq[s as usize]) => {
                    self.open &= !(1 << s);
                    p
                }
                // Open, or closed within the hold: it keeps the chip (a muted slot only for its dwell, HOME-15). The
                // hold doesn't keep a priority slot from its turns (APRS-21.1).
                Some(s) if open & !c.muted & 1 << s != 0 => Some(s),
                Some(s) if self.recent & !c.muted & 1 << s != 0 && p.is_none() => Some(s),
                // WATCH-17.5: its squelch rising: the turn waits for the count
                Some(s) if self.rising[n as usize] && listened[n as usize] && c.muted & 1 << s == 0 => Some(s),
                Some(s) => {
                    if now.wrapping_sub(self.since[n as usize]) < dwell(Some(s), active, prio, p.is_none() && prio & listen != 0, c.first) {
                        Some(s)
                    } else if let (true, Some(p)) = (c.first, p) {
                        // WATCH-17.3: APRS first: the priority slot after every look; after its turn, the next slot
                        // round from the last one that looked here.
                        if s != p {
                            Some(p)
                        } else {
                            let l = self.last[n as usize];
                            (1..=SLOTS as u8).map(|i| (l + i) % SLOTS as u8).find(|&x| x != p && mine(x)).or(Some(p))
                        }
                    } else {
                        (1..=SLOTS as u8).map(|i| (s + i) % SLOTS as u8).find(|&x| mine(x))
                    }
                }
                None => (0..SLOTS as u8).map(|i| (active + i) % SLOTS as u8).find(|&x| mine(x)),
            };
            if next != self.on[n as usize] {
                self.since[n as usize] = now;
                self.got[n as usize] = now; // retuned to another slot: reacquiring
                if let Some(x) = next.filter(|&x| prio & 1 << x == 0) {
                    self.last[n as usize] = x;
                }
            }
            self.on[n as usize] = next;
        }
        if let Some(p) = to {
            // APRS-21.2: a deferring slot's squelch is the APRS slot's.
            let open = (self.open >> p & 1) * defer;
            self.open = self.open & !defer | open;
        }
        out.chip = self.on;
        out.first = c.first;
        out.rising = self.rising;
        out.scan = (0..2u8).find(|&n| self.on[n as usize].is_some_and(|s| s == active && c.scan & 1 << s != 0));
        let open = self.open | c.held & set;
        // APRS-21.4: held as if open while the window lasts, so the path stays on it and its decoder hears it
        let holding = open | if after { prio } else { 0 };
        out.hold = (0..2u8).find(|&n| self.on[n as usize].is_some_and(|s| prio & holding & 1 << s != 0));
        let heard = |s: u8| open & 1 << s != 0 && c.muted & 1 << s == 0;
        // APRS-21.2: a deferring slot heard plays the APRS slot's chip.
        let via = |s: u8| if defer & 1 << s != 0 { to.unwrap_or(s) } else { s };
        out.audio = if heard(active) { Some(active) } else { (0..SLOTS as u8).find(|&s| s != active && heard(s)) }.map(via);
        out
    }
}

/// WATCH-2, PWR-7 (decision 111): which chips stay awake after a watch turn. One BK4829 at a time, as stock: the
/// chip whose receive path [`Hop`] selected; the other sleeps until the hop comes back to it. A chip on a slot in
/// `keep` (APRS mode's C, APRS-16) stays awake beside it. A chip with no slot sleeps.
pub fn awake(out: &Out, path: Option<u8>, keep: u8) -> [bool; 2] {
    let on = |n: usize| out.chip[n].is_some_and(|s| path == Some(n as u8) || keep & 1 << s != 0);
    [on(0), on(1)]
}

/// BCR-24: the broadcast comes back this long after the last amateur slot heard goes quiet.
pub const BCAST_RETURN_MS: u32 = 2_000;

/// The broadcast's turn on the speaker (BCR-24): any amateur slot heard takes it at once and mutes the broadcast,
/// which comes back [`BCAST_RETURN_MS`] after that slot's audio ends (its squelch tail); a new signal meanwhile
/// starts the wait again.
#[derive(Clone, Copy, Debug, Default)]
pub struct BcastTurn {
    last: Option<u32>,
}

impl BcastTurn {
    pub const fn new() -> Self {
        BcastTurn { last: None }
    }

    /// `slot_heard`: an amateur slot's audio is on the speaker now. True when the broadcast plays.
    pub fn tick(&mut self, now: u32, slot_heard: bool) -> bool {
        if slot_heard {
            self.last = Some(now);
            return false;
        }
        match self.last {
            Some(t) if now.wrapping_sub(t) < BCAST_RETURN_MS => false,
            _ => {
                self.last = None;
                true
            }
        }
    }
}

/// SCAN-21 (decision 189): chip `c`'s turn on the receive path while it scans and the other chip's slot has no
/// priority: [`SCAN_TURN_MS`]. None otherwise (the ordinary dwell).
/// WATCH-17.2: the other chip has a priority slot (the path's turn on chip `c` is the short one).
fn beside(out: &Out, c: u8) -> bool {
    out.chip[(c ^ 1) as usize].is_some_and(|x| out.prio & 1 << x != 0)
}

fn scan_dwell(out: &Out, c: u8) -> Option<u32> {
    let other = out.chip[(c ^ 1) as usize];
    (out.scan == Some(c) && !other.is_some_and(|s| out.prio & 1 << s != 0)).then_some(SCAN_TURN_MS)
}

/// The receive path select (the RF path machine's one RX select, SAFE-11) between the two chips while both have a
/// slot: one band's front end at a time, as stock's dual watch hops. The chip with the slot being heard keeps it;
/// otherwise it alternates, each chip for its slot's dwell. A chip without its path is taken as hearing nothing
/// (to confirm on the bench: if the other band's front end leaks enough, both could listen at once).
#[derive(Clone, Copy, Debug, Default)]
pub struct Hop {
    cur: Option<u8>,
    since: u32,
    /// The slot on the selected chip at the last call.
    slot: Option<u8>,
}

impl Hop {
    pub const fn new() -> Self {
        Hop { cur: None, since: 0, slot: None }
    }

    /// The chip whose path is selected now.
    pub fn chip(&self) -> Option<u8> {
        self.cur
    }

    /// The chip to select after a [`Watch::tick`] that gave `out`; `active` and `freq` as in [`Cfg`].
    pub fn next(&mut self, now: u32, out: &Out, active: u8) -> Option<u8> {
        let has = |n: u8| out.chip[n as usize].is_some();
        let next = match (has(0), has(1)) {
            (false, false) => None,
            (true, false) => Some(0),
            (false, true) => Some(1),
            (true, true) => {
                // APRS-21.1: an open priority slot holds the path first, then the chip being heard.
                let heard = out.hold.or_else(|| out.audio.and_then(|s| (0..2u8).find(|&n| out.chip[n as usize] == Some(s))));
                match (heard, self.cur) {
                    (Some(n), _) => Some(n),
                    // WATCH-17.5: the chip whose squelch is rising keeps the path
                    (None, Some(c)) if out.rising[c as usize] => Some(c),
                    (None, Some(c)) => {
                        let s = out.chip[c as usize];
                        let was_prio = self.slot.is_some_and(|x| out.prio & 1 << x != 0);
                        if s != self.slot && was_prio {
                            // APRS-21.1: the priority slot's turn on this chip is over: the other chip's turn.
                            Some(c ^ 1)
                        } else {
                            if s != self.slot && s.is_some_and(|x| out.prio & 1 << x != 0) {
                                self.since = now; // the priority slot's turn on the path, whole
                            }
                            let dwell = scan_dwell(out, c).unwrap_or_else(|| dwell(s, active, out.prio, beside(out, c), out.first));
                            Some(if now.wrapping_sub(self.since) < dwell { c } else { c ^ 1 })
                        }
                    }
                    (None, None) => out.chip.iter().position(|&s| s == Some(active)).map_or(Some(0), |n| Some(n as u8)),
                }
            }
        };
        if next != self.cur {
            (self.cur, self.since) = (next, now);
        }
        self.slot = next.and_then(|n| out.chip[n as usize]);
        next
    }

    /// PWR-21: the chip the path turns to within `within` ms if nothing changes (both chips have a slot, neither
    /// is heard, the current dwell ends by then), after a [`Hop::next`] with the same `out`. The Radio task turns
    /// that chip's supplies on a tick early, so their settle overlaps the dwell instead of delaying the hop.
    pub fn due(&self, now: u32, out: &Out, active: u8, within: u32) -> Option<u8> {
        let c = self.cur?;
        if out.chip.iter().any(Option::is_none) || out.hold.is_some() || out.rising[c as usize] || out.audio.is_some_and(|s| out.chip.contains(&Some(s))) {
            return None;
        }
        let dwell = scan_dwell(out, c).unwrap_or_else(|| dwell(out.chip[c as usize], active, out.prio, beside(out, c), out.first));
        (now.wrapping_sub(self.since).saturating_add(within) >= dwell).then_some(c ^ 1)
    }
}

/// WATCH-17.1 (decision 262): each slot's whole turns on a chip and the ms they lasted, since boot or a reset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Turns {
    pub n: [u32; SLOTS],
    pub ms: [u32; SLOTS],
    /// WATCH-17.4: each slot's frequency steps inside its turns (a scan step, a priority look's channel, a tune):
    /// how many, and the ms each frequency was held before the next, so `step_ms / steps` is a scan's time per
    /// channel.
    pub steps: [u32; SLOTS],
    pub step_ms: [u32; SLOTS],
    /// WATCH-17.4: the chips' retunes to a frequency (not a sleep): how many, and how long each took from its first
    /// register write to receive on, the settle wait included (us): the sum, and the longest.
    pub tunes: u32,
    pub tune_us: u32,
    pub tune_max_us: u32,
}

/// WATCH-17.1 (decision 262, a bench measure): the turns the chips actually took, fed by the Radio task where it
/// retunes a chip (`tune_start`): a turn runs from the retune to a slot until the chip is retuned to another slot
/// or put to sleep. A retune to the same slot (a new frequency) continues its turn; other owners (the scope, HF
/// mode: slot numbers past [`SLOTS`]) only end one. Only whole turns count, so the mean of two reads' difference
/// is exact whatever the reads' timing. The bench builds' (`bk turns`, 40 bytes).
#[derive(Clone, Copy, Debug, Default)]
pub struct TurnClock {
    /// Each chip's slot, when its turn began, its frequency and when that frequency began.
    on: [Option<(u8, u32, Freq10, u32)>; 2],
    t: Turns,
}

impl TurnClock {
    pub const fn new() -> Self {
        TurnClock { on: [None; 2], t: Turns { n: [0; SLOTS], ms: [0; SLOTS], steps: [0; SLOTS], step_ms: [0; SLOTS], tunes: 0, tune_us: 0, tune_max_us: 0 } }
    }

    /// Chip `chip` retuned at `now` to `want`, a slot and its frequency (None: asleep).
    pub fn tuned(&mut self, now: u32, chip: u8, want: Option<(u8, Freq10)>) {
        let on = &mut self.on[chip as usize & 1];
        match (*on, want) {
            (Some((s, _, f, _)), Some((w, g))) if s == w && f == g => {}
            // WATCH-17.4: a new frequency on the same slot continues its turn and ends a step
            (Some((s, since, _, at)), Some((w, g))) if s == w => {
                if (s as usize) < SLOTS {
                    self.t.steps[s as usize] += 1;
                    self.t.step_ms[s as usize] += now.wrapping_sub(at);
                }
                *on = Some((s, since, g, now));
            }
            _ => {
                if let Some((s, since, _, _)) = *on {
                    if (s as usize) < SLOTS {
                        self.t.n[s as usize] += 1;
                        self.t.ms[s as usize] += now.wrapping_sub(since);
                    }
                }
                *on = want.map(|(s, f)| (s, now, f, now));
            }
        }
    }

    /// WATCH-17.4: a retune to a frequency took `us`, its settle wait included.
    pub fn tune_took(&mut self, us: u32) {
        self.t.tunes += 1;
        self.t.tune_us = self.t.tune_us.saturating_add(us);
        self.t.tune_max_us = self.t.tune_max_us.max(us);
    }

    /// The counts so far; `reset` zeroes them after (a turn in progress still counts whole when it ends).
    pub fn take(&mut self, reset: bool) -> Turns {
        let t = self.t;
        if reset {
            self.t = Turns::default();
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::freq::mhz;

    /// WATCH-17.1, WATCH-17: A (146.55, active) and B (147.00) both on chip 0, watch A+B, nothing heard, in the
    /// Radio task's 10 ms ticks: the turn clock fed at each retune counts turns of 150 ms on A and 70 ms on B, C
    /// none; the difference of two reads gives the same means, and a reset keeps the turn in progress whole.
    /// WATCH-17.4: frequency steps inside a turn are counted and timed, and so are the retunes.
    #[test]
    fn turn_clock_times_the_turns() {
        let c = Cfg { set: 0b011, active: 0, freq: [mhz(146) + 55_000, mhz(147), mhz(446) + 10_000], muted: 0, off: 0, held: 0, prio: 0, aprs: 0, scan: 0, first: false, swap: 0 };
        let (mut w, mut clock) = (Watch::new(), TurnClock::new());
        let mut run = |from: u32, to: u32, clock: &mut TurnClock| {
            for t in (from..to).step_by(10) {
                let out = w.tick(t, &c, [false; 2], false);
                assert_eq!(out.chip[1], None, "B shares chip 0");
                for n in 0..2u8 {
                    clock.tuned(t, n, out.chip[n as usize].map(|s| (s, c.freq[s as usize])));
                }
            }
        };
        run(0, 2_000, &mut clock);
        let t0 = clock.take(false);
        run(2_000, 8_000, &mut clock);
        let t1 = clock.take(true);
        assert_eq!(t1.n[2], 0, "C isn't watched");
        let mean = |s: usize| (t1.ms[s] - t0.ms[s]) / (t1.n[s] - t0.n[s]);
        assert!(t1.n[0] - t0.n[0] >= 25 && t1.n[1] - t0.n[1] >= 25, "{t0:?} {t1:?}");
        assert_eq!((mean(0), mean(1)), (ACTIVE_DWELL_MS, WATCHED_DWELL_MS));
        assert_eq!(t1.ms[0] / t1.n[0], ACTIVE_DWELL_MS, "since boot too");
        assert_eq!(clock.take(false), Turns::default(), "reset");
        run(8_000, 8_500, &mut clock);
        let t2 = clock.take(false);
        assert!(t2.n[0] > 0 && t2.n[1] > 0);
        assert_eq!((t2.ms[0] / t2.n[0], t2.ms[1] / t2.n[1]), (ACTIVE_DWELL_MS, WATCHED_DWELL_MS), "whole turns after a reset");
        // A new frequency on the same slot continues its turn; sleep or another owner ends it.
        // WATCH-17.4: each new frequency is a step, timed from the last; the retunes' times add up.
        let mut k = TurnClock::new();
        k.tuned(0, 0, Some((0, 1)));
        k.tuned(20, 0, Some((0, 1)));
        k.tuned(50, 0, Some((0, 2)));
        k.tuned(65, 0, Some((0, 3)));
        k.tuned(100, 0, Some((9, 1)));
        k.tuned(130, 0, None);
        k.tune_took(4_000);
        k.tune_took(2_500);
        let want = Turns { n: [1, 0, 0], ms: [100, 0, 0], steps: [2, 0, 0], step_ms: [65, 0, 0], tunes: 2, tune_us: 6_500, tune_max_us: 4_000 };
        assert_eq!(k.take(false), want);
    }

    /// TERM-29.4 (the fox-hunt measurement): a slot in `swap` is put on the other band's chip: A's 2 m on chip 1,
    /// so B (70 cm) and A share it while C (airband) has chip 0 alone.
    #[test]
    fn swap_puts_a_slot_on_the_other_chip() {
        let mut w = Watch::new();
        let out = w.turn(0, &cfg(), [false; 2], [true; 2], false);
        assert!(out.chip[0] == Some(0) || out.chip[0] == Some(2), "2 m and airband on chip 0: {:?}", out.chip);
        let c = Cfg { swap: 0b001, ..cfg() };
        let mut seen = [[false; 3]; 2];
        for t in (0..4_000).step_by(100) {
            let out = w.turn(t, &c, [false; 2], [true; 2], false);
            for n in 0..2 {
                if let Some(s) = out.chip[n] {
                    seen[n][s as usize] = true;
                }
            }
        }
        assert!(seen[1][0], "A on chip 1");
        assert!(!seen[0][0], "never on chip 0");
        assert!(seen[0][2] && seen[1][1], "C on chip 0, B on chip 1");
    }

    /// A on 2 m and C on airband share chip 0; B on 70 cm has chip 1.
    fn cfg() -> Cfg {
        Cfg { set: 0b111, active: 0, freq: [mhz(146) + 52_000, mhz(443) + 47_500, mhz(118) + 95_000], muted: 0, off: 0, held: 0, prio: 0, aprs: 0, scan: 0, first: false, swap: 0 }
    }

    /// APRS-21.4: after our own data frame the APRS slot keeps the shared chip and the path (held) for
    /// AFTER_TX_MS with no looks at the others; then the turns go on.
    #[test]
    fn aprs_slot_held_after_its_frame() {
        let mut w = Watch::new();
        let c = Cfg { set: 0b101, freq: [mhz(146) + 52_000, mhz(443), mhz(144) + 39_000], aprs: 0b100, prio: 0b100, first: true, ..cfg() };
        // into the turns until A has a look
        let mut t = 0;
        while w.tick(t, &c, [false; 2], false).chip[0] != Some(0) {
            t += 10;
            assert!(t < 5_000);
        }
        w.tick(t + 10, &c, [false; 2], true); // keyed (our frame)
        let end = t + 500;
        w.after_tx(end);
        for k in (end..end + AFTER_TX_MS).step_by(10) {
            let o = w.tick(k, &c, [false; 2], false);
            assert_eq!((o.chip[0], o.hold), (Some(2), Some(0)), "{} ms after", k - end);
        }
        let looked = (end + AFTER_TX_MS..end + AFTER_TX_MS + 2_000).step_by(10).any(|k| w.tick(k, &c, [false; 2], false).chip[0] == Some(0));
        assert!(looked, "the turns go on after the window");
    }

    /// WATCH-16.1: a pause keeps the APRS slot listening, on its own chip or taking its turns beside the active
    /// slot; while keyed only the active slot has a chip.
    #[test]
    fn pause_keeps_the_aprs_slot() {
        for fc in [mhz(144) + 39_000, mhz(445)] {
            let mut w = Watch::new();
            let c = Cfg { set: 0b111, freq: [mhz(146) + 52_000, mhz(147), fc], aprs: 0b100, prio: 0b100, first: true, ..cfg() };
            w.pause(0, 30);
            let (mut on_b, mut on_c) = (0, 0);
            for t in (0..10_000).step_by(10) {
                let o = w.tick(t, &c, [false; 2], false);
                assert!(o.paused);
                on_b += o.chip.contains(&Some(1)) as u32;
                on_c += o.chip.contains(&Some(2)) as u32;
            }
            assert!(on_c > 300 && on_b == 0, "C {on_c} ticks, B {on_b}");
            assert!(!w.tick(10_000, &c, [false; 2], true).chip.contains(&Some(2)), "keyed: the active slot alone");
        }
    }

    /// WATCH-2, WATCH-17: slots sharing a chip take turns on it, 150 ms on the active slot and 70 ms on each
    /// watched one; a slot alone on its chip keeps it.
    #[test]
    fn turns_on_a_shared_chip() {
        let mut w = Watch::new();
        let c = cfg();
        let o = w.tick(0, &c, [false; 2], false);
        assert_eq!(o.chip, [Some(0), Some(1)]);
        assert_eq!(w.tick(149, &c, [false; 2], false).chip[0], Some(0));
        assert_eq!(w.tick(150, &c, [false; 2], false).chip[0], Some(2), "active dwell over");
        assert_eq!(w.tick(219, &c, [false; 2], false).chip[0], Some(2));
        assert_eq!(w.tick(220, &c, [false; 2], false).chip[0], Some(0), "watched dwell over");
        assert_eq!(w.tick(220, &c, [false; 2], false).chip[1], Some(1), "alone on its chip");
    }

    /// Decision 166: a slot whose frequency crosses to the other band (a scan step from 2 m to 70 cm) has that
    /// band's chip in the turn that is given the new frequency, and the old chip none; the Radio task steps the
    /// scans before the turn, so the right chip is tuned in the step's own tick.
    #[test]
    fn band_crossing_takes_the_other_chip_at_once() {
        let mut w = Watch::new();
        let mut c = Cfg { set: 0b001, active: 0, freq: [mhz(145) + 49_500, 0, 0], muted: 0, off: 0, held: 0, prio: 0, aprs: 0, scan: 0, first: false, swap: 0 };
        assert_eq!(w.tick(0, &c, [false; 2], false).chip, [Some(0), None]);
        c.freq[0] = mhz(446) + 47_500;
        assert_eq!(w.tick(10, &c, [false; 2], false).chip, [None, Some(0)], "70 cm: chip 1 in the same turn");
        let mut h = Hop::new();
        let o = w.tick(20, &c, [false; 2], false);
        assert_eq!(h.next(20, &o, 0), Some(1), "and the path with it");
    }

    #[test]
    fn open_slot_keeps_the_chip_and_audio_priority() {
        let mut w = Watch::new();
        let c = cfg();
        w.tick(0, &c, [false; 2], false);
        w.tick(150, &c, [false; 2], false); // C on chip 0
        let o = w.tick(200, &c, [true, true], false); // C and B open
        assert_eq!(o.chip[0], Some(2));
        assert_eq!(o.audio, Some(1), "B first among the watched");
        let o = w.tick(5_000, &c, [true, true], false);
        assert_eq!(o.chip[0], Some(2), "still open: stays");
        let o = w.tick(5_100, &c, [false, true], false);
        assert_eq!(o.chip[0], Some(2), "closed, within the hold");
        assert_eq!(w.tick(7_100, &c, [false, true], false).chip[0], Some(0), "hold over");
        // A opens: its audio wins over B's.
        assert_eq!(w.tick(7_200, &c, [true, true], false).audio, Some(0));
        let o = w.tick(7_300, &Cfg { muted: 0b010, ..c }, [false, true], false);
        assert_eq!(o.audio, None, "B muted, A closed");
    }

    /// HOME-15, WATCH-3.3, WATCH-3.4: a muted active slot is never heard and takes no audio priority: a watched slot
    /// open beside it plays. On a shared chip it keeps only its dwell, so the slot sharing it still gets turns; it
    /// still listens (its squelch is reported as open).
    #[test]
    fn muted_slot_never_blocks() {
        let mut w = Watch::new();
        let c = Cfg { muted: 0b001, ..cfg() };
        w.tick(0, &c, [false; 2], false);
        let o = w.tick(100, &c, [true, true], false); // A (chip 0) and B (chip 1) open
        assert!(w.open(0), "A still receives");
        assert_eq!(o.audio, Some(1), "A muted: B is heard");
        let o = w.tick(150, &c, [true, true], false);
        assert_eq!(o.chip[0], Some(2), "A open but muted: C gets its turn on chip 0");
        let o = w.tick(200, &c, [false, false], false);
        assert_eq!(o.audio, None, "B closed, A muted: nothing");
        // Unmuted, A's open squelch keeps the chip and the speaker again.
        let c = cfg();
        w.tick(220, &c, [false, false], false); // back to A
        let o = w.tick(300, &c, [true, true], false);
        assert_eq!((o.chip[0], o.audio), (Some(0), Some(0)));
        assert_eq!(w.tick(400, &c, [true, true], false).chip[0], Some(0), "open, unmuted: keeps it");
    }

    #[test]
    fn paused_while_keyed_and_by_time() {
        let mut w = Watch::new();
        let c = cfg();
        let o = w.tick(0, &c, [false; 2], true);
        assert!(o.paused);
        assert_eq!(o.chip, [Some(0), None]);
        w.pause(0, 30);
        assert!(w.tick(29_999, &c, [false; 2], false).paused);
        let o = w.tick(30_000, &c, [false; 2], false);
        assert!(!o.paused && o.chip[1] == Some(1));
        w.pause(31_000, 60);
        w.pause(32_000, 0);
        assert!(!w.tick(32_000, &c, [false; 2], false).paused, "a key resumes");
    }

    /// WATCH-11.1: a watch set needing different relay states cancels watch until the set agrees again.
    #[test]
    fn relay_mismatch_cancels() {
        let mut w = Watch::new();
        let c = Cfg { freq: [mhz(146), mhz(28) + 40_000, mhz(446)], ..cfg() };
        let o = w.tick(0, &c, [false; 2], false);
        assert!(o.cancelled);
        assert_eq!(o.chip, [Some(0), None]);
        let o = w.tick(100, &Cfg { set: 0b101, ..c }, [false; 2], false);
        assert!(!o.cancelled && o.chip[1] == Some(2), "resumes once the set agrees");
    }

    /// WATCH-5 (decision 81): a watched slot that is heard plays while the active one is closed; the engine
    /// never asks for another active slot (it has no such output), and B's audio stops when A opens.
    #[test]
    fn heard_slot_plays_active_stays() {
        let mut w = Watch::new();
        let c = cfg();
        w.tick(0, &c, [false; 2], false);
        let o = w.tick(100, &c, [false, true], false);
        assert_eq!((o.chip[1], o.audio), (Some(1), Some(1)));
        let o = w.tick(200, &c, [true, true], false);
        assert_eq!(o.audio, Some(0), "the active slot's audio first (WATCH-3)");
    }

    /// The path hops between the two chips at their dwells, and stays with the chip being heard.
    #[test]
    fn hop_between_chips() {
        let mut h = Hop::new();
        let both = Out { chip: [Some(0), Some(1)], ..Out::default() };
        assert_eq!(h.next(0, &both, 0), Some(0), "the active slot's chip first");
        assert_eq!(h.next(149, &both, 0), Some(0));
        assert_eq!(h.next(150, &both, 0), Some(1), "active dwell over");
        assert_eq!(h.next(219, &both, 0), Some(1));
        assert_eq!(h.next(220, &both, 0), Some(0), "watched dwell over");
        let heard = Out { audio: Some(1), ..both };
        assert_eq!(h.next(5_000, &heard, 0), Some(1), "B heard: its chip keeps the path");
        assert_eq!(h.next(9_000, &heard, 0), Some(1));
        let one = Out { chip: [None, Some(1)], ..Out::default() };
        assert_eq!(h.next(9_010, &one, 1), Some(1));
        assert_eq!(h.next(9_020, &Out::default(), 0), None);
    }

    /// WATCH-17.5: a chip whose squelch is rising (its bit open, the debounce counting) keeps the path past its
    /// dwell, and isn't named due; once it clears the hop goes on.
    #[test]
    fn hop_waits_for_a_rising_squelch() {
        let mut h = Hop::new();
        let both = Out { chip: [Some(0), Some(1)], ..Out::default() };
        h.next(0, &both, 0);
        h.next(150, &both, 0); // B's watched turn on chip 1
        let rising = Out { rising: [false, true], ..both };
        assert_eq!(h.due(215, &rising, 0, 10), None, "not due while rising");
        assert_eq!(h.next(220, &rising, 0), Some(1), "past the 70 ms dwell: kept");
        assert_eq!(h.next(250, &rising, 0), Some(1));
        assert_eq!(h.next(260, &both, 0), Some(0), "cleared: the hop goes on");
    }

    /// WATCH-17.5: slots sharing a chip: the watched slot whose squelch is rising keeps the chip past its dwell; a
    /// muted one doesn't.
    #[test]
    fn shared_chip_waits_for_a_rising_squelch() {
        let c = Cfg { set: 0b101, ..cfg() }; // A and C, both on chip 0
        for muted in [false, true] {
            let c = Cfg { muted: if muted { 0b100 } else { 0 }, ..c };
            let mut w = Watch::new();
            let mut t = 0;
            while w.turn(t, &c, [false; 2], [true; 2], false).chip[0] != Some(2) {
                t += 10;
                assert!(t < 1_000);
            }
            w.rising([true, false]);
            let o = w.turn(t + WATCHED_DWELL_MS + 20, &c, [false; 2], [true; 2], false);
            assert_eq!(o.chip[0] == Some(2), !muted, "muted {muted}");
            assert_eq!(o.rising, [true, false]);
            w.rising([false; 2]);
            assert_eq!(w.turn(t + WATCHED_DWELL_MS + 30, &c, [false; 2], [true; 2], false).chip[0], Some(0), "cleared");
        }
    }

    /// PWR-21.2: the chip the hop turns to by the next tick is named a tick early, never while a chip is heard or
    /// only one has a slot; in a 10 ms tick run it is named on exactly the tick before each hop.
    #[test]
    fn hop_due_a_tick_early() {
        let mut h = Hop::new();
        let both = Out { chip: [Some(0), Some(1)], ..Out::default() };
        assert_eq!(h.due(0, &both, 0, 10), None, "no path yet");
        h.next(0, &both, 0);
        assert_eq!(h.due(139, &both, 0, 10), None);
        assert_eq!(h.due(140, &both, 0, 10), Some(1), "active dwell (150) over by the next tick");
        h.next(150, &both, 0);
        assert_eq!(h.due(209, &both, 0, 10), None);
        assert_eq!(h.due(210, &both, 0, 10), Some(0), "watched dwell (70)");
        let heard = Out { audio: Some(1), ..both };
        assert_eq!(h.due(210, &heard, 0, 10), None, "B heard: its chip keeps the path");
        let one = Out { chip: [None, Some(1)], ..Out::default() };
        assert_eq!(h.due(210, &one, 1, 10), None, "one chip: no hop");
        // A whole run in 10 ms ticks: each hop was named on the tick before it, and only then.
        let mut h = Hop::new();
        let mut early = None;
        let mut hops = 0;
        for t in (0..3_000).step_by(10) {
            let prev = h.chip();
            let p = h.next(t, &both, 0);
            if prev.is_some() && p != prev {
                assert_eq!(early, p, "the hop at {t} was named a tick early");
                hops += 1;
            } else if prev.is_some() {
                assert_eq!(early, None, "named early before {t}, then no hop");
            }
            early = h.due(t, &both, 0, 10);
        }
        assert!(hops > 20, "{hops}");
    }

    /// WATCH-2, PWR-7 (decision 111): only the hop's chip is awake; APRS mode's C keeps its chip awake beside it.
    #[test]
    fn one_chip_awake() {
        let both = Out { chip: [Some(0), Some(1)], ..Out::default() };
        assert_eq!(awake(&both, Some(0), 0), [true, false]);
        assert_eq!(awake(&both, Some(1), 0), [false, true], "the hop wakes the other chip, this one sleeps");
        let aprs = Out { chip: [Some(2), Some(1)], ..Out::default() };
        assert_eq!(awake(&aprs, Some(1), 0b100), [true, true], "APRS mode's C stays awake");
        assert_eq!(awake(&aprs, Some(0), 0b100), [true, false]);
        let one = Out { chip: [None, Some(1)], ..Out::default() };
        assert_eq!(awake(&one, Some(1), 0b100), [false, true], "no slot: asleep");
        assert_eq!(awake(&Out::default(), None, 0b111), [false, false]);
        // A whole run: A on 2 m (chip 0) and B on 70 cm (chip 1) watched: never both awake.
        let (mut w, mut h) = (Watch::new(), Hop::new());
        let c = Cfg { set: 0b011, ..cfg() };
        for t in (0..2_000).step_by(10) {
            let o = w.tick(t, &c, [false; 2], false);
            let a = awake(&o, h.next(t, &o, 0), 0);
            assert!(!(a[0] && a[1]), "both awake at {t}");
            assert!(a[0] || a[1]);
        }
    }

    /// WATCH-2.1, HOME-15.1: a slot turned off with \* takes no turns: A, B and C all on 2 m (one chip), B off: the
    /// chip alternates A and C only, and B's carrier is never heard; A off as the active slot: B and C take the turns.
    #[test]
    fn turned_off_slots_take_no_turns() {
        let f = [mhz(146) + 52_000, mhz(145) + 50_000, mhz(144) + 39_000];
        for (off, want) in [(0b010u8, 0b101u8), (0b001, 0b110), (0b000, 0b111)] {
            let c = Cfg { set: 0b111, freq: f, off, ..cfg() };
            let (mut w, mut hop) = (Watch::new(), Hop::new());
            let mut had = 0u8;
            for t in (0..5_000u32).step_by(10) {
                let o = w.tick(t, &c, [false; 2], false);
                hop.next(t, &o, 0);
                assert_eq!(o.chip[1], None, "nothing on 70 cm");
                had |= o.chip[0].map_or(0, |s| 1 << s);
                assert!(o.audio.is_none_or(|s| off & 1 << s == 0), "an off slot is never heard");
            }
            assert_eq!(had, want, "off {off:03b}: the slots that had the chip");
        }
        // B off with its squelch open: never on a chip, so never heard; A's audio as before
        let c = Cfg { set: 0b011, freq: f, off: 0b010, ..cfg() };
        let mut w = Watch::new();
        for t in (0..1_000u32).step_by(10) {
            let o = w.tick(t, &c, [true, false], false);
            assert_ne!(o.audio, Some(1));
            assert!(!w.open(1));
        }
    }

    /// BCR-23: an off slot gets no chip and no audio, even as the active slot; the rest are watched as before.
    #[test]
    fn off_slots() {
        let mut w = Watch::new();
        // A (active) is the broadcast slot on 100.5 MHz: B and C keep their chips.
        let c = Cfg { freq: [mhz(100) + 50_000, mhz(443) + 47_500, mhz(146) + 52_000], off: 0b001, ..cfg() };
        let o = w.tick(0, &c, [false, false], false);
        assert_eq!(o.chip, [Some(2), Some(1)]);
        assert!(!o.cancelled);
        let o = w.tick(100, &c, [true, false], false);
        assert_eq!(o.audio, Some(2), "C heard while A is off");
        // Keyed or paused: only the active slot, which is off: nothing.
        let o = w.tick(200, &c, [false, false], true);
        assert_eq!(o.chip, [None, None]);
        // A 6 m slot needing the relay, off while broadcast plays: no cancelling for it.
        let c = Cfg { freq: [mhz(100) + 50_000, mhz(50) + 12_500, mhz(146) + 52_000], off: 0b011, ..cfg() };
        let o = w.tick(5_000, &c, [false, false], false);
        assert!(!o.cancelled && o.chip == [Some(2), None], "{o:?}");
    }

    /// BCR-24: a slot heard mutes the broadcast at once; it returns 2 s after the slot's audio ends, and a new
    /// signal in the wait starts it again.
    #[test]
    fn broadcast_turn() {
        let mut b = BcastTurn::new();
        assert!(b.tick(0, false));
        assert!(!b.tick(100, true));
        assert!(!b.tick(1_000, true));
        assert!(!b.tick(2_900, false), "1.9 s after");
        assert!(!b.tick(2_950, true), "a new signal");
        assert!(!b.tick(4_900, false));
        assert!(b.tick(4_950, false), "2 s after the last");
        assert!(b.tick(9_000, false));
    }

    /// HOME-15, WATCH-2: A (2 m, chip 0) active and muted with a carrier, B (70 cm, chip 1) watched: the hop
    /// alternates the receive path, and A's squelch reads open throughout, also while chip 1 has the path.
    #[test]
    fn muted_slot_stays_open_across_the_hop() {
        let c = Cfg { set: 0b011, active: 0, freq: [mhz(145) + 49_500, mhz(446) + 47_500, 0], muted: 0b001, off: 0, held: 0, prio: 0, aprs: 0, scan: 0, first: false, swap: 0 };
        let (mut w, mut hop) = (Watch::new(), Hop::new());
        let mut path = None;
        let mut paths = [0u32; 2];
        for t in (0..2_000u32).step_by(10) {
            let listened = [path == Some(0), path == Some(1)];
            let sq = [listened[0], false]; // A's carrier, heard only while chip 0 has the path
            let out = w.turn(t, &c, sq, listened, false);
            path = hop.next(t, &out, c.active);
            if let Some(p) = path {
                paths[p as usize] += 1;
            }
            if t >= 100 {
                assert!(w.open(0), "A open at {t} ms (path {path:?})");
            }
            assert_eq!(out.audio, None, "muted: never heard");
        }
        assert!(paths[0] > 0 && paths[1] > 0, "the hop still gives B its turns: {paths:?}");
    }

    /// HOME-15, WATCH-3.3, WATCH-17: as above, but as on the radio a chip given the path back reads its squelch
    /// shut for its first ticks (woken, tuned, the open delay): A must stay open through that, not flicker to
    /// Muted each hop.
    #[test]
    fn muted_slot_stays_open_through_the_chips_reacquire() {
        let c = Cfg { set: 0b011, active: 0, freq: [mhz(145) + 49_500, mhz(446) + 47_500, 0], muted: 0b001, off: 0, held: 0, prio: 0, aprs: 0, scan: 0, first: false, swap: 0 };
        let (mut w, mut hop) = (Watch::new(), Hop::new());
        let (mut path, mut got) = (None, 0u32);
        for t in (0..3_000u32).step_by(10) {
            let listened = [path == Some(0), path == Some(1)];
            // chip 0 reads open only once it has had the path 40 ms (the reacquire)
            let sq = [listened[0] && t >= got + 40, false];
            let out = w.turn(t, &c, sq, listened, false);
            let was = path;
            path = hop.next(t, &out, c.active);
            if path == Some(0) && was != Some(0) {
                got = t + 10;
            }
            if t >= 300 {
                assert!(w.open(0), "A open at {t} ms (path {path:?})");
            }
        }
    }

    /// WATCH-3.2, SQL-14: a latched monitor holds the active slot open (`held`), as stock's squelch 0 never hops:
    /// the path stays on A's chip and B (the other chip) is never listened to, so it is never heard.
    #[test]
    fn monitored_active_slot_holds_the_path() {
        let c = Cfg { set: 0b011, active: 0, freq: [mhz(145) + 49_500, mhz(446) + 47_500, 0], muted: 0, off: 0, held: 0b001, prio: 0, aprs: 0, scan: 0, first: false, swap: 0 };
        let (mut w, mut hop) = (Watch::new(), Hop::new());
        let mut path = None;
        for t in (0..2_000u32).step_by(10) {
            let listened = [path == Some(0), path == Some(1)];
            let sq = [false, listened[1]]; // A's channel empty, B's carrier there
            let out = w.turn(t, &c, sq, listened, false);
            path = hop.next(t, &out, c.active);
            assert_eq!(path, Some(0), "the path stays on A at {t} ms");
            assert_eq!(out.audio, Some(0), "A's audio at {t} ms");
            assert!(awake(&out, path, 0) == [true, false], "B's chip asleep");
        }
    }
}

#[cfg(test)]
mod sim {
    extern crate std;
    use super::*;
    use std::vec::Vec;
    use crate::freq::{mhz, Freq10};

    /// A carrier on `freq` from `start` to `end` ms.
    #[derive(Clone, Copy)]
    pub struct Carrier {
        pub freq: Freq10,
        pub start: u32,
        pub end: u32,
    }

    /// One 10 ms tick of the model radio.
    #[derive(Clone, Copy, Debug)]
    pub struct Step {
        pub t: u32,
        pub chip: [Option<u8>; 2],
        pub path: Option<u8>,
        pub open: u8,
    }

    /// The Radio task's loop over a model of the radio (decision 166's measurements): a chip reads a slot's squelch
    /// open once it has been tuned to that slot with the receive path for `settle(slot)` ms (some 40 ms after a
    /// retune with the open delay at 0; longer with stock's 7) while a carrier is on the slot's frequency.
    pub fn run(c: &Cfg, air: &[Carrier], settle: impl Fn(u8) -> u32, until: u32) -> Vec<Step> {
        let (mut w, mut hop) = (Watch::new(), Hop::new());
        let mut path = None;
        let mut got: [(Option<u8>, u32); 2] = [(None, 0); 2];
        let mut steps = Vec::new();
        for t in (0..until).step_by(10) {
            let listened = [path == Some(0), path == Some(1)];
            let mut sq = [false; 2];
            for n in 0..2 {
                if let (true, (Some(s), since)) = (listened[n], got[n]) {
                    let f = c.freq[s as usize];
                    sq[n] = t >= since + settle(s) && air.iter().any(|a| a.freq == f && (a.start..a.end).contains(&t));
                }
            }
            let out = w.turn(t, c, sq, listened, false);
            let was = path;
            path = hop.next(t, &out, c.active);
            for n in 0..2u8 {
                if out.chip[n as usize] != got[n as usize].0 || path == Some(n) && was != Some(n) {
                    got[n as usize] = (out.chip[n as usize], t);
                }
            }
            let open = (0..SLOTS as u8).filter(|&s| w.open(s)).fold(0, |m, s| m | 1 << s);
            steps.push(Step { t, chip: out.chip, path, open });
        }
        steps
    }

    /// When slot `s` first read open at or after `from`, and how long it then held its chip with the path.
    pub fn caught(steps: &[Step], s: u8, from: u32) -> Option<(u32, u32)> {
        let i = steps.iter().position(|x| x.t >= from && x.open & 1 << s != 0)?;
        let n = steps[i].chip.iter().position(|&c| c == Some(s))? as u8;
        let held = steps[i..].iter().take_while(|x| x.chip[n as usize] == Some(s) && x.path == Some(n)).count() as u32 * 10;
        Some((steps[i].t, held))
    }

    pub const APRS: Freq10 = mhz(144) + 39_000;
    pub const A2M: Freq10 = mhz(146) + 52_000;
    pub const B70: Freq10 = mhz(446) + 47_500;

    /// The model's squelch: stock's open delay 7 (ordinary receive, SQL-17) needs longer than 50 ms after a retune
    /// (decision 166: reads 30-50 ms after never opened; stock's 90-110 ms turn does), modelled as 80 ms; at 0 (the
    /// APRS slot, decision 181) 40 ms.
    fn settle(fast: u8) -> impl Fn(u8) -> u32 {
        move |s| if fast & 1 << s != 0 { 40 } else { 80 }
    }

    fn aprs(set: u8, prio: u8) -> Cfg {
        Cfg { set, active: 0, freq: [A2M, B70, APRS], muted: 0, off: 0, held: 0, prio, aprs: 0, scan: 0, first: false, swap: 0 }
    }

    /// Decision 181's cause, and APRS-29 (beacons alone give C no priority): C without priority, sharing chip 0 with
    /// the active A on another 2 m frequency, gets 70 ms turns, each from a retune; with stock's open delay its
    /// squelch never reads open within one, so a whole 1.2 s APRS frame is missed wherever it starts. With B watched
    /// on chip 1 too, C's turn mostly falls while chip 1 has the receive path. The hop is the plain WATCH-2 one.
    #[test]
    fn without_priority_a_shared_aprs_slot_misses_the_frame() {
        for set in [0b101, 0b111] {
            let c = aprs(set, 0);
            for start in (1_000..1_600).step_by(10) {
                let st = run(&c, &[Carrier { freq: APRS, start, end: start + 1_200 }], settle(0), start + 2_000);
                assert_eq!(caught(&st, 2, start), None, "set {set:03b}, frame at {start}");
            }
            // C's turns are the ordinary watched ones.
            let st = run(&c, &[], settle(0), 2_000);
            let turn = st.iter().skip_while(|x| x.chip[0] != Some(2)).take_while(|x| x.chip[0] == Some(2)).count() as u32 * 10;
            assert_eq!(turn, WATCHED_DWELL_MS, "set {set:03b}");
        }
    }

    /// APRS-21.1 (decision 181): C with priority (APRS mode or KISS), its squelch's open delay 0, catches a 1.2 s
    /// frame within its 300 ms of flags wherever it starts during A's turn (and with B watched on chip 1 too), keeps
    /// chip 0 and the receive path to the frame's end, and A has chip 0 back soon after.
    #[test]
    fn aprs_priority_catches_and_holds_a_frame() {
        for (set, within) in [(0b101, 200), (0b111, 260)] {
            let c = aprs(set, 0b100);
            for start in (1_000..1_600).step_by(10) {
                let end = start + 1_200;
                let st = run(&c, &[Carrier { freq: APRS, start, end }], settle(0b100), end + 1_000);
                let (t, held) = caught(&st, 2, start).unwrap_or_else(|| panic!("set {set:03b}: frame at {start} missed"));
                assert!(t - start <= within, "set {set:03b}: frame at {start} caught after {} ms", t - start);
                assert!(t + held >= end, "set {set:03b}: frame at {start} held only to {}", t + held);
                let back = st.iter().find(|x| x.t >= end && x.chip[0] == Some(0) && x.path == Some(0)).map(|x| x.t);
                assert!(back.is_some_and(|b| b <= end + 200), "set {set:03b}: A back at {back:?}, frame ended {end}");
            }
            // Without a carrier every slot still has its turns: A about half the time or more.
            let st = run(&c, &[], settle(0b100), 3_000);
            let a = st.iter().filter(|x| x.path == Some(0) && x.chip[0] == Some(0)).count();
            assert!(a * 2 >= st.len() * 45 / 50, "set {set:03b}: A listened {a} of {} ticks", st.len());
            if set & 2 != 0 {
                assert!(st.iter().any(|x| x.path == Some(1)), "B still has its turns");
            }
        }
    }

    /// WATCH-17.2 (decision 279): A (active) and B on 70 cm share chip 1, C APRS mode's on 2 m (chip 0). The path
    /// turns on chip 1 are 70 ms, each a whole look for A, then B, with C's 100 ms between: C is never deaf longer
    /// than 70 ms, so a frame is caught early in its flags wherever it starts. A and B (the software squelch: no
    /// retune at a path turn, about 30 ms to read open) still catch a call in their turns and keep the path for it.
    #[test]
    fn aprs_beside_two_uhf_slots_is_never_deaf_long() {
        const A70: Freq10 = mhz(446) + 49_500;
        let c = Cfg { freq: [A70, B70, APRS], ..aprs(0b111, 0b100) };
        let settle = |s: u8| if s == 2 { 40 } else { 30 };
        let st = run(&c, &[], settle, 4_000);
        let (mut gap, mut worst) = (0, 0);
        for x in &st[50..] {
            gap = if x.path == Some(0) { 0 } else { gap + 10 };
            worst = worst.max(gap);
        }
        assert!(worst <= WATCHED_DWELL_MS, "C deaf for {worst} ms");
        for s in [0u8, 1] {
            assert!(st.iter().any(|x| x.path == Some(1) && x.chip[1] == Some(s)), "slot {s} gets path turns");
        }
        for start in (1_000..1_400).step_by(10) {
            let end = start + 1_200;
            let st = run(&c, &[Carrier { freq: APRS, start, end }], settle, end + 500);
            let (t, held) = caught(&st, 2, start).unwrap_or_else(|| panic!("frame at {start} missed"));
            assert!(t - start <= 120, "frame at {start} caught after {} ms", t - start);
            assert!(t + held >= end, "frame at {start} held only to {}", t + held);
            let st = run(&c, &[Carrier { freq: B70, start, end }], settle, end + 500);
            let (t, held) = caught(&st, 1, start).unwrap_or_else(|| panic!("B's call at {start} missed"));
            assert!(t - start <= 400 && t + held >= end, "B's call at {start}: caught {} ms in, held to {}", t - start, t + held);
        }
    }

    /// WATCH-17.3 (decision 295): A (active) and B both on 2 m share chip 0 with APRS mode's C (Alex, Oct 25: "APRS
    /// RX is terrible ... if i have VFO a or b on VHF"). Each turn there starts with a retune (C settles some 40 ms,
    /// the software squelch's A and B about 80, as [`settle`]). Voice first (WATCH-17.2's turns: A 150, B 70, C 100)
    /// leaves C deaf for long stretches; APRS first (C 400 ms after every 100 ms look) catches a frame early in its
    /// flags wherever it starts, and A's and B's calls are still caught in their looks and then keep the chip.
    #[test]
    fn aprs_first_on_a_shared_vhf_chip() {
        const B2M: Freq10 = mhz(146) + 94_000;
        let mut worst_catch = [0u32; 2];
        for (k, first) in [false, true].into_iter().enumerate() {
            let c = Cfg { freq: [A2M, B2M, APRS], first, ..aprs(0b111, 0b100) };
            let set = settle(0b100);
            // C's share: ticks chip 0 is on C with the path and past its settle; and its longest deaf stretch.
            let st = run(&c, &[], &set, 6_000);
            let (mut since, mut prev, mut gap, mut worst, mut heard) = (0, None, 0, 0, 0);
            for x in &st[50..] {
                if x.chip[0] != prev {
                    (since, prev) = (x.t, x.chip[0]);
                }
                let live = x.chip[0] == Some(2) && x.path == Some(0) && x.t >= since + 40;
                heard += live as u32;
                gap = if live { 0 } else { gap + 10 };
                worst = worst.max(gap);
            }
            let share = heard * 100 / (st.len() as u32 - 50);
            // A frame of 1.2 s starting anywhere in a second: when C reads it open.
            let (mut sum, mut n, mut missed) = (0u32, 0u32, 0u32);
            for start in (1_000..2_000).step_by(10) {
                let end = start + 1_200;
                let st = run(&c, &[Carrier { freq: APRS, start, end }], &set, end + 500);
                match caught(&st, 2, start) {
                    Some((t, held)) => {
                        worst_catch[k] = worst_catch[k].max(t - start);
                        sum += t - start;
                        n += 1;
                        assert!(t + held >= end, "first {first}: frame at {start} held only to {}", t + held);
                    }
                    None => missed += 1,
                }
            }
            std::println!("first {first}: C hears {share}%, deaf up to {worst} ms; frames caught {n}/{}, mean {} ms, worst {} ms after the start", n + missed, sum / n.max(1), worst_catch[k]);
            if first {
                assert!(worst <= 2 * LOOK_MS, "APRS first: C deaf for {worst} ms");
                assert!(worst_catch[k] <= 250, "APRS first: a frame caught {} ms in", worst_catch[k]);
                // A's and B's calls (3 s) are still caught in their looks, and keep the chip to the end.
                for (s, f) in [(0u8, A2M), (1, B2M)] {
                    for start in (1_000..2_000).step_by(50) {
                        let end = start + 3_000;
                        let st = run(&c, &[Carrier { freq: f, start, end }], &set, end + 500);
                        let (t, held) = caught(&st, s, start).unwrap_or_else(|| panic!("slot {s}'s call at {start} missed"));
                        assert!(t - start <= 1_200 && t + held >= end, "slot {s}'s call at {start}: {} ms in, held to {}", t - start, t + held);
                    }
                }
            }
        }
        assert!(worst_catch[1] < worst_catch[0], "APRS first catches sooner: {worst_catch:?}");
    }

    /// APRS-21.1: A on the APRS frequency itself (the two-radio run's set-up) opens on a packet in its own turn; without
    /// priority it keeps chip 0 to the end and C never hears it, with priority it hands the carrier to C at once.
    #[test]
    fn a_slot_on_the_aprs_frequency_hands_the_packet_to_c() {
        let start = 1_005; // during A's turn
        let end = start + 1_200;
        let air = [Carrier { freq: APRS, start, end }];
        let c = Cfg { freq: [APRS, B70, APRS], ..aprs(0b101, 0) };
        let st = run(&c, &air, settle(0b100), end + 500);
        assert_eq!(caught(&st, 2, start), None, "no priority: A keeps it");
        let st = run(&Cfg { prio: 0b100, ..c }, &air, settle(0b100), end + 500);
        let (t, held) = caught(&st, 2, start).expect("C has it");
        assert!(t - start <= 300 && t + held >= end, "caught at {t}, held {held}");
    }

    /// APRS-21.2: with C the APRS slot ([`Cfg::aprs`]), A on the APRS frequency takes no turns: chip 0 is C's alone
    /// and B's turns on chip 1 go on; a packet opening C opens A with it, and A heard plays C's chip (C muted or
    /// not; A muted, nothing). Beacons alone (no priority) defer the same. A latched monitor, a pause (keyed) or no
    /// APRS slot: A listens itself.
    #[test]
    fn a_slot_on_the_aprs_frequency_defers_to_c() {
        let c = Cfg { freq: [APRS, B70, APRS], aprs: 0b100, ..aprs(0b111, 0b100) };
        let st = run(&c, &[], settle(0b100), 2_000);
        assert!(st.iter().all(|x| x.chip[0] != Some(0)), "A never has chip 0");
        assert!(st.iter().any(|x| x.chip[1] == Some(1)), "B is still watched");
        let (start, end) = (1_000, 2_200);
        let st = run(&c, &[Carrier { freq: APRS, start, end }], settle(0b100), end + 500);
        assert!(caught(&st, 2, start).is_some(), "C has the packet");
        assert!(st.iter().all(|x| x.open & 1 == (x.open >> 2) & 1), "A's squelch is C's throughout");
        assert!(st.iter().any(|x| x.open & 0b101 == 0b101));
        let mut w = Watch::new();
        let o = w.tick(0, &c, [false; 2], false);
        assert_eq!((o.defer, o.defer_to, o.chip[0]), (0b001, Some(2), Some(2)));
        let o = w.tick(10, &c, [true, false], false);
        assert!(w.open(0) && w.open(2));
        assert_eq!(o.audio, Some(2), "A heard: C's chip plays");
        let o = w.tick(20, &Cfg { muted: 0b100, ..c }, [true, false], false);
        assert_eq!(o.audio, Some(2), "C muted (packets off the speaker), A still heard through it");
        let o = w.tick(30, &Cfg { muted: 0b101, ..c }, [true, false], false);
        assert_eq!(o.audio, None, "A muted too: nothing");
        assert_eq!(Watch::new().tick(0, &Cfg { prio: 0, ..c }, [false; 2], false).defer, 0b001, "beacons alone");
        let o = Watch::new().tick(0, &Cfg { held: 0b001, ..c }, [false; 2], false);
        assert_eq!((o.defer, o.chip[0]), (0, Some(0)), "A's monitor latched: A listens");
        let o = Watch::new().tick(0, &c, [false; 2], true);
        assert_eq!((o.defer, o.chip[0]), (0, Some(0)), "keyed: only the active slot");
        let o = Watch::new().tick(0, &Cfg { aprs: 0, ..c }, [false; 2], false);
        assert_eq!((o.defer, o.chip[0]), (0, Some(0)), "no APRS slot: an ordinary frequency");
    }

    /// APRS-21.1, HOME-15: a muted priority slot still keeps its chip for the packet (its decoder listens), unheard.
    #[test]
    fn muted_aprs_slot_still_holds_the_packet() {
        let c = Cfg { muted: 0b100, ..aprs(0b101, 0b100) };
        let (start, end) = (1_000, 2_200);
        let st = run(&c, &[Carrier { freq: APRS, start, end }], settle(0b100), end + 500);
        let (t, held) = caught(&st, 2, start).expect("caught");
        assert!(t + held >= end);
        let mut w = Watch::new();
        w.tick(0, &c, [false; 2], false);
        w.tick(150, &c, [false; 2], false); // C's turn
        let o = w.tick(200, &c, [true, false], false);
        assert_eq!((o.chip[0], o.hold, o.audio), (Some(2), Some(0), None), "held, not heard");
    }

    /// APRS-21.1: A already open on its own frequency (a voice call) keeps chip 0, so a packet meanwhile is missed (the
    /// accepted cost); once A closes, its 2 s hold doesn't keep C from its turns.
    #[test]
    fn open_active_slot_keeps_the_chip_its_hold_does_not() {
        let c = aprs(0b101, 0b100);
        let voice = Carrier { freq: A2M, start: 0, end: 3_000 };
        let st = run(&c, &[voice, Carrier { freq: APRS, start: 1_000, end: 2_200 }], settle(0b100), 4_000);
        assert!(st.iter().filter(|x| (500..3_000).contains(&x.t)).all(|x| x.chip[0] == Some(0)), "A keeps it while open");
        let c_turn = st.iter().find(|x| x.t >= 3_000 && x.chip[0] == Some(2)).map(|x| x.t);
        assert!(c_turn.is_some_and(|t| t <= 3_000 + 300), "C's turn at {c_turn:?}, within A's 2 s hold");
    }

    /// SCAN-21 (decision 189), on the 10 ms tick with 2 ms of work: the active slot A's range scan on 2 m (chip 0)
    /// steps only while chip 0 has the receive path. With B watched on 70 cm (chip 1) the plain hop gives B 70 ms of
    /// every 220 and the fast pass falls under 30 a second (the radio's 27.2); with the scan in [`Cfg::scan`] the path
    /// stays on chip 0 for [`SCAN_TURN_MS`] between B's turns and the rate is 30 or more, B still listened to about
    /// once a second. C watched on chip 0 too gets no turns there while A scans; C with APRS priority still does.
    #[test]
    fn active_scan_keeps_its_chip_and_most_of_the_path() {
        use crate::scan::{tick_wake, Range, Resume, Scan};
        let skipped = [];
        let r = Range { lo: mhz(144), hi: mhz(148), step: 1_250, skipped: &skipped };
        // (steps a second, B's path turns, C's chip turns) over 10 s
        let rate = |set: u8, scan: u8, prio: u8| {
            let mut sc = Scan::new();
            (sc.fast, sc.resume) = (true, Resume::Drop);
            sc.start(0, 0);
            let (mut w, mut hop) = (Watch::new(), Hop::new());
            let mut c = Cfg { set, active: 0, freq: [mhz(144), mhz(446) + 47_500, mhz(144) + 39_000], muted: 0, off: 0, held: 0, prio, aprs: 0, scan, first: false, swap: 0 };
            let (mut started, mut ticked, mut steps, mut b, mut cturns) = (0u64, 0u64, 0u32, 0u32, 0u32);
            let (mut path, mut last_c) = (None, None);
            while ticked < 10_000 {
                let t = tick_wake(started, ticked, 10, None).max(ticked + 1);
                started = t;
                if path == Some(0) {
                    if let Some(f) = sc.tick(&r, t as u32, false).tune {
                        c.freq[0] = f;
                        steps += 1;
                    }
                }
                let out = w.turn(t as u32, &c, [false; 2], [path == Some(0), path == Some(1)], false);
                let was = path;
                path = hop.next(t as u32, &out, 0);
                b += (path == Some(1) && was != Some(1)) as u32;
                let on_c = out.chip[0] == Some(2);
                cturns += (on_c && last_c != Some(true)) as u32;
                last_c = Some(on_c);
                ticked = t + 2;
            }
            (steps / 10, b, cturns)
        };
        let (plain, b0, _) = rate(0b011, 0, 0);
        assert!(plain <= 30, "the plain hop: {plain} steps a second");
        assert!(b0 >= 40, "B's turns: {b0}");
        let (fast, b1, _) = rate(0b011, 0b001, 0);
        assert!(fast >= 35 && fast > plain, "the scan keeps the path: {fast} steps a second (plain {plain})");
        assert!((8..=11).contains(&b1), "B still watched about once a second: {b1} turns in 10 s");
        let (_, _, c0) = rate(0b111, 0b001, 0);
        assert_eq!(c0, 0, "C watched on the scan's chip: no turns");
        let (with_c, _, c1) = rate(0b111, 0b001, 0b100);
        assert!(c1 >= 20, "C with APRS priority keeps its turns: {c1}");
        assert!(with_c >= 30, "and the scan still makes {with_c} a second");
        // a watched slot's scan (not the active one's) changes nothing
        assert_eq!(rate(0b011, 0b010, 0).0, plain);
    }

    /// WATCH-17.3: APRS first is the default; the UI task's setting reads back. (The hop itself takes it in its
    /// `Cfg`, so no other test here reads this flag.)
    #[test]
    fn aprs_first_setting() {
        assert!(aprs_first());
        set_aprs_first(false);
        assert!(!aprs_first());
        set_aprs_first(true);
        assert!(aprs_first());
    }
}

