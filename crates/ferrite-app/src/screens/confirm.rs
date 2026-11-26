//! Confirm, shared by every destructive action (manual chapters 11, 10, 34 and 22; MENU-23, CHAN-12, CHAN-13,
//! PIN-6, PIN-7, TSRCH-4): it names what goes and what is kept; OK confirms, BACK cancels. The reset (MENU 0 9 7
//! 2, or MENU held at power on: BOOT-14, PIN-6) is one, with the same deliberate steps both ways: what goes, OK,
//! then hold # for 3 s. Other screens ask with [`ask`] (the browser's delete, the zones list, tone search's "TX
//! too?").

use crate::text::Txt;
use super::forms::para;
use super::nav::Dest;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::{ChanWrite, ChannelStore, ZoneId};
use crate::data::Op;
use crate::keys::{KeyEvent, Press};
use crate::slot::{chan_label, tone_text};
use crate::{App, Screen, Slot, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_core::msg::{RadioCmd, SLOTS};
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;

/// What is being confirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ask {
    /// The reset (BOOT-14): what goes, then (armed) hold # for 3 s.
    Reset { armed: bool },
    /// Delete a channel, by number (CHAN-13).
    DeleteChannel(u16),
    /// Delete a zone (CHAN-12).
    DeleteZone(ZoneId),
    /// Delete a range preset, by index (MENU 0 1 4).
    DeletePreset(u8),
    /// The tone hint's tone is the RX tone now: set it as the TX tone too (TSRCH-16, as tone search's TSRCH-4).
    ToneTx(u16),
    /// Hold 9 on a channel (KEY-23): the active slot's channel to its VFO (VFO-23).
    CopyToVfo,
    /// CAL-20: OK on About while the calibration reads "Changed": use the sector as it is now.
    AcceptCal,
    /// SCAN-17.1: # on the skip list clears it all (one at a time is OK there).
    ClearSkips,
    /// HF-28.1: # on HF mode's recent list clears it all.
    ClearRecent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Confirm {
    pub ask: Ask,
    /// The settings reset asked at power on (hold MENU, PIN-6, BOOT-14): modal, so hold BACK, PTT and the menu
    /// timeout can't skip past a PIN; BACK on its first question goes on to the PIN (if one is set).
    pub boot: bool,
    /// When # went down on the armed reset (ms; 0 while it isn't held).
    pub since: u32,
}

/// The reset happens after # is held this long (BOOT-14).
pub const RESET_HOLD_MS: u32 = 3000;

/// Opens the confirm for `ask`, remembering the screen it came from for BACK.
pub fn ask(app: &mut App, ask: Ask) {
    app.open(Dest::Confirm);
    app.screen = Screen::Confirm(Confirm { ask, boot: false, since: 0 });
}

/// Title, what goes, what stays (PIN-7: what will be cleared and what is kept; the last two in the text pack, UPD-22).
pub(crate) fn words(ask: Ask) -> (&'static str, Option<Txt>, Txt, &'static str) {
    match ask {
        Ask::Reset { armed } => (
            if armed { "Hold # for 3 s to reset" } else { "Reset the radio?" },
            Some(txt!("Every setting and the PIN, channels, zones, range presets, the home channel, messages, the event log, waypoints. Setup runs again.")),
            txt!("Calibration, the SSB patch, the bootloader."),
            "Reset",
        ),
        Ask::DeleteChannel(_) => ("Delete this channel?", None, txt!("Every other channel, and the zone it was in."), "Delete"),
        Ask::DeleteZone(_) => ("Delete this zone?", None, txt!("Its channels: they move to no zone and stay in All."), "Delete"),
        Ask::DeletePreset(_) => ("Delete this range?", Some(txt!("Its skipped frequencies too.")), txt!("Every other range preset, and the channels."), "Delete"),
        Ask::ClearSkips => ("Clear the skip list?", None, txt!("The channels and frequencies themselves, and the range presets."), "Clear"),
        Ask::ClearRecent => ("Clear the recent list?", None, txt!("The HF VFOs and your channels."), "Clear"),
        Ask::ToneTx(_) => ("Set the TX tone too?", None, txt!("The RX tone is already set. BACK leaves TX as it is."), "Set"),
        Ask::AcceptCal => (
            "Use the calibration as it is?",
            Some(txt!("Ferrite's record of it. It changed since Ferrite first read it: another firmware may have written it.")),
            txt!("The calibration, read only. Each value is checked first: it is used only if all of them pass."),
            "Use it",
        ),
        Ask::CopyToVfo => (
            "Copy to VFO?",
            Some(txt!("The VFO's frequency and settings: the channel's take their place.")),
            txt!("The channel, untouched: V/M goes back to it."),
            "Copy",
        ),
    }
}

impl View for Confirm {
    fn name(self) -> &'static str {
        "Confirm"
    }

    fn timeout(self) -> Option<u32> {
        (!self.boot).then_some(MENU_TIMEOUT_MS)
    }

    fn modal(self) -> bool {
        self.boot
    }

    fn ptt(self, app: &mut App, down: bool) {
        // At power on PTT waits, as it does for the PIN (PIN-1).
        if down && !self.boot {
            super::nav::transmit(app);
        }
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        match (e.key, e.press) {
            (Key::Menu, Press::Short) => match self.ask {
                Ask::Reset { armed: false } => app.screen = Screen::Confirm(Confirm { ask: Ask::Reset { armed: true }, ..self }),
                Ask::Reset { armed: true } => {}
                Ask::DeleteChannel(num) => {
                    app.delete_channel(num, store); // slots to VFO, no longer home (CHAN-18)
                    let mut l = Buf::<32>::new();
                    chan_label(num, &mut l);
                    app.show_toast(l.as_bytes(), b"deleted", false);
                    app.back();
                    if let Screen::Browse(b) = app.screen {
                        app.screen = Screen::Browse(super::browse::after_delete(b, store, app));
                    }
                }
                Ask::DeleteZone(z) => {
                    let name = store.zone_label(z, b"Zone", b"Zone");
                    if !store.data().request(Op::DeleteZone(z)) {
                        app.write(ChanWrite::DeleteZone(z));
                    }
                    app.zone_deleted(z); // CHAN-12.1
                    app.show_toast(name.as_bytes(), b"deleted", false);
                    app.back();
                    if let Screen::Zones(_) = app.screen {
                        app.screen = Screen::Zones(super::zones::after_delete(store));
                    }
                }
                Ask::DeletePreset(i) => {
                    let d = store.data();
                    let name = d.preset(i).map(|p| p.name);
                    if d.request(Op::DeletePreset(i)) {
                        app.show_toast(b"Deleted", name.as_ref().map_or(b"".as_slice(), |n| n.as_bytes()), false);
                    }
                    app.back();
                    if let Screen::ScanLists(s) = app.screen {
                        app.screen = Screen::ScanLists(super::scanlists::ScanLists { cur: Default::default(), ..s });
                    }
                }
                Ask::ClearSkips => {
                    store.data().request(Op::ClearSkips);
                    app.show_toast(b"Skip list", b"cleared", false);
                    app.back();
                    if let Screen::ScanLists(s) = app.screen {
                        app.screen = Screen::ScanLists(super::scanlists::ScanLists { cur: Default::default(), ..s });
                    }
                }
                Ask::ClearRecent => {
                    app.set.hf.recent = [0; super::hf::RECENT];
                    app.show_toast(b"Recent list", b"cleared", false);
                    app.back();
                    if let Screen::Recall(_) = app.screen {
                        app.screen = Screen::Recall(super::recall::Recall { cur: Default::default() });
                    }
                }
                // TSRCH-16 (the tone hint), as tone search asks (TSRCH-4): TX too, loaded on a channel.
                Ask::ToneTx(t) => {
                    let a = app.active;
                    super::probe::set_tone(app, a, t, true);
                    super::probe::applied(app, a, b"RX and TX tone set");
                }
                // VFO-23, KEY-23: home, then the copy and its toast.
                Ask::CopyToVfo => {
                    app.back();
                    super::home::copy_to_vfo(app);
                }
                // CAL-20: the store checks the sector and records it; About shows the outcome (Valid or Invalid).
                Ask::AcceptCal => {
                    app.back();
                    if store.data().request(Op::AcceptCal) {
                        app.show_toast(b"Calibration", b"checking", false);
                    }
                }
            },
            (Key::Exit, Press::Short) if matches!(self.ask, Ask::ToneTx(_)) => {
                super::probe::applied(app, app.active, b"RX tone set");
            }
            // TSRCH-16, TSRCH-9: on a channel # saves the RX tone just set into its record.
            (Key::Hash, Press::Short) if matches!(self.ask, Ask::ToneTx(_)) && app.slot(app.active).on_channel() => {
                app.save_channel(app.active, None);
                app.back();
                app.show_toast(b"Tone saved", b"in the channel", false);
            }
            (Key::Exit, Press::Short) if self.ask == (Ask::Reset { armed: true }) => {
                app.screen = Screen::Confirm(Confirm { ask: Ask::Reset { armed: false }, since: 0, ..self })
            }
            (Key::Exit, Press::Short) => app.back(),
            _ => {}
        }
    }

    /// The armed reset counts # held from its key down, not from the long press.
    fn raw(self, app: &mut App, key: Option<Key>, _: Option<u8>, down: bool) {
        if self.ask == (Ask::Reset { armed: true }) && key == Some(Key::Hash) {
            app.screen = Screen::Confirm(Confirm { since: if down { app.now.max(1) } else { 0 }, ..self });
        }
    }

    fn tick(self, app: &mut App, _: &ferrite_core::msg::LiveAll) {
        if self.since != 0 && app.now.wrapping_sub(self.since) >= RESET_HOLD_MS {
            app.screen = Screen::Confirm(Confirm { since: 0, ..self });
            app.want_reset = true; // done in App::tick, which has the store
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let (mut title, goes, kept, _) = words(self.ask);
        if self.since != 0 {
            title = ["Resetting in 3 s", "Resetting in 2 s", "Resetting in 1 s"][(app.now.wrapping_sub(self.since) / 1000).min(2) as usize];
        }
        // What it names: the channel (the one to delete, or the active slot's for the copy), the zone, the tone.
        let mut what = Buf::<40>::new();
        let chan = match self.ask {
            Ask::DeleteChannel(n) => Some(n),
            Ask::CopyToVfo => app.slot(app.active).ch.filter(|_| app.slot(app.active).on_channel()).map(|c| c.num),
            _ => None,
        };
        match self.ask {
            Ask::DeleteChannel(_) | Ask::CopyToVfo => {
                if let Some(n) = chan {
                    let mut l = Buf::<32>::new();
                    chan_label(n, &mut l);
                    what.push(l.as_bytes());
                    if let Some(c) = cx.store.get(n) {
                        what.push(" \u{b7} ".as_bytes());
                        what.push(c.name());
                        // The frequency only when it fits whole: a cut-off number is worse than none.
                        let mut full = what;
                        full.push(" \u{b7} ".as_bytes());
                        full.push(w::freq3(c.rx).as_bytes());
                        if ferrite_ui::gfx::text4_width(full.as_bytes(), &UI_12B) <= 208 {
                            what = full;
                        }
                    }
                }
            }
            Ask::DeleteZone(z) => {
                what.set(cx.store.zone_label(z, b"?", b"?").as_bytes());
                let _ = ufmt::uwrite!(what, " \u{b7} {} channels", cx.store.zone_len(z));
            }
            Ask::DeletePreset(i) => what.set(cx.store.data().preset(i).map_or(Buf::<12>::new(), |p| p.name).as_bytes()),
            Ask::ToneTx(t) => {
                let mut b = Buf::<16>::new();
                tone_text(t, &mut b);
                what.set(b.as_bytes());
            }
            Ask::Reset { .. } | Ask::AcceptCal | Ask::ClearSkips | Ask::ClearRecent => {}
        }
        if app.dirty(R_TOP, Mix::new().any(&self.ask).h(&what).h(&title).get()) {
            lcd.fill(0, 21, 240, 279, BG);
            let col = if matches!(self.ask, Ask::ToneTx(_) | Ask::CopyToVfo) { TEXT } else { AMBER };
            w::field(lcd, 12, 216, 36, 54, 50, title.as_bytes(), &w::st(&UI_12B, col, BG, Align::Left));
            let mut y = 64;
            if !what.is_empty() {
                lcd.fill_r(8, y, 224, 30, 2, SURFACE);
                w::field(lcd, 16, 208, y + 6, y + 24, y + 20, what.as_bytes(), const { &w::st(&UI_12B, TEXT, SURFACE, Align::Left) });
                y += 40;
            }
            if let Some(goes) = goes {
                w::field(lcd, 12, 216, y, y + 17, y + 13, b"Goes", const { &w::st(&UI_12B, SUB, BG, Align::Left) });
                y += 18 + 17 * para(lcd, 12, y + 18, 216, 4, 17, crate::text::load(goes).as_bytes(), &UI_12, TEXT, BG);
                y += 8;
            }
            w::field(lcd, 12, 216, y, y + 17, y + 13, b"Kept", const { &w::st(&UI_12B, SUB, BG, Align::Left) });
            para(lcd, 12, y + 18, 216, 4, 17, crate::text::load(kept).as_bytes(), &UI_12, TEXT, BG);
        }
    }

    fn hints(self, app: &App) -> Hints {
        if matches!(self.ask, Ask::ToneTx(_)) {
            return if app.slot(app.active).on_channel() {
                Hints::new(&[("OK", "TX too"), ("#", "Save"), ("BACK", "RX only")])
            } else {
                Hints::new(&[("OK", "TX too"), ("BACK", "RX only")])
            };
        }
        match words(self.ask).3 {
            "Reset" if self.ask == (Ask::Reset { armed: true }) => Hints::new(&[("Hold #", "Reset"), ("BACK", "Cancel")]),
            "Reset" => Hints::new(&[("OK", "Next"), ("BACK", "Cancel")]),
            "Set" => Hints::new(&[("OK", "Set"), ("BACK", "Cancel")]),
            "Copy" => Hints::new(&[("OK", "Copy"), ("BACK", "Cancel")]),
            _ => Hints::new(&[("OK", "Delete"), ("BACK", "Cancel")]),
        }
    }
}

/// The reset (BOOT-14): every setting and the PIN, the slots to VFO, and everything the stores keep
/// ([`Op::Wipe`]); calibration and the SSB patch are never touched. Setup runs again, as on a new radio.
pub(crate) fn reset_all(app: &mut App, store: &impl ChannelStore) {
    reset_settings(app);
    app.set.setup_done = false;
    if !store.data().request(Op::Wipe) {
        app.write(ChanWrite::EraseAll); // a store without a data owner yet: its channels at least
    }
    for s in app.slots.iter_mut() {
        (s.ch, s.vfo_mode, s.zone, s.pos) = (None, true, 0, 0);
    }
    for s in 0..SLOTS as u8 {
        app.send_slot(s);
    }
    (app.inbox, app.log_unread, app.alert, app.pages_unread) = (0, 0, 0, 0);
    app.reset_aprs();
    app.first_boot();
    app.show_toast(b"Reset done", b"", false);
}

/// Every setting back to its default, the PIN with them (PIN-4); channels, calibration and the SSB patch are
/// not touched here (PIN-8).
pub(crate) fn reset_settings(app: &mut App) {
    *app.set = crate::settings::Settings { setup_done: app.set.setup_done, ..crate::settings::Settings::DEFAULT };
    (app.watch, app.watch_on) = (1, 1);
    app.bsave = 1;
    app.recent = [None; 3];
    app.undo = None;
    app.unlock();
    for s in app.slots.iter_mut() {
        let n = Slot::new(s.vfo);
        (s.cfg, s.mem) = (n.cfg, n.mem);
    }
    for s in 0..SLOTS as u8 {
        app.send_slot(s);
    }
    app.out.push(RadioCmd::Watch(app.watch_mask()));
    app.send_radio_settings();
    app.out.push(RadioCmd::BatterySave(crate::level(app.bsave)));
}
