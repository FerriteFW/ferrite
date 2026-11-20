//! Local (CHAN-34, decision 326): a zone the radio works out for itself, every channel with a position within the
//! range set at MENU 0 2 5 of where the radio is (the GPS fix, or the fixed position standing in: GPS-3), nearest
//! first. On by default at 50 km; Off takes it away.
//!
//! Nothing is stored: the members are channel numbers in RAM ([`Local`], 112 bytes in the App), found again in one
//! pass over the channels (a record read each, as the Nearby row does) only when the position has moved about a
//! kilometre, the channels changed or the range did, never per draw. A channel stays in until it is 10 % past the
//! range, so one at the edge doesn't come and go as the fix wanders. Without a position the zone is there but empty
//! and says so ("no position"), and a scan of it is refused.
//!
//! The zone is number [`LOCAL`] (0xFFFE, never a stored zone's: those are 0 to `MAX_ZONES` - 1, CHAN-6), last after the stored zones
//! on ◀▶ and on the zones page: a stored zone keeps its place in both, as the browser's computed views follow the
//! zones in its picker (CHAN-14). Screens that step through a slot's zone ask the App ([`App::zlen`],
//! [`App::zat`]) rather than the store, which knows nothing of it.

use crate::chan::{Channel, ChannelStore, ZoneId, MAX_CHANNELS, NO_ZONE};
use crate::data::Pos;
use crate::App;

/// Local's zone number: not a stored zone's (0 to `MAX_ZONES` - 1), not "no zone" (0xFFFF): 0xFFFE (decision 333).
pub const LOCAL: ZoneId = 0xFFFE;
const _: () = assert!(LOCAL != NO_ZONE);
/// The most channels Local holds: the nearest this many within the range.
pub const LOCAL_MAX: usize = 48;
/// MENU 0 2 5's ranges in km (0 Off) and their names.
pub const RANGES_KM: [u16; 6] = [0, 10, 20, 30, 50, 80];
pub const RANGES: [&str; 6] = ["Off", "10 km", "20 km", "30 km", "50 km", "80 km"];
/// The default: 50 km (Alex, Oct 30, 2026: "i think it is on by default, and go with 50k by default").
pub const DEFAULT_RANGE: u8 = 4;
/// Moved this far in latitude or longitude (1e-7 degree: 0.009°, about 1 km north-south) from where the list was
/// made: made again.
const MOVE: i32 = 90_000;
/// CHAN-34.1: the channels quiet this long (ms) before Local is found again for them: a Console write changes them
/// thousands of times, and each find reads every channel record (seconds at 4,000 on the radio).
pub const SETTLE_MS: u32 = 2000;

/// What Local holds now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    /// MENU 0 2 5 is Off: there is no Local zone.
    Off,
    /// On, but no fix and no fixed position: the zone is there, empty, and says so.
    NoPos,
    /// The list for a position.
    Ready,
}

/// Local's members, nearest first, and what they were found for. 120 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Local {
    nums: [u16; LOCAL_MAX],
    n: u8,
    state: State,
    /// The range (km) the list is for.
    km: u16,
    /// Where it was made, and the channels as they were ([`ChannelStore::chan_version`]).
    at: Pos,
    ver: u32,
    /// CHAN-34.1: the channels' version last seen changing, and when (App ms): a change waits until they are quiet.
    seen: u32,
    seen_at: u32,
}

impl Local {
    pub const OFF: Local = Local { nums: [0; LOCAL_MAX], n: 0, state: State::Off, km: 0, at: Pos { lat: 0, lon: 0 }, ver: 0, seen: 0, seen_at: 0 };

    pub fn state(&self) -> State {
        self.state
    }
    /// How many channels it holds.
    pub fn len(&self) -> u16 {
        self.n as u16
    }
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
    /// The i-th nearest's number.
    pub fn num(&self, i: u16) -> Option<u16> {
        self.nums[..self.n as usize].get(i as usize).copied()
    }
    /// Where channel `num` is in it.
    pub fn index(&self, num: u16) -> Option<u16> {
        self.nums[..self.n as usize].iter().position(|&n| n == num).map(|i| i as u16)
    }
    pub fn has(&self, num: u16) -> bool {
        self.index(num).is_some()
    }
    /// The range it is for, km (0 when off or without a position).
    pub fn km(&self) -> u16 {
        self.km
    }
}

/// MENU 0 2 5's range in km; 0 Off.
pub fn range_km(app: &App) -> u16 {
    RANGES_KM[app.set.local as usize % RANGES_KM.len()]
}

/// Whether the zone exists (MENU 0 2 5 not Off).
pub fn on(app: &App) -> bool {
    range_km(app) != 0
}

/// Time passes (the App's tick): Local found again when the range, the channels or the position (about 1 km)
/// changed; nothing otherwise. A slot on Local keeps its channel and takes its place in the new list.
pub fn tick<S: ChannelStore>(app: &mut App, store: &S) {
    let km = range_km(app);
    if km == 0 {
        if app.local.state != State::Off {
            app.local = Local::OFF;
            leave(app, store);
        }
        return;
    }
    let Some(here) = crate::screens::zones::here(store) else {
        if app.local.state != State::NoPos {
            app.local = Local { state: State::NoPos, ..Local::OFF };
        }
        return;
    };
    let l = &app.local;
    let ver = store.chan_version();
    let near = (here.lat - l.at.lat).abs() < MOVE && (here.lon - l.at.lon).abs() < MOVE;
    if l.state == State::Ready && l.km == km && l.ver == ver && near {
        return;
    }
    // CHAN-34.1: only the channels changed: found again once they have been quiet SETTLE_MS
    if l.state == State::Ready && l.km == km && near {
        if l.seen != ver {
            (app.local.seen, app.local.seen_at) = (ver, app.now);
            return;
        }
        if app.now.wrapping_sub(l.seen_at) < SETTLE_MS {
            return;
        }
    }
    app.local = find(store, here, km, ver, &app.local);
    for s in app.slots.iter_mut().filter(|s| s.zone == LOCAL) {
        if let Some(c) = s.ch {
            s.pos = app.local.index(c.num).unwrap_or(s.pos.min(app.local.len().saturating_sub(1)));
        }
    }
}

/// The channels with a position within `km` of `here` (or within 10 % more for those already in `old`), the
/// nearest [`LOCAL_MAX`] by (distance, number): one pass over the channels, a record read each.
#[inline(never)] // one copy: the tick's
fn find<S: ChannelStore>(store: &S, here: Pos, km: u16, ver: u32, old: &Local) -> Local {
    ferrite_core::flashstats::add(&ferrite_core::flashstats::LOCAL_FINDS, 1);
    let range = km as u32 * 1000;
    let out = range + range / 10;
    let kept = |num: u16| old.state == State::Ready && old.km == km && old.has(num);
    let mut l = Local { state: State::Ready, km, at: here, ver, ..Local::OFF };
    let mut dist = [0u32; LOCAL_MAX];
    for num in 0..MAX_CHANNELS {
        // one look at an empty number, one record read for a channel (decision 333: no zone map to skip by)
        let Some((_, p)) = store.located(num) else { continue };
        let m = crate::screens::lists::dist_bearing(here, p).0;
        if m > out || m > range && !kept(num) {
            continue;
        }
        let n = l.n as usize;
        let i = (0..n).find(|&i| (m, num) < (dist[i], l.nums[i])).unwrap_or(n);
        if i >= LOCAL_MAX {
            continue;
        }
        for j in (i + 1..=n.min(LOCAL_MAX - 1)).rev() {
            (dist[j], l.nums[j]) = (dist[j - 1], l.nums[j - 1]);
        }
        (dist[i], l.nums[i]) = (m, num);
        l.n = (n + 1).min(LOCAL_MAX) as u8;
    }
    l
}

/// MENU 0 2 5 turned Off: a slot on Local goes to its channel's own zone, on the same channel (nothing retunes).
fn leave<S: ChannelStore>(app: &mut App, store: &S) {
    for s in app.slots.iter_mut().filter(|s| s.zone == LOCAL) {
        match s.ch {
            Some(c) => {
                let pos = (0..store.zone_len(c.zone)).position(|i| store.zone_at(c.zone, i).is_some_and(|o| o.num == c.num));
                (s.zone, s.pos) = (c.zone, pos.unwrap_or(0) as u16);
            }
            None => (s.zone, s.pos) = (NO_ZONE, 0),
        }
    }
}

/// Local has nothing to go to or scan (CHAN-34): the toast says why, "no position" or none in range.
pub(crate) fn refuse(app: &mut App) {
    if app.local.state == State::NoPos {
        app.show_toast(b"Local", b"no position", false);
    } else {
        app.show_toast(b"Local", b"no channels in range", false);
    }
}

impl App {
    /// How many channels zone `z` holds: Local's from its list (CHAN-34), a stored zone's from the store.
    pub(crate) fn zlen(&self, store: &impl ChannelStore, z: ZoneId) -> u16 {
        if z == LOCAL { self.local.len() } else { store.zone_len(z) }
    }

    /// The i-th channel of zone `z`: Local's nearest first (CHAN-34), a stored zone's in number order (CHAN-10).
    pub(crate) fn zat(&self, store: &impl ChannelStore, z: ZoneId, i: u16) -> Option<Channel> {
        if z == LOCAL { self.local.num(i).and_then(|n| store.get(n)) } else { store.zone_at(z, i) }
    }

    /// Local as it stands (the readback, the tests).
    pub fn local(&self) -> &Local {
        &self.local
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chan::ChanWrite;
    use crate::data::Fix;
    use crate::sample::{Sample, HERE};
    use crate::screens::lists::dist_bearing;
    use std::vec::Vec;

    fn app() -> App {
        let set = std::boxed::Box::leak(std::boxed::Box::new(crate::settings::Settings::DEFAULT));
        let alert = std::boxed::Box::leak(std::boxed::Box::new(crate::screens::alert::AlertInfo::NONE));
        App::new(set, alert)
    }

    /// A position `km` from HERE toward `deg` (flat earth, as dist_bearing measures).
    fn toward(km: f64, deg: f64) -> Pos {
        let (n, e) = (km * deg.to_radians().cos(), km * deg.to_radians().sin());
        let lat = HERE.lat as f64 / 1e7 + n / 111.195;
        let lon = HERE.lon as f64 / 1e7 + e / (111.195 * (lat.to_radians()).cos());
        Pos { lat: (lat * 1e7) as i32, lon: (lon * 1e7) as i32 }
    }

    fn at(s: &Sample, p: Pos) {
        s.fix.set(Some(Fix { pos: p, ..Sample::sample_fix().unwrap() }));
    }

    fn nums(a: &App) -> Vec<u16> {
        (0..a.local.len()).filter_map(|i| a.local.num(i)).collect()
    }

    /// What Local should hold: every stored channel with a position within `km` of `here`, by (distance, number).
    fn want(s: &Sample, here: Pos, km: u32) -> Vec<u16> {
        let mut v: Vec<(u32, u16)> = (0..MAX_CHANNELS).filter(|&n| s.get(n).is_some()).filter_map(|n| s.position(n).map(|p| (dist_bearing(here, p).0, n))).filter(|x| x.0 <= km * 1000).collect();
        v.sort();
        v.into_iter().map(|x| x.1).collect()
    }

    /// CHAN-34: on by default at 50 km (decision 326); its members are the channels with a position within the
    /// range of the fix, nearest first; another range takes effect at once; Off takes the zone away.
    #[test]
    fn members_within_the_range_nearest_first() {
        assert_eq!(crate::settings::Settings::DEFAULT.local, DEFAULT_RANGE);
        assert_eq!((RANGES[DEFAULT_RANGE as usize], RANGES_KM[DEFAULT_RANGE as usize]), ("50 km", 50));
        let (mut a, s) = (app(), Sample::new());
        assert_eq!(a.local.state(), State::Off, "nothing until the first tick");
        tick(&mut a, &s);
        assert_eq!(a.local.state(), State::Ready);
        let w50 = want(&s, HERE, 50);
        assert!(w50.len() >= 3, "{w50:?}");
        assert_eq!(nums(&a), w50);
        a.set.local = 1; // 10 km
        tick(&mut a, &s);
        assert_eq!(nums(&a), want(&s, HERE, 10));
        assert!(nums(&a).len() < w50.len());
        a.set.local = 0;
        tick(&mut a, &s);
        assert_eq!((a.local.state(), a.local.len()), (State::Off, 0));
        assert!(!on(&a));
    }

    /// CHAN-34.3: a channel leaves only when it is 10 % past the range, and a channel outside the range doesn't join
    /// until it is within it: no coming and going at the edge as the fix wanders.
    #[test]
    fn hysteresis_at_the_edge() {
        let (mut a, s) = (app(), Sample::new());
        a.set.local = 3; // 30 km: leaves past 33
        s.apply(ChanWrite::PutPos { ch: crate::chan::channel(200, 0, 14_652_000, "Edge", crate::slot::DEFAULT_CFG), pos: Some(toward(29.0, 90.0)) });
        tick(&mut a, &s);
        assert!(a.local.has(200), "29 km: in");
        for (west, inside, why) in [(2.0, true, "31 km: stays"), (4.0, true, "33 km: stays"), (6.0, false, "35 km: out"), (3.5, false, "32.5 km: not back until within the range"), (0.0, true, "29 km: back")] {
            at(&s, toward(west, 270.0)); // each move more than 1 km from the last: found again
            tick(&mut a, &s);
            assert_eq!(a.local.has(200), inside, "{why}");
        }
    }

    /// CHAN-34.3: found again only when the position moved about 1 km, the channels changed or the range did; a
    /// small move (a fix wandering) keeps the list it has, and where it was made.
    #[test]
    fn found_again_only_when_it_matters() {
        let (mut a, s) = (app(), Sample::new());
        tick(&mut a, &s);
        let first = a.local;
        at(&s, toward(0.5, 0.0));
        tick(&mut a, &s);
        assert_eq!(a.local, first, "500 m: kept as it was");
        at(&s, toward(1.2, 0.0));
        tick(&mut a, &s);
        assert_ne!(a.local.at, first.at, "1.2 km: found again");
        let moved = a.local;
        s.apply(ChanWrite::PutPos { ch: crate::chan::channel(201, 0, 14_653_000, "New", crate::slot::DEFAULT_CFG), pos: Some(toward(1.0, 0.0)) });
        tick(&mut a, &s);
        assert!(!a.local.has(201), "CHAN-34.1: not while the channels are changing");
        a.now += SETTLE_MS - 1;
        tick(&mut a, &s);
        assert!(!a.local.has(201), "CHAN-34.1: not before they're quiet {SETTLE_MS} ms");
        a.now += 1;
        tick(&mut a, &s);
        assert!(a.local.has(201) && a.local.ver != moved.ver, "a channel written: found again once quiet");
    }

    /// CHAN-34.4: without a fix or the fixed position the zone is there and says so, empty; a slot on Local keeps its
    /// channel; the fixed position (GPS-3) stands in for a fix.
    #[test]
    fn no_position() {
        let (mut a, s) = (app(), Sample::new());
        tick(&mut a, &s);
        a.set_channel(0, LOCAL, 0, &s);
        let c = a.slot(0).ch;
        assert!(c.is_some());
        s.fix.set(None);
        tick(&mut a, &s);
        assert_eq!((a.local.state(), a.local.len()), (State::NoPos, 0));
        assert!(on(&a), "still a zone");
        assert_eq!((a.slot(0).zone, a.slot(0).ch), (LOCAL, c), "the slot keeps its channel");
    }

    /// CHAN-34.3, CHAN-34.5: a slot on Local keeps its channel as the list changes, at its new place; turned Off, the slot goes
    /// to its channel's own zone on the same channel (nothing retunes).
    #[test]
    fn slots_follow_and_leave() {
        let (mut a, s) = (app(), Sample::new());
        tick(&mut a, &s);
        a.set_channel(0, LOCAL, 1, &s);
        let c = a.slot(0).ch.unwrap();
        assert_eq!(a.slot(0).pos, 1);
        // a nearer channel joins: the slot's channel is one further down
        s.apply(ChanWrite::PutPos { ch: crate::chan::channel(202, 0, 14_654_000, "Here", crate::slot::DEFAULT_CFG), pos: Some(HERE) });
        tick(&mut a, &s);
        a.now += SETTLE_MS; // CHAN-34.1: once the channels are quiet
        tick(&mut a, &s);
        assert_eq!(a.local.num(0), Some(202));
        assert_eq!((a.slot(0).ch.map(|x| x.num), a.slot(0).pos), (Some(c.num), 2));
        a.set.local = 0;
        tick(&mut a, &s);
        let sl = a.slot(0);
        assert_eq!((sl.zone, sl.ch.map(|x| x.num)), (c.zone, Some(c.num)));
        assert_eq!(s.zone_at(c.zone, sl.pos).map(|x| x.num), Some(c.num), "its place in that zone");
    }
}
