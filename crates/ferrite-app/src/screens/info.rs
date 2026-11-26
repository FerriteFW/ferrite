//! The system's read-only pages, one module because they are one pattern (a title, label + value rows, a note
//! at the foot): About, System (SELF-9) and the self-test detail (D7; chapter 36, SELF-5 to SELF-8), calibration (D8; CAL-5 to
//! CAL-7), advanced squelch (D10; SQL-1, SQL-2: read only for now), SSB patch status (D16; HF-20, SELF-6), the
//! SSB patch confirm at boot (D15; INST-12), add a device (D19), power off (D22; PWR-17), empty battery
//! (D23; PWR-16) and the mailbox (D24; chapter 38, no menu number yet).
//!
//! Each page is data ([`def`]): its rows are a [`super::table`] table naming a value function, so adding a row is
//! one line.

use super::help::{self, Help};
use super::shared::*;
use super::table::{Ctx, Table, Val};
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_core::msg::{CalState, Device};
use ferrite_core::VERSION;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::{Buf, Zero};
use ferrite_ui::widgets as w;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Page {
    About,
    /// SELF-9: the live numbers, after About.
    System,
    Checks,
    /// HELP-1: the Help pages between About and the self-test.
    Help(Help),
    Calibration,
    AdvSquelch,
    SsbPatch,
    SsbConfirm,
    PowerOff,
    EmptyBattery,
    Mailbox,
    /// D25, UPD-16: the chip's data is another layout's, or Console is writing it raw (LINK-61).
    FinishUpdate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Info {
    pub page: Page,
}

pub(crate) type InfoRow = (&'static str, Val<()>);

/// A page: status-bar name, rows, the note at the foot, hints.
pub(crate) struct PageDef {
    pub title: &'static str,
    pub rows: &'static [InfoRow],
    /// The note at the foot, in the text pack (UPD-22), or fixed in the MCU's flash (About's licence, SELF-10).
    pub note: Note,
    hints: &'static [(&'static str, &'static str)],
}

fn fixed(b: &mut Buf<40>, s: &str, c: u16) -> u16 {
    b.set(s.as_bytes());
    c
}

/// Why transmit is locked, if it is (BOOT-11, SELF-2).
pub(crate) fn tx_lock(dev: &Device) -> Option<&'static str> {
    let t = &dev.selftest;
    if !dev.tx_built {
        Some("RX-only build")
    } else if !t.mcu_ok {
        Some("Locked: MCU check")
    } else if t.cal != CalState::Ok {
        Some("Locked: calibration")
    } else if let Some(row) = super::groups::band_locked(t) {
        Some(["UHF locked", "VHF locked", "200 MHz locked", "HF locked"][row as usize % 4]) // CAL-6.1: the rest transmits
    } else if dev.battery_cutoff {
        Some("Battery too low")
    } else {
        None
    }
}

/// The self-test's checks: name and result (None = not run).
fn checks(dev: &Device) -> [(&'static str, Option<bool>); 6] {
    let t = &dev.selftest;
    let read = |v: u32, ok: bool| (v != 0).then_some(ok);
    [
        ("MCU", read(t.idcode, t.mcu_ok)),
        ("SPI flash", read(t.flash_id, t.flash_mb > 0)),
        ("BK4829 VHF", Some(t.chips & 1 != 0)),
        ("BK4829 UHF", Some(t.chips & 2 != 0)),
        // The SI4732 isn't tested at boot (it stays powered down until FM or HF mode): "Not run" until it answers.
        ("SI4732", t.si4732.then_some(true)),
        ("Calibration", (t.cal != CalState::Unknown).then_some(t.cal == CalState::Ok)),
    ]
}

fn check_row(i: usize, d: &Device, b: &mut Buf<40>) -> u16 {
    match checks(d)[i].1 {
        Some(true) => fixed(b, "OK", GREEN),
        Some(false) => fixed(b, "Failed", RED),
        None => fixed(b, "Not run", SUB),
    }
}

fn volts(b: &mut Buf<40>, mv: u16) {
    let _ = uwrite!(b, "{}.{} V", mv / 1000, Zero((mv % 1000 / 10) as u32, 2));
}

/// SELF-5.1 (decision 466): the version row's label, "Unofficial build" on any build but an official release's.
const VERSION_LABEL: &str = if ferrite_core::OFFICIAL { "Version" } else { "Unofficial build" };

const ABOUT: [InfoRow; 9] = [
    // SELF-5: version and build (the git commit; "+" built from uncommitted changes); SELF-5.1: amber when unofficial.
    (VERSION_LABEL, |_, b| {
        let _ = uwrite!(b, "{} {}", VERSION, ferrite_core::build());
        if ferrite_core::OFFICIAL { TEXT } else { AMBER }
    }),
    ("MCU", |Ctx { dev: d, .. }, b| {
        let t = &d.selftest;
        match (t.idcode, t.mcu_ok) {
            (0, _) => fixed(b, "Not read", SUB),
            (_, true) => fixed(b, "AT32F403A", TEXT),
            (id, false) => {
                let _ = uwrite!(b, "Unknown {:08X}", id);
                RED
            }
        }
    }),
    // SELF-8: the MCU's 96-bit factory unique ID, 20 base32 digits on one row ("ID": the label leaves it room).
    ("ID", |Ctx { dev: d, .. }, b| uid_text(b, d)),
    ("SPI flash", |Ctx { dev: d, .. }, b| {
        let t = &d.selftest;
        if t.flash_id == 0 {
            return fixed(b, "No answer", RED);
        }
        let _ = uwrite!(b, "{} MB \u{b7} {:06X}", t.flash_mb, t.flash_id);
        TEXT
    }),
    ("Calibration", |Ctx { dev: d, .. }, b| fixed(b, super::groups::cal_label(&d.selftest), if d.selftest.cal == CalState::Ok { TEXT } else { AMBER })),
    ("Transmit", |Ctx { dev: d, .. }, b| match tx_lock(d) {
        None => fixed(b, "Ready", GREEN),
        Some(why) => fixed(b, why, AMBER),
    }),
    ("Self-test", |Ctx { dev: d, .. }, b| {
        let c = checks(d);
        let ok = c.iter().filter(|x| x.1 == Some(true)).count();
        let failed = c.iter().any(|x| x.1 == Some(false));
        let _ = uwrite!(b, "{} of {} passed", ok, c.len());
        if failed { RED } else { TEXT }
    }),
    // SELF-5: the SI4732's firmware, chip and component revisions, from the boot probe
    ("SI4732", |Ctx { dev: d, .. }, b| {
        let r = d.selftest.si_rev;
        if r == [0; 5] {
            return fixed(b, "Not read", SUB);
        }
        let c = |v: u8| if v.is_ascii_alphanumeric() { v } else { b'?' };
        b.push(b"FW ");
        b.push(&[c(r[0]), b'.', c(r[1])]);
        b.push(b", rev ");
        b.push(&[c(r[4])]);
        b.push(b", cmp ");
        b.push(&[c(r[2]), b'.', c(r[3])]);
        TEXT
    }),
    ("SSB patch", |Ctx { dev: d, .. }, b| if d.selftest.ssb_patch { fixed(b, "Installed", TEXT) } else { fixed(b, "Not installed", SUB) }),
];

/// The unique ID (SELF-8): "R28JM FRR1S 27V4T WPTH0", its 20 base32 digits in four groups of five
/// (`ferrite_core::uid::text`), "Not read" while all zero.
fn uid_text(b: &mut Buf<40>, d: &Device) -> u16 {
    let u = d.selftest.uid;
    if u == [0; 3] {
        return fixed(b, "Not read", SUB);
    }
    let t = ferrite_core::uid::text(u);
    for (i, g) in t.chunks(5).enumerate() {
        if i > 0 {
            b.push(b" ");
        }
        b.push(g);
    }
    TEXT
}

/// SELF-9: the live numbers, on the page after About (they change; About's don't).
const SYSTEM: [InfoRow; 7] = [
    // SELF-9.1: the share of the last 10 s the MCU was awake.
    ("CPU", |Ctx { dev: d, .. }, b| match d.cpu_pct {
        Some(p) => {
            let _ = uwrite!(b, "{} %", p);
            TEXT
        }
        None => fixed(b, "Measuring", SUB),
    }),
    // SELF-9.2: the statics and the stack's peak, of the 96 KB.
    ("RAM", |Ctx { dev: d, .. }, b| {
        if d.ram_statics == 0 || d.stack_free == 0 {
            return fixed(b, "Measuring", SUB);
        }
        let used = 96 * 1024 - d.stack_free;
        let _ = uwrite!(b, "{} of 96 KB", (used + 512) / 1024);
        TEXT
    }),
    // SBAR-23: the stack's room never used since power-on.
    ("Stack free", |Ctx { dev: d, .. }, b| {
        if d.stack_free == 0 {
            return fixed(b, "Measuring", SUB);
        }
        let _ = uwrite!(b, "{}.{} KB", d.stack_free / 1024, d.stack_free % 1024 * 10 / 1024);
        TEXT
    }),
    // SBAR-20: the MCU's die temperature (the Radio task reads it every 10 s), in 0 6 4's units.
    ("MCU temp", |Ctx { app: a, dev: d, .. }, b| match d.mcu_temp {
        Some(t) if a.set.units == 1 => {
            let _ = uwrite!(b, "{} \u{b0}F", t as i32 * 9 / 5 + 32);
            TEXT
        }
        Some(t) => {
            let _ = uwrite!(b, "{} \u{b0}C", t);
            TEXT
        }
        None => fixed(b, "Not read yet", SUB),
    }),
    ("Battery", |Ctx { dev: d, .. }, b| {
        volts(b, d.battery_mv); // the trim is on Calibration and trims
        TEXT
    }),
    ("Uptime", |Ctx { app: a, .. }, b| {
        let m = a.now / 60_000;
        let _ = uwrite!(b, "{} h {} min", m / 60, m % 60);
        TEXT
    }),
    // PWR-26.6: the MCU clock now and its switches since power on.
    ("Clock", |Ctx { dev: d, .. }, b| {
        if d.mcu.mhz == 0 {
            return fixed(b, "Not measured", SUB);
        }
        let _ = uwrite!(b, "{} MHz \u{b7} ", d.mcu.mhz);
        grouped(b, d.mcu.switches);
        b.push(if d.mcu.switches == 1 { b" switch".as_slice() } else { b" switches" });
        TEXT
    }),
];

/// PWR-26.6: a count with thousands commas: 1,234.
fn grouped(b: &mut Buf<40>, n: u32) {
    if n >= 1000 {
        grouped(b, n / 1000);
        let _ = uwrite!(b, ",{}{}{}", n / 100 % 10, n / 10 % 10, n % 10);
    } else {
        let _ = uwrite!(b, "{}", n);
    }
}

const CHECKS: [InfoRow; 7] = [
    ("MCU", |Ctx { dev: d, .. }, b| check_row(0, d, b)),
    ("SPI flash", |Ctx { dev: d, .. }, b| check_row(1, d, b)),
    ("BK4829 VHF", |Ctx { dev: d, .. }, b| check_row(2, d, b)),
    ("BK4829 UHF", |Ctx { dev: d, .. }, b| check_row(3, d, b)),
    ("SI4732", |Ctx { dev: d, .. }, b| check_row(4, d, b)),
    ("Calibration", |Ctx { dev: d, .. }, b| check_row(5, d, b)),
    // UPD-21: the text pack, logo and terminal templates for this build (not a transmit lock)
    ("SPI parts", |_, b| {
        let (ok, all) = ferrite_core::part::count();
        let _ = ufmt::uwrite!(b, "{} of {}", ok, all);
        if ok == all { GREEN } else { AMBER }
    }),
];

const CALIBRATION: [InfoRow; 4] = [
    ("Status", |Ctx { dev: d, .. }, b| fixed(b, super::groups::cal_label(&d.selftest), if d.selftest.cal == CalState::Ok { GREEN } else { AMBER })),
    // CAL-20.4: the sector's own checks: which one failed, and where in the power curves
    ("Check", |Ctx { dev: d, .. }, b| {
        let t = &d.selftest;
        if t.cal == CalState::Unknown {
            return fixed(b, "Not checked", SUB);
        }
        let f = if t.cal == CalState::CrcMismatch && t.cal_fault != ferrite_core::msg::CalFault::Unchecked { ferrite_core::msg::CalFault::None } else { t.cal_fault };
        super::groups::cal_fault_text(f, t.cal_at, b);
        if f == ferrite_core::msg::CalFault::None && t.cal != CalState::Blank { GREEN } else { AMBER }
    }),
    ("CRC at install", |Ctx { dev: d, .. }, b| match d.selftest.cal {
        CalState::CrcMismatch if d.selftest.cal_fault == ferrite_core::msg::CalFault::Unchecked => fixed(b, "Not checked", AMBER),
        CalState::CrcMismatch => fixed(b, "Changed", RED),
        CalState::Unknown => fixed(b, "Not checked", SUB),
        _ => fixed(b, "Matches", GREEN),
    }),
    ("User trims", |_, b| fixed(b, "MENU 0 9 9", SUB)),
];

const ADV_SQUELCH: [InfoRow; 3] = [
    ("Thresholds", |_, b| fixed(b, "From calibration", SUB)),
    ("Decided by", |a, b| fixed(b, if a.app.settings().sq_soft { "Software" } else { "The chip" }, TEXT)), // SQL-18.3
    ("Levels", |_, b| fixed(b, "0-9 per slot", TEXT)),
];

const SSB_PATCH: [InfoRow; 3] = [
    ("State", |Ctx { dev: d, .. }, b| if d.selftest.ssb_patch { fixed(b, "Verified", GREEN) } else { fixed(b, "Not installed", AMBER) }),
    ("Loads", |_, b| fixed(b, "Entering HF mode", TEXT)),
    ("Source", |_, b| fixed(b, "Your stock .BTF", TEXT)),
];

const MAILBOX: [InfoRow; 4] = [
    ("Mailbox", |_, b| fixed(b, "Off", TEXT)),
    ("SSID", |Ctx { app: a, .. }, b| {
        if a.set.callsign.is_empty() {
            return fixed(b, "Needs a callsign", AMBER);
        }
        let c = a.set.callsign.as_bytes();
        b.set(c.split(|&x| x == b'-').next().unwrap_or(c)); // without an SSID typed into it (APRS-30)
        b.push(b"-1");
        TEXT
    }),
    ("Bulletins", |_, b| fixed(b, "On", TEXT)),
    ("Messages", |_, b| fixed(b, "None", SUB)),
];

const NONE: [InfoRow; 0] = [];

/// HELP-1: About's pages in ◀▶ order; ▶ goes on, ◀ back, round.
const CYCLE: [Page; 8] = [
    Page::About,
    Page::System,
    Page::Help(Help::Manual),
    Page::Help(Help::HomeKeys),
    Page::Help(Help::SideKeys),
    Page::Help(Help::Digits),
    Page::Help(Help::MenuNos),
    Page::Checks,
];

/// HOME-10: the quick help's pages in #'s order, round, the holds first (Alex: "start with the hold page"); ◀▶
/// still go through About's ring (KEY-19).
const HELP_PAGES: [Help; 5] = [Help::Digits, Help::HomeKeys, Help::SideKeys, Help::MenuNos, Help::Manual];

/// The help page # goes to next.
fn next_help(h: Help) -> Help {
    let i = HELP_PAGES.iter().position(|&x| x == h).unwrap_or(0);
    HELP_PAGES[(i + 1) % HELP_PAGES.len()]
}

const BACK_ONLY: [(&str, &str); 1] = [("BACK", "Back")];

pub(crate) fn def(p: Page) -> PageDef {
    match p {
        Page::About => PageDef {
            title: "About",
            rows: &ABOUT,
            // SELF-10: the licence and the copyright holder, with the update hint.
            note: Note::Fixed("Copyright 2026 Alex Harvey. Free for any noncommercial use: PolyForm Noncommercial 1.0.0. To update, hold PF1 + PF2 while switching on."),
            hints: &[(ARROWS_LR, "System"), ("BACK", "Back")], // HELP-1: the page ▶ goes to
        },
        Page::System => PageDef {
            title: "System",
            rows: &SYSTEM,
            note: Note::Txt(txt!("Live: CPU is the share of the last 10 s the MCU was awake; RAM is the statics and the stack's peak since power-on; Clock is the MCU's speed now and its switches.")),
            hints: &[(ARROWS_LR, "Manual"), ("BACK", "Back")],
        },
        Page::Checks => PageDef {
            title: "Self-test",
            rows: &CHECKS,
            note: Note::Txt(txt!("A failed check never stops receive. Transmit needs the MCU and calibration checks to pass.")),
            hints: &[(ARROWS_LR, "About"), ("BACK", "Back")],
        },
        // The hint bar names the page ▶ goes to.
        Page::Help(h) => {
            // HOME-10: # names the help page it goes to; ◀▶ still go round About's pages
            let (title, hints): (_, &'static [(&str, &str)]) = match h {
                Help::Digits => ("Holds", &[("#", "Home keys"), ("BACK", "Back")]),
                Help::HomeKeys => ("Home keys", &[("#", "Side keys"), ("BACK", "Back")]),
                Help::SideKeys => ("Side keys", &[("#", "Menu numbers"), ("BACK", "Back")]),
                Help::MenuNos => ("Menu numbers", &[("#", "Manual"), ("BACK", "Back")]),
                Help::Manual => ("Manual", &[("#", "Holds"), ("BACK", "Back")]),
            };
            PageDef { title, rows: &NONE, note: Note::None, hints }
        }
        Page::Calibration => PageDef {
            title: "Calibrate",
            rows: &CALIBRATION,
            note: Note::Txt(txt!("Read only: Ferrite never writes the factory calibration. A failed check locks transmit on every band.")),
            hints: &BACK_ONLY,
        },
        Page::AdvSquelch => PageDef {
            title: "Squelch",
            rows: &ADV_SQUELCH,
            note: Note::Txt(txt!("Noise open, ceiling, RSSI open, glitch and hysteresis per band come from the calibration. Editing comes later.")),
            hints: &BACK_ONLY,
        },
        Page::SsbPatch => PageDef {
            title: "SSB patch",
            rows: &SSB_PATCH,
            note: Note::Txt(txt!("Import it from your own stock firmware with the flasher (--ssb-from) or the web app. Without it HF mode has AM only.")),
            hints: &BACK_ONLY,
        },
        Page::SsbConfirm => PageDef {
            title: "SSB patch",
            rows: &NONE,
            note: Note::None,
            hints: &[("OK", "Use it"), ("BACK", "Not now")],
        },
        Page::PowerOff | Page::EmptyBattery => PageDef { title: "Power", rows: &NONE, note: Note::None, hints: &[] },
        Page::FinishUpdate => PageDef { title: "Update", rows: &NONE, note: Note::None, hints: &[("BACK", "Use it")] },
        Page::Mailbox => PageDef {
            title: "Mailbox",
            rows: &MAILBOX,
            note: Note::Txt(txt!("Callers connect to your callsign-SSID at 1200 baud to leave and read mail. Settings come with the packet station.")),
            hints: &BACK_ONLY,
        },
    }
}

/// A page's note at the foot.
#[derive(Clone, Copy)]
pub(crate) enum Note {
    None,
    /// In the text pack (UPD-22).
    Txt(crate::text::Txt),
    /// In the MCU's flash: About's licence (SELF-10), there whatever the SPI flash holds.
    Fixed(&'static str),
}

/// A page's note, a text pack's read when drawn (UPD-22). Without the pack the self-test says so (the update
/// installs it); the other notes are left out.
pub(crate) fn note_text(p: Page, n: Note) -> crate::text::Text {
    match n {
        Note::None => crate::text::Text::EMPTY,
        Note::Fixed(s) => crate::text::Text::from_bytes(s.as_bytes()),
        Note::Txt(_) if p == Page::Checks && !crate::text::present() => {
            crate::text::Text::from_bytes(b"Text pack missing: install this version's update file (.ffw) again with Ferrite Console or flash.py.")
        }
        Note::Txt(t) => crate::text::load(t),
    }
}

/// Pages that are a message, not rows: heading, text lines, colour.
/// `busy`: raw SPI writes are coming over the link (DISP-30's transfer), the update being written (UPD-16.1).
pub(crate) fn message(p: Page, dev: &Device, busy: bool, b: &mut Buf<40>) -> Option<(&'static str, &'static str, u16)> {
    Some(match p {
        Page::SsbConfirm => (
            "Unknown SSB patch",
            "The patch in flash doesn't match a known stock version. Use it for USB, LSB and CW? You can change this in MENU 0 9 6.",
            AMBER,
        ),
        Page::PowerOff => ("Saving...", "Finishing writes, then off.", TEXT),
        // UPD-16.1: while Console (or flash.py) writes the radio's flash, say so, with a moving bar
        Page::FinishUpdate if busy => (
            "Updating...",
            "Writing the update to the radio's flash. Keep the cable in; the radio restarts when it's done.",
            BLUE,
        ),
        // UPD-16: nothing is lost; the radio works, but saves nothing until Console has moved the data
        Page::FinishUpdate => (
            "Finish the update",
            "Your channels and settings are waiting in the old layout. Connect the cable and open Ferrite Console to finish the update. Until then nothing is saved.",
            AMBER,
        ),
        Page::EmptyBattery => {
            volts(b, dev.battery_mv);
            ("Battery empty", "Switching off safely.", RED)
        }
        _ => return None,
    })
}

impl View for Info {
    /// KEY-19: ◀▶ go through About, Help and the self-test (HELP-1).
    fn own_arrows(self) -> bool {
        true
    }

    fn name(self) -> &'static str {
        def(self.page).title
    }

    fn modal(self) -> bool {
        matches!(self.page, Page::PowerOff | Page::EmptyBattery)
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        if e.press != Press::Short || self.modal() {
            return;
        }
        match (self.page, e.key) {
            (p, Key::Left | Key::Right) if CYCLE.contains(&p) => {
                let i = CYCLE.iter().position(|&c| c == p).unwrap_or(0) as i32;
                let d = if e.key == Key::Right { 1 } else { -1 };
                app.screen = Screen::Info(Info { page: CYCLE[(i + d).rem_euclid(CYCLE.len() as i32) as usize] });
            }
            // CAL-20: the calibration reads "Changed": OK asks to use it as it is now
            (Page::About, Key::Menu) if app.cal_changed => super::confirm::ask(app, super::confirm::Ask::AcceptCal),
            (Page::SsbConfirm, Key::Menu) => {
                app.back();
                app.show_toast(b"SSB patch", b"in use", false);
            }
            (Page::Help(h), Key::Hash) => app.screen = Screen::Info(Info { page: Page::Help(next_help(h)) }), // HOME-10
            (_, Key::Exit) => app.back(),
            _ => {}
        }
    }

    fn ptt(self, app: &mut App, down: bool) {
        if down && !self.modal() {
            super::nav::transmit(app);
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let d = def(self.page);
        // CAL-20: OK's action; not while the store couldn't check it (accepting needs the store)
        let t = &cx.dev.selftest;
        app.cal_changed = t.cal == ferrite_core::msg::CalState::CrcMismatch && t.cal_fault != ferrite_core::msg::CalFault::Unchecked;
        app.new_page(lcd, Mix::new().any(&self.page).get());
        if let Page::Help(h) = self.page {
            return help::render(app, lcd, h);
        }
        let mut b = Buf::<40>::new();
        let busy = app.transferring();
        if let Some((head, text, c)) = message(self.page, cx.dev, busy, &mut b) {
            // UPD-16.2: which part, and whether it is being written or read back (each part's bar runs twice)
            if let (Page::FinishUpdate, true, (i, n @ 1..)) = (self.page, busy, app.update_part) {
                let _ = uwrite!(b, "Part {} of {}: {}", i, n, if app.update_checking { "checking" } else { "writing" });
            }
            app.message(lcd, head.as_bytes(), b.as_bytes(), text.as_bytes(), c);
            if self.page == Page::PowerOff || self.page == Page::FinishUpdate && busy {
                // A bar that moves while writes finish (PWR-17) or the update is written (UPD-16.1).
                let pct = ((app.now / 40) % 100) as u8;
                if app.dirty(R_NOTE, Mix::new().h(&pct).get()) {
                    w::progress(lcd, 60, 240, 120, 4, pct, BLUE);
                }
            } else if app.dirty(R_NOTE, 1) {
                lcd.fill(60, 240, 120, 4, BG); // the writes stopped: no bar left behind
            }
            return;
        }
        app.table(lcd, &Table { rows: d.rows, go: &[], y: 22, h: 20, style: w::KV_STATUS }, cx.live, cx.dev, &(), None);
        if !matches!(d.note, Note::None) && app.dirty(R_HELP, Mix::new().any(&self.page).get()) {
            let y = 30 + 20 * d.rows.len() as i32;
            let h = (292 - y).min(110);
            let note = note_text(self.page, d.note);
            ferrite_ui::canvas::banded(lcd, 0, y - 6, 240, 300 - y + 6, BG, |lcd| {
                lcd.fill_r(8, y, 224, h, 2, SURFACE);
                w::wrap(lcd, 16, 208, y + 3, y + h - 3, y + 16, 15, note.as_bytes(), &UI_12, TEXT, SURFACE);
            });
        }
    }

    fn hints(self, app: &App) -> Hints {
        if self.page == Page::About && app.cal_changed {
            return Hints::new(&[("OK", "Calibration"), (ARROWS_LR, "System"), ("BACK", "Back")]); // CAL-20
        }
        Hints::new(def(self.page).hints)
    }
}
