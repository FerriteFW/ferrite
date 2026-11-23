//! The scan engine (manual chapter 14): one slot walking a [`List`] (channels in number order, or a range at
//! its step, SCAN-4), stopping on a carrier and going on by the resume rule (SCAN-5), skipping what is flagged
//! (SCAN-9, SCAN-10) or skipped for this scan (SCAN-11, SCAN-18), looking at the priority channels at their
//! interval (SCAN-12), turned by ▲▼ (SCAN-16) and ended by PTT (SCAN-19). The Radio task tunes the frequency
//! it names and reports the squelch back on the next [`Scan::tick`].
//!
//! Range scans take a fast pass (SCAN-21): each step reads the RSSI only for [`FAST_STEP_MS`], and a step over
//! the floor is held for a full squelch check before it counts as a hit. [`Scan::check`] tells the Radio task
//! which of the two its `open` answers. [`WithTx`] adds a channel's transmit frequency after its receive one
//! (SCAN-22).
//!
//! Channel scans (the slot's zone and the zones added with digits) run in the Radio task from [`ChanSpan`] and
//! [`FreqCache`] (see there): the UI sends how many positions the scan's list has and the Radio task asks it for
//! what is at a position, a few steps ahead, so it never holds a copy of the channels, nor a bit per channel.
//!
//! Timings without a rule of their own are written into the manual as "to confirm on the bench" (SCAN-20).

use crate::freq::Freq10;

/// How long each step listens before going on (SCAN-20.1, decision 166): stock's 70 ms watch dwell. A carrier
/// counts as a hit from half of it on, the chip's squelch settled after the retune: with 50 ms (a hit from 25 ms)
/// the radio never stopped on a 70 cm channel a band away (Oct 15, 2026).
pub const STEP_MS: u32 = 70;
/// A range scan's fast pass: how long each step reads the RSSI before going on (to confirm on the bench; the
/// target is at least nicFW's 30 steps a second, SCAN-21).
pub const FAST_STEP_MS: u32 = 15;
/// After the carrier drops on a hit, it waits this long before going on (to confirm on the bench).
pub const RESUME_WAIT_MS: u32 = 2_000;
/// How long a priority look listens on each priority channel (SCAN-20.1: a step's).
pub const PRIO_DWELL_MS: u32 = STEP_MS;
/// Nuisance skips held for one scan (SCAN-11).
pub const NUISANCE: usize = 8;

/// What a scan walks: positions 0..len, each a frequency or skipped.
pub trait List {
    fn len(&self) -> u16;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// The frequency at `i`; None when it is never stopped on (a channel's Skip flag, an empty channel, a
    /// preset's skipped frequency).
    fn at(&self, i: u16) -> Option<Freq10>;
    /// The `k`th priority position (SCAN-12), None past the last.
    fn priority(&self, _k: u8) -> Option<u16> {
        None
    }
    /// The transmit frequency at `i` when it differs from the receive one (a shift or split), for [`WithTx`].
    fn tx_at(&self, _i: u16) -> Option<Freq10> {
        None
    }
}

/// SCAN-22: a list with each channel's transmit frequency after its receive one, for channels with a shift or
/// split (the others' odd positions are never stopped on). Positions are doubled: channel `i` is `2 * i`.
#[derive(Clone, Copy, Debug)]
pub struct WithTx<L>(pub L);

impl<L: List> List for WithTx<L> {
    fn len(&self) -> u16 {
        self.0.len().saturating_mul(2)
    }

    fn at(&self, i: u16) -> Option<Freq10> {
        if i.is_multiple_of(2) {
            self.0.at(i / 2)
        } else {
            self.0.at(i / 2).and(self.0.tx_at(i / 2)).filter(|&t| Some(t) != self.0.at(i / 2))
        }
    }

    fn priority(&self, k: u8) -> Option<u16> {
        self.0.priority(k).map(|p| p * 2)
    }
}

/// A range preset: `lo` to `hi` at `step` (SCAN-4), its skipped frequencies never stopped on (SCAN-10).
#[derive(Clone, Copy, Debug)]
pub struct Range<'a> {
    pub lo: Freq10,
    pub hi: Freq10,
    pub step: Freq10,
    pub skipped: &'a [Freq10],
}

impl List for Range<'_> {
    fn len(&self) -> u16 {
        ((self.hi.saturating_sub(self.lo)) / self.step.max(1) + 1).min(u16::MAX as u32) as u16
    }

    fn at(&self, i: u16) -> Option<Freq10> {
        let f = self.lo + i as u32 * self.step;
        (f <= self.hi && !self.skipped.contains(&f)).then_some(f)
    }
}

/// The band a range scan walks when it isn't given a preset: the band `f` is in, at its usual step (the Radio
/// task's band scan, as the simulators' fake radio scans). Outside these, a window of 1 MHz each side at 12.5 kHz.
pub fn band_range(f: Freq10) -> Range<'static> {
    use crate::freq::mhz;
    // lo, hi, step (10 Hz units): HF, 6 m, airband, 2 m and VHF, marine, 1.25 m, UHF. 5 kHz on the VHF bands
    // covers both the 15 kHz and 12.5 kHz channel plans' usual frequencies (146.520, 145.500).
    // A `static`: the Radio task reads it, and only named statics stay in the firmware's fast flash (memory.x).
    static BANDS: [(Freq10, Freq10, Freq10); 7] = [
        (mhz(1) + 80_000, mhz(30), 500),
        (mhz(50), mhz(54), 500),
        (mhz(108), mhz(137), 2_500),
        (mhz(137), mhz(156), 500),
        (mhz(156), mhz(163), 2_500),
        (mhz(220), mhz(225), 500),
        (mhz(400), mhz(520), 1_250),
    ];
    let (lo, hi, step) = BANDS
        .iter()
        .copied()
        .find(|&(lo, hi, _)| (lo..hi).contains(&f))
        .unwrap_or((f.saturating_sub(mhz(1)), f + mhz(1), 1_250));
    Range { lo, hi: hi - step, step, skipped: &[] }
}

/// Where `f` is in `r` (its nearest step at or below).
pub fn range_pos(r: &Range<'_>, f: Freq10) -> u16 {
    (f.saturating_sub(r.lo) / r.step.max(1)).min(r.len().saturating_sub(1) as u32) as u16
}

/// SCAN-5's resume rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resume {
    /// After this many seconds on a hit (or [`RESUME_WAIT_MS`] after the carrier drops, if sooner).
    Time(u8),
    /// [`RESUME_WAIT_MS`] after the carrier drops.
    Drop,
    /// Stop on the hit: the scan ends there.
    Never,
}

impl Resume {
    /// The settings' encoding (0 1 2, a zone's, a preset's): 1-10 seconds, 11 carrier drop, 12 stop on hit;
    /// anything else is 5 s.
    pub const fn from_code(c: u8) -> Resume {
        match c {
            1..=10 => Resume::Time(c),
            11 => Resume::Drop,
            12 => Resume::Never,
            _ => Resume::Time(5),
        }
    }

    /// A zone's or a preset's rule over the radio's: 0 (Global) takes `global`'s (SCAN-5, SCAN-24).
    pub const fn code_over(own: u8, global: u8) -> u8 {
        if own == 0 {
            global
        } else {
            own
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum St {
    Off,
    /// Listening at `pos` since.
    Step(u32),
    /// The fast pass found the RSSI over the floor at `pos` since: listening with the squelch to confirm.
    Confirm(u32),
    /// On a hit at `pos` since; the carrier dropped at the second, if it has.
    Hit(u32, Option<u32>),
}

/// What [`Scan::tick`]'s `open` should answer for the step tuned now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// The RSSI is over the noise floor (the fast pass: no squelch wait).
    Rssi,
    /// The squelch is open (a carrier, SQL-1).
    Squelch,
}

/// What to do now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Out {
    /// Tune here (only when it changes).
    pub tune: Option<Freq10>,
    /// A new hit at this position (count it, log it).
    pub hit: Option<u16>,
    /// The scan ended (stop on hit, or nothing left to scan).
    pub ended: bool,
}

/// The engine (60 bytes).
#[derive(Clone, Copy, Debug)]
pub struct Scan {
    st: St,
    pos: u16,
    start: u16,
    up: bool,
    pub resume: Resume,
    /// The priority look's interval (0 1 3; 0 = off).
    pub prio_ms: u32,
    prio_at: u32,
    /// Looking at priority `k` since, coming back to `pos` after.
    look: Option<(u8, u32)>,
    nuisance: [u16; NUISANCE],
    nn: u8,
    /// Nuisance skips this scan (SCAN-7's count).
    pub skips: u8,
    pub hits: u8,
    /// A range scan: steps take the fast pass (SCAN-21).
    pub fast: bool,
}

impl Default for Scan {
    fn default() -> Self {
        Self::new()
    }
}

impl Scan {
    pub const fn new() -> Self {
        Scan {
            st: St::Off,
            pos: 0,
            start: 0,
            up: true,
            resume: Resume::Time(5),
            prio_ms: 0,
            prio_at: 0,
            look: None,
            nuisance: [0; NUISANCE],
            nn: 0,
            skips: 0,
            hits: 0,
            fast: false,
        }
    }

    pub fn running(&self) -> bool {
        self.st != St::Off
    }

    /// Stopped on a hit, or in the resume wait after one.
    pub fn on_hit(&self) -> bool {
        matches!(self.st, St::Hit(..))
    }

    pub fn pos(&self) -> u16 {
        self.pos
    }

    /// Scanning upward (SCAN-16).
    pub fn up(&self) -> bool {
        self.up
    }

    /// The step at the position starts again now: its frequency arrived late (a channel scan's [`PENDING`]),
    /// so it gets its full listen once tuned.
    pub fn restep(&mut self, now: u32) {
        if let St::Step(_) | St::Confirm(_) = self.st {
            self.st = St::Step(now);
        }
    }

    /// Start at position `from` (the slot's channel or frequency, already tuned): it listens there a step first.
    pub fn start(&mut self, from: u16, now: u32) {
        *self = Scan { resume: self.resume, prio_ms: self.prio_ms, up: self.up, fast: self.fast, ..Scan::new() };
        (self.pos, self.start, self.st, self.prio_at) = (from, from, St::Step(now), now);
    }

    /// What `open` answers on the next tick: the RSSI on a fast step, the squelch otherwise.
    pub fn check(&self) -> Check {
        if self.fast && self.look.is_none() && matches!(self.st, St::Step(_)) {
            Check::Rssi
        } else {
            Check::Squelch
        }
    }

    /// SCAN-21: when the fast step now on ends (`FAST_STEP_MS` after it began), for the Radio task to tick on that
    /// moment rather than at its next 10 ms tick, which cost up to a whole tick and the tick's own time a step (27
    /// steps a second on the radio, Oct 15, 2026). None when no fast step runs.
    pub fn due(&self) -> Option<u32> {
        match self.st {
            St::Step(since) if self.fast && self.look.is_none() => Some(since.wrapping_add(FAST_STEP_MS)),
            _ => None,
        }
    }

    pub fn stop(&mut self) {
        self.st = St::Off;
    }

    /// SQL-17: a step or a priority look listens now (the chip's squelch open delay at 0); false off, and on a
    /// hit or in its resume wait (ordinary receive, the open delay stock's).
    pub fn listening(&self) -> bool {
        self.look.is_some() || matches!(self.st, St::Step(_) | St::Confirm(_))
    }

    fn skipped(&self, i: u16) -> bool {
        self.nuisance[..self.nn as usize].contains(&i)
    }

    /// A position stopped on: it has a frequency (not a channel's Skip flag, an empty channel or a preset's skipped
    /// frequency, SCAN-9, SCAN-10) and isn't skipped for this scan (SCAN-11).
    fn stops_at(&self, l: &impl List, i: u16) -> bool {
        l.at(i).is_some() && !self.skipped(i)
    }

    /// The first priority channel from the `k`th on to look at (SCAN-12): a stop (a priority channel with its
    /// Skip flag or skipped with \* is passed over, never looked at: the look heard the squelch of the frequency
    /// still tuned and took it as a hit there), not the position the scan is on. Ends at one that is.
    fn prio_from(&self, l: &impl List, k: u8) -> Option<(u8, u16)> {
        (k..=u8::MAX).map_while(|k| l.priority(k).map(|q| (k, q))).take_while(|&(_, q)| q != self.pos).find(|&(_, q)| self.stops_at(l, q))
    }

    /// The next position from `pos` in the scan's direction that is stopped on; None if there is none.
    fn next(&self, l: &impl List) -> Option<u16> {
        let n = l.len();
        (1..=n).map(|k| if self.up { (self.pos as u32 + k as u32) % n as u32 } else { (self.pos as u32 + n as u32 * 2 - k as u32) % n as u32 } as u16)
            .find(|&i| l.at(i).is_some() && !self.skipped(i))
    }

    /// Off the hit and on to the next position (counted as no skip).
    fn leave(&mut self, l: &impl List, now: u32, out: &mut Out) {
        match self.next(l) {
            Some(i) => {
                self.pos = i;
                self.st = St::Step(now);
                out.tune = l.at(i);
            }
            None => {
                self.st = St::Off;
                out.ended = true;
            }
        }
    }

    /// On from a position that turned out not to be a stop (a channel scan's answer, SCAN-25): the next one now,
    /// as a step that ended.
    pub fn pass(&mut self, l: &impl List, now: u32) -> Out {
        let mut out = Out::default();
        if self.running() {
            self.leave(l, now, &mut out);
        }
        out
    }

    /// ▲▼ (SCAN-16): the direction from here; on a hit they leave it now, without counting a skip.
    pub fn dir(&mut self, up: bool, l: &impl List, now: u32) -> Out {
        self.up = up;
        let mut out = Out::default();
        if self.on_hit() {
            self.leave(l, now, &mut out);
        }
        out
    }

    /// \* (SCAN-11, SCAN-18): skip the hit for the rest of this scan, counted.
    pub fn skip_once(&mut self, l: &impl List, now: u32) -> Out {
        let mut out = Out::default();
        if self.on_hit() {
            if (self.nn as usize) < NUISANCE {
                self.nuisance[self.nn as usize] = self.pos;
                self.nn += 1;
            }
            self.skips = self.skips.saturating_add(1);
            self.leave(l, now, &mut out);
        }
        out
    }

    /// \# (SCAN-17): the hit to skip for good, for the caller to store (the channel's Skip flag, the preset's
    /// skipped list) before the next tick; the scan leaves it.
    pub fn skip_lasting(&mut self, l: &impl List, now: u32) -> (Option<u16>, Out) {
        let mut out = Out::default();
        if !self.on_hit() {
            return (None, out);
        }
        let p = self.pos;
        self.leave(l, now, &mut out);
        (Some(p), out)
    }

    /// PTT (SCAN-19): the scan ends; the position to transmit on: the hit, or where the scan started.
    pub fn ptt(&mut self) -> u16 {
        let p = if self.on_hit() { self.pos } else { self.start };
        self.st = St::Off;
        p
    }

    /// A tick: `open` is the squelch where the last tune put it.
    pub fn tick(&mut self, l: &impl List, now: u32, open: bool) -> Out {
        let mut out = Out::default();
        // A priority look: open there is a hit there; otherwise back where it was.
        if let Some((k, since)) = self.look {
            let p = l.priority(k).filter(|&q| self.stops_at(l, q));
            if open && p.is_some() {
                self.look = None;
                self.pos = p.unwrap_or(self.pos);
                self.hits = self.hits.saturating_add(1);
                (self.st, out.hit) = (St::Hit(now, None), Some(self.pos));
                return out;
            }
            if now.wrapping_sub(since) < PRIO_DWELL_MS && p.is_some() {
                return out;
            }
            match self.prio_from(l, k + 1) {
                Some((k, q)) => {
                    self.look = Some((k, now));
                    out.tune = l.at(q);
                }
                None => {
                    self.look = None;
                    out.tune = l.at(self.pos);
                    if let St::Step(_) | St::Confirm(_) = self.st {
                        self.st = St::Step(now);
                    }
                }
            }
            return out;
        }
        match self.st {
            St::Off => {}
            St::Step(_) if self.fast && open => self.st = St::Confirm(now),
            St::Step(since) if self.fast => {
                if now.wrapping_sub(since) >= FAST_STEP_MS {
                    self.leave(l, now, &mut out);
                }
            }
            St::Step(since) | St::Confirm(since) if open && now.wrapping_sub(since) >= STEP_MS / 2 => {
                self.hits = self.hits.saturating_add(1);
                out.hit = Some(self.pos);
                if self.resume == Resume::Never {
                    self.st = St::Off;
                    out.ended = true;
                    return out;
                }
                self.st = St::Hit(now, None);
            }
            St::Step(since) | St::Confirm(since) => {
                if now.wrapping_sub(since) >= STEP_MS {
                    self.leave(l, now, &mut out);
                }
            }
            St::Hit(t0, dropped) => {
                let dropped = if open { None } else { dropped.or(Some(now)) };
                self.st = St::Hit(t0, dropped);
                let timed = matches!(self.resume, Resume::Time(s) if now.wrapping_sub(t0) >= s as u32 * 1000);
                if timed || dropped.is_some_and(|d| now.wrapping_sub(d) >= RESUME_WAIT_MS) {
                    self.leave(l, now, &mut out);
                }
            }
        }
        // SCAN-12: the priority channels at their interval, from a step or a hit.
        if self.running() && self.prio_ms > 0 && now.wrapping_sub(self.prio_at) >= self.prio_ms {
            self.prio_at = now;
            if let Some((k, q)) = self.prio_from(l, 0) {
                self.look = Some((k, now));
                out.tune = l.at(q);
            }
        }
        out
    }
}

/// The least time the Radio task leaves between the end of a tick and the next one woken by a scan step's end
/// ([`tick_wake`]): the tasks below it (the UI, the link) always get this much.
pub const MIN_GAP_MS: u64 = 2;

/// SCAN-21 (decision 165): when the Radio task ticks next, in its ms clock. `period` after the last tick ended
/// (`ticked`), or sooner at a fast step's end (`due`: [`Scan::due`] in the same clock), but only while that due
/// time is still ahead of the last tick's start (`started`). A due time the last tick had already reached and left
/// where it was is stale: that tick returned before stepping the scan (HF mode, Battery Save asleep, the scope,
/// the counter, keyed, the crossband repeater, or any other early return), and waking on it again ran a whole tick
/// every 1 ms on the Radio task's interrupt executor, above the UI and the link (the radio hung in a range scan,
/// Oct 15, 2026). Never sooner than [`MIN_GAP_MS`] after the last tick ended.
pub fn tick_wake(started: u64, ticked: u64, period: u64, due: Option<u64>) -> u64 {
    let next = ticked + period.max(MIN_GAP_MS);
    match due {
        Some(d) if d > started => next.min(d.max(ticked + MIN_GAP_MS)),
        _ => next,
    }
}

/// Channel numbers 0-3999 (shown 1-4000; CHAN-1, decision 333).
pub const MAX_CHANNELS: u16 = 4000;
/// Priority channels a channel scan looks at (SCAN-12: the zone's priority 1 and 2, then flagged ones).
pub const PRIO_MAX: usize = 4;
/// Frequencies held for the channel scans, shared by the slots.
pub const CACHE: usize = 16;
/// How many stops ahead the Radio task asks for (a step is [`STEP_MS`], an answer a few ms).
pub const AHEAD: u16 = 4;
/// What [`ChanList::at`] answers for a stop whose frequency hasn't arrived: don't tune yet.
pub const PENDING: Freq10 = 0;
/// No position wanted, no priority channel.
pub const NO_WANT: u16 = u16::MAX;
/// A channel scan's positions fit 12 bits (SCAN-25: the slot's zone and the zones added with digits, 256 channels
/// each at most, CHAN-35, then the zone's two priority channels outside it).
pub const POS_MAX: u16 = 0x0FFF;

/// One slot's channel scan as the Radio task holds it (SCAN-25, decision 333): how many positions its list has
/// (the UI's list: the slot's zone's channels in number order, then those of the zones added with digits; Local's
/// nearest first) and the positions of its priority channels (a zone priority channel outside the list has a
/// position after it). 10 bytes: which channel is at a position, and its frequency, the UI answers when asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChanSpan {
    len: u16,
    prio: [u16; PRIO_MAX],
}

impl Default for ChanSpan {
    fn default() -> Self {
        Self::new()
    }
}

impl ChanSpan {
    pub const fn new() -> Self {
        ChanSpan { len: 0, prio: [NO_WANT; PRIO_MAX] }
    }

    /// Positions in the list (the priority channels after them not counted).
    pub fn len(&self) -> u16 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// A position the UI can be asked about: the list's, or a priority channel's after it.
    pub fn valid(&self, p: u16) -> bool {
        p < self.len || self.prio.contains(&p) && p != NO_WANT
    }

    /// Priority `k` (SCAN-12) at position `p`, [`NO_WANT`] for none.
    pub fn set_prio(&mut self, k: u8, p: u16) {
        if let Some(x) = self.prio.get_mut(k as usize) {
            *x = if p <= POS_MAX { p } else { NO_WANT };
        }
    }

    /// The `k`th priority channel's position, in order, skipping empty places.
    pub fn prio(&self, k: u8) -> Option<u16> {
        self.prio.iter().copied().filter(|&p| p != NO_WANT).nth(k as usize)
    }
}

/// One answer kept: under slot `slot - 1` (0: an empty place, so an empty cache is all zeros and is set up without
/// a table from slow flash: tools/slowcheck.py), position `pos` receives on `rx` (0: not a stop: a Skip-flagged, HF
/// or empty channel) and transmits on `tx` (0: the same).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    slot: u8,
    pub pos: u16,
    pub rx: Freq10,
    pub tx: Freq10,
}

impl Entry {
    const NONE: Entry = Entry { slot: 0, pos: 0, rx: 0, tx: 0 };

    /// The slot it is for.
    pub fn slot(&self) -> u8 {
        self.slot.wrapping_sub(1)
    }

    fn is(&self, slot: u8, pos: u16) -> bool {
        self.slot == slot.wrapping_add(1) && self.pos == pos
    }
}

/// The channel scans' frequencies by (slot, position), shared by the slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FreqCache {
    e: [Entry; CACHE],
    /// The next place to replace.
    next: u8,
}

impl Default for FreqCache {
    fn default() -> Self {
        Self::new()
    }
}

impl FreqCache {
    pub const fn new() -> Self {
        FreqCache { e: [Entry::NONE; CACHE], next: 0 }
    }

    pub fn get(&self, slot: u8, pos: u16) -> Option<Entry> {
        self.e.iter().find(|e| e.is(slot, pos)).copied()
    }

    /// A position's receive frequency arrived (0: not a stop). `keep` are (slot, position) pairs not to be
    /// replaced (the stops being listened on and the priority channels).
    pub fn put_rx(&mut self, slot: u8, pos: u16, rx: Freq10, keep: &[(u8, u16)]) {
        if let Some(e) = self.e.iter_mut().find(|e| e.is(slot, pos)) {
            (e.rx, e.tx) = (rx, 0);
            return;
        }
        for _ in 0..CACHE {
            let i = self.next as usize % CACHE;
            self.next = ((i + 1) % CACHE) as u8;
            let x = self.e[i];
            if x.slot == 0 || !keep.contains(&(x.slot(), x.pos)) {
                self.e[i] = Entry { slot: slot.wrapping_add(1), pos, rx, tx: 0 };
                return;
            }
        }
    }

    /// The position's transmit frequency (SCAN-22), after its receive one.
    pub fn put_tx(&mut self, slot: u8, pos: u16, tx: Freq10) {
        if let Some(e) = self.e.iter_mut().find(|e| e.is(slot, pos)) {
            e.tx = tx;
        }
    }

    /// Forget one slot's (its scan starts again: the list may have changed).
    pub fn clear_slot(&mut self, slot: u8) {
        // field by field: a whole `Entry::NONE` would be copied from a constant in slow flash, read here from the
        // Radio task's interrupt context (tools/slowcheck.py)
        for e in self.e.iter_mut().filter(|e| e.slot == slot.wrapping_add(1)) {
            (e.slot, e.pos, e.rx, e.tx) = (0, 0, 0, 0);
        }
    }

    /// Forget everything.
    pub fn clear(&mut self) {
        *self = FreqCache::new();
    }
}

/// A channel scan's [`List`]: positions in the slot's list; a stop not yet in the cache is [`PENDING`], a position
/// the UI answered as not a stop is passed.
#[derive(Clone, Copy, Debug)]
pub struct ChanList<'a> {
    pub slot: u8,
    pub span: &'a ChanSpan,
    pub cache: &'a FreqCache,
}

impl List for ChanList<'_> {
    fn len(&self) -> u16 {
        self.span.len
    }

    fn at(&self, i: u16) -> Option<Freq10> {
        if !self.span.valid(i) {
            return None;
        }
        match self.cache.get(self.slot, i) {
            Some(e) => (e.rx != 0).then_some(e.rx),
            None => Some(PENDING),
        }
    }

    fn priority(&self, k: u8) -> Option<u16> {
        self.span.prio(k)
    }

    fn tx_at(&self, i: u16) -> Option<Freq10> {
        self.cache.get(self.slot, i).map(|e| e.tx).filter(|&t| t != 0)
    }
}

/// The position a channel scan is on in its list: the engine's, halved when it walks [`WithTx`] (`tx`).
pub fn chan_at(scan: &Scan, tx: bool) -> u16 {
    if tx {
        scan.pos() / 2
    } else {
        scan.pos()
    }
}

/// The position the Radio task asks for next for slot `slot`'s channel scan (`tx`: walking [`WithTx`]), if any:
/// the stop it is on, then its priority channels, then the next [`AHEAD`] positions in the scan's direction; the
/// first not yet answered.
pub fn want(scan: &Scan, slot: u8, span: &ChanSpan, cache: &FreqCache, tx: bool) -> Option<u16> {
    if !scan.running() || span.is_empty() {
        return None;
    }
    let missing = |p: u16| span.valid(p) && cache.get(slot, p).is_none();
    let here = chan_at(scan, tx);
    if missing(here) {
        return Some(here);
    }
    if let Some(p) = (0..PRIO_MAX as u8).filter_map(|k| span.prio(k)).find(|&p| missing(p)) {
        return Some(p);
    }
    let n = span.len as u32;
    let mut at = here as u32 % n;
    for _ in 0..AHEAD {
        at = if scan.up() { (at + 1) % n } else { (at + n - 1) % n };
        if at == here as u32 {
            return None;
        }
        if missing(at as u16) {
            return Some(at as u16);
        }
    }
    None
}

/// The (slot, position) pairs a slot's channel scan wants kept in the cache: where it is and its priority channels.
pub fn keep(scan: &Scan, slot: u8, span: &ChanSpan, tx: bool) -> [(u8, u16); 1 + PRIO_MAX] {
    let mut k = [(0xFF, NO_WANT); 1 + PRIO_MAX];
    k[0] = (slot, chan_at(scan, tx));
    for (i, p) in (0..PRIO_MAX as u8).filter_map(|j| span.prio(j)).enumerate() {
        k[1 + i] = (slot, p);
    }
    k
}

/// [`ChanList`], or with each channel's transmit frequency after its receive one (SCAN-22).
#[derive(Clone, Copy, Debug)]
pub enum ChanWalk<'a> {
    Plain(ChanList<'a>),
    Tx(WithTx<ChanList<'a>>),
}

impl List for ChanWalk<'_> {
    fn len(&self) -> u16 {
        match self {
            ChanWalk::Plain(l) => l.len(),
            ChanWalk::Tx(l) => l.len(),
        }
    }
    fn at(&self, i: u16) -> Option<Freq10> {
        match self {
            ChanWalk::Plain(l) => l.at(i),
            ChanWalk::Tx(l) => l.at(i),
        }
    }
    fn priority(&self, k: u8) -> Option<u16> {
        match self {
            ChanWalk::Plain(l) => l.priority(k),
            ChanWalk::Tx(l) => l.priority(k),
        }
    }
}

/// The Radio task's channel scans, for the three slots (the firmware's and the simulators' fake radio's): the
/// slots' lists as lengths and the shared frequency cache, the commands that fill them, and the engine run on them.
/// About 300 bytes (decision 333: no bitmap of channel numbers).
#[derive(Clone, Debug)]
pub struct ChanScans {
    spans: [ChanSpan; 3],
    cache: FreqCache,
    /// The slot's scan is a channel scan (not a band or range scan).
    on: [bool; 3],
    /// Waiting for the frequency of the stop the scan is on: not ticked until it arrives.
    pending: [bool; 3],
    /// SCAN-22 (0 1 6 3): channel scans walk [`WithTx`].
    pub tx: bool,
}

impl Default for ChanScans {
    fn default() -> Self {
        Self::new()
    }
}

impl ChanScans {
    pub const fn new() -> Self {
        ChanScans { spans: [ChanSpan::new(); 3], cache: FreqCache::new(), on: [false; 3], pending: [false; 3], tx: false }
    }

    /// The slot runs a channel scan.
    pub fn is_on(&self, slot: usize) -> bool {
        self.on[slot % 3]
    }

    /// The slot's scan ended or was replaced (a band scan, a tune, stop).
    pub fn off(&mut self, slot: usize) {
        (self.on[slot % 3], self.pending[slot % 3]) = (false, false);
    }

    fn walk(&self, slot: usize) -> ChanWalk<'_> {
        let l = ChanList { slot: (slot % 3) as u8, span: &self.spans[slot % 3], cache: &self.cache };
        if self.tx {
            ChanWalk::Tx(WithTx(l))
        } else {
            ChanWalk::Plain(l)
        }
    }

    /// The position `slot`'s scan is on in its list (whatever [`WithTx`] position it is at).
    pub fn chan(&self, scan: &Scan) -> u16 {
        chan_at(scan, self.tx)
    }

    /// A command for the channel scans: the list's length, priorities, resume rule, start, and the frequencies the
    /// UI sends. A frequency the stopped scan waited for comes back as `(slot, frequency)` to tune.
    pub fn cmd(&mut self, c: &crate::msg::RadioCmd, scans: &mut [Scan; 3], now: u32) -> Option<(usize, Freq10)> {
        use crate::msg::RadioCmd as C;
        match *c {
            C::ScanSet { slot, len } => {
                let i = slot as usize % 3;
                self.spans[i] = ChanSpan { len: len.min(POS_MAX), ..ChanSpan::new() };
                self.cache.clear_slot(i as u8);
            }
            C::ScanPrio { slot, k, pos } => self.spans[slot as usize % 3].set_prio(k, pos),
            C::ScanResume { slot, code } => scans[slot as usize % 3].resume = Resume::from_code(code),
            C::ScanTx(on) => self.tx = on,
            C::ScanChans { slot, from, prio_s } => {
                let i = slot as usize % 3;
                let sc = &mut scans[i];
                (sc.fast, sc.prio_ms) = (false, prio_s as u32 * 1000);
                sc.start(if self.tx { from.saturating_mul(2) } else { from }, now);
                (self.on[i], self.pending[i]) = (true, false);
                self.cache.clear_slot(i as u8); // the list may have changed since the last scan
            }
            C::ScanEntryTx { slot, pos, tx } => self.cache.put_tx(slot % 3, pos, tx),
            C::ScanEntry { slot, pos, rx } => {
                let i = slot as usize % 3;
                let keep = self.keeps(scans);
                self.cache.put_rx(i as u8, pos, rx, &keep);
                let sc = &mut scans[i];
                if self.on[i] && self.pending[i] && chan_at(sc, self.tx) == pos {
                    self.pending[i] = false;
                    if rx == 0 {
                        // not a stop after all (Skip, HF, gone): on to the next, as a step that ended
                        let o = sc.pass(&self.walk(i), now);
                        let o = self.tune(i, o);
                        return o.tune.map(|f| (i, f));
                    }
                    sc.restep(now);
                    let f = self.walk(i).at(sc.pos()).filter(|&f| f != PENDING);
                    if let Some(f) = f {
                        return Some((i, f));
                    }
                }
            }
            _ => {}
        }
        None
    }

    fn keeps(&self, scans: &[Scan; 3]) -> [(u8, u16); 3 * (1 + PRIO_MAX)] {
        let mut k = [(0xFF, NO_WANT); 3 * (1 + PRIO_MAX)];
        for i in (0..3).filter(|&i| self.on[i]) {
            k[i * (1 + PRIO_MAX)..(i + 1) * (1 + PRIO_MAX)].copy_from_slice(&keep(&scans[i], i as u8, &self.spans[i], self.tx));
        }
        k
    }

    /// The tune a step asks for: a [`PENDING`] one waits for its frequency (the scan isn't ticked meanwhile).
    fn tune(&mut self, slot: usize, mut out: Out) -> Out {
        if out.tune == Some(PENDING) {
            out.tune = None;
            self.pending[slot] = true;
        }
        out
    }

    /// A tick of `slot`'s channel scan: `open` as [`Scan::tick`]. Nothing while its frequency is awaited.
    pub fn tick(&mut self, slot: usize, scan: &mut Scan, now: u32, open: bool) -> Out {
        let i = slot % 3;
        if self.pending[i] {
            return Out::default();
        }
        let o = scan.tick(&self.walk(i), now, open);
        self.tune(i, o)
    }

    /// \* on a hit (SCAN-11).
    pub fn skip_once(&mut self, slot: usize, scan: &mut Scan, now: u32) -> Out {
        let o = scan.skip_once(&self.walk(slot % 3), now);
        self.tune(slot % 3, o)
    }

    /// ▲▼ (SCAN-16).
    pub fn dir(&mut self, slot: usize, scan: &mut Scan, up: bool, now: u32) -> Out {
        let o = scan.dir(up, &self.walk(slot % 3), now);
        self.tune(slot % 3, o)
    }

    /// The (slot, position) to ask the UI for next ([`crate::msg::LiveAll::want`]), over the slots' channel scans.
    pub fn want(&self, scans: &[Scan; 3]) -> Option<(u8, u16)> {
        (0..3).filter(|&i| self.on[i]).find_map(|i| want(&scans[i], i as u8, &self.spans[i], &self.cache, self.tx).map(|p| (i as u8, p)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::freq::mhz;

    /// Ten channels; 3 has its Skip flag; 7 is priority.
    struct Chans;
    impl List for Chans {
        fn len(&self) -> u16 {
            10
        }
        fn at(&self, i: u16) -> Option<Freq10> {
            (i != 3 && i < 10).then(|| mhz(146) + i as u32 * 1_250)
        }
        fn priority(&self, k: u8) -> Option<u16> {
            (k == 0).then_some(7)
        }
    }

    fn walk(s: &mut Scan, l: &impl List, from: u32, to: u32, open: impl Fn(u16) -> bool) -> u32 {
        let mut t = from;
        while t < to {
            s.tick(l, t, open(s.pos));
            t += 10;
        }
        t
    }

    /// SCAN-20.1 (decision 166): a step listens 70 ms and a carrier counts from 35 ms after its retune, the chip's
    /// squelch settled; SQL-17: `listening` (the open delay at 0) on a step and a priority look, not on a hit.
    #[test]
    fn step_listens_70_ms_and_open_delay_follows() {
        let mut s = Scan { resume: Resume::Drop, prio_ms: 1_000, ..Scan::new() };
        assert!(!s.listening(), "off");
        s.start(0, 0);
        assert!(s.listening(), "a step");
        assert_eq!(s.tick(&Chans, 30, true), Out::default(), "open 30 ms after the retune: not yet a hit");
        assert_eq!(s.tick(&Chans, 60, false), Out::default(), "still listening at 60 ms");
        assert_eq!(s.tick(&Chans, 70, false).tune, Some(mhz(146) + 1_250), "on after 70 ms");
        assert_eq!(s.tick(&Chans, 100, true).hit, None);
        assert_eq!(s.tick(&Chans, 105, true).hit, Some(1), "open 35 ms after its retune: a hit");
        assert!(!s.listening(), "on a hit: ordinary receive");
        s.tick(&Chans, 1_000, true);
        assert!(s.listening(), "a priority look from the hit");
    }

    #[test]
    fn range_list() {
        let skipped = [mhz(146) + 2_500];
        let r = Range { lo: mhz(146), hi: mhz(146) + 5_000, step: 1_250, skipped: &skipped };
        assert_eq!(r.len(), 5);
        assert_eq!(r.at(1), Some(mhz(146) + 1_250));
        assert_eq!(r.at(2), None, "a skipped frequency");
        assert_eq!(r.at(5), None);
    }

    #[test]
    fn walks_in_order_past_flagged_channels() {
        let mut s = Scan::new();
        s.start(2, 0);
        let mut seen = std::vec::Vec::new();
        for t in (0..400).step_by(10) {
            if let Some(f) = s.tick(&Chans, t, false).tune {
                seen.push(((f - mhz(146)) / 1_250) as u16);
            }
        }
        assert_eq!(seen[..4], [4, 5, 6, 7]);
        s.dir(false, &Chans, 400);
        let mut down = std::vec::Vec::new();
        for t in (400..600).step_by(10) {
            if let Some(f) = s.tick(&Chans, t, false).tune {
                down.push(((f - mhz(146)) / 1_250) as u16);
            }
        }
        assert!(down.windows(2).all(|w| w[1] == (w[0] + 9) % 10 || w[1] == 2 && w[0] == 4), "down, past 3: {down:?}");
    }

    #[test]
    fn hit_resume_on_drop_and_time() {
        let mut s = Scan { resume: Resume::Drop, ..Scan::new() };
        s.start(0, 0);
        let t = walk(&mut s, &Chans, 0, 200, |p| p == 2);
        assert!(s.on_hit() && s.pos() == 2 && s.hits == 1);
        let t = walk(&mut s, &Chans, t, t + 10_000, |p| p == 2);
        assert!(s.on_hit(), "carrier still there: stays (Drop)");
        let t2 = walk(&mut s, &Chans, t, t + RESUME_WAIT_MS - 20, |_| false);
        assert!(s.on_hit(), "in the resume wait");
        walk(&mut s, &Chans, t2, t2 + 40, |_| false);
        assert!(!s.on_hit() && s.pos() == 4);

        let mut s = Scan { resume: Resume::Time(5), ..Scan::new() };
        s.start(0, 0);
        let t = walk(&mut s, &Chans, 0, 200, |p| p == 2);
        walk(&mut s, &Chans, t, t + 5_010, |p| p == 2);
        assert!(s.pos() != 2, "time's up with the carrier on");

        let mut s = Scan { resume: Resume::Never, ..Scan::new() };
        s.start(0, 0);
        walk(&mut s, &Chans, 0, 200, |p| p == 2);
        assert!(!s.running() && s.pos() == 2);
    }

    #[test]
    fn keys_on_a_hit() {
        let mut s = Scan::new();
        s.start(0, 0);
        let t = walk(&mut s, &Chans, 0, 200, |p| p == 2 || p == 5);
        assert_eq!(s.pos(), 2);
        // * : skipped for this scan, counted.
        let o = s.skip_once(&Chans, t);
        assert_eq!((s.skips, o.tune), (1, Some(mhz(146) + 4 * 1_250)));
        // Round again: 2 is passed over.
        let t = walk(&mut s, &Chans, t, t + 2_000, |p| p == 5);
        assert_eq!(s.pos(), 5);
        // # : handed back to store; not counted.
        let (p, _) = s.skip_lasting(&Chans, t);
        assert_eq!((p, s.skips), (Some(5), 1));
        // ▲▼ off a hit: no skip counted.
        let t = walk(&mut s, &Chans, t, t + 2_000, |p| p == 8);
        assert!(s.on_hit() && s.pos() == 8);
        s.dir(false, &Chans, t);
        assert_eq!((s.pos(), s.skips), (7, 1));
        // Not on a hit: the keys only turn.
        assert_eq!(s.skip_lasting(&Chans, t).0, None);
    }

    #[test]
    fn ptt_ends_on_the_hit_or_the_start() {
        let mut s = Scan::new();
        s.start(1, 0);
        walk(&mut s, &Chans, 0, 200, |p| p == 4);
        assert_eq!(s.ptt(), 4);
        assert!(!s.running());
        s.start(1, 0);
        walk(&mut s, &Chans, 0, 100, |_| false);
        assert_eq!(s.ptt(), 1);
    }

    /// The Radio task's main loop over [`tick_wake`] for 1 s of time: each tick takes `cost` ms and steps the scan
    /// only when `steps(t)` (the tick returned early otherwise); `setup` runs on the scan first. Returns (ticks,
    /// steps, the least gap between a tick's end and the next tick).
    fn run_loop(cost: u64, steps: impl Fn(u64) -> bool, setup: impl FnOnce(&mut Scan)) -> (u32, u32, u64) {
        let skipped = [];
        let r = Range { lo: mhz(430), hi: mhz(450), step: 1_250, skipped: &skipped };
        let mut s = Scan { fast: true, resume: Resume::Drop, ..Scan::new() };
        s.start(0, 1_000);
        setup(&mut s);
        let (mut started, mut ticked) = (1_000u64, 1_000u64);
        let (mut ticks, mut stepped, mut gap) = (0u32, 0u32, u64::MAX);
        while ticked < 2_000 {
            let due = s.due().map(|d| d as u64);
            // The firmware's floor (never a wait on a time already past) is 1 ms after now = `ticked`.
            let t = tick_wake(started, ticked, 10, due).max(ticked + 1);
            gap = gap.min(t - ticked);
            (started, ticks) = (t, ticks + 1);
            if steps(t) {
                stepped += s.tick(&r, t as u32, false).tune.is_some() as u32;
            }
            ticked = t + cost;
        }
        (ticks, stepped, gap)
    }

    /// SCAN-21: a Radio task ticking every 10 ms (its tick taking 2 ms) and also at each fast step's end (`due`)
    /// steps every FAST_STEP_MS plus the tick's own time (about 58 a second); on the 10 ms tick alone a step waits
    /// for the second tick past its 15 ms (24 ms, 41 a second, before any false alarm's 70 ms confirm).
    #[test]
    fn fast_pass_ticks_on_its_due_time() {
        let (_, with, gap) = run_loop(2, |_| true, |_| {});
        assert!(with >= 1_000 / (FAST_STEP_MS + 2) - 1, "{with} steps a second ticking on the due time");
        assert!(gap >= MIN_GAP_MS);
        assert_eq!(Scan::new().due(), None, "nothing running: nothing due");
        assert_eq!(tick_wake(0, 2, 10, None), 12, "no scan: the 10 ms tick");
    }

    /// SCAN-21, decision 165: whatever the tick does, the loop never runs it more often than its period, plus once per
    /// fast step the tick takes; a due time the tick didn't move is never woken on twice. The 1 ms loop this
    /// replaced (a due time in the past, `min`ed into every wait) is checked to spin in the same model.
    #[test]
    fn scan_due_never_spins_the_radio_task() {
        // The tick never steps the scan (HF mode, Battery Save asleep, scope, counter, keyed, crossband): the
        // 10 ms tick alone, not one tick a ms.
        let (ticks, steps, gap) = run_loop(1, |_| false, |_| {});
        assert_eq!(steps, 0);
        assert!(ticks <= 1_000 / 11 + 2, "{ticks} ticks a second with a stale due time");
        assert!(gap >= MIN_GAP_MS);
        // The same with the earlier wake (the due time `min`ed in whether or not it was past): a tick every 2 ms.
        let (mut ticked, mut ticks) = (1_000u64, 0);
        let due = 1_015u64;
        while ticked < 2_000 {
            let t = (ticked + 10).min(due).max(ticked + 1);
            (ticked, ticks) = (t + 1, ticks + 1);
        }
        assert!(ticks >= 450, "{ticks}: the old wake spins");
        // Excluded for a while (keyed for 300 ms, say), then stepped again: the scan goes on at full speed after.
        let (_, steps, gap) = run_loop(2, |t| !(1_200..1_500).contains(&t), |_| {});
        assert!(steps >= 700 / (FAST_STEP_MS + 2) - 2, "{steps} steps in the 700 ms it ran");
        assert!(gap >= MIN_GAP_MS);
        // A due time already in the past when the loop starts (the step began 1 s ago, the slot only now has a
        // chip): one tick steps it, then the usual pace.
        let (ticks, steps, _) = run_loop(2, |_| true, |s| s.restep(0));
        assert!(steps >= 1_000 / (FAST_STEP_MS + 2) - 1 && ticks <= 2 * steps + 2, "{ticks} ticks, {steps} steps");
        // The scan stops: nothing due, the 10 ms tick.
        let (ticks, steps, _) = run_loop(2, |_| true, |s| s.stop());
        assert_eq!(steps, 0);
        assert!(ticks <= 1_000 / 12 + 1, "{ticks}");
        // A tick slower than a fast step (20 ms): the next one still waits MIN_GAP_MS.
        let (_, steps, gap) = run_loop(20, |_| true, |_| {});
        assert!(steps >= 1_000 / 23 - 1 && gap >= MIN_GAP_MS, "{steps} steps, gap {gap}");
    }

    /// SCAN-21 as built (`SCAN_WAKE_OFF` in the Radio task: the step-end wake hung the radio, decision 165), re-
    /// measured Oct 15, 2026 with the 70 ms step (decision 166): the 10 ms tick alone, counted from each tick's end,
    /// the scan's time its start. A step leaves on the first tick at least FAST_STEP_MS after the tick that tuned
    /// it: with a tick costing under 5 ms that is the second tick, 20 ms plus two ticks' cost (2 ms: 24 ms, 41 a
    /// second); 5 ms or more, the first (10 + cost). A false alarm (over the floor, no carrier) holds its step for
    /// the 70 ms squelch check. Watch off, so the scan has its chip throughout: with a watched slot on the other
    /// band the hop takes the chip away (150 ms of A's, 70 ms of B's) and the rate falls to 24-30 a second, which
    /// is what the radio measured (27.2, 27.5).
    #[test]
    fn fast_pass_on_the_ten_ms_tick() {
        let skipped = [];
        let r = Range { lo: mhz(430), hi: mhz(450), step: 1_250, skipped: &skipped };
        let rate = |cost: u64, every: u32| {
            let mut s = Scan { fast: true, resume: Resume::Drop, ..Scan::new() };
            s.start(0, 0);
            let (mut started, mut ticked, mut steps) = (0u64, 0u64, 0u32);
            while ticked < 10_000 {
                let t = tick_wake(started, ticked, 10, None).max(ticked + 1);
                started = t;
                let over = s.check() == Check::Rssi && every > 0 && steps % every == every - 1;
                steps += s.tick(&r, t as u32, over).tune.is_some() as u32;
                ticked = t + cost;
            }
            steps / 10
        };
        for (cost, want) in [(1, 45), (2, 41), (3, 38), (4, 35), (5, 66)] {
            assert_eq!(rate(cost, 0), want, "a quiet band, a tick of {cost} ms");
        }
        // One false alarm in 20 steps: each costs a 70 ms step, still over 30 a second at 2 ms a tick.
        let fa = rate(2, 20);
        assert!((30..41).contains(&fa), "{fa} steps a second with a false alarm in 20");
    }

    #[test]
    fn fast_pass_confirms_with_the_squelch() {
        let skipped = [];
        let r = Range { lo: mhz(146), hi: mhz(147), step: 1_250, skipped: &skipped };
        let mut s = Scan { fast: true, resume: Resume::Drop, ..Scan::new() };
        s.start(0, 0);
        // An empty band: one step per FAST_STEP_MS (about 4x the 70 ms dwell).
        let mut t = 0;
        let mut steps = 0;
        while t < 1_000 {
            assert_eq!(s.check(), Check::Rssi);
            steps += s.tick(&r, t, false).tune.is_some() as u32;
            t += 5;
        }
        assert!(steps >= 1_000 / FAST_STEP_MS - 2, "{steps} steps a second");
        // Noise over the floor at 30 but no carrier: held for the squelch check, then on.
        let p = s.pos();
        s.tick(&r, t, true);
        assert_eq!((s.check(), s.pos()), (Check::Squelch, p));
        let t2 = walk(&mut s, &r, t + 5, t + STEP_MS + 10, |_| false);
        assert!(s.pos() != p && !s.on_hit() && s.hits == 0, "a false alarm costs one full step");
        // A real carrier: the RSSI, then the squelch, then a hit.
        let target = (s.pos() + 3) % r.len();
        walk(&mut s, &r, t2, t2 + 400, |q| q == target);
        assert!(s.on_hit() && s.pos() == target && s.hits == 1);
        assert_eq!(s.check(), Check::Squelch);
    }

    /// Ten channels, each with a 600 kHz shift but 4 (simplex).
    struct Shifted;
    impl List for Shifted {
        fn len(&self) -> u16 {
            10
        }
        fn at(&self, i: u16) -> Option<Freq10> {
            Chans.at(i)
        }
        fn priority(&self, k: u8) -> Option<u16> {
            Chans.priority(k)
        }
        fn tx_at(&self, i: u16) -> Option<Freq10> {
            Chans.at(i).map(|f| if i == 4 { f } else { f + 60_000 })
        }
    }

    #[test]
    fn with_tx_adds_the_inputs() {
        let l = WithTx(Shifted);
        assert_eq!(l.len(), 20);
        assert_eq!(l.at(4), Shifted.at(2));
        assert_eq!(l.at(5), Shifted.at(2).map(|f| f + 60_000), "channel 2's input");
        assert_eq!(l.at(9), None, "4 is simplex: nothing more to hear");
        assert_eq!((l.at(6), l.at(7)), (None, None), "3 is flagged Skip: neither side");
        assert_eq!(l.priority(0), Some(14));
        // A scan stops on an input.
        let mut s = Scan::new();
        s.start(0, 0);
        walk(&mut s, &l, 0, 600, |p| p == 5);
        assert!(s.on_hit() && s.pos() == 5);
    }

    #[test]
    fn priority_look() {
        let mut s = Scan { prio_ms: 1_000, resume: Resume::Drop, ..Scan::new() };
        s.start(0, 0);
        // On a hit at 1; at the interval it looks at 7, finds nothing, comes back.
        let t = walk(&mut s, &Chans, 0, 100, |p| p == 1);
        assert_eq!(s.pos(), 1);
        let mut tuned = std::vec::Vec::new();
        let mut t = t;
        while t < 1_200 {
            if let Some(f) = s.tick(&Chans, t, s.pos() == 1 && s.look.is_none()).tune {
                tuned.push(f);
            }
            t += 10;
        }
        assert_eq!(tuned, [mhz(146) + 7 * 1_250, mhz(146) + 1_250]);
        assert!(s.on_hit() && s.pos() == 1);
        // Next interval: 7 is open, it becomes the hit.
        while s.look.is_none() {
            s.tick(&Chans, t, true);
            t += 10;
        }
        s.tick(&Chans, t, true);
        assert_eq!((s.pos(), s.hits), (7, 2));
    }

    #[test]
    fn band_ranges() {
        let r = band_range(mhz(146) + 52_000);
        assert_eq!((r.lo, r.step), (mhz(137), 500));
        assert_eq!(r.at(range_pos(&r, mhz(146) + 52_000)), Some(mhz(146) + 52_000));
        let r = band_range(mhz(121) + 50_000);
        assert_eq!((r.lo, r.step), (mhz(108), 2_500));
        let r = band_range(mhz(300) + 10_000);
        assert_eq!((r.lo, r.hi), (mhz(299) + 10_000, mhz(301) + 10_000 - 1_250), "no band: a window around it");
        assert_eq!(range_pos(&r, mhz(400)), r.len() - 1, "clamped to the range");
    }

    /// SCAN-25: a slot's list as the Radio task holds it: a length and the priority positions (empty places
    /// skipped; one past [`POS_MAX`] is none); a position is one to ask about when it is the list's or a priority's.
    #[test]
    fn chan_span_positions_and_priorities() {
        let mut a = ChanSpan::default();
        assert!(a.is_empty() && a == ChanSpan::new());
        a.len = 5;
        a.set_prio(1, 7);
        a.set_prio(0, NO_WANT);
        a.set_prio(2, POS_MAX + 1);
        a.set_prio(PRIO_MAX as u8, 3);
        assert_eq!((a.prio(0), a.prio(1), a.len()), (Some(7), None, 5), "empty places are skipped");
        assert!(a.valid(0) && a.valid(4) && !a.valid(5) && a.valid(7) && !a.valid(NO_WANT));
    }

    /// Fills a slot's cache with answers to its wants until none is left: position p answers `f(p)`.
    fn answer_all(cs: &mut ChanScans, scans: &mut [Scan; 3], f: &dyn Fn(u16) -> Freq10, t: u32, tuned: &mut std::vec::Vec<(usize, Freq10)>) {
        while let Some((slot, p)) = cs.want(scans) {
            if let Some(x) = cs.cmd(&crate::msg::RadioCmd::ScanEntry { slot, pos: p, rx: f(p) }, scans, t) {
                tuned.push(x);
            }
        }
    }

    /// SCAN-25: the Radio task's channel scan asks what is at each position as it needs it (one answer per tick
    /// here, as the UI answers); every stop is listened on for its full step once tuned, in order, wrapping; a
    /// position the UI says is no stop (rx 0: a Skip-flagged or HF channel) is passed.
    #[test]
    fn chan_scan_fetches_ahead() {
        let span = ChanSpan { len: 6, ..ChanSpan::new() };
        let mut cache = FreqCache::new();
        let f = |p: u16| if p == 2 { 0 } else { mhz(146) + p as u32 * 500 };
        let mut s = Scan::new();
        s.start(0, 0);
        let mut tuned = std::vec::Vec::new();
        let mut pending = false;
        let mut asked = 0;
        let mut t = 0;
        while t < 3_000 {
            // the UI's answer to last tick's want
            if let Some(p) = want(&s, 0, &span, &cache, false) {
                cache.put_rx(0, p, f(p), &keep(&s, 0, &span, false));
                asked += 1;
                if pending && p == s.pos() {
                    pending = false;
                    s.restep(t);
                    tuned.push((t, p));
                }
            }
            if !pending {
                let o = s.tick(&ChanList { slot: 0, span: &span, cache: &cache }, t, false);
                match o.tune {
                    Some(PENDING) => pending = true,
                    Some(fr) => tuned.push((t, ((fr - mhz(146)) / 500) as u16)),
                    None => {}
                }
            }
            t += 10;
        }
        let order: std::vec::Vec<u16> = tuned.iter().map(|x| x.1).take(6).collect();
        assert_eq!(order, [1, 3, 4, 5, 0, 1], "in list order, wrapping, position 2 passed");
        for w in tuned.windows(2) {
            assert!(w[1].0 - w[0].0 >= STEP_MS, "each stop listened a full step: {:?}", w);
        }
        assert!(asked >= 6, "fetched as needed");
    }

    /// The cache keeps what is being listened on and the priority channels when it fills; it is by slot.
    #[test]
    fn cache_keeps_what_is_in_use() {
        let mut c = FreqCache::new();
        for p in 0..CACHE as u16 {
            c.put_rx(1, p, 1_000 + p as u32, &[]);
        }
        c.put_tx(1, 2, 77);
        for p in 100..100 + CACHE as u16 {
            c.put_rx(1, p, 2_000, &[(1, 2), (1, 5)]);
        }
        assert_eq!(c.get(1, 2).map(|e| (e.rx, e.tx)), Some((1_002, 77)));
        assert_eq!(c.get(1, 5).map(|e| (e.rx, e.tx)), Some((1_005, 0)));
        assert_eq!(c.get(1, 0), None);
        assert_eq!(c.get(0, 2), None, "another slot's position 2 is another channel");
        c.clear_slot(1);
        assert_eq!(c.get(1, 2), None);
        assert_eq!(Resume::from_code(11), Resume::Drop);
        assert_eq!(Resume::from_code(Resume::code_over(0, 3)), Resume::Time(3));
        assert_eq!(Resume::from_code(Resume::code_over(12, 3)), Resume::Never);
    }

    /// The Radio task's side end to end, as the UI and the firmware drive it: the list's length, priorities, start,
    /// then answering wants; a tune comes as soon as the awaited frequency arrives.
    #[test]
    fn chan_scans_driven_by_commands() {
        use crate::msg::RadioCmd as C;
        let mut cs = ChanScans::new();
        let mut scans = [Scan::new(); 3];
        cs.cmd(&C::ScanSet { slot: 1, len: 3 }, &mut scans, 0);
        cs.cmd(&C::ScanResume { slot: 1, code: 11 }, &mut scans, 0);
        cs.cmd(&C::ScanChans { slot: 1, from: 0, prio_s: 0 }, &mut scans, 0);
        assert!(cs.is_on(1) && scans[1].running() && scans[1].resume == Resume::Drop);
        let f = |p: u16| mhz(446) + p as u32 * 1_250;
        let mut tuned = std::vec::Vec::new();
        let mut t = 0;
        while t < 1_000 {
            if let Some((slot, p)) = cs.want(&scans) {
                if let Some(x) = cs.cmd(&C::ScanEntry { slot, pos: p, rx: f(p) }, &mut scans, t) {
                    tuned.push(x);
                }
            }
            if let Some(fr) = cs.tick(1, &mut scans[1], t, false).tune {
                tuned.push((1, fr));
            }
            t += 10;
        }
        assert_eq!(&tuned[..3], &[(1, f(1)), (1, f(2)), (1, f(0))]);
        assert!(tuned.iter().all(|&(_, fr)| fr != PENDING));
        assert_eq!(cs.chan(&scans[1]), scans[1].pos());
    }

    /// Nothing to stop on.
    struct Empty;
    impl List for Empty {
        fn len(&self) -> u16 {
            0
        }
        fn at(&self, _i: u16) -> Option<Freq10> {
            None
        }
    }

    /// A list's defaults: no priority, no transmit side; a scan with no stop ends at its first step's end.
    #[test]
    fn empty_list_ends_the_scan() {
        assert!(Empty.is_empty());
        assert_eq!((Empty.at(0), Empty.priority(0), Empty.tx_at(0)), (None, None, None));
        let r = Range { lo: mhz(146), hi: mhz(146), step: 1_250, skipped: &[] };
        assert!(!r.is_empty());
        assert_eq!((r.len(), r.priority(0), r.tx_at(0)), (1, None, None));
        let mut s = Scan::default();
        assert!(!s.running() && s.up() && s.resume == Resume::Time(5) && s.due().is_none());
        s.start(0, 0);
        assert_eq!(s.tick(&Empty, STEP_MS - 10, false), Out::default());
        assert_eq!(s.tick(&Empty, STEP_MS, false), Out { tune: None, hit: None, ended: true });
        assert!(!s.running());
    }

    /// SCAN-5: the settings' resume codes; anything out of range is the 5 s default.
    #[test]
    fn resume_codes() {
        for c in 1..=10 {
            assert_eq!(Resume::from_code(c), Resume::Time(c));
        }
        assert_eq!(Resume::from_code(12), Resume::Never);
        for c in [0, 13, 255] {
            assert_eq!(Resume::from_code(c), Resume::Time(5), "{c}");
        }
    }

    /// SCAN-20.3: eight nuisance skips are held; a ninth \* still leaves the hit and counts, but the scan stops on
    /// it again next time round.
    #[test]
    fn nuisance_skips_held_up_to_eight() {
        let r = Range { lo: mhz(146), hi: mhz(146) + 11 * 1_250, step: 1_250, skipped: &[] };
        let mut s = Scan { resume: Resume::Drop, ..Scan::new() };
        s.start(0, 0);
        let mut t = 0;
        let mut hits = std::vec::Vec::new();
        for _ in 0..12 {
            t = walk(&mut s, &r, t, t + 50, |_| true);
            assert!(s.on_hit());
            hits.push(s.pos());
            s.skip_once(&r, t);
        }
        assert_eq!(hits, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
        assert_eq!(s.skips, 12);
        assert_eq!(s.pos(), 8, "0-7 held as skipped; 8 (the ninth) stopped on again");
        // A skip off the only stop left: the scan ends.
        let mut s = Scan { resume: Resume::Drop, ..Scan::new() };
        let one = Range { lo: mhz(146), hi: mhz(146), step: 1_250, skipped: &[] };
        s.start(0, 0);
        let t = walk(&mut s, &one, 0, 50, |_| true);
        assert!(s.on_hit());
        assert!(s.skip_once(&one, t).ended && !s.running());
        // Not on a hit: * and ▲▼ change nothing but the direction.
        s.start(0, 0);
        assert_eq!(s.skip_once(&r, 0), Out::default());
        assert_eq!(s.dir(false, &r, 0), Out::default());
        assert!(!s.up() && s.skips == 0 && s.pos() == 0);
    }

    /// Chans with a second priority channel (2).
    struct TwoPrio;
    impl List for TwoPrio {
        fn len(&self) -> u16 {
            10
        }
        fn at(&self, i: u16) -> Option<Freq10> {
            Chans.at(i)
        }
        fn priority(&self, k: u8) -> Option<u16> {
            [7, 2].get(k as usize).copied()
        }
    }

    /// SCAN-12: from a step, the look goes through each priority channel a dwell each, then back to the step,
    /// which listens a full step again; a priority channel that is the stop itself isn't looked at.
    #[test]
    fn priority_look_from_a_step() {
        let mut s = Scan { prio_ms: 1_000, resume: Resume::Drop, ..Scan::new() };
        s.start(0, 0);
        walk(&mut s, &TwoPrio, 0, 1_000, |_| false);
        let p = s.pos();
        assert!(p != 7 && p != 2);
        let mut tuned = std::vec::Vec::new();
        let mut t = 1_000;
        while tuned.len() < 3 {
            if let Some(f) = s.tick(&TwoPrio, t, false).tune {
                tuned.push((t, f));
            }
            t += 10;
        }
        let f = |i: u32| mhz(146) + i * 1_250;
        assert_eq!(tuned, [(1_000, f(7)), (1_000 + PRIO_DWELL_MS, f(2)), (1_000 + 2 * PRIO_DWELL_MS, f(p as u32))]);
        assert!(s.listening() && s.pos() == p && s.hits == 0);
        // Back on the step for a full STEP_MS from its return.
        assert_eq!(s.tick(&TwoPrio, t + STEP_MS - 30, false).tune, None);
        assert!(s.tick(&TwoPrio, t + STEP_MS, false).tune.is_some());

        // On a hit at the priority channel itself: no look at the interval.
        let mut s = Scan { prio_ms: 1_000, resume: Resume::Drop, ..Scan::new() };
        s.start(7, 0);
        let t = walk(&mut s, &Chans, 0, 100, |p| p == 7);
        assert!(s.on_hit() && s.pos() == 7);
        let mut t = t;
        while t < 2_100 {
            assert_eq!(s.tick(&Chans, t, true).tune, None, "at {t}");
            t += 10;
        }
        assert!(!s.listening());
    }

    /// SCAN-9, SCAN-11, SCAN-12: a priority channel with its Skip flag, or skipped with \* this scan, is never looked
    /// at: the look tuned nothing there, heard the squelch still open on the step's frequency and stopped on the
    /// skipped channel as a hit. The next priority channel is still looked at.
    #[test]
    fn priority_look_passes_skipped_channels() {
        /// Ten channels, 3 with its Skip flag; priority 3 then 7.
        struct SkipPrio;
        impl List for SkipPrio {
            fn len(&self) -> u16 {
                10
            }
            fn at(&self, i: u16) -> Option<Freq10> {
                Chans.at(i)
            }
            fn priority(&self, k: u8) -> Option<u16> {
                [3, 7].get(k as usize).copied()
            }
        }
        let mut s = Scan { prio_ms: 1_000, resume: Resume::Drop, ..Scan::new() };
        s.start(1, 0);
        // A carrier on 1 throughout: a hit there, then the looks with the squelch still open.
        let mut tuned = std::vec::Vec::new();
        let mut t = 0;
        while t < 1_500 {
            let o = s.tick(&SkipPrio, t, true);
            assert_ne!(o.hit, Some(3), "a hit on the skipped channel at {t}");
            tuned.extend(o.tune);
            t += 10;
        }
        assert!(tuned.contains(&(mhz(146) + 7 * 1_250)), "7 is still looked at: {tuned:?}");
        assert!(!tuned.contains(&(mhz(146) + 3 * 1_250)));
        // \* on a hit at the priority channel 7: no look stops there again this scan.
        let mut s = Scan { prio_ms: 1_000, resume: Resume::Drop, ..Scan::new() };
        s.start(7, 0);
        let mut t = walk(&mut s, &SkipPrio, 0, 100, |p| p == 7);
        assert!(s.on_hit() && s.pos() == 7);
        s.skip_once(&SkipPrio, t);
        let end = t + 5_000;
        while t < end {
            assert_ne!(s.tick(&SkipPrio, t, true).hit, Some(7), "a priority look stopped on 7 again at {t}");
            t += 10;
        }
    }

    /// The list's edges: a length past [`POS_MAX`] is held to it; a list of none ends the scan at its first step.
    #[test]
    fn chan_span_edges() {
        use crate::msg::RadioCmd as C;
        let mut cs = ChanScans::new();
        let mut scans = [Scan::new(); 3];
        cs.cmd(&C::ScanSet { slot: 0, len: u16::MAX }, &mut scans, 0);
        assert_eq!(cs.spans[0].len(), POS_MAX);
        cs.cmd(&C::ScanSet { slot: 0, len: 0 }, &mut scans, 0);
        cs.cmd(&C::ScanChans { slot: 0, from: 0, prio_s: 0 }, &mut scans, 0);
        assert_eq!(cs.want(&scans), None, "nothing to ask");
        assert!(cs.tick(0, &mut scans[0], STEP_MS, false).ended);
    }

    /// A position already cached is updated in place (its transmit side forgotten until it comes again); a cache
    /// full of kept positions takes no new one.
    #[test]
    fn cache_updates_and_full_of_kept() {
        let mut c = FreqCache::new();
        c.put_rx(0, 4, 100, &[]);
        c.put_tx(0, 4, 7);
        c.put_rx(0, 4, 200, &[]);
        assert_eq!(c.get(0, 4).map(|e| (e.rx, e.tx)), Some((200, 0)));
        c.put_tx(0, 9, 5);
        assert_eq!(c.get(0, 9), None, "a transmit side for a position not cached is dropped");
        let keep: std::vec::Vec<(u8, u16)> = (0..CACHE as u16).map(|p| (2, p)).collect();
        let mut c = FreqCache::new();
        for &(sl, p) in &keep {
            c.put_rx(sl, p, 1_000 + p as u32, &keep);
        }
        c.put_rx(2, 50, 2, &keep);
        assert_eq!(c.get(2, 50), None);
        assert!(keep.iter().all(|&(sl, p)| c.get(sl, p).map(|e| e.rx) == Some(1_000 + p as u32)));
        c.clear();
        assert_eq!(c, FreqCache::new());
    }

    /// What a channel scan asks for: nothing when stopped; the stop it is on, then a priority channel (here one
    /// outside the list, after it), then the next stops; nothing when all it would look at is cached.
    #[test]
    fn want_order_and_none() {
        let mut span = ChanSpan { len: 3, ..ChanSpan::new() };
        span.set_prio(0, 3);
        let mut cache = FreqCache::new();
        let mut s = Scan::new();
        assert_eq!(want(&s, 0, &span, &cache, false), None, "not running");
        s.start(1, 0);
        assert_eq!(want(&s, 0, &span, &cache, false), Some(1));
        cache.put_rx(0, 1, 1, &[]);
        assert_eq!(want(&s, 0, &span, &cache, false), Some(3), "the priority channel before the next stop");
        cache.put_rx(0, 3, 1, &[]);
        assert_eq!(want(&s, 0, &span, &cache, false), Some(2));
        cache.put_rx(0, 2, 1, &[]);
        assert_eq!(want(&s, 0, &span, &cache, false), Some(0));
        cache.put_rx(0, 0, 1, &[]);
        assert_eq!(want(&s, 0, &span, &cache, false), None, "round to where it is");
        assert_eq!(keep(&s, 0, &span, false), [(0, 1), (0, 3), (0xFF, NO_WANT), (0xFF, NO_WANT), (0xFF, NO_WANT)]);
        // Walking WithTx: engine position 2 is list position 1.
        s.start(2, 0);
        assert_eq!((chan_at(&s, true), keep(&s, 0, &span, true)[0]), (1, (0, 1)));
        // A single stop: nothing ahead of it.
        let one = ChanSpan { len: 1, ..ChanSpan::new() };
        s.start(0, 0);
        assert_eq!(want(&s, 0, &one, &cache, false), None);
        // More stops than AHEAD, all those ahead cached: no further look.
        let many = ChanSpan { len: 11, ..ChanSpan::new() };
        let mut cache = FreqCache::new();
        for p in 0..10u16 {
            cache.put_rx(0, p, 1, &[]);
        }
        s.start(0, 0);
        assert_eq!(want(&s, 0, &many, &cache, false), None, "only AHEAD stops ahead are fetched");
    }

    /// SCAN-22 through the Radio task's walk: a shifted channel's input right after it, a priority as its
    /// doubled position, and the transmit side arriving by its own command.
    #[test]
    fn chan_scans_walk_the_inputs() {
        use crate::msg::RadioCmd as C;
        let mut cs = ChanScans::default();
        let mut scans = [Scan::new(); 3];
        cs.cmd(&C::ScanTx(true), &mut scans, 0);
        cs.cmd(&C::ScanSet { slot: 0, len: 2 }, &mut scans, 0);
        cs.cmd(&C::ScanPrio { slot: 0, k: 0, pos: 1 }, &mut scans, 0);
        cs.cmd(&C::ScanChans { slot: 0, from: 0, prio_s: 0 }, &mut scans, 0);
        assert!(cs.tx && cs.is_on(0) && !cs.is_on(1));
        assert_eq!((scans[0].pos(), cs.chan(&scans[0])), (0, 0));
        let f = |p: u16| mhz(446) + p as u32 * 1_250;
        assert_eq!(cs.want(&scans), Some((0, 0)));
        assert_eq!(cs.cmd(&C::ScanEntry { slot: 0, pos: 0, rx: f(0) }, &mut scans, 0), None, "not waited for");
        assert_eq!(cs.want(&scans), Some((0, 1)));
        cs.cmd(&C::ScanEntry { slot: 0, pos: 1, rx: f(1) }, &mut scans, 0);
        cs.cmd(&C::ScanEntryTx { slot: 0, pos: 1, tx: f(1) + 60_000 }, &mut scans, 0);
        assert_eq!(cs.want(&scans), None);
        assert_eq!(cs.walk(0).len(), 4);
        assert_eq!(cs.walk(0).priority(0), Some(2));
        let mut tuned = std::vec::Vec::new();
        let mut t = 0;
        while tuned.len() < 3 {
            if let Some(fr) = cs.tick(0, &mut scans[0], t, false).tune {
                tuned.push(fr);
            }
            t += 10;
        }
        assert_eq!(tuned, [f(1), f(1) + 60_000, f(0)], "0 is simplex: no input of its own");
        cs.tx = false;
        assert_eq!(cs.walk(0).priority(0), Some(1));
        assert_eq!(cs.cmd(&C::Tune(f(0)), &mut scans, t), None, "not a scan command");
        cs.off(0);
        assert!(!cs.is_on(0));
        assert_eq!(cs.want(&scans), None);
    }

    /// A step onto a position whose frequency hasn't arrived waits (not ticked) and tunes when it comes; * and ▲▼
    /// onto one wait the same way (SCAN-11, SCAN-16); an answer that it is no stop moves on at once (SCAN-25).
    #[test]
    fn chan_scan_waits_for_a_pending_stop() {
        use crate::msg::RadioCmd as C;
        let mut cs = ChanScans::new();
        let mut scans = [Scan::new(); 3];
        cs.cmd(&C::ScanSet { slot: 2, len: 4 }, &mut scans, 0);
        cs.cmd(&C::ScanResume { slot: 2, code: 11 }, &mut scans, 0);
        cs.cmd(&C::ScanChans { slot: 2, from: 0, prio_s: 0 }, &mut scans, 0);
        let f = |p: u16| mhz(446) + p as u32 * 1_250;
        assert_eq!(cs.tick(2, &mut scans[2], 0, false), Out::default());
        assert_eq!(cs.tick(2, &mut scans[2], STEP_MS, false), Out::default(), "PENDING never reaches the chip");
        assert_eq!(scans[2].pos(), 1);
        assert_eq!(cs.tick(2, &mut scans[2], STEP_MS + 50, true), Out::default(), "not ticked while waiting");
        assert!(!scans[2].on_hit());
        assert_eq!(cs.cmd(&C::ScanEntry { slot: 2, pos: 1, rx: f(1) }, &mut scans, 90), Some((2, f(1))));
        let mut t = 100;
        while !scans[2].on_hit() {
            cs.tick(2, &mut scans[2], t, true);
            t += 10;
        }
        assert!(t - 90 >= STEP_MS / 2, "a full listen from when it was tuned");
        assert_eq!(cs.skip_once(2, &mut scans[2], t), Out::default());
        assert_eq!((scans[2].pos(), scans[2].skips), (2, 1));
        // position 2 is no stop (a Skip-flagged channel): on to 3 at once, which waits in turn
        assert_eq!(cs.cmd(&C::ScanEntry { slot: 2, pos: 2, rx: 0 }, &mut scans, t), None);
        assert_eq!(scans[2].pos(), 3);
        assert_eq!(cs.cmd(&C::ScanEntry { slot: 2, pos: 3, rx: f(3) }, &mut scans, t), Some((2, f(3))));
        while !scans[2].on_hit() {
            t += 10;
            cs.tick(2, &mut scans[2], t, true);
        }
        assert_eq!(cs.dir(2, &mut scans[2], false, t), Out::default());
        assert_eq!(scans[2].pos(), 0, "down past 2 (no stop) and 1 (skipped for this scan)");
        assert_eq!(cs.cmd(&C::ScanEntry { slot: 2, pos: 0, rx: f(0) }, &mut scans, t), Some((2, f(0))));
        let mut tuned = std::vec::Vec::new();
        answer_all(&mut cs, &mut scans, &|_| f(0), t, &mut tuned);
    }
}
