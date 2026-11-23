//! The scan screen (manual chapter 14, SCAN-1 to SCAN-15; no mock-up): SIDE2 on home starts a scan on the
//! active slot and opens this screen. It shows the target, the resume rule, where the scan is, the hits (when,
//! what, how strong) and the skip count (SCAN-7). \* skips the current hit for this scan (SCAN-11, KEY-18),
//! ▲▼ set the direction and leave a hit (SCAN-16), # skips a channel for good (SCAN-17), digits add or remove
//! zones 0-9 from the set (SCAN-3), SIDE2 stops (SCAN-1), BACK goes back (home: SIDE2 starts it there) with the
//! scan still running (it shows on the card).
//!
//! A scan keeps its last 16 hits with how often each stopped it and how long its last stop lasted (SCAN-23):
//! OK opens the list, where OK tunes the slot's VFO to one and hold 9 saves it as a channel (KEY-23); on a
//! channel scan the rows are the channels' names and OK puts the slot on the channel.

use super::activity::Hit;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::{ChannelStore, FLAG_SKIP, ZoneId};
use crate::keys::{KeyEvent, Press};
use crate::slot::{band_name, chan_label, BANDS, band_near};
use crate::App;
use ferrite_core::keys::Key;
use ferrite_core::msg::{LiveAll, RadioCmd};
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Scan {
    pub slot: u8,
    /// The hit list is up (SCAN-23), with its selection.
    pub list: bool,
    pub sel: u8,
}

/// SIDE2 on home (SCAN-1): start a scan on the active slot and show it, or stop the one running. A slot on a
/// channel scans its zone (and the zones added with digits) in the Radio task, which asks for the channels'
/// frequencies as it goes ([`crate::chanscan`]); a VFO scans its band, with the radio's resume rule.
pub fn side2(app: &mut App, store: &impl ChannelStore) {
    let a = app.active;
    if app.scan_mask & 1 << a != 0 {
        return app.stop_scan(a, store); // SCAN-6
    }
    if app.low_power() {
        return app.lpm_refuse(); // PWR-24.4
    }
    if app.off_refuse(a) {
        return; // HOME-15.4: an off slot doesn't scan
    }
    if app.slot(a).on_channel() && app.slot(a).zone == crate::local::LOCAL && app.local.is_empty() {
        return crate::local::refuse(app); // CHAN-34: nothing to scan, and why
    }
    app.radio.clear_hits();
    app.radio.hits_seen = 0;
    app.radio.zones = 0;
    app.last_hit[a as usize] = 0;
    if app.slot(a).on_channel() {
        crate::chanscan::start(app, a);
    } else {
        app.out.push(RadioCmd::ScanResume { slot: a, code: crate::chanscan::preset_resume(app, 0) });
        app.out.push(RadioCmd::Scan { slot: a, on: true });
    }
    if app.set.scan_down {
        app.out.push(RadioCmd::ScanDir { slot: a, up: false, resume: false }); // SCAN-26: 0 1 6 2
    }
    app.screen = crate::Screen::Scan(Scan { slot: a, list: false, sel: 0 });
}

/// SCAN-27 (ferrite#83): SIDE2 on a range preset in MENU 0 1 4: the active slot's VFO on the preset (where it is,
/// if that's inside the range; else its start), at the preset's mode and step, then a scan of the preset's range
/// at its step, its skipped frequencies left out (SCAN-10) and its own resume rule (SCAN-24). A scan already
/// running there stops first; refused as SIDE2 on home is (Low Power Mode, an off slot).
pub fn preset(app: &mut App, i: u8, store: &impl ChannelStore) {
    use ferrite_core::msg::{RANGE_HI, RANGE_LO, RANGE_STEP};
    let d = store.data();
    let Some(p) = d.preset(i) else { return };
    let a = app.active;
    if app.low_power() {
        return app.lpm_refuse(); // PWR-24.4
    }
    if app.off_refuse(a) {
        return; // HOME-15.4
    }
    if app.scan_mask & 1 << a != 0 {
        app.stop_scan(a, store);
    }
    let cur = app.slot(a).freq();
    let f = if !app.slot(a).on_channel() && (p.start..=p.end).contains(&cur) { cur } else { p.start };
    app.set_vfo(a, f);
    let mut c = app.slot(a).cfg;
    (c.mode, c.narrow, c.step) = (p.mode, p.narrow, p.step);
    app.set_vfo_cfg(a, c);
    // the slot's retune and settings first: a retune stops a scan, so it goes before the scan starts
    app.sync_now(a);
    app.radio.clear_hits();
    app.radio.hits_seen = 0;
    app.radio.zones = 0;
    app.last_hit[a as usize] = 0;
    let step = crate::slot::STEPS[p.step as usize % crate::slot::STEPS.len()].0;
    app.out.push(RadioCmd::ScanRangeSet { slot: a, what: RANGE_LO, f: p.start });
    app.out.push(RadioCmd::ScanRangeSet { slot: a, what: RANGE_HI, f: p.end });
    app.out.push(RadioCmd::ScanRangeSet { slot: a, what: RANGE_STEP, f: step });
    // the skips (up to 48) and then the start, a few each tick ([`feed_range`]); the queue holds 32 commands
    app.range_feed = Some((a, i, 0, crate::chanscan::preset_resume(app, p.resume)));
    feed_range(app, store);
    app.screen = crate::Screen::Scan(Scan { slot: a, list: false, sel: 0 });
}

/// SCAN-27: the waiting preset scan's next skips to the Radio task, [`FEED_SKIPS`] a tick, then its resume rule
/// and its start (and SCAN-26's direction) once they are all there.
pub(crate) fn feed_range(app: &mut App, store: &impl ChannelStore) {
    use ferrite_core::msg::RANGE_SKIP;
    let Some((a, i, next, resume)) = app.range_feed else { return };
    let total = store.data().preset(i).map_or(0, |p| p.skips);
    let to = (next + FEED_SKIPS).min(total);
    for j in next..to {
        if let Some(s) = store.data().preset_skip(i, j) {
            app.out.push(RadioCmd::ScanRangeSet { slot: a, what: RANGE_SKIP, f: s });
        }
    }
    if to < total {
        app.range_feed = Some((a, i, to, resume));
        return;
    }
    app.range_feed = None;
    app.out.push(RadioCmd::ScanResume { slot: a, code: resume });
    app.out.push(RadioCmd::Scan { slot: a, on: true });
    if app.set.scan_down {
        app.out.push(RadioCmd::ScanDir { slot: a, up: false, resume: false }); // SCAN-26
    }
}

/// SCAN-27: skips sent each tick while a preset scan waits to start.
const FEED_SKIPS: u8 = 8;

/// The hits listed (SCAN-23): those kept, newest first.
fn listed(app: &App) -> u8 {
    app.radio.hits.iter().take_while(|h| h.what != 0).count() as u8
}

/// OK in the hit list: the scan stops and the slot's VFO goes to the hit; a channel scan's slot goes to the
/// hit's channel instead (SCAN-23.2). True when there was one.
fn tune_hit(app: &mut App, s: Scan, store: &impl ChannelStore) -> bool {
    let f = app.radio.hits[s.sel as usize % super::activity::HIT_LIST].what;
    if f == 0 {
        return false;
    }
    app.out.push(RadioCmd::Scan { slot: s.slot, on: false });
    match app.scan_chan(s.slot, f, store).filter(|_| app.slot(s.slot).on_channel()) {
        Some((z, pos)) => app.set_channel(s.slot, z, pos, store),
        None => app.set_vfo(s.slot, f),
    }
    app.go_home();
    true
}

/// A hit's channel name on a channel scan (SCAN-23.2); empty on a band scan, or for a channel without one.
fn hit_chan_name(app: &App, slot: u8, f: u32, store: &impl ChannelStore) -> Buf<16> {
    let mut b = Buf::new();
    if app.slot(slot).on_channel() {
        if let Some(c) = app.scan_chan(slot, f, store).and_then(|(z, p)| app.zat(store, z, p)) {
            b.set(c.name());
        }
    }
    b
}

/// A stop's length (SCAN-23.1): "<1 s", "14 s", "3 min".
pub fn span(s: u32, b: &mut Buf<16>) {
    b.clear();
    if s == 0 {
        b.set(b"<1 s");
    } else if s < 60 {
        let _ = uwrite!(b, "{} s", s);
    } else {
        let _ = uwrite!(b, "{} min", s / 60);
    }
}

/// The hit list's keys (SCAN-23).
fn list_key(s: Scan, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
    let n = listed(app).max(1);
    match (e.key, e.press) {
        (Key::Up | Key::Down, Press::Short | Press::Repeat) => {
            let d = if e.key == Key::Up { n - 1 } else { 1 };
            app.screen = crate::Screen::Scan(Scan { sel: (s.sel + d) % n, ..s });
        }
        (Key::Menu, Press::Short) => {
            let chan = app.slot(s.slot).on_channel();
            if tune_hit(app, s, store) {
                if chan {
                    let sl = *app.slot(s.slot);
                    let name = sl.ch.map_or(Buf::<16>::new(), |c| Buf::from(c.name()));
                    app.show_toast(b"Channel", name.as_bytes(), false);
                } else {
                    let fq = w::freq3(app.slot(s.slot).freq());
                    app.show_toast(b"VFO set", fq.as_bytes(), false);
                }
            }
        }
        // hold 9: the save form (VFO-9) on the hit, as on a home VFO (KEY-23); hold V/M does nothing here. A
        // channel scan's hits are channels already.
        (Key::Digit(9), Press::Long) if !app.slot(s.slot).on_channel() => {
            if tune_hit(app, s, store) {
                app.open(super::nav::Dest::SaveChannel);
            }
        }
        (Key::Exit, Press::Short) => app.screen = crate::Screen::Scan(Scan { list: false, ..s }),
        _ => {}
    }
}

impl View for Scan {
    fn name(self) -> &'static str {
        "Scan"
    }

    fn own_arrows(self) -> bool {
        true
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        if self.list {
            return list_key(self, app, e, store);
        }
        let s = self.slot;
        match (e.key, e.press) {
            (Key::Menu, Press::Short) => {
                if listed(app) == 0 {
                    return app.show_toast(b"No hits yet", b"", false);
                }
                app.screen = crate::Screen::Scan(Scan { list: true, sel: 0, ..self });
            }
            (Key::Side(2), Press::Short) => {
                app.stop_scan(s, store); // SCAN-6
                app.go_home();
            }
            (Key::Star, Press::Short) => skip(app, s),
            (Key::Digit(d), Press::Short) if app.slot(s).on_channel() => {
                // SCAN-3: zones by number join or leave the set (0-9 here).
                app.radio.zones ^= 1 << d;
                crate::chanscan::start(app, s); // the new set, the scan going on from here
                let mut b = Buf::<28>::new();
                let _ = uwrite!(b, "Zone {} ", d);
                b.push(if app.radio.zones & 1 << d != 0 { b"added" } else { b"removed" });
                app.show_toast(b.as_bytes(), b"", false);
            }
            (Key::Up | Key::Down, Press::Short) => {
                // SCAN-16: the direction from here on; stopped on a hit, leave it now.
                let (up, resume) = (e.key == Key::Up, app.scan_hit[s as usize] != 0);
                app.out.push(RadioCmd::ScanDir { slot: s, up, resume });
                app.show_toast(if resume { b"Skipped" } else { b"Scanning" }, if up { b"up" } else { b"down" }, false);
            }
            (Key::Hash, Press::Short) => lasting_skip(app, s, store),
            (Key::Exit, Press::Short) => app.back(), // the scan keeps running
            _ => {}
        }
    }

    fn tick(self, app: &mut App, live: &LiveAll) {
        let lv = &live.slots[self.slot as usize];
        if lv.hits != app.radio.hits_seen {
            // a new stop (SCAN-23): counted, at the front
            app.radio.hits_seen = lv.hits;
            let f = if lv.last_hit != 0 { lv.last_hit } else { lv.freq };
            app.radio.push_hit(Hit::new(f, app.now, lv.rssi), true);
        } else if lv.scanning && lv.squelch_open {
            app.radio.push_hit(Hit::new(lv.freq, app.now, lv.rssi), false);
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        app.page(lcd, self.list);
        if self.list {
            return hit_list(self, app, lcd, cx.store);
        }
        let s = self.slot;
        let lv = cx.live.slots[s as usize];
        let sl = *app.slot(s);
        // The slot card: letter, SCAN and band, the state; the frequency it is on now; the channel's name.
        let (y, h) = (25, 92);
        let (state, sc) = if !lv.scanning {
            ("stopped", AMBER)
        } else if lv.squelch_open {
            ("RX", GREEN)
        } else {
            ("scanning", SUB)
        };
        let first = app.drawn.hash[1] == 0;
        if app.dirty(1, Mix::new().h(&(state == "RX")).get()) {
            let border = if state == "RX" { GREEN } else { BLUE };
            if first {
                lcd.fill(6, y, 228, h, BG);
                lcd.fill_r(6, y, 228, h, 3, SURFACE);
                lcd.frame_r(6, y, 228, h, 3, border);
                app.drawn.hash[2..5].fill(0);
            } else {
                lcd.frame_r(6, y, 228, h, 3, border); // the border's colour alone (DISP-25)
            }
        }
        let mut head = Buf::<32>::new();
        head.push(b"SCAN");
        head.push(" \u{b7} ".as_bytes());
        head.push(band_name(lv.freq).as_bytes());
        if app.dirty(2, Mix::new().h(&head).h(&state).get()) {
            w::badge(lcd, 14, y + 8, s, true, SURFACE);
            w::field(lcd, 34, 120, y + 7, y + 24, y + 20, head.as_bytes(), const { &w::st(&UI_12, SUB, SURFACE, Align::Left) });
            if state == "RX" {
                ferrite_ui::canvas::banded(lcd, 154, y + 6, 74, 18, SURFACE, |lcd| {
                    w::tag(lcd, 226, y + 7, b"RX", GREEN, SURFACE);
                });
            } else {
                w::field(lcd, 154, 72, y + 7, y + 24, y + 20, state.as_bytes(), &w::st(&UI_12, sc, SURFACE, Align::Right));
            }
        }
        if app.dirty(3, Mix::new().h(&lv.freq).get()) {
            w::freq(lcd, 15, 226, y + 64, lv.freq, &DIG_46, &DIG_28, TEXT, SUB, SURFACE);
        }
        // The channel it stopped on, or the S-meter text.
        let mut name = Buf::<32>::new();
        if sl.on_channel() {
            if let Some(c) = channel_on(app, cx.store, sl.zone, lv.freq) {
                chan_label(c.num, &mut name);
                name.push(" \u{b7} ".as_bytes());
                name.push(c.name());
            }
        }
        let st = if lv.squelch_open { w::s_text(lv.rssi) } else { Buf::new() };
        if app.dirty(4, Mix::new().h(&name).h(&st).get()) {
            w::field(lcd, 15, 170, y + 70, y + 88, y + 84, name.as_bytes(), const { &w::st(&UI_12, TEXT, SURFACE, Align::Left) });
            w::field(lcd, 185, 41, y + 70, y + 88, y + 84, st.as_bytes(), const { &w::st(&UI_12B, GREEN, SURFACE, Align::Right) });
        }
        // SCAN-7: target, resume, position, hits, skips.
        let mut target = Buf::<28>::new();
        let mut pos = Buf::<28>::new();
        if sl.on_channel() {
            target.push(b"Zone ");
            target.push(cx.store.zone_label(sl.zone, b"?", b"?").as_bytes()); // "Local" for Local (CHAN-34)
            let z = app.radio.zones;
            if z != 0 {
                for d in (0..10).filter(|d| z & 1 << d != 0) {
                    let _ = uwrite!(target, " +{}", d);
                }
            }
            let n = app.zlen(cx.store, sl.zone);
            let i = (0..n).position(|i| app.zat(cx.store, sl.zone, i).is_some_and(|c| c.rx == lv.freq));
            match i {
                Some(i) => {
                    let _ = uwrite!(pos, "{} of {}", i + 1, n);
                }
                None => pos.set(b"-"),
            }
        } else {
            let b = &BANDS[band_near(lv.freq)];
            target.push(b"Band ");
            target.push(b.name.as_bytes());
            let _ = uwrite!(pos, "{} to {} MHz", b.lo / 100_000, b.hi / 100_000);
        }
        let mut hits = Buf::<28>::new();
        let _ = uwrite!(hits, "{}", lv.hits);
        let mut skips = Buf::<28>::new();
        let _ = uwrite!(skips, "{}", lv.skips);
        // SCAN-5: the zone's own rule over the radio's (0 1 2)
        let own = if sl.on_channel() && sl.zone != crate::chan::NO_ZONE { cx.store.data().zone_cfg(sl.zone).resume } else { 0 };
        let mut resume = Buf::<24>::new();
        super::groups::resume_text(&mut resume, ferrite_core::scan::Resume::code_over(own, app.set.scan_resume));
        let rows: [(&[u8], &[u8]); 5] = [
            (b"Target", target.as_bytes()),
            (b"Resume", resume.as_bytes()),
            (b"Position", pos.as_bytes()),
            (b"Hits", hits.as_bytes()),
            (b"Skips", skips.as_bytes()),
        ];
        for (i, (l, v)) in rows.iter().enumerate() {
            app.kv(lcd, 4 + i, 122 + 18 * i as i32, 18, &w::KV_STATUS, l, v, TEXT, false);
        }
        // The last hits: how long ago, what (a channel scan's by name), how long it lasted (SCAN-23.1), how strong.
        if app.dirty(10, 1) {
            w::field(lcd, 8, 224, 214, 230, 226, b"Last hits", const { &w::st(&UI_12B, SUB, BG, Align::Left) });
        }
        for i in 0..super::activity::HITS {
            let hit = app.radio.hits[i];
            let mut a = Buf::<16>::new();
            let mut what = Buf::<32>::new();
            let mut len = Buf::<16>::new();
            let mut sig = Buf::<4>::new();
            if hit.what != 0 {
                ago(hit.age_ms(app.now), &mut a);
                let name = hit_chan_name(app, s, hit.what, cx.store);
                if name.is_empty() {
                    what.push(w::freq3(hit.what).as_bytes());
                } else {
                    what.push(name.as_bytes());
                }
                span(hit.len_s(), &mut len);
                sig = w::s_text(hit.rssi());
            } else if i == 0 {
                what.set(b"None yet");
            }
            let y = 232 + 18 * i as i32;
            if app.dirty(11 + i, Mix::new().h(&a).h(&what).h(&len).h(&sig).get()) {
                w::field(lcd, 8, 46, y, y + 18, y + 13, a.as_bytes(), const { &w::st(&UI_12, SUB, BG, Align::Left) });
                w::field(lcd, 54, 104, y, y + 18, y + 13, w::fit(what.as_bytes(), &UI_12, 104), const { &w::st(&UI_12, TEXT, BG, Align::Left) });
                w::field(lcd, 158, 40, y, y + 18, y + 13, len.as_bytes(), const { &w::st(&UI_12, SUB, BG, Align::Right) });
                w::field(lcd, 200, 32, y, y + 18, y + 13, sig.as_bytes(), const { &w::st(&UI_12B, GREEN, BG, Align::Right) });
            }
        }
    }

    fn hints(self, app: &App) -> Hints {
        if self.list {
            return Hints::new(&[("OK", "Tune"), ("Hold 9", "Save"), ("BACK", "Back")]);
        }
        if app.scan_hit[self.slot as usize] != 0 && app.slot(self.slot).on_channel() {
            Hints::new(&[("*", "Skip"), ("#", "Skip always"), (ARROWS_UD, "Go on")])
        } else if app.scan_hit[self.slot as usize] != 0 {
            Hints::new(&[("*", "Skip"), (ARROWS_UD, "Go on"), ("SIDE2", "Stop")])
        } else if app.slot(self.slot).on_channel() {
            // SCAN-23.2: OK for the hits here too; ▲▼ still set the direction, unlabelled for room
            Hints::new(&[("0-9", "Zones"), ("OK", "Hits"), ("SIDE2", "Stop")])
        } else {
            Hints::new(&[(ARROWS_UD, "Direction"), ("OK", "Hits"), ("SIDE2", "Stop")])
        }
    }
}

/// SCAN-23: the hits, newest first, each with how often it stopped the scan, how long its last stop lasted
/// (SCAN-23.1) and when that was; a channel scan's by name (SCAN-23.2).
fn hit_list(s: Scan, app: &mut App, lcd: &mut impl Panel, store: &impl ChannelStore) {
    let n = listed(app);
    let mut r = Buf::<16>::new();
    let _ = uwrite!(r, "{} of {}", n, super::activity::HIT_LIST);
    app.list_head(lcd, b"Scan hits", r.as_bytes());
    // 9 rows on the page, scrolled to keep the selection in view
    let top = s.sel.saturating_sub(8);
    for k in 0..9u8 {
        let i = (top + k) as usize;
        let h = app.radio.hits.get(i).copied().unwrap_or_default();
        let (mut name, mut v) = (Buf::<16>::new(), Buf::<24>::new());
        if h.what != 0 {
            name = hit_chan_name(app, s.slot, h.what, store);
            if name.is_empty() {
                name.set(w::freq3(h.what).as_bytes());
            }
            let (mut a, mut l) = (Buf::<16>::new(), Buf::<16>::new());
            ago(h.age_ms(app.now), &mut a);
            span(h.len_s(), &mut l);
            let _ = uwrite!(v, "{}x  ", h.n);
            v.push(l.as_bytes());
            v.push(b"  ");
            v.push(a.as_bytes());
        } else if i == 0 {
            name.set(b"None yet");
        }
        app.row1(lcd, k as usize, name.as_bytes(), v.as_bytes(), false, h.what != 0 && i == s.sel as usize, false);
    }
    let help = if app.slot(s.slot).on_channel() { txt!("OK goes to that channel.") } else { txt!("OK tunes the VFO there; hold 9 saves it.") };
    app.help_txt(lcd, 300 - 38, 34, help, b"");
}

/// SCAN-17: # on a hit sets the channel's Skip flag (CHAN-4), so no scan stops there again, and the scan goes
/// on. A band scan has no list to keep it in: \* still skips it for this scan.
fn lasting_skip(app: &mut App, s: u8, store: &impl ChannelStore) {
    let f = app.scan_hit[s as usize];
    if f == 0 {
        return app.show_toast(b"Nothing to skip", b"", false);
    }
    let sl = *app.slot(s);
    if !sl.on_channel() {
        return app.show_toast(b"No skip list", b"* skips it once", false);
    }
    let zones = core::iter::once(sl.zone).chain((0..10 as ZoneId).filter(|z| app.radio.zones & 1 << z != 0));
    let Some(mut c) = zones.filter_map(|z| channel_on(app, store, z, f)).next() else {
        return app.show_toast(b"Nothing to skip", b"", false);
    };
    c.flags |= FLAG_SKIP;
    app.put_channel(c);
    app.out.push(RadioCmd::ScanSkip { slot: s });
    let mut b = Buf::<32>::new();
    chan_label(c.num, &mut b);
    b.push(b" skipped");
    app.show_toast(b.as_bytes(), b"for every scan", false);
}

/// The zone's channel on this frequency, if any (Local's too, CHAN-34).
fn channel_on(app: &App, store: &impl ChannelStore, zone: ZoneId, f: u32) -> Option<crate::chan::Channel> {
    (0..app.zlen(store, zone)).filter_map(|i| app.zat(store, zone, i)).find(|c| c.rx == f)
}

/// "12 s", "3 min", "2 h".
pub fn ago(ms: u32, b: &mut Buf<16>) {
    let s = ms / 1000;
    b.clear();
    if s == 0 {
        b.set(b"now"); // "0 s" reads like a fault
    } else if s < 60 {
        let _ = uwrite!(b, "{} s", s);
    } else if s < 3600 {
        let _ = uwrite!(b, "{} min", s / 60);
    } else {
        let _ = uwrite!(b, "{} h", s / 3600);
    }
}

/// SCAN-11, KEY-18: \* skips the current hit for this scan, on a hit or in the resume wait after one (the scan
/// screen, and home on a scanning slot).
pub(crate) fn skip(app: &mut App, s: u8) {
    if app.rx_mask & 1 << s != 0 || app.radio.hits[0].what != 0 && app.radio.hits[0].age_ms(app.now) < 6000 {
        app.out.push(RadioCmd::ScanSkip { slot: s });
        app.show_toast(b"Skipped", b"for this scan", false);
    } else {
        app.show_toast(b"Nothing to skip", b"", false);
    }
}
