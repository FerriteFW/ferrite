//! The boot screen: logo, wordmark, version and callsign, nothing else while the start-up is quick (BOOT-9,
//! decision 215: no progress bar, no "Self-test n of m"); a step that takes long gets one line naming it ([`BootInfo::note`]).
//! A failed check shows on the same screen, in red, naming the check; the radio then waits for a key (BOOT-10).
//! With the boot screen set to the owner card (MENU 0 7 3 3, SETUP-11), the card fills the screen instead and
//! the wordmark, version and self-test shrink to its bottom line (BootC mock-up, less its small logo); a failed
//! check still shows the logo screen, as the failure matters more.
//!
//! BOOT-18 (decision 321): the logo screen animates for its first [`ANIM_MS`] ("Charge and flash",
//! research/boot-animation/preview.html): the 14 windings fill in dim 25 ms apart, then the logo and the wordmark
//! flash bright together with a glow that settles. [`draw_at`] draws the screen as the animation has it `t` ms
//! in, [`frame`] only the logo and the wordmark (the version, the line and the status are never redrawn by it);
//! from [`ANIM_MS`] on both are the still screen's pixels. The owner card doesn't animate.
use crate::assets::*;
use crate::bootanim::{self, Logo, Name, WINDINGS};
use crate::gfx::{blend, text4_ink, text4_width, Align, Gfx, BG, BLUE, LINE, RED, SUB, TEXT};
use crate::panel::Panel;
use crate::theme::{ink_on, map, map_bg};
use crate::text::Buf;
use crate::v;
use crate::widgets::{fit, hint_bar};

#[derive(Clone, Copy, PartialEq)]
pub enum State {
    Pending,
    Ok,
    Off,
    Fail,
}

#[derive(Clone, Copy)]
pub struct Check {
    pub label: &'static str,
    pub value: &'static str,
    pub state: State,
}

/// The owner card's lines (SETUP-11): user data, any bytes. Characters the fonts lack draw as '?', and a line
/// too long for the screen is cut.
#[derive(Clone, Copy, Default)]
pub struct Owner<'a> {
    pub name: &'a [u8],
    pub phone: &'a [u8],
    pub email: &'a [u8],
}

pub struct BootInfo<'a> {
    pub version: &'a str,
    /// The owner's callsign, on the owner card; None until one is set.
    pub callsign: Option<&'a [u8]>,
    /// The logo screen's line under the version: Power-on message (0 7 3 2) gives the callsign, a text or None.
    pub line: Option<&'a [u8]>,
    /// The owner card, when the boot screen is set to it (MENU 0 7 3 3); None: the logo.
    pub owner: Option<Owner<'a>>,
    pub checks: &'a [Check],
    /// A slow step's line ("Checking GPS…", "Installing SSB patch"); None: the status line stays empty.
    pub note: Option<&'a [u8]>,
}

/// The text column's width, 12 px margins.
const COL: i32 = 216;

/// The callsign in capitals (SETUP-12), at most 16 characters.
fn upper(call: &[u8], out: &mut [u8; 16]) -> usize {
    let n = call.len().min(out.len());
    for (d, s) in out.iter_mut().zip(call) {
        *d = s.to_ascii_uppercase();
    }
    n
}

/// Full screen.
pub fn draw(lcd: &mut impl Panel, b: &BootInfo) {
    paint(lcd, b, true)
}

/// Only what the self-test changes: the bar and the status line (the logo, wordmark and card stay). Redrawing the
/// whole screen per check flashed the logo on the radio (Oct 12, 2026). Call after [`draw`], with the same
/// owner/failed layout (a failure switches from the card to the logo screen: [`draw`] again then).
pub fn update(lcd: &mut impl Panel, b: &BootInfo) {
    paint(lcd, b, false)
}

/// Whether `b` shows the owner card (the logo screen otherwise): a change needs [`draw`].
pub fn shows_card(b: &BootInfo) -> bool {
    b.owner.is_some() && !b.checks.iter().any(|c| c.state == State::Fail)
}

/// BOOT-18: [`draw`] with the logo and the wordmark as the animation has them `t` ms in (at 0 not there yet).
pub fn draw_at(lcd: &mut impl Panel, b: &BootInfo, t: u32) {
    paint_at(lcd, b, true, Some(t))
}

/// BOOT-18: one frame of the animation, `t` ms in: only the logo's square and the wordmark's line (every pixel of
/// them once, no blank between). Nothing on the owner card. From [`ANIM_MS`] on, the still screen's pixels.
pub fn frame(lcd: &mut impl Panel, b: &BootInfo, t: u32) {
    if !shows_card(b) {
        anim(lcd, t)
    }
}

/// BOOT-18: the animation's length, ms: the windings fill from 100 ms, 25 ms apart; the flash at [`FLASH_AT`];
/// the glow settled by here, and the screen is the still one.
pub const ANIM_MS: u32 = 1300;
/// The first winding starts to fill, ms.
const FILL_AT: u32 = 100;
/// One winding to the next, ms.
const FILL_MS: u32 = 25;
/// A winding's fade in, ms.
const FILL_RISE: u32 = 90;
/// The flash: the last winding filled, then a 180 ms pause.
pub const FLASH_AT: u32 = FILL_AT + WINDINGS as u32 * FILL_MS + 180;
/// The flash's strength (0-255) every 10 ms from [`FLASH_AT`]: a 60 ms rise, then the preview's decay
/// (e^(-t/260 ms)) scaled to reach 0 at [`ANIM_MS`] so the last frame is the still one. Read once a frame: a
/// `const`, in the slow flash.
const FLASH: [u8; 68] = [
    0, 42, 85, 128, 170, 207, 198, 190, 182, 174, 167, 160, 153, 146, 140, 134, 128, 123, 117, 112, 107, 102, 97, 93,
    89, 85, 81, 77, 73, 69, 66, 63, 60, 57, 54, 51, 48, 46, 43, 41, 38, 36, 34, 32, 30, 28, 26, 24, 23, 21, 19, 18,
    16, 15, 14, 12, 11, 10, 9, 8, 6, 5, 4, 3, 3, 2, 1, 0,
];
/// The wordmark's line box: its top and bottom rows (the version's box starts at 196).
const NAME_Y: (i32, i32) = (188 - 40, 188 - 40 + 48);
/// How far the wordmark's glow reaches past its core box, px; the core box is the ink's, less this much each side
/// and between these rows.
const NAME_REACH: i32 = 15;
const NAME_IN: i32 = 10;
const NAME_CORE_Y: (i32, i32) = (168, 180);
/// The halos' strongest levels (of 15) at the flash's peak: about the preview's glow seen through its blur.
const LOGO_HALO: u32 = 6;
const NAME_HALO: u32 = 6;

/// x/den eased out (cubic) as 0-256; x past den is 256.
fn ease(x: u32, den: u32) -> u32 {
    let k = 256 - x.min(den) * 256 / den;
    256 - k * k * k / 65536
}

/// 0-256 as a blend level 0-15.
fn level(a: u32) -> u8 {
    ((a * 15 + 128) >> 8).min(15) as u8
}

/// The flash's strength `t` ms in (0-255).
fn flash(t: u32) -> u32 {
    if t < FLASH_AT {
        return 0;
    }
    FLASH.get(((t - FLASH_AT) / 10) as usize).map_or(0, |&f| f as u32)
}

/// One frame: the logo and the wordmark as they are `t` ms in. Worked out here once a frame; the pixels are
/// `bootanim`'s.
#[inline(never)]
fn anim(lcd: &mut dyn Panel, t: u32) {
    let t = t.min(ANIM_MS);
    let bg = map_bg(BG);
    let (ring, wind, hi) = (ink_on(map(SUB), bg), ink_on(map(BLUE), bg), ink_on(map(TEXT), bg));
    let f = flash(t);
    let flashed = t >= FLASH_AT;
    let mut halo = [0u16; 16];
    for (a, c) in halo.iter_mut().enumerate() {
        *c = blend(bg, wind, a as u8);
    }
    // The ring: up to a third while the windings fill, then full in 120 ms from the flash.
    let ring_a = if flashed { 82 + 174 * ease(t - FLASH_AT, 120) / 256 } else { 82 * ease(t, 200) / 256 };
    let ring = match level(ring_a) {
        15 => ring,
        a => blend(bg, ring, a),
    };
    // Each winding: dim (0.37) in 90 ms from its turn; from the flash full, tinted toward white by the flash.
    let tint = blend(wind, hi, level(230 * f / 255));
    let wind: [(u16, u8); WINDINGS] = core::array::from_fn(|i| {
        let at = FILL_AT + i as u32 * FILL_MS;
        if flashed {
            (tint, 15)
        } else {
            (wind, level(95 * ease(t.saturating_sub(at), FILL_RISE) * (t >= at) as u32 / 256))
        }
    });
    let peak = |k: u32| ((k * f + 127) / 255) as u8;
    if crate::logo::present() {
        // UPD-23: no logo part, the wordmark alone
        bootanim::logo(lcd, 64, 28, &Logo { bg, ring, wind, halo, peak: peak(LOGO_HALO) });
    }
    if !flashed {
        return; // the wordmark's line stays as drawn: empty
    }
    // The wordmark in 90 ms from the flash, as the still screen places it (text4_aligned, centred on its ink).
    let s = b"FERRITE";
    let w = text4_width(s, &WORD_40);
    let (l, r) = text4_ink(s, &WORD_40).unwrap_or((0, w));
    let x = 120 - (l + r) / 2;
    let core = (x + l + NAME_IN, x + r - NAME_IN, NAME_CORE_Y.0, NAME_CORE_Y.1);
    let area = (x.min(x + l - NAME_REACH), (x + w).max(x + r + NAME_REACH), NAME_Y.0, NAME_Y.1);
    let name = Name { bg, fg: hi, alpha: level(ease(t - FLASH_AT, 90)), halo, peak: peak(NAME_HALO), core, reach: NAME_REACH };
    bootanim::name(lcd, s, &WORD_40, x, 188, area, &name);
}

/// The still logo (the animation's last frame: the ring and the windings at full), when the logo part is there
/// (UPD-23); the wordmark alone otherwise.
#[inline(never)]
fn still_logo(lcd: &mut dyn Panel) {
    if crate::logo::present() {
        let bg = map_bg(BG);
        let (ring, wind) = (ink_on(map(SUB), bg), ink_on(map(BLUE), bg));
        bootanim::logo(lcd, 64, 28, &Logo { bg, ring, wind: [(wind, 15); WINDINGS], halo: [bg; 16], peak: 0 });
    }
}

fn paint(lcd: &mut impl Panel, b: &BootInfo, full: bool) {
    paint_at(lcd, b, full, None)
}

fn paint_at(lcd: &mut impl Panel, b: &BootInfo, full: bool, t: Option<u32>) {
    let n = b.checks.len() as i32;
    let done = b.checks.iter().filter(|c| c.state != State::Pending).count() as i32;
    let failed = b.checks.iter().find(|c| c.state == State::Fail);
    if let (Some(o), None) = (b.owner, failed) {
        return card(lcd, b, &o, done, n, full);
    }
    if full {
        lcd.fill(0, 0, 240, 320, BG);
        if let Some(t) = t {
            anim(lcd, t);
        } else {
            still_logo(lcd);
            lcd.text4_aligned(120, 188, Align::Center, b"FERRITE", &WORD_40, TEXT, BG);
        }
        lcd.text4_aligned(120, 209, Align::Center, b.version.as_bytes(), &UI_12, SUB, BG);
        if let Some(l) = b.line {
            lcd.text4_aligned(120, 243, Align::Center, fit(l, &UI_15B, COL), &UI_15B, TEXT, BG);
        }
    }
    let _ = (done, n);
    let mut line = Buf::<31>::new();
    let (text, col): (&[u8], u16) = if let Some(c) = failed {
        (v!(line, "{}: {}", c.label, if c.value.is_empty() { "failed" } else { c.value }), RED)
    } else {
        (b.note.unwrap_or(b""), SUB)
    };
    // Each line paints its own full width (no blank first).
    crate::widgets::field(lcd, 0, 240, 276, 298, 290, text, &crate::widgets::st(&UI_12, col, BG, Align::Center));
    // BOOT-9.1: the foot says # is help (HOME-10), unless a failed check needs its "any key"
    let more: &[u8] = if failed.is_some() { b"Any key: continue" } else { b"Press # for help" };
    crate::widgets::field(lcd, 0, 240, 298, 320, 308, more, const { &crate::widgets::st(&UI_12, SUB, BG, Align::Center) });
}

/// The owner card (D13, BootC mock-up): who owns the radio and how to reach them, with the logo, version and
/// self-test in one line at the bottom. Any key skips it (the firmware's UI task).
fn card(lcd: &mut impl Panel, b: &BootInfo, o: &Owner, done: i32, n: i32, full: bool) {
    if full {
        card_static(lcd, b, o);
    }
    // The bottom line: wordmark and version, the self-test on the right (the version gives way); the bar.
    let x = 12 + crate::gfx::text4_width(b"FERRITE", &UI_12B);
    // A slow step's note on the right (the version gives way); nothing while the start-up is quick (decision 215).
    let _ = (done, n);
    let status: &[u8] = b.note.unwrap_or(b"");
    let right = 228 - crate::gfx::text4_width(status, &UI_12);
    crate::widgets::field(lcd, x + 6, 222 - x, 265, 281, 277, fit(b.version.as_bytes(), &UI_12, right - x - 8), const { &crate::widgets::st(&UI_12, SUB, BG, Align::Left) });
    crate::widgets::field(lcd, right, 228 - right, 265, 281, 277, status, const { &crate::widgets::st(&UI_12, SUB, BG, Align::Left) });
}

/// The card's fixed part: drawn once.
fn card_static(lcd: &mut impl Panel, b: &BootInfo, o: &Owner) {
    lcd.fill(0, 0, 240, 320, BG);
    lcd.text4(12, 32, b"THIS RADIO BELONGS TO", &UI_12, SUB, BG);
    if let Some(call) = b.callsign {
        let mut up = [0u8; 16];
        let k = upper(call, &mut up);
        lcd.text4(12, 66, fit(&up[..k], &NAME_26, COL), &NAME_26, TEXT, BG); // 13 characters fit (decision 220)
    }
    lcd.text4(12, 93, fit(o.name, &UI_12B, COL), &UI_12B, TEXT, BG);
    if !o.phone.is_empty() || !o.email.is_empty() {
        lcd.fill(12, 113, COL, 1, LINE);
        lcd.text4(12, 137, b"IF FOUND, PLEASE CONTACT", &UI_12, SUB, BG);
        let mut y = 155;
        for line in [o.phone, o.email].into_iter().filter(|l| !l.is_empty()) {
            lcd.text4(12, y, fit(line, &UI_12B, COL), &UI_12B, TEXT, BG);
            y += 17;
        }
    }
    lcd.text4(12, 277, b"FERRITE", &UI_12B, TEXT, BG);
    hint_bar(lcd, &[("Any key", "skip"), ("", "Set in Console")]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fb::FrameBuffer;

    fn count(fb: &FrameBuffer, y0: i32, y1: i32, c: u16) -> usize {
        (y0..y1).flat_map(|y| (0..240).map(move |x| (x, y))).filter(|&(x, y)| fb.at(x, y) == c).count()
    }

    fn info<'a>(owner: Option<Owner<'a>>, checks: &'a [Check]) -> BootInfo<'a> {
        BootInfo { version: "v0.9.0", callsign: Some(b"va7fer"), line: Some(b"Hello"), owner, checks, note: None }
    }

    /// BOOT-10: a failed check names itself in red ("failed" when it has no value of its own) with "Any key:
    /// continue" under it, on the logo screen even when the owner card is set.
    #[test]
    fn a_failed_check_shows_in_red_on_the_logo_screen() {
        let owner = Owner { name: b"Alex", phone: b"", email: b"" };
        let bad = [Check { label: "Flash", value: "", state: State::Fail }];
        let b = info(Some(owner), &bad);
        assert!(!shows_card(&b));
        let mut fb = FrameBuffer::new();
        draw(&mut fb, &b);
        assert!(count(&fb, 276, 298, RED) > 50, "the check's line in red");
        assert!((298..320).flat_map(|y| (0..240).map(move |x| (x, y))).any(|(x, y)| fb.at(x, y) != BG), "any key: continue");
        assert!(count(&fb, 28, 140, BLUE) > 100, "the logo");
    }

    /// BOOT-9.1: with every check fine, the logo screen's foot says "Press # for help" (a line of text there).
    #[test]
    fn the_foot_says_hash_is_help() {
        let ok = [Check { label: "Flash", value: "", state: State::Ok }];
        let mut fb = FrameBuffer::new();
        draw(&mut fb, &info(None, &ok));
        let lit = (298..320).flat_map(|y| (0..240).map(move |x| (x, y))).filter(|&(x, y)| fb.at(x, y) != BG).count();
        assert!(lit > 100, "the help line: {lit} pixels");
    }

    /// SETUP-11: the owner card with only a name has no contact block (no rule, no "If found"); with a phone or an
    /// email it has. The callsign shows in capitals.
    #[test]
    fn the_owner_card_shows_contact_lines_only_when_set() {
        let mut fb = FrameBuffer::new();
        let ok = [Check { label: "Flash", value: "", state: State::Ok }];
        let b = info(Some(Owner { name: b"Alex", phone: b"", email: b"" }), &ok);
        assert!(shows_card(&b));
        draw(&mut fb, &b);
        assert_eq!(count(&fb, 113, 114, LINE), 0);
        assert_eq!(count(&fb, 130, 190, TEXT) + count(&fb, 130, 190, SUB), 0, "no contact block");
        draw(&mut fb, &info(Some(Owner { name: b"Alex", phone: b"", email: b"a@example.ca" }), &ok));
        assert_eq!(count(&fb, 113, 114, LINE), COL as usize, "the rule over the contact block");
        assert!(count(&fb, 150, 170, TEXT) > 50, "the email on the first contact line");
        let mut up = [0u8; 16];
        let n = upper(b"va7fer-9", &mut up);
        assert_eq!(&up[..n], b"VA7FER-9");
    }

    fn shot(fb: &FrameBuffer, y0: i32, y1: i32) -> std::vec::Vec<u16> {
        fb.px[(y0 * 240) as usize..(y1 * 240) as usize].to_vec()
    }

    fn nib(raw: &[u8], i: usize) -> u8 {
        if i.is_multiple_of(2) { raw[i / 2] >> 4 } else { raw[i / 2] & 15 }
    }

    /// The pixels of winding `k` off the ring (by the logo's masks) that differ from the ground.
    fn lit(fb: &FrameBuffer, k: usize) -> usize {
        let mut n = 0;
        for row in 0..112 {
            for col in 0..112 {
                let i = (row * 112 + col) as usize;
                let ours = nib(&LOGO112_WIND_RAW, i) >= 8 && nib(&LOGO112_RING_RAW, i) == 0;
                if ours && bootanim::winding(2 * col + 1 - 112, 2 * row + 1 - 112) == k && fb.at(64 + col, 28 + row) != BG {
                    n += 1;
                }
            }
        }
        n
    }

    /// BOOT-18.1: from [`ANIM_MS`] on, the animated screen is the still one, pixel for pixel, in every theme: the
    /// boot screen after the animation is unchanged. A frame writes only the logo's square and the wordmark's
    /// line, never the version under them.
    #[test]
    fn the_animation_ends_on_the_still_screen() {
        for theme in [crate::theme::DARK, crate::theme::DAYLIGHT, crate::theme::NIGHT_RED, crate::theme::CYBERPUNK] {
            crate::theme::set(theme);
            let b = info(None, &[]);
            let (mut still, mut anim) = (FrameBuffer::new(), FrameBuffer::new());
            draw(&mut still, &b);
            draw_at(&mut anim, &b, ANIM_MS);
            assert!(still.px == anim.px, "theme {theme}: draw_at(ANIM_MS) is the still screen");
            let mut f = FrameBuffer::new();
            draw_at(&mut f, &b, 0);
            for t in (0..=ANIM_MS + 40).step_by(33) {
                frame(&mut f, &b, t);
            }
            frame(&mut f, &b, ANIM_MS);
            assert!(still.px == f.px, "theme {theme}: frames end on the still screen");
            let before = f.writes.clone();
            frame(&mut f, &b, 700);
            for y in 0..320 {
                for x in 0..240 {
                    let i = (y * 240 + x) as usize;
                    let ours = (64..176).contains(&x) && (28..140).contains(&y) || (NAME_Y.0..NAME_Y.1).contains(&y);
                    assert!(ours || f.writes[i] == before[i], "theme {theme}: ({x}, {y}) written by a frame");
                }
            }
        }
        crate::theme::set(crate::theme::DARK);
    }

    /// BOOT-18.1: the version (and the line and status under it) are on the screen from the first frame and never
    /// animated; the logo and the wordmark aren't there yet.
    #[test]
    fn the_version_is_there_from_the_first_frame() {
        let b = info(None, &[]);
        let (mut still, mut first) = (FrameBuffer::new(), FrameBuffer::new());
        draw(&mut still, &b);
        draw_at(&mut first, &b, 0);
        assert!(shot(&still, NAME_Y.1, 320) == shot(&first, NAME_Y.1, 320), "below the wordmark: as the still screen");
        assert!(shot(&first, 0, NAME_Y.1).iter().all(|&c| c == BG), "no logo or wordmark yet");
    }

    /// BOOT-18.1: the windings fill in turn, 25 ms apart from 100 ms, dim; the wordmark only from the flash.
    #[test]
    fn the_windings_fill_in_turn() {
        let b = info(None, &[]);
        let mut fb = FrameBuffer::new();
        for k in 0..WINDINGS {
            let t = FILL_AT + k as u32 * FILL_MS + 10;
            draw_at(&mut fb, &b, t);
            assert!(lit(&fb, k) > 50, "winding {k} filling at {t} ms");
            if k + 1 < WINDINGS {
                assert_eq!(lit(&fb, k + 1), 0, "winding {} not yet at {t} ms", k + 1);
            }
            assert!(shot(&fb, NAME_Y.0, NAME_Y.1).iter().all(|&c| c == BG), "no wordmark before the flash");
        }
        // Dim: at the end of the charge every winding is lit, but well under its full colour.
        draw_at(&mut fb, &b, FLASH_AT - 1);
        assert!((0..WINDINGS).all(|k| lit(&fb, k) > 50));
        let mut sfb = FrameBuffer::new();
        draw(&mut sfb, &b);
        // The blue channel over the ground's, summed over the logo's square.
        let blue = |f: &FrameBuffer| -> u32 { (28..140).flat_map(|y| (64..176).map(move |x| (x, y))).map(|(x, y)| ((f.at(x, y) & 31) - (BG & 31)) as u32).sum() };
        assert!(blue(&fb) * 2 < blue(&sfb), "dim before the flash: {} against {}", blue(&fb), blue(&sfb));
    }

    /// BOOT-18.1: at the flash the logo and the wordmark light together, brighter than the still screen, with a
    /// glow past the ring (where the still screen has the ground); it settles back by [`ANIM_MS`].
    #[test]
    fn the_flash_lights_logo_and_name_together() {
        let b = info(None, &[]);
        let (mut sfb, mut fb) = (FrameBuffer::new(), FrameBuffer::new());
        draw(&mut sfb, &b);
        draw_at(&mut fb, &b, FLASH_AT + 60);
        let red = |f: &FrameBuffer, y0: i32, y1: i32| -> u32 { (y0..y1).flat_map(|y| (0..240).map(move |x| (x, y))).map(|(x, y)| (f.at(x, y) >> 11) as u32).sum() };
        assert!(red(&fb, 28, 140) > red(&sfb, 28, 140) + 1000, "the logo brighter than still");
        assert!(red(&fb, NAME_Y.0, NAME_Y.1) * 2 > red(&sfb, NAME_Y.0, NAME_Y.1), "the wordmark there");
        assert_ne!(fb.at(120, 28 + 56 - 47), BG, "a glow above the ring");
        assert_eq!(sfb.at(120, 28 + 56 - 47), BG);
        assert_ne!(fb.at(120, NAME_Y.1 - 8), BG, "a glow under the wordmark");
        assert_eq!(fb.at(120, NAME_Y.1 - 1), BG, "faded out by the version's line");
        draw_at(&mut fb, &b, FLASH_AT + 400);
        let mid = red(&fb, 28, 140);
        draw_at(&mut fb, &b, FLASH_AT + 60);
        assert!(mid < red(&fb, 28, 140), "settling");
    }

    /// BOOT-18.4: the owner card isn't animated: a frame draws nothing on it.
    #[test]
    fn the_owner_card_stays_still() {
        let b = info(Some(Owner { name: b"Alex", phone: b"", email: b"" }), &[]);
        let mut fb = FrameBuffer::new();
        draw(&mut fb, &b);
        let before = fb.writes.clone();
        frame(&mut fb, &b, 700);
        assert!(fb.writes == before);
    }

    /// BOOT-9: an update repaints only the status line (the logo stays as drawn: redrawing it flashed on the radio).
    #[test]
    fn an_update_leaves_the_logo_alone() {
        let mut fb = FrameBuffer::new();
        let pending = [Check { label: "GPS", value: "", state: State::Pending }];
        let b = BootInfo { note: Some(b"Checking GPS"), ..info(None, &pending) };
        draw(&mut fb, &b);
        let before = fb.writes.clone();
        update(&mut fb, &BootInfo { note: None, ..b });
        for y in 0..270 {
            for x in 0..240 {
                let i = (y * 240 + x) as usize;
                assert_eq!(fb.writes[i], before[i], "({x}, {y}) written again");
            }
        }
        assert_eq!(count(&fb, 276, 298, SUB), 0, "the note cleared");
    }
}
