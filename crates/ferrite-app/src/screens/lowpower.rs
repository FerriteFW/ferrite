//! Low Power Mode's home (PWR-24.7, decision 234; the approved mock-up `LowPowerMode`): while the mode is on, home is
//! drawn this way instead of 0 7 2's layout ([`App::home_layout`]). The keys are home's (tuning, V/M, editing);
//! only the drawing differs.
//!
//! The whole 240 x 320 in fixed colours (theme-independent, so it never passes for the normal home): an amber band
//! top (LOW POWER, the clock, the battery %) and bottom (home's own hints), amber sides, and the active slot alone,
//! big, on black: its head line, the frequency, mode/tone/shift/power, then RX with an 8-block meter, or TX with
//! the time keyed. Over the frequency, while SIDE1 + SIDE2 are held, a "Leaving" box fills over the second
//! ([`App::lpm_leave_progress`]).

use super::shared::*;
use super::Cx;
use crate::chan::ChannelStore;
use crate::slot::{card_tone, mode_chip, shift_text};
use crate::App;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::{rgb565, Align, Gfx};
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;

/// The mode's own colours, the same in every theme (PWR-24.7).
pub const LP_AMBER: u16 = rgb565(0xFF, 0xB0, 0x00);
const LP_BLACK: u16 = 0;
const LP_TEXT: u16 = rgb565(0xFF, 0xFF, 0xFF);
const LP_SUB: u16 = rgb565(0xE8, 0xE4, 0xD8);
const LP_DIM: u16 = rgb565(0xC8, 0xC2, 0xB2);
const LP_EMPTY: u16 = rgb565(0x8A, 0x84, 0x74);
const LP_GREEN: u16 = rgb565(0x4C, 0xFF, 0x8F);
const LP_RED: u16 = rgb565(0xFF, 0x50, 0x50);

/// The bands' heights, and the sides' width.
const TOP_H: i32 = 26;
const BOTTOM_Y: i32 = 294;
const SIDE_W: i32 = 4;
/// The text column: 14 to 226.
const X: i32 = 14;
const W: i32 = 212;
/// The rows (y): the head line's box and baseline, the frequency's box and baseline, the settings line's box and
/// baseline, then the S-meter region (its text's baseline, the blocks' top). Each box is filled before it is
/// drawn, and the frequency's and settings line's boxes meet, so the leaving box going leaves nothing behind.
const HEAD: (i32, i32, i32) = (34, 64, 56);
const FREQ: (i32, i32, i32) = (64, 130, 120);
const INFO: (i32, i32, i32) = (130, 172, 158);
const SUB: (i32, i32) = (172, 270);
const SUB_BASE: i32 = 210;
/// The meter: 8 blocks of 23 x 32, 4 apart, across the text column (big to read at the mode's dim backlight).
const BLOCKS: i32 = 8;
const BLOCK: (i32, i32, i32) = (23, 32, 27); // width, height, pitch
const METER_Y: i32 = 228;
/// PWR-24.8: the last-heard line's baseline, between the meter and the bottom band.
const HEARD_BASE: i32 = 284;
/// The leaving box: over the frequency and the settings line, inside their boxes (redrawn when it goes), the
/// whole inside's width so no digit shows past it.
const BOX: (i32, i32, i32, i32) = (8, 74, 224, 96);

/// Low Power Mode's home, on the panel.
pub(crate) fn render(app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
    let live = cx.live;
    let a = app.active;
    // The frame: the sides and the black inside (the bands are their own regions).
    let mut drew = false;
    if app.dirty(R_FRAME, 0x4C50) {
        lcd.fill(0, TOP_H, SIDE_W, BOTTOM_Y - TOP_H, LP_AMBER);
        lcd.fill(240 - SIDE_W, TOP_H, SIDE_W, BOTTOM_Y - TOP_H, LP_AMBER);
        lcd.fill(SIDE_W, TOP_H, 240 - 2 * SIDE_W, BOTTOM_Y - TOP_H, LP_BLACK);
        drew = true;
    }
    // The top band: LOW POWER, the clock, the battery.
    let (clock, pct) = (cx.dev.clock_min, cx.dev.battery_pct);
    if app.dirty(R_TOP, Mix::new().h(&clock).h(&pct).get()) {
        lcd.fill(0, 0, 240, TOP_H, LP_AMBER);
        w::field(lcd, 8, 90, 0, TOP_H, 18, b"LOW POWER", const { &w::st(&UI_12B, LP_BLACK, LP_AMBER, Align::Left) });
        if let Some(m) = clock {
            let mut t = Buf::<8>::new();
            let _ = ufmt::uwrite!(t, "{}:{}", ferrite_ui::text::Zero((m / 60) as u32, 2), ferrite_ui::text::Zero((m % 60) as u32, 2));
            w::field(lcd, 98, 44, 0, TOP_H, 18, t.as_bytes(), const { &w::st(&UI_12B, LP_BLACK, LP_AMBER, Align::Center) });
        }
        let mut p = Buf::<6>::new();
        let _ = ufmt::uwrite!(p, "{}%", pct);
        w::field(lcd, 172, 60, 0, TOP_H, 18, p.as_bytes(), const { &w::st(&UI_12B, LP_BLACK, LP_AMBER, Align::Right) });
    }
    // The head line, and RX or TX on its right.
    let keyed = app.radio.keyed.is_some();
    let rx = live.slots[a as usize].squelch_open;
    let mut h = Buf::<32>::new();
    app.head_text(a, live, cx.store, &mut h);
    if app.dirty(R_HEAD, Mix::new().h(&h).h(&keyed).h(&rx).get()) || drew {
        lcd.fill(SIDE_W, HEAD.0, 240 - 2 * SIDE_W, HEAD.1 - HEAD.0, LP_BLACK);
        let tag_w = if keyed || rx { 44 } else { 0 };
        let name = w::fit(h.as_bytes(), &UI_15B, W - tag_w);
        w::field(lcd, X, W - tag_w, HEAD.0, HEAD.1, HEAD.2, name, const { &w::st(&UI_15B, LP_SUB, LP_BLACK, Align::Left) });
        if keyed {
            filled_tag(lcd, X + W, HEAD.2, b"TX", LP_RED);
        } else if rx {
            filled_tag(lcd, X + W, HEAD.2, b"RX", LP_GREEN);
        }
        drew = true;
    }
    // The frequency (the transmit one while keyed), big.
    let f = if keyed { app.tuned_cfg(a).tx_of(app.tuned_freq(a)) } else { app.label(a, app.shown_freq(a, live)) }; // VFO-18.2
    if app.dirty(R_FREQ, Mix::new().h(&f).get()) || drew {
        lcd.fill(SIDE_W, FREQ.0, 240 - 2 * SIDE_W, FREQ.1 - FREQ.0, LP_BLACK);
        w::freq(lcd, 10, 232, FREQ.2, f, &DIG_46, &DIG_28, LP_TEXT, LP_SUB, LP_BLACK);
        drew = true;
    }
    // Mode, tone, shift, power.
    let cfg = app.tuned_cfg(a);
    let mut line = Buf::<40>::new();
    let mut part = Buf::<16>::new();
    mode_chip(&cfg, &mut part);
    line.push(part.as_bytes());
    for i in 0..3 {
        part.clear();
        match i {
            0 => card_tone(&cfg, &mut part),
            1 => shift_text(&cfg, false, &mut part),
            _ => part.set(super::transmit::power_name(app, a).as_bytes()),
        }
        if !part.is_empty() {
            line.push(b"  ");
            line.push(part.as_bytes());
        }
    }
    if app.dirty(R_INFO, Mix::new().h(&line).get()) || drew {
        // the strips either side of the text column too: the leaving box reaches into them
        lcd.fill(SIDE_W, INFO.0, X - SIDE_W, INFO.1 - INFO.0, LP_BLACK);
        lcd.fill(X + W, INFO.0, 240 - SIDE_W - X - W, INFO.1 - INFO.0, LP_BLACK);
        w::field(lcd, X, W, INFO.0, INFO.1, INFO.2, line.as_bytes(), const { &w::st(&UI_15B, LP_SUB, LP_BLACK, Align::Left) });
        drew = true;
    }
    // RX: the S-meter's text and 8 blocks; TX: the time keyed; else Quiet and empty blocks.
    let rssi = live.slots[a as usize].rssi;
    let lit = if rx { ((w::meter_level(rssi) as u32 * BLOCKS as u32).div_ceil(w::METER_SEGS as u32)) as i32 } else { 0 };
    let secs = if keyed { app.now.wrapping_sub(app.radio.t0) / 1000 } else { 0 };
    // the S text as well as the blocks: S7 and S8 can light the same blocks
    let st = if rx { w::s_text(rssi) } else { Buf::new() };
    if app.dirty(R_SUB, Mix::new().h(&keyed).h(&rx).h(&lit).h(&secs).b(st.as_bytes()).get()) || drew {
        lcd.fill(SIDE_W, SUB.0, 240 - 2 * SIDE_W, SUB.1 - SUB.0, LP_BLACK);
        let mut t = Buf::<8>::new();
        let (text, c) = if keyed {
            super::activity::mmss(secs, &mut t);
            (t.as_bytes(), LP_RED)
        } else if rx {
            (st.as_bytes(), LP_GREEN)
        } else {
            (b"Quiet".as_slice(), LP_DIM)
        };
        w::field(lcd, X, W, SUB.0, METER_Y - 4, SUB_BASE, text, &w::st(&UI_15B, c, LP_BLACK, Align::Left));
        if !keyed {
            let (bw, bh, pitch) = BLOCK;
            for i in 0..BLOCKS {
                let x = X + i * pitch;
                if i < lit {
                    lcd.fill(x, METER_Y, bw, bh, LP_GREEN);
                } else {
                    lcd.frame(x, METER_Y, bw, bh, LP_EMPTY);
                    lcd.frame(x + 1, METER_Y + 1, bw - 2, bh - 2, LP_EMPTY); // 2 px: one is lost at the dim backlight
                }
            }
        }
        drew = true;
    }
    // PWR-24.8: the last reception under the meter ("Heard 4 min ago · S7"), blank while one is on and before
    // the first; redrawn as its age changes (every second for the first minute, then every minute).
    let mut heard = Buf::<32>::new();
    let (slot, _, to, peak) = app.last_rx;
    if slot == a && to != 0 && !rx && !keyed {
        let ago = app.now.wrapping_sub(to) / 1000;
        let (n, unit) = match ago {
            0..=59 => (ago, " s"),
            60..=3599 => (ago / 60, " min"),
            _ => (ago / 3600, " h"),
        };
        heard.push(b"Heard ");
        push_num(&mut heard, n);
        heard.push(unit.as_bytes());
        heard.push(" ago \u{b7} ".as_bytes());
        heard.push(w::s_text(peak).as_bytes());
    }
    if app.dirty(R_NOTE, Mix::new().h(&heard).get()) || drew {
        lcd.fill(SIDE_W, SUB.1, 240 - 2 * SIDE_W, BOTTOM_Y - SUB.1, LP_BLACK);
        w::field(lcd, X, W, SUB.1, BOTTOM_Y - 4, HEARD_BASE, w::fit(heard.as_bytes(), &UI_15B, W), const { &w::st(&UI_15B, LP_SUB, LP_BLACK, Align::Left) });
    }
    // No standing note of what is off or how to leave: both are said on entering and whenever something the mode
    // refuses is tried (PWR-24.6); the room went to bigger text (Alex, Oct 21, 2026).
    // The bottom band: home's own hints in black on amber, PTT Talk in the middle (Alex, Oct 21, 2026: the one VFO
    // is for talking; OK still opens the quick menu), a hint-bar toast covering it while it shows.
    let home = app.screen.hints(app);
    let hints = match home.as_slice() {
        [first, _, last] => Hints::new(&[*first, ("PTT", "Talk"), *last]),
        _ => home,
    };
    let toast = app.toast_shown();
    let line = app.dirty(R_LINE, Mix::new().any(&hints.as_slice()).h(&toast).get());
    if line {
        // a toast covers the hint bar's rows only: the band's top rows stay amber above it
        lcd.fill(0, BOTTOM_Y, 240, if toast { w::HINT_Y } else { 320 } - BOTTOM_Y, LP_AMBER);
    }
    if line && !toast {
        let items = hints.as_slice();
        let n = items.len();
        let colours = (LP_BLACK, LP_BLACK, LP_AMBER);
        for (i, (k, l)) in items.iter().enumerate() {
            let width = w::key_label_in(lcd, 0, 0, k, l, colours, false);
            let x = match i {
                0 => 8,
                _ if i == n - 1 => 232 - width,
                _ => (240 - width) / 2,
            };
            w::key_label_in(lcd, x, 312, k, l, colours, true);
        }
    }
    // Leaving: the box over the frequency, its bar filling over the second; when it goes, what was under it again.
    match app.lpm_leave_progress() {
        Some(p) => {
            let fill = (BOX.2 - 24) * p as i32 / 1000;
            if app.dirty(R_PAGE, Mix::new().h(&fill).get()) || drew {
                let (x, y, bw, bh) = BOX;
                lcd.fill(x, y, bw, bh, LP_AMBER);
                lcd.fill(x + 2, y + 2, bw - 4, bh - 4, LP_BLACK);
                w::field(lcd, x + 12, bw - 24, y + 10, y + 36, y + 32, b"Leaving", const { &w::st(&UI_15B, LP_AMBER, LP_BLACK, Align::Left) });
                w::field(lcd, x + 12, bw - 24, y + 36, y + 62, y + 58, b"Keep both held.", const { &w::st(&UI_15B, LP_TEXT, LP_BLACK, Align::Left) });
                lcd.fill(x + 12, y + 72, bw - 24, 10, LP_EMPTY);
                lcd.fill(x + 12, y + 72, fill, 10, LP_AMBER);
            }
        }
        None => {
            if app.dirty(R_PAGE, 0) {
                for r in [R_HEAD, R_FREQ, R_INFO] {
                    app.drawn.hash[r] = 1; // redrawn next time
                }
            }
        }
    }
}

/// RX or TX in black on a filled block of its colour, on the head line's baseline: an outline all but vanished
/// at the mode's dim backlight.
fn filled_tag(lcd: &mut impl Panel, x_right: i32, baseline: i32, s: &[u8], c: u16) {
    let wd = ferrite_ui::gfx::text4_width(s, &UI_15B) + 12;
    let y = baseline - 16;
    w::field(lcd, x_right - wd, wd, y, y + 20, baseline, s, &w::st(&UI_15B, LP_BLACK, c, Align::Center));
}

fn push_num(b: &mut Buf<32>, n: u32) {
    let mut d = [0u8; 10];
    let mut i = d.len();
    let mut v = n;
    loop {
        i -= 1;
        d[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    b.push(&d[i..]);
}
