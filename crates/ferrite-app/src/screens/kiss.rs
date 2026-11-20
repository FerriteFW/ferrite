//! KISS TNC mode (MENU 0 3 7): a full screen, on only while it is up. Slot C sits on the TNC's own
//! frequency for the stay; BACK gives it back to the radio. The host keys the transmitter.

use super::entry::{self, Entry, EntryFor, FreqSet};
use super::shared::*;
use super::{Cx, Hints, View};
use crate::aprs::{region_freq, APRS_SLOT};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::settings::KISS_PORTS;
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_core::msg::LiveAll;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// No cursor: the frequency and the port live in settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Kiss;

/// MENU 0 3 7. Off becomes Cable, so the screen opens on a port. Low Power Mode is refused by [`App::open`].
pub fn open(app: &mut App) -> Screen {
    if app.set.kiss_port == 0 {
        app.set.kiss_port = 1;
    }
    Screen::Kiss(Kiss)
}

/// The mode screen is the one up, or under it (the frequency box).
pub(crate) fn up(app: &App) -> bool {
    matches!(app.screen, Screen::Kiss(_)) || app.back.iter().flatten().any(|s| matches!(s, Screen::Kiss(_)))
}

/// Cable or Bluetooth. SIDE2 flips between those two; Off is not a choice on the screen.
fn flip_port(app: &mut App) {
    app.set.kiss_port = if app.set.kiss_port == 1 { 2 } else { 1 };
}

fn port_name(app: &App) -> &'static str {
    KISS_PORTS[app.set.kiss_port as usize % KISS_PORTS.len()]
}

/// Bluetooth chosen while Bluetooth is off: the screen stays, and says so.
fn bt_off(app: &App) -> bool {
    app.set.kiss_port == 2 && !app.set.bt
}

/// Idle, RX or TX from slot C. Bluetooth off is its own line.
fn traffic(live: &LiveAll) -> &'static str {
    if live.tx.keyed && live.tx.slot == APRS_SLOT {
        "TX"
    } else if live.slots[APRS_SLOT as usize].squelch_open {
        "RX"
    } else {
        "Idle"
    }
}

/// The readback line: the same facts the screen draws.
pub(crate) fn line<S: ChannelStore>(app: &App, live: &LiveAll, store: &S, b: &mut Buf<80>) {
    let (sent, heard) = store.data().kiss_counts();
    let f = crate::slot::mhz3(app.kiss_freq());
    let _ = uwrite!(
        b,
        "kiss {} {} sent {} heard {}",
        core::str::from_utf8(f.as_bytes()).unwrap_or(""),
        port_name(app),
        sent,
        heard,
    );
    if bt_off(app) {
        b.push(b" bluetooth off");
    } else {
        b.push(b" ");
        b.push(traffic(live).as_bytes());
    }
}

impl View for Kiss {
    fn name(self) -> &'static str {
        "KISS"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"KISS");
    }

    /// SIDE1 is not the monitor here.
    fn own_side1(self) -> bool {
        true
    }

    /// The host keys the transmitter. PTT, SLOT and V/M do nothing.
    fn ptt(self, _: &mut App, _: bool) {}

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        match (e.key, e.press) {
            (Key::Up, Press::Short | Press::Repeat) => app.step_kiss_freq(1),
            (Key::Down, Press::Short | Press::Repeat) => app.step_kiss_freq(-1),
            (Key::Menu, Press::Short) => entry::open_setting(app, FreqSet::Kiss),
            // A digit starts the frequency, as on a VFO: the box opens with it typed, and the mode stays under it.
            (Key::Digit(d), Press::Short) => {
                app.open_screen(Screen::Entry(Entry::new(EntryFor::Setting(FreqSet::Kiss), Some(d))));
            }
            (Key::Hash, Press::Short) => {
                let _ = app.set_kiss_freq(region_freq(app.set.region()));
                let f = crate::slot::mhz3(app.kiss_freq());
                app.show_toast(b"Region default", f.as_bytes(), false);
            }
            (Key::Side(2), Press::Short) => flip_port(app),
            (Key::Exit, Press::Short) => app.back(),
            (Key::Slot | Key::VfoMem | Key::Ptt(_) | Key::Side(1), _) => {}
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        app.step_kiss_freq(d);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let f = app.kiss_freq();
        if app.dirty(1, Mix::new().h(&f).get()) {
            w::field(lcd, 8, 224, 28, 46, 40, b"Packet frequency", const { &w::st(&UI_12, SUB, BG, Align::Left) });
            ferrite_ui::canvas::banded(lcd, 0, 48, 240, 52, BG, |lcd| {
                w::freq(lcd, 8, 232, 94, f, &DIG_46, &DIG_28, TEXT, SUB, BG);
            });
        }
        let port = port_name(app);
        let off = bt_off(app);
        let (sent, heard) = cx.store.data().kiss_counts();
        let state = traffic(cx.live);
        if app.dirty(2, Mix::new().h(&port).h(&off).h(&sent).h(&heard).h(&state).get()) {
            ferrite_ui::canvas::banded(lcd, 0, 108, 240, 150, BG, |lcd| {
                w::field(lcd, 8, 224, 116, 138, 132, port.as_bytes(), const { &w::st(&UI_15B, TEXT, BG, Align::Left) });
                if off {
                    w::field(lcd, 8, 224, 144, 162, 156, b"Bluetooth is off", const { &w::st(&UI_12B, AMBER, BG, Align::Left) });
                    w::field(lcd, 8, 224, 164, 182, 176, b"Turn it on: MENU 0 9 2 1", const { &w::st(&UI_12, SUB, BG, Align::Left) });
                }
                let mut counts = Buf::<32>::new();
                let _ = uwrite!(counts, "Sent {} \u{b7} Heard {}", sent, heard);
                let y = if off { 196 } else { 156 };
                w::field(lcd, 8, 224, y, y + 18, y + 13, counts.as_bytes(), const { &w::st(&UI_12, TEXT, BG, Align::Left) });
                if !off {
                    let c = match state {
                        "TX" => AMBER,
                        "RX" => GREEN,
                        _ => DIM,
                    };
                    w::field(lcd, 8, 224, 184, 206, 200, state.as_bytes(), &w::st(&UI_15B, c, BG, Align::Left));
                }
            });
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[("0-9", "Freq"), ("SIDE2", "Port"), ("BACK", "Back")])
    }
}
