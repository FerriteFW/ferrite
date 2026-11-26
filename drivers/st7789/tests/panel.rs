//! The driver against a recording bus.

use st7789::{
    Bus8080, St7789, Step, CASET, DISPOFF, DISPON, IDMOFF, IDMON, INVOFF, INVON, NORON, PTLAR, PTLON, RAMWR, RASET, SLPIN, SLPOUT, MODES, STARTUP,
    VSCRDEF, VSCSAD,
};
use uradio_bus::mock::MockDelay;

#[derive(Debug, PartialEq, Clone)]
enum Ev {
    Cmd(u8),
    Data(Vec<u8>),
    Reset(bool),
}

#[derive(Default)]
struct Rec(Vec<Ev>);

impl Bus8080 for Rec {
    fn command(&mut self, c: u8) {
        self.0.push(Ev::Cmd(c));
    }
    fn data(&mut self, d: &[u8]) {
        // Merge consecutive data so tests compare whole payloads.
        if let Some(Ev::Data(v)) = self.0.last_mut() {
            v.extend_from_slice(d);
        } else {
            self.0.push(Ev::Data(d.to_vec()));
        }
    }
    fn reset(&mut self, a: bool) {
        self.0.push(Ev::Reset(a));
    }
}

#[test]
fn startup_matches_stock_order_and_waits() {
    let mut p = St7789::new(Rec::default(), 240, 320);
    let mut d = MockDelay::default();
    p.init(&mut d, 0x0000);
    let ev = p.release().0;
    // Reset pulse, then SWRESET, SLPOUT, COLMOD 0x55, MADCTL 0xC0, INVON, NORON.
    assert_eq!(&ev[..3], &[Ev::Reset(false), Ev::Reset(true), Ev::Reset(false)]);
    assert_eq!(ev[3..11], [Ev::Cmd(0x01), Ev::Cmd(0x11), Ev::Cmd(0x3A), Ev::Data(vec![0x55]), Ev::Cmd(0x36), Ev::Data(vec![0xC0]), Ev::Cmd(0x21), Ev::Cmd(0x13)]);
    // Waits: 1 + 10 + 120 + 150 + 120 ms.
    assert_eq!(d.total_us, 401_000);
    // Cleared (a full-screen window, 240 x 320 x 2 bytes) before the display goes on.
    assert_eq!(ev[ev.len() - 1], Ev::Cmd(DISPON));
    assert_eq!(ev[ev.len() - 2], Ev::Data(vec![0; 240 * 320 * 2]));
    let waits: u32 = STARTUP.iter().map(|s| if let Step::Wait(ms) = s { *ms } else { 0 }).sum();
    assert_eq!(waits, 401);
}

#[test]
fn windows_clip_and_count() {
    let mut p = St7789::new(Rec::default(), 240, 320);
    assert_eq!(p.window(230, 310, 20, 20), 100); // clipped to 10 x 10
    assert_eq!(p.window(240, 0, 1, 1), 0); // off the panel: nothing sent
    assert_eq!(p.window(0, 0, 0, 5), 0);
    let ev = p.release().0;
    assert_eq!(ev, vec![Ev::Cmd(CASET), Ev::Data(vec![0, 230, 0, 239]), Ev::Cmd(RASET), Ev::Data(vec![1, 54, 1, 63]), Ev::Cmd(RAMWR)]);
}

#[test]
fn pixels_and_fill_are_big_endian() {
    let mut p = St7789::new(Rec::default(), 240, 320);
    p.blit(0, 0, 2, 1, &[0x1234, 0xABCD]);
    p.fill(5, 5, 3, 1, 0xF800);
    let ev = p.release().0;
    assert_eq!(ev[5], Ev::Data(vec![0x12, 0x34, 0xAB, 0xCD]));
    assert_eq!(ev[11], Ev::Data(vec![0xF8, 0, 0xF8, 0, 0xF8, 0]));
}

#[test]
fn scrolling() {
    let mut p = St7789::new(Rec::default(), 240, 320);
    p.scroll_area(20, 20); // status and hint bars fixed, 280 lines scroll
    p.scroll_to(100);
    let ev = p.release().0;
    assert_eq!(ev, vec![Ev::Cmd(VSCRDEF), Ev::Data(vec![0, 20, 1, 24, 0, 20]), Ev::Cmd(VSCSAD), Ev::Data(vec![0, 100])]);
}

#[test]
fn size_is_what_it_was_built_with() {
    let p = St7789::new(Rec::default(), 240, 320);
    assert_eq!((p.width(), p.height()), (240, 320));
    // `new` sends nothing.
    assert!(p.release().0.is_empty());
}

/// Each mode switch is one bare command with the datasheet's code (chapter 9: 10h/11h sleep, 20h/21h inversion,
/// 28h/29h display, 38h/39h idle).
#[test]
fn mode_switches_are_single_commands() {
    let mut p = St7789::new(Rec::default(), 240, 320);
    p.on(true);
    p.on(false);
    p.invert(true);
    p.invert(false);
    p.sleep(true);
    p.sleep(false);
    p.idle(true);
    p.idle(false);
    let ev = p.release().0;
    let want: Vec<Ev> = [DISPON, DISPOFF, INVON, INVOFF, SLPIN, SLPOUT, IDMON, IDMOFF].iter().map(|&c| Ev::Cmd(c)).collect();
    assert_eq!(ev, want);
    assert_eq!([DISPON, DISPOFF, INVON, INVOFF, SLPIN, SLPOUT, IDMON, IDMOFF], [0x29, 0x28, 0x21, 0x20, 0x10, 0x11, 0x39, 0x38]);
}

/// Partial mode: PTLAR (30h) with start and end rows high byte first, then PTLON (12h); back with NORON (13h).
#[test]
fn partial_mode_and_back() {
    let mut p = St7789::new(Rec::default(), 240, 320);
    p.partial(Some((10, 300)));
    p.partial(None);
    let ev = p.release().0;
    assert_eq!(ev, vec![Ev::Cmd(PTLAR), Ev::Data(vec![0, 10, 1, 44]), Ev::Cmd(PTLON), Ev::Cmd(NORON)]);
    assert_eq!((PTLAR, PTLON, NORON), (0x30, 0x12, 0x13));
}

#[test]
fn steps_return_their_waits() {
    let mut p = St7789::new(Rec::default(), 240, 320);
    assert_eq!(p.step(Step::Wait(150)), Some(150));
    assert_eq!(p.step(Step::Reset(true)), None);
    assert_eq!(p.step(Step::Command(0x3A, &[0x55])), None);
    // A command with no data sends no data phase.
    assert_eq!(p.step(Step::Command(0x01, &[])), None);
    let ev = p.release().0;
    assert_eq!(ev, vec![Ev::Reset(true), Ev::Cmd(0x3A), Ev::Data(vec![0x55]), Ev::Cmd(0x01)]);
}

#[test]
fn repeat_and_pixels_stream_after_a_window() {
    let mut p = St7789::new(Rec::default(), 240, 320);
    assert_eq!(p.window(0, 0, 40, 2), 80);
    // More than one 32-pixel chunk through the default `pixels`, then a run through `repeat`.
    let px: Vec<u16> = (0..40u16).map(|i| 0x0100 | i).collect();
    p.pixels(&px);
    p.repeat(0x07E0, 40);
    let ev = p.release().0;
    let mut want: Vec<u8> = px.iter().flat_map(|c| c.to_be_bytes()).collect();
    for _ in 0..40 {
        want.extend_from_slice(&[0x07, 0xE0]);
    }
    assert_eq!(ev[5], Ev::Data(want));
    assert_eq!(ev.len(), 6);
}

#[test]
fn off_panel_fill_and_short_blit() {
    let mut p = St7789::new(Rec::default(), 240, 320);
    p.fill(0, 320, 10, 10, 0xFFFF); // below the panel: nothing at all
    p.blit(0, 0, 4, 1, &[0xAAAA, 0xBBBB]); // fewer pixels than the window: only those sent
    p.blit(0, 0, 1, 1, &[0x1111, 0x2222]); // more: the rest ignored
    let ev = p.release().0;
    assert_eq!(ev[..5], [Ev::Cmd(CASET), Ev::Data(vec![0, 0, 0, 3]), Ev::Cmd(RASET), Ev::Data(vec![0, 0, 0, 0]), Ev::Cmd(RAMWR)]);
    assert_eq!(ev[5], Ev::Data(vec![0xAA, 0xAA, 0xBB, 0xBB]));
    assert_eq!(ev[11], Ev::Data(vec![0x11, 0x11]));
    assert_eq!(ev.len(), 12);
}

#[test]
fn scroll_area_saturates_when_the_fixed_bars_cover_the_panel() {
    let mut p = St7789::new(Rec::default(), 240, 320);
    p.scroll_area(200, 200);
    let ev = p.release().0;
    assert_eq!(ev, vec![Ev::Cmd(VSCRDEF), Ev::Data(vec![0, 200, 0, 0, 0, 200])]);
}

/// DISP-32: `modes` sends the start-up's mode registers again (COLMOD 0x55, MADCTL 0xC0, INVON), in the start-up's
/// order and nothing else: no reset, no sleep or display command, no wait.
#[test]
fn modes_resend_the_startup_mode_registers() {
    let mut p = St7789::new(Rec::default(), 240, 320);
    p.modes();
    let ev = p.release().0;
    assert_eq!(ev, [Ev::Cmd(0x3A), Ev::Data(vec![0x55]), Ev::Cmd(0x36), Ev::Data(vec![0xC0]), Ev::Cmd(INVON)]);
    // The same steps as in STARTUP, in the same order.
    let at = STARTUP.iter().position(|s| *s == MODES[0]).unwrap();
    assert_eq!(&STARTUP[at..at + MODES.len()], MODES);
}
