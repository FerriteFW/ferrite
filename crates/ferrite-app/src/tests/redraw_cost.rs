//! What redraws cost on the radio's bus and in its slow flash (DISP-29): bytes and address windows sent to the
//! panel, pixels written more than once, and how often a region's drawing code runs; and that painting each pixel
//! once ends in the same picture as clearing first (`lcd paint old`, TERM-40).

use super::*;
use crate::screens::nav::{Dest, ALL, MENU0};
use ferrite_ui::canvas::{draw_runs, set_single_pass};
use ferrite_ui::widgets as w;

/// The panel as the radio's bus sees it: pixels, address windows (CASET, RASET, RAMWR: 11 bytes), calls of any
/// kind, and the picture with each pixel's writes. A window with no pixels after it (the simulator's text-hint
/// pass) is not counted.
struct Probe {
    fb: FrameBuffer,
    windows: u64,
    px: u64,
    calls: u64,
    open: Option<i32>,
    /// The rows of the first windows that got pixels, in order.
    order: Vec<i32>,
}

impl Probe {
    fn new() -> Self {
        Probe { fb: FrameBuffer::new(), windows: 0, px: 0, calls: 0, open: None, order: Vec::new() }
    }
    fn bytes(&self) -> u64 {
        2 * self.px + 11 * self.windows
    }
    /// A new frame's counts.
    fn reset(&mut self) {
        (self.windows, self.px, self.calls, self.open) = (0, 0, 0, None);
        self.order.clear();
        self.fb.writes.iter_mut().for_each(|w| *w = 0);
    }
    /// The most writes any pixel of rows y0..y1 got since [`Probe::reset`].
    fn most(&self, y0: i32, y1: i32) -> u16 {
        self.fb.writes[(y0 * 240) as usize..(y1 * 240) as usize].iter().copied().max().unwrap_or(0)
    }
    fn sent(&mut self, n: u64) {
        if let Some(y) = self.open.take() {
            self.windows += 1;
            self.order.push(y);
        }
        self.px += n;
    }
}

impl Panel for Probe {
    fn window(&mut self, a: i32, b: i32, c: i32, d: i32) {
        self.calls += 1;
        self.open = Some(b);
        self.fb.window(a, b, c, d)
    }
    fn push(&mut self, c: u16, n: u32) {
        self.calls += 1;
        self.sent(n as u64);
        self.fb.push(c, n)
    }
    fn push_buf(&mut self, p: &[u16]) {
        self.calls += 1;
        self.sent(p.len() as u64);
        self.fb.push_buf(p)
    }
}

fn dev() -> Device {
    Device { clock_min: Some(600), gps: GpsState::Fix, battery_pct: 80, ..Default::default() }
}

/// Home with A receiving, drawn once whole.
fn home() -> (T, Probe) {
    let mut t = T::new();
    t.live.slots[0] = SlotLive { freq: t.freq(), squelch_open: true, carrier: true, rssi: -100, ..Default::default() };
    let mut p = Probe::new();
    t.app.render(&mut p, &t.live, &dev(), &store());
    (t, p)
}

/// One frame into `p`, counted afresh; returns how many times a region's drawing ran.
fn frame(t: &mut T, p: &mut Probe) -> u32 {
    p.reset();
    let runs = draw_runs();
    t.app.render(p, &t.live, &dev(), &store());
    draw_runs() - runs
}

/// DISP-29: a frame where nothing changed sends nothing and asks nothing of the panel, and runs no region's drawing.
#[test]
fn a_still_home_frame_costs_nothing() {
    let (mut t, mut p) = home();
    for _ in 0..3 {
        t.wait(100);
        let runs = frame(&mut t, &mut p);
        assert_eq!((p.bytes(), p.windows, p.calls, runs), (0, 0, 0, 0), "nothing changed, nothing drawn");
    }
}

/// DISP-29: SLOT draws the new large card first, runs each card's drawing once (not once per band), writes each
/// pixel at most once, and ends in the same picture as before (each card drawn once per band, `lcd paint old`).
#[test]
fn slot_draws_the_large_card_first_and_once() {
    set_single_pass(false);
    let (mut old, mut po) = home();
    set_single_pass(true);
    let (mut new, mut pn) = home();
    for _ in 0..3 {
        new.tap(Key::Slot);
        let runs = frame(&mut new, &mut pn);
        let a = new.app.active();
        let (y, h) = crate::screens::home::card(a, a);
        assert!((y..y + h).contains(&pn.order[0]), "the large card first: rows {:?}", &pn.order[..4]);
        assert!(runs <= 4, "each card's drawing once (and the hint bar's): {runs}");
        assert!(pn.most(21, 300) <= 1, "a pixel written twice");
        set_single_pass(false);
        old.tap(Key::Slot);
        let old_runs = frame(&mut old, &mut po);
        set_single_pass(true);
        assert!(old_runs > 30, "before: once per band ({old_runs})");
        assert!(po.fb.px == pn.fb.px, "the same picture");
        assert!(pn.bytes() <= po.bytes() + 100, "no more bytes: {} vs {}", pn.bytes(), po.bytes());
    }
}

/// DISP-29: a new screen (every destination, home, and the panel woken) no longer clears its content area before
/// painting it: each pixel gets the writes it got before, less the clear's where the content paints it whole (the
/// background alone where nothing does), never more, and the picture is the one clearing first gives (`lcd paint old`).
/// A screen whose drawing doesn't fit the recording clears first as before, write for write.
#[test]
fn new_screens_drop_the_clear() {
    let dests: Vec<Dest> = ALL.into_iter().chain(MENU0.map(|(_, d)| d)).collect();
    let (mut fresh, mut saved) = (0, 0u64);
    for dest in &dests {
        let mut runs: Vec<(Vec<u16>, Vec<u16>, u64)> = Vec::new();
        for single in [true, false] {
            set_single_pass(single);
            let (mut t, mut p) = home();
            t.hold(Key::Exit);
            frame(&mut t, &mut p);
            t.app.open(*dest);
            t.wait(100);
            frame(&mut t, &mut p);
            runs.push((p.fb.px.clone(), p.fb.writes.clone(), p.bytes()));
        }
        set_single_pass(true);
        let (new, old) = (&runs[0], &runs[1]);
        assert!(new.0 == old.0, "{dest:?}: not the picture clearing first gives");
        let area = (w::SBAR_H * 240) as usize..(w::HINT_Y * 240) as usize;
        // each pixel: as before, or one write fewer (never none); outside the content area as before
        assert!(new.1.iter().zip(&old.1).all(|(&n, &o)| n <= o && n >= o.saturating_sub(1).max(1).min(o)), "{dest:?}: writes");
        assert!(new.1[..area.start] == old.1[..area.start] && new.1[area.end..] == old.1[area.end..], "{dest:?}: outside");
        // without the clear, most of the area has one write fewer (outlines and rounded corners keep a background)
        let fewer = area.clone().filter(|&i| new.1[i] < old.1[i]).count();
        fresh += (fewer > area.len() / 2) as u32;
        saved += old.2 - new.2;
    }
    // the few screens too big to record clear first, as before
    assert!(fresh as usize >= dests.len() * 9 / 10, "{fresh} of {} screens without the clear", dests.len());
    assert!(saved / dests.len() as u64 > 50_000, "{} bytes saved a screen", saved / dests.len() as u64);
    // the panel woken, and home itself
    let (mut t, mut p) = home();
    for _ in 0..2 {
        t.app.redraw_all();
        frame(&mut t, &mut p);
        assert!(p.most(0, 320) <= 1, "woken: a pixel of home written twice");
        t.tap(Key::Menu);
        frame(&mut t, &mut p);
        t.tap(Key::Exit);
        frame(&mut t, &mut p);
        assert!(p.most(w::SBAR_H, w::HINT_Y) <= 1, "home again: a pixel written twice");
    }
}

/// The numbers for the record (DISP-29): `cargo test -p ferrite-app --release --lib redraw_cost_report --
/// --ignored --nocapture`. Bytes and windows a frame, region drawing runs, host time as a rough CPU proxy.
#[test]
#[ignore]
fn redraw_cost_report() {
    for single in [false, true] {
        set_single_pass(single);
        std::println!("--- {}", if single { "new: each pixel once" } else { "old: clear, then paint" });
        let (mut t, mut p) = home();
        let say = |what: &str, p: &Probe, runs: u32, ns: u128| {
            std::println!("{what}: {} B, {} windows, {} region runs, {} us host", p.bytes(), p.windows, runs, ns / 1000);
        };
        let t0 = std::time::Instant::now();
        let runs = frame(&mut t, &mut p);
        say("still home frame", &p, runs, t0.elapsed().as_nanos());
        t.tap(Key::Slot);
        let t0 = std::time::Instant::now();
        let runs = frame(&mut t, &mut p);
        say("SLOT", &p, runs, t0.elapsed().as_nanos());
        t.tap(Key::Menu);
        let t0 = std::time::Instant::now();
        let runs = frame(&mut t, &mut p);
        say(&std::format!("open {}", t.app.screen().name()), &p, runs, t0.elapsed().as_nanos());
        t.tap(Key::Exit);
        let t0 = std::time::Instant::now();
        let runs = frame(&mut t, &mut p);
        say("back to home", &p, runs, t0.elapsed().as_nanos());
        t.app.redraw_all();
        let t0 = std::time::Instant::now();
        let runs = frame(&mut t, &mut p);
        say("panel woken", &p, runs, t0.elapsed().as_nanos());
        // a scan on home: SIDE2 starts it (the scan screen), BACK to home, then steps
        t.tap(Key::Side(2));
        frame(&mut t, &mut p);
        t.tap(Key::Exit);
        let base = t.freq();
        for i in 0..3u32 {
            t.live.slots[0] = SlotLive { freq: base + i * 2_500, scanning: true, rssi: -121, ..Default::default() };
            t.wait(100);
            let t0 = std::time::Instant::now();
            let runs = frame(&mut t, &mut p);
            say(&std::format!("scan step {i} on {}", t.app.screen().name()), &p, runs, t0.elapsed().as_nanos());
        }
    }
    set_single_pass(true);
}
