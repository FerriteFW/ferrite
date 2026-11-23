//! Entry and editing (docs/screens.md group A): direct entry, the channel editor, the value editor, the tone
//! picker, text entry, save as channel, add repeater, confirm, PIN, setup, the shortcut map; and the start-up
//! state sync.

use super::*;
use crate::chan::{ChanWrite, FLAG_SKIP};
use crate::screens::chedit::ChEdit;
use crate::screens::confirm::Ask;
use crate::screens::nav::Dest;
use crate::slot::{DCS_FLAG, STEPS};

impl T {
    fn digits(&mut self, s: &str) {
        for c in s.chars() {
            match c {
                '*' => self.tap(Key::Star),
                d => self.tap(Key::Digit(d as u8 - b'0')),
            }
        }
    }

    fn writes(&mut self) -> Vec<ChanWrite> {
        let mut v = Vec::new();
        while let Some(w) = self.app.pop_write() {
            v.push(w);
        }
        v
    }
}

/// The old start-up queued 33 commands (now 48) into a 16-place ring and the first went missing: every slot's
/// frequency and settings, and the active slot, watch and Battery Save, must all reach the Radio task.
#[test]
fn start_up_sends_every_slot_whole() {
    let mut app = App::new(crate::tests::set(), crate::tests::alert());
    let mut v = Vec::new();
    while let Some(c) = app.pop_cmd() {
        v.push(c);
    }
    for s in 0..3u8 {
        assert!(v.contains(&RadioCmd::TuneSlot { slot: s, freq: app.slot(s).freq() }), "slot {s} tuned");
        for (param, value) in slot::params(app.slot(s).cfg()) {
            assert!(v.contains(&RadioCmd::SlotParam { slot: s, param, value }), "slot {s} {param:?}");
        }
    }
    assert!(v.contains(&RadioCmd::Active(0)) && v.contains(&RadioCmd::Watch(0b011)));
    assert!(v.iter().any(|c| matches!(c, RadioCmd::BatterySave(_))));
    assert!(v.contains(&RadioCmd::ScanTx(false)) && v.contains(&RadioCmd::ToneLevel(1)), "SCAN-22, TONE-16 defaults");
    assert!(v.contains(&RadioCmd::TxTones { phase: 0, cw: 0, sidetone: true }), "the Transmit group's tones");
    for s in 0..3u8 {
        assert!(v.contains(&RadioCmd::SlotParam { slot: s, param: Param::EndTone, value: 0 }), "slot {s} end tone");
        assert!(v.contains(&RadioCmd::SlotParam { slot: s, param: Param::Tail, value: 0 }), "slot {s} tail");
    }
    assert!(v.contains(&RadioCmd::MicGain(8)), "VOX-15: the default mic gain");
    assert!(v.contains(&RadioCmd::TxTimeout(120)), "TX-30: the default timeout");
    // Ten radio-wide (the SI4732's settings, BCR-32, twice: the region's start-up and the settings'; with the beep volume, the mic gain, the timeout and VOX: SND-3, VOX-15, TX-30.1, VOX-13), the
    // tones' four (TxTones, the callsign's two chunks, DTMF timing); per slot: tune, ten parameters (split
    // included), end tone, tail and receive-only (CHAN-4, TX audit F11).
    assert!(v.iter().any(|c| matches!(c, RadioCmd::SiOpts(_))), "BCR-32: the SI4732's receive settings");
    assert_eq!(v.len(), 15 + 3 * 14);
    // Stepping through channels faster than the commands drain: the slot goes out once, as it is now.
    let mut t = T::new();
    t.tap(Key::VfoMem);
    let s = store();
    for _ in 0..30 {
        t.app.knob(1, t.t, &s);
    }
    let v = t.cmds();
    let tunes: Vec<_> = v.iter().filter(|c| matches!(c, RadioCmd::TuneSlot { .. })).collect();
    assert_eq!(tunes, [&RadioCmd::TuneSlot { slot: 0, freq: t.freq() }], "one tune, the latest");
}

/// NAV-4, VFO-11 to VFO-14: typed frequencies, the point, snapping, HF refused.
#[test]
fn frequency_entry() {
    let mut t = T::new();
    t.digits("147225"); // a whole frequency: in without OK (VFO-17)
    assert_eq!(t.app.screen(), Screen::Home);
    assert_eq!(t.freq(), mhz(147) + 22_500);
    assert!(t.cmds().contains(&RadioCmd::TuneSlot { slot: 0, freq: mhz(147) + 22_500 }));
    // UHF, typed with the point by itself; OK takes it exactly as typed, off the step (VFO-12)
    t.digits("44612");
    t.tap(Key::Menu);
    assert_eq!(t.freq(), mhz(446) + 12_000);
    assert_eq!(slot::band_name(t.freq()), "UHF");
    // BACK deletes, then leaves
    t.digits("12");
    t.tap(Key::Exit);
    t.tap(Key::Exit);
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home);
    // < 18 MHz: HF mode (VFO-14)
    let f = t.freq();
    t.digits("7*1");
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Entry(_)), "stays, with the reason");
    assert_eq!(t.freq(), f);
    t.hold(Key::Exit);
    // 6 m with the point typed
    t.digits("52*525"); // whole on its step: in without OK
    if matches!(t.app.screen(), Screen::Entry(_)) {
        t.tap(Key::Menu);
    }
    assert_eq!(t.freq(), mhz(52) + 52_500);
}

/// VFO-24: \* after the point and a digit enters the frequency exactly as OK does (one-handed); \* \* keeps its
/// meaning (VFO-11, VFO-21: the point, then a fourth MHz digit on 100-130); \* never types a second point.
#[test]
fn star_enters_a_frequency() {
    let mut t = T::new();
    t.digits("146*52*");
    assert_eq!(t.app.screen(), Screen::Home, "the second * entered it");
    assert_eq!(t.freq(), mhz(146) + 52_000);
    assert!(t.cmds().contains(&RadioCmd::TuneSlot { slot: 0, freq: mhz(146) + 52_000 }));
    // one decimal is enough
    t.digits("4465*");
    assert_eq!((t.app.screen(), t.freq()), (Screen::Home, mhz(446) + 50_000));
    // * * on 100-130 is still the fourth MHz digit; then the point, a digit and * enter it
    t.digits("129**6*0*");
    assert_eq!((t.app.screen(), t.freq()), (Screen::Home, mhz(1296)), "1296 MHz");
    // the point with nothing after it: * again types nothing (no second point), and waits
    t.digits("146**");
    assert!(matches!(t.app.screen(), Screen::Entry(e) if e.buf[..e.n as usize] == *b"146."), "{:?}", t.app.screen());
    t.digits("5*");
    assert_eq!((t.app.screen(), t.freq()), (Screen::Home, mhz(146) + 50_000));
    // refused as OK is: the entry stays with its digits (VFO-14)
    let f = t.freq();
    t.digits("7*1*");
    assert!(matches!(t.app.screen(), Screen::Entry(e) if e.buf[..e.n as usize] == *b"7.1"), "{:?}", t.app.screen());
    assert_eq!(t.freq(), f);
    t.hold(Key::Exit);
    // channel entry: * is nothing there (no point in a channel number)
    t.tap(Key::VfoMem);
    t.digits("11*");
    assert!(matches!(t.app.screen(), Screen::Entry(e) if e.n == 2));
}

/// VFO-14, VFO-21: every reason entry gives fits its note on one line (224 px, never wrapped) and the toast it
/// splits into fits the hint bar.
#[test]
fn entry_reasons_fit_one_line() {
    use ferrite_ui::assets::UI_12;
    use ferrite_ui::gfx::text4_width;
    for f in [mhz(7), mhz(700), mhz(2000)] {
        let why = crate::screens::entry::refuse(f).expect("refused");
        assert!(text4_width(why.as_bytes(), &UI_12) <= 224, "{why}: wraps on the entry screen");
        let (head, body) = why.split_once(": ").unwrap();
        assert!(ferrite_ui::widgets::toast_fits(head.as_bytes(), body.as_bytes(), false), "{why}: toast cut");
    }
}

/// NAV-4 in channel mode: a number, its name or "empty".
#[test]
fn channel_entry() {
    let mut t = T::new();
    t.tap(Key::VfoMem);
    t.digits("11");
    assert!(matches!(t.app.screen(), Screen::Entry(_)));
    t.tap(Key::Menu);
    assert_eq!(t.app.screen(), Screen::Home);
    let s = t.app.slot(0);
    assert_eq!((s.ch.unwrap().name(), s.zone, s.pos), (&b"North hill"[..], 2, 0));
    t.digits("999");
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Entry(_)), "empty: stays");
}

/// EDIT-2, EDIT-5, CHAN-18, CHAN-19: the channel editor, renumbering, Home, undo, hold 0.
#[test]
fn channel_editor() {
    let mut t = T::new();
    t.tap(Key::VfoMem); // CH 001 Calling
    t.hold(Key::Slot);
    assert_eq!(t.app.screen(), Screen::ChEdit(ChEdit { cell: 0 }), "EDIT-2: the channel version");
    t.writes();
    // Down to the grid, then to Number (row 4, col 1) and renumber: the next free number up is CH 003.
    for k in [Key::Down, Key::Down, Key::Down, Key::Down, Key::Down, Key::Down, Key::Right] {
        t.tap(k);
    }
    assert_eq!(t.app.screen(), Screen::ChEdit(ChEdit { cell: 13 }));
    t.knob(1);
    assert_eq!(t.app.slot(0).ch.unwrap().num, 2);
    let w = t.writes();
    assert!(matches!(w[..], [ChanWrite::Put { ch, from: Some(0) }] if ch.num == 2), "{w:?}");
    // Home (EDIT-5, CHAN-18), then undo puts the number back and Home off.
    t.tap(Key::Right);
    t.knob(1);
    assert_eq!(t.app.settings().home_ch, Some(2));
    t.tap(Key::Star);
    assert_eq!(t.app.slot(0).ch.unwrap().num, 0);
    assert_eq!(t.app.settings().home_ch, None);
    assert!(matches!(t.writes()[..], [ChanWrite::Put { ch, from: Some(2) }] if ch.num == 0));
    // A flag: Skip.
    t.tap(Key::Up);
    t.tap(Key::Left); // Skip
    assert_eq!(t.app.screen(), Screen::ChEdit(ChEdit { cell: 10 }));
    t.knob(1);
    assert_ne!(t.app.slot(0).ch.unwrap().flags & FLAG_SKIP, 0);
    // A setting saves the channel too (EDIT-4).
    t.tap(Key::Up);
    t.tap(Key::Up); // RX tone
    t.writes();
    let rx0 = t.app.slot(0).cfg().rx_tone;
    t.knob(1);
    assert_ne!(t.app.slot(0).cfg().rx_tone, rx0);
    assert!(matches!(t.writes()[..], [ChanWrite::Put { ch, .. }] if ch.cfg.rx_tone != rx0));
    // Home on, leave, go to the VFO; hold 0 brings the home channel back (CHAN-19).
    t.app.settings_mut().home_ch = Some(10);
    t.tap(Key::Exit);
    t.tap(Key::VfoMem);
    assert!(!t.app.slot(0).on_channel());
    t.hold(Key::Digit(0));
    assert_eq!(t.app.screen(), Screen::Home);
    assert_eq!(t.app.slot(0).ch.unwrap().name(), b"North hill");
    assert!(t.app.slot(0).on_channel());
}

/// CHAN-32, decision 223: the channel editor keeps the shared channel rules (crate::chfield). An RX-only channel's
/// power is greyed with the reason and the knob leaves it (CHAN-4, EDIT-7); the knob steps RX over 630-756 MHz
/// (CHAN-22); a shift that would put TX above 1000 MHz is refused with a toast and nothing saved (CHAN-22); typed RX
/// likewise.
#[test]
fn channel_editor_shared_rules() {
    use crate::chan::{Channel, FLAG_RX_ONLY};
    use crate::screens::entry::{Entry, EntryFor};
    use ferrite_core::freq::mhz;
    let mut t = T::new();
    t.tap(Key::VfoMem);
    t.hold(Key::Slot);
    let c = t.app.slot(0).ch.unwrap();
    t.app.slots[0].ch = Some(Channel { flags: c.flags | FLAG_RX_ONLY, ..c });
    t.app.screen = Screen::ChEdit(ChEdit { cell: 7 }); // Power
    let p = t.app.slot(0).cfg().power;
    t.knob(1);
    assert_eq!(t.app.slot(0).cfg().power, p, "RX only: unchanged");
    assert_eq!(t.app.greyed(Item::Power), Some("this channel is RX only"));
    t.app.slots[0].ch = Some(Channel { rx: mhz(630), ..c });
    t.app.screen = Screen::ChEdit(ChEdit { cell: 2 });
    t.knob(1);
    assert_eq!(t.app.slot(0).ch.unwrap().rx, mhz(756), "over the gap");
    t.app.slots[0].ch = Some(Channel { rx: mhz(999) + 90_000, ..c });
    t.app.screen = Screen::ChEdit(ChEdit { cell: crate::screens::chedit::OFFSET_CELL });
    t.writes();
    t.knob(1);
    assert_eq!(t.app.slot(0).cfg().offset, 0, "+5 MHz would TX at 1004.9");
    assert!(t.writes().is_empty());
    assert!(t.app.toast.is_some_and(|x| x.head.as_bytes() == b"Offset" && x.body.as_bytes() == b"TX outside 64 to 1000 MHz"));
    // Typed: a simplex channel above 1000 MHz is receive only and kept (VFO-21); with a shift it isn't.
    t.app.slots[0].ch = Some(Channel { rx: mhz(446), cfg: crate::slot::SlotCfg { offset: 500_000, ..c.cfg }, ..c });
    t.app.screen = Screen::Entry(Entry::new(EntryFor::ChannelFreq, None));
    t.digits("129**6");
    t.tap(Key::Menu);
    assert_eq!(t.app.slot(0).ch.unwrap().rx, mhz(446), "TX 1301 MHz: refused");
    assert!(t.app.toast.is_some_and(|x| x.body.as_bytes() == b"TX outside 64 to 1000 MHz"));
    t.app.slots[0].ch = Some(Channel { rx: mhz(446), ..c });
    t.app.screen = Screen::Entry(Entry::new(EntryFor::ChannelFreq, None));
    t.digits("129**6");
    t.tap(Key::Menu);
    assert_eq!(t.app.slot(0).ch.unwrap().rx, mhz(1296), "simplex: kept");
}

/// MENU-17, MENU-20, SQL-7: the bigger editor changes live; BACK puts the old value back, OK keeps.
#[test]
fn value_editor() {
    let mut t = T::new();
    t.tap(Key::Menu);
    t.tap(Key::Menu); // OK on Squelch
    assert!(matches!(t.app.screen(), Screen::Value(_)));
    t.knob(2);
    assert_eq!(t.app.slot(0).cfg().squelch, 6);
    t.tap(Key::Digit(9)); // digits type a level
    assert_eq!(t.app.slot(0).cfg().squelch, 9);
    t.tap(Key::Exit);
    assert_eq!(t.app.slot(0).cfg().squelch, 4, "BACK restores");
    assert_eq!(t.app.screen(), Screen::Quick(crate::screens::quick::Quick { row: 0 }));
    t.tap(Key::Digit(2));
    t.tap(Key::Menu); // TX power
    t.tap(Key::Right);
    t.tap(Key::Menu); // OK keeps
    assert_eq!(t.app.slot(0).cfg().power, 1);
    let mut fb = FrameBuffer::new();
    t.tap(Key::Digit(1));
    t.tap(Key::Menu);
    t.render(&mut fb);
    assert!(count_px(&fb, 44, 206, |c| c == BLUE) > 300, "the box and the levels");
}

/// TONE-2, TONE-2.1: RX and TX apart; digits type a tone; ▶ to DCS-N starts at D023; BACK puts both back.
#[test]
fn tone_picker() {
    let mut t = T::new();
    t.tap(Key::Menu);
    t.tap(Key::Digit(4));
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Tones(_)));
    t.digits("885");
    assert_eq!(t.app.slot(0).cfg().tx_tone, 885);
    assert!(t.cmds().contains(&RadioCmd::SlotParam { slot: 0, param: Param::TxTone, value: 885 }));
    t.tap(Key::Up); // RX
    t.tap(Key::Right);
    t.tap(Key::Right); // Off, CTCSS, DCS-N: its first code
    assert_eq!(t.app.slot(0).cfg().rx_tone, DCS_FLAG | 0o023);
    assert_eq!(t.app.slot(0).cfg().tx_tone, 885, "TX untouched");
    t.wait(2000);
    t.digits("754"); // on a DCS code, digits are octal
    assert_eq!(t.app.slot(0).cfg().rx_tone, DCS_FLAG | 0o754);
    t.tap(Key::Exit);
    assert_eq!((t.app.slot(0).cfg().rx_tone, t.app.slot(0).cfg().tx_tone), (0, 0), "BACK restores both");
}

/// TONE-15: typed tones off the lists, as nicFW: any CTCSS 60.0-260.0 Hz, any DCS code; the knob steps on from
/// the standard ones beside it.
#[test]
fn custom_tones() {
    let mut t = T::new();
    t.tap(Key::Menu);
    t.tap(Key::Digit(4));
    t.tap(Key::Menu);
    t.digits("1234");
    assert_eq!(t.app.slot(0).cfg().tx_tone, 1234, "123.4 Hz");
    t.knob(1);
    assert_eq!(t.app.slot(0).cfg().tx_tone, 1273, "up: the next standard tone");
    t.wait(2000);
    t.digits("1234");
    t.knob(-1);
    assert_eq!(t.app.slot(0).cfg().tx_tone, 1230, "down: the standard tone below");
    t.wait(2000);
    t.digits("2700");
    assert_ne!(t.app.slot(0).cfg().tx_tone, 2700, "above 260.0 Hz: no");
    t.tap(Key::Right); // the mode: DCS-N
    t.wait(2000);
    t.digits("024");
    assert_eq!(t.app.slot(0).cfg().tx_tone & !crate::slot::DCS_INV, DCS_FLAG | 0o024, "D024 is not standard, but taken");
}

/// TONE-2.1: each tone is a mode and a value. ◀▶ go round Off, CTCSS, DCS-N, DCS-I; a new mode starts at its
/// first value (no value remembered per mode), DCS-N and DCS-I keep the code; the knob steps the value within the
/// mode only, wrapping there, and does nothing on Off; ▲▼ still pick RX or TX; the hint bar names ◀▶ Mode.
#[test]
fn tone_picker_mode_then_value() {
    use crate::screens::tones::{mode_of, MODE_NAMES};
    use crate::slot::DCS_INV;
    let mut t = T::new();
    t.tap(Key::Menu);
    t.tap(Key::Digit(4));
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Tones(_)));
    let tx = |t: &T| t.app.slot(0).cfg().tx_tone;
    assert_eq!(tx(&t), 0, "TX starts off");
    t.knob(3);
    assert_eq!(tx(&t), 0, "the knob does nothing on Off");
    t.tap(Key::Right);
    assert_eq!(tx(&t), 670, "CTCSS: its first tone");
    t.knob(-1);
    assert_eq!(tx(&t), 2541, "the knob wraps within CTCSS, never into DCS");
    t.knob(1);
    t.knob(1);
    assert_eq!(tx(&t), 693);
    t.tap(Key::Right);
    assert_eq!(tx(&t), DCS_FLAG | 0o023, "DCS-N: its first code");
    t.knob(1);
    assert_eq!(tx(&t), DCS_FLAG | 0o025);
    t.knob(-2);
    assert_eq!(tx(&t), DCS_FLAG | 0o754, "the knob wraps within DCS-N");
    t.tap(Key::Right);
    assert_eq!(tx(&t), DCS_FLAG | DCS_INV | 0o754, "DCS-I keeps the code");
    t.tap(Key::Left);
    assert_eq!(tx(&t), DCS_FLAG | 0o754, "and back");
    t.tap(Key::Left);
    assert_eq!(tx(&t), 670, "CTCSS again starts at its first: 69.3 isn't remembered");
    t.tap(Key::Left);
    assert_eq!(tx(&t), 0, "Off");
    t.tap(Key::Left);
    assert_eq!(tx(&t), DCS_FLAG | DCS_INV | 0o023, "◀ from Off wraps to DCS-I");
    assert_eq!(MODE_NAMES[mode_of(tx(&t)) as usize], "DCS-I");
    assert_eq!(t.app.screen().hints(&t.app).as_slice().get(1).map(|h| h.1), Some("Mode"), "◀▶ Mode in the hint bar");
    t.tap(Key::Up);
    t.tap(Key::Right);
    assert_eq!(t.app.slot(0).cfg().rx_tone, 670, "▲: RX, its own mode");
    assert_eq!(tx(&t), DCS_FLAG | DCS_INV | 0o023, "TX untouched");
    t.tap(Key::Exit);
    assert_eq!((t.app.slot(0).cfg().rx_tone, tx(&t)), (0, 0), "BACK restores both (MENU-20)");
}

/// KEY-12: multi-tap, case, space, delete; OK names the channel.
#[test]
fn text_entry_names_a_channel() {
    let mut t = T::new();
    t.tap(Key::VfoMem);
    t.hold(Key::Slot);
    t.tap(Key::Down); // Name
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Text(_)));
    while !t.app.text.is_empty() {
        t.tap(Key::Exit);
    }
    t.digits("44"); // H
    t.wait(1100);
    t.tap(Key::Hash); // lower case
    t.digits("444"); // i
    t.tap(Key::Star); // space
    t.tap(Key::Hash); // 123
    t.digits("2");
    assert_eq!(t.app.text.as_bytes(), b"Hi 2");
    t.tap(Key::Exit);
    t.tap(Key::Menu);
    assert_eq!(t.app.screen(), Screen::ChEdit(ChEdit { cell: 1 }));
    assert_eq!(t.app.slot(0).ch.unwrap().name(), b"Hi ");
    assert!(matches!(t.writes()[..], [ChanWrite::Put { ch, from: None }] if ch.name() == b"Hi "));
    assert!(t.app.callsign().is_empty(), "a channel name is never the callsign");
}

/// Text entry for the callsign (Dest::Text(Callsign)) sets only the callsign; for Save as channel's name, the
/// form's draft only (nothing written until Save).
#[test]
fn text_entry_callsign_and_save_name() {
    use crate::screens::text::TextFor;
    let mut t = T::new();
    t.app.open(Dest::Text(TextFor::Callsign));
    t.digits("2"); // A
    t.tap(Key::Hash); // digits
    t.digits("7");
    t.tap(Key::Menu);
    assert_eq!(t.app.callsign(), b"A7");
    assert!(t.writes().is_empty());
    // Save as channel: the name stays in the form until it saves.
    t.hold(Key::Digit(9));
    assert!(matches!(t.app.screen(), Screen::Save(_)));
    t.tap(Key::Down);
    t.tap(Key::Down); // Name
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Text(x) if x.to == TextFor::SaveName));
    t.digits("33"); // E
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Save(_)));
    assert_eq!(t.app.text.as_bytes(), b"E");
    assert!(t.writes().is_empty());
    assert_eq!(t.app.callsign(), b"A7", "the callsign is untouched");
}

/// VFO-9, CHAN-22, KEY-23: hold 9 saves the VFO as a channel at the first free number; below 64 MHz it can't.
#[test]
fn save_as_channel() {
    let mut t = T::new();
    t.hold(Key::Digit(9));
    assert!(matches!(t.app.screen(), Screen::Save(_)));
    t.tap(Key::Down);
    t.tap(Key::Down); // Name
    t.tap(Key::Menu);
    t.digits("7");
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Save(_)), "back on the form");
    t.tap(Key::Down);
    t.tap(Key::Menu); // Save
    assert_eq!(t.app.screen(), Screen::Home);
    let w = t.writes();
    let [ChanWrite::Put { ch, from: None }] = w[..] else { panic!("{w:?}") };
    assert_eq!((ch.num, ch.rx, ch.name(), ch.zone), (2, mhz(146) + 52_000, &b"P"[..], 0));
    assert!(!t.app.slot(0).on_channel(), "stays on the VFO");
    // 6 m: refused
    t.digits("52*525");
    t.tap(Key::Menu);
    t.hold(Key::Digit(9));
    t.tap(Key::Up);
    t.tap(Key::Menu);
    assert!(t.writes().is_empty());
}

/// MENU-10: Add repeater asks frequency, shift and tone, and ends in the channel editor.
#[test]
fn add_repeater() {
    let mut t = T::new();
    t.app.open(Dest::AddRepeater);
    t.tap(Key::Menu); // Frequency: type it
    t.digits("14694");
    t.tap(Key::Menu);
    let Screen::Save(s) = t.app.screen() else { panic!() };
    assert_eq!((s.freq, s.offset), (mhz(146) + 94_000, -60_000), "2 m below 147: minus");
    t.tap(Key::Down);
    t.tap(Key::Down); // Tone
    for _ in 0..13 {
        t.knob(1); // 100.0 Hz is the 13th CTCSS tone
    }
    t.tap(Key::Down);
    t.tap(Key::Down);
    t.tap(Key::Down);
    t.tap(Key::Down); // Save
    t.tap(Key::Menu);
    assert_eq!(t.app.screen(), Screen::ChEdit(ChEdit { cell: 0 }));
    let c = t.app.slot(0).ch.unwrap();
    assert!(t.app.slot(0).on_channel());
    assert_eq!((c.rx, c.cfg.offset, c.cfg.tx_tone), (mhz(146) + 94_000, -60_000, 1000));
}

/// MENU-23, BOOT-14.1, BOOT-14.3, BOOT-14.5, PIN-7: the reset says what goes, OK, then hold # for 3 s; BACK steps back;
/// everything goes and setup runs again.
#[test]
fn confirm_reset() {
    let mut t = T::new();
    t.tap(Key::Menu);
    t.tap(Key::Right); // squelch 5
    t.tap(Key::Exit);
    t.hold(Key::Menu);
    t.digits("972");
    assert!(matches!(t.app.screen(), Screen::Confirm(c) if c.ask == Ask::Reset { armed: false }));
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Group(_)), "BACK: one level, to the menu the reset came from");
    t.digits("2");
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Confirm(c) if c.ask == Ask::Reset { armed: true }));
    t.press(Key::Hash, 2000);
    assert!(matches!(t.app.screen(), Screen::Confirm(_)), "2 s isn't enough");
    assert!(!t.app.locked(), "hold # here is the reset's, not the key lock");
    assert_eq!(t.app.slot(0).cfg().squelch, 5);
    t.tap(Key::Exit);
    assert!(matches!(t.app.screen(), Screen::Confirm(c) if c.ask == Ask::Reset { armed: false }), "BACK: the first step");
    t.tap(Key::Menu);
    t.press(Key::Hash, 3200);
    assert_eq!(t.app.slot(0).cfg().squelch, 4, "settings back to default");
    assert!(matches!(t.app.screen(), Screen::Setup(_)), "setup runs again");
    assert!(!t.app.set.setup_done);
    assert_eq!(t.writes(), [ChanWrite::EraseAll], "a store without a data owner: its channels");
    t.app.go_home();
    // A channel delete names it and clears it from the slot.
    t.tap(Key::VfoMem);
    crate::screens::confirm::ask(&mut t.app, Ask::DeleteChannel(0));
    t.tap(Key::Menu);
    assert_eq!(t.writes(), [ChanWrite::Delete(0)]);
    assert!(!t.app.slot(0).on_channel());
}

/// PIN-1 to PIN-3, KEY-16: unlocking asks; three misses wait 30 s; right unlocks.
#[test]
fn pin_at_unlock() {
    let mut t = T::new();
    let p = t.app.settings_mut();
    (p.pin, p.pin_len, p.pin_mode) = (1234, 4, 2);
    t.hold(Key::Hash);
    assert!(t.app.locked());
    t.hold(Key::Hash);
    assert!(matches!(t.app.screen(), Screen::Pin(_)));
    for _ in 0..3 {
        t.digits("0000");
        t.tap(Key::Menu);
    }
    t.digits("1234"); // waiting: ignored
    t.tap(Key::Menu);
    assert!(t.app.locked());
    t.wait(30_100);
    t.wake();
    t.digits("1234");
    t.tap(Key::Menu);
    assert!(!t.app.locked());
    assert_eq!(t.app.screen(), Screen::Home);
    // At power on, PTT does nothing until it is right.
    t.app.ask_pin();
    let s = store();
    t.app.key_down(Key::PTT, t.t, &s);
    assert!(matches!(t.app.screen(), Screen::Pin(_)));
    t.app.key_up(Key::PTT, t.t + 100, &s);
}

/// HOME-8: holding a digit runs its shortcut at the long press, while held, with a popup in the middle; the
/// release does nothing. The map is a reference page at MENU 0 9 1 3.
#[test]
fn shortcut_map() {
    let mut t = T::new();
    t.app.radio.tx_built = true; // hold 2 sets power where the slot can transmit (HOME-4.2)
    t.app.set_region(Some(ferrite_core::txplan::Region::Us), false);
    let s = store();
    t.app.key_down(Key::Digit(2), t.t, &s);
    t.wait(800);
    assert_eq!(t.app.screen(), Screen::Home, "no page during the hold");
    assert_eq!(t.app.slot(0).cfg().power, 1, "run at the long press");
    let toast = t.app.toast.expect("a popup");
    assert!(toast.center, "in the middle, not over the hint bar");
    assert_eq!((toast.head.as_bytes(), toast.body.as_bytes()), (&b"TX power"[..], &b"Mid"[..]));
    t.app.key_up(Key::Digit(2), t.t, &s);
    assert_eq!(t.app.slot(0).cfg().power, 1, "the release does nothing");
    // A screen-opening shortcut names itself.
    t.app.key_down(Key::Digit(1), t.t, &s);
    t.wait(800);
    assert!(matches!(t.app.screen(), Screen::Scope(_)), "{:?}", t.app.screen());
    let toast = t.app.toast.expect("a popup");
    assert!(toast.center);
    assert_eq!((toast.head.as_bytes(), toast.body.as_bytes()), (&b"Band scope"[..], &b""[..]), "just the name");
    // It goes after POPUP_MS.
    t.wait(crate::POPUP_MS);
    assert!(t.app.toast.is_none());
    t.app.key_up(Key::Digit(1), t.t, &s);
    assert!(matches!(t.app.screen(), Screen::Scope(_)), "the release stays");
    // The reference page.
    t.app.go_home();
    t.app.open(crate::screens::nav::menu0(&[9, 1, 3]).expect("0 9 1 3"));
    assert!(matches!(t.app.screen(), Screen::Shortcut(_)));
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home);
    // KEY-21: OK runs the row from home (nicFW's function menu): 5 opens the tone picker, with its popup.
    t.app.open(crate::screens::nav::menu0(&[9, 1, 3]).expect("0 9 1 3"));
    t.tap(Key::Down);
    t.tap(Key::Digit(5));
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Tones(_)), "{:?}", t.app.screen());
    assert_eq!(t.app.toast.expect("a popup").head.as_bytes(), b"Tones");
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home, "BACK from it goes home, where it ran");
}

/// The digit's release after a shortcut opened a screen doesn't reach that screen: hold 3 opens the squelch
/// editor and the release leaves the level as it was.
#[test]
fn shortcut_release_does_not_leak() {
    let mut t = T::new();
    let s = store();
    let sq = t.app.slot(0).cfg().squelch;
    t.app.key_down(Key::Digit(3), t.t, &s);
    t.wait(800);
    assert!(matches!(t.app.screen(), Screen::Value(_)), "{:?}", t.app.screen());
    t.app.key_up(Key::Digit(3), t.t, &s);
    t.wait(100);
    assert!(matches!(t.app.screen(), Screen::Value(_)));
    assert_eq!(t.app.slot(0).cfg().squelch, sq, "the release typed nothing");
    // Hold 5: tones; hold 9 on a VFO: the save form; hold 8 on a VFO: the offset editor. Each stays open after the release.
    for d in [5, 9, 8] {
        t.app.go_home();
        t.app.key_down(Key::Digit(d), t.t, &s);
        t.wait(800);
        let open = t.app.screen();
        assert_ne!(open, Screen::Home, "hold {d}");
        t.app.key_up(Key::Digit(d), t.t, &s);
        t.wait(100);
        // the screen, not its state: the save form fills in its number and frequency once it is up
        assert_eq!(t.app.screen().name(), open.name(), "hold {d}: the release leaves it");
    }
}

/// Every string the group's screens draw is in the fonts (as `fonts_have_every_character_the_ui_draws`).
#[test]
fn group_a_strings_are_in_the_fonts() {
    use ferrite_ui::assets::{UI_12, UI_12B};
    let sources = [
        include_str!("../screens/forms.rs"),
        include_str!("../screens/entry.rs"),
        include_str!("../screens/chedit.rs"),
        include_str!("../screens/value.rs"),
        include_str!("../screens/tones.rs"),
        include_str!("../screens/text.rs"),
        include_str!("../screens/save.rs"),
        include_str!("../screens/confirm.rs"),
        include_str!("../screens/shortcut.rs"),
    ];
    let mut n = 0;
    for src in sources {
        for s in string_literals(src) {
            for c in s.chars().filter(|c| !('\x18'..='\x1b').contains(c) && *c != '\n') {
                assert!(UI_12.has(c) && UI_12B.has(c), "{c:?} in {s:?}");
                n += 1;
            }
        }
    }
    assert!(n > 2000, "{n}");
    let _ = STEPS;
}

/// Every group A screen draws in each of its states without leaving the content area.
#[test]
fn group_a_screens_draw() {
    let mut t = T::new();
    let mut fb = FrameBuffer::new();
    t.tap(Key::VfoMem);
    for dest in [Dest::Entry(1), Dest::ChannelEdit, Dest::Value(Item::Squelch), Dest::Value(Item::Step), Dest::Tones, Dest::Text(crate::screens::text::TextFor::Callsign), Dest::SaveChannel, Dest::AddRepeater, Dest::Confirm, Dest::ExtendTx, Dest::Pin, Dest::Setup] {
        t.app.open(dest);
        t.render(&mut fb);
        assert!(count_px(&fb, 21, 299, |c| c != BG) > 400, "{dest:?}");
        t.app.go_home();
        t.render(&mut fb);
    }
}

/// VFO-12, VFO-17: what's typed is tuned exactly; a whole frequency goes in without OK once three decimals sit
/// on the step's grid (or five are typed); a leading 0 is a MHz digit (0 7 8 0 0 0 = 78.000 MHz).
#[test]
fn whole_frequency_goes_in_without_ok() {
    let step = |hz10: u32| crate::slot::STEPS.iter().position(|s| s.0 == hz10).unwrap() as u8;
    let mut t = T::new();
    t.app.slots[0].vfo_mode = true;
    t.app.slots[0].cfg.step = step(500); // 5 kHz (US 2 m)
    t.digits("146940");
    assert_eq!(t.app.screen(), Screen::Home, "146.940 on a 5 kHz step: in");
    assert_eq!(t.app.slots[0].vfo, 14_694_000);
    t.digits("078000");
    assert_eq!(t.app.screen(), Screen::Home, "a leading 0: 78.000 MHz");
    assert_eq!(t.app.slots[0].vfo, 7_800_000);
    // 12.5 kHz: 145.500 is on the grid; 145.512 waits, and 5 more makes 145.5125.
    t.digits("145500");
    t.app.slots[0].cfg.step = step(1250); // tuning to 2 m took the band's step; set the one under test
    t.digits("145500");
    assert_eq!(t.app.screen(), Screen::Home);
    assert_eq!(t.app.slots[0].vfo, 14_550_000);
    t.digits("145512");
    assert!(matches!(t.app.screen(), Screen::Entry(_)), "off the grid: wait for a digit");
    t.digits("5");
    assert_eq!(t.app.screen(), Screen::Home);
    assert_eq!(t.app.slots[0].vfo, 14_551_250);
    // 146.520 at 12.5 kHz waits; OK takes it exactly, never snapped to 146.525.
    t.digits("146520");
    assert!(matches!(t.app.screen(), Screen::Entry(_)));
    t.tap(Key::Menu);
    assert_eq!(t.app.screen(), Screen::Home);
    assert_eq!(t.app.slots[0].vfo, 14_652_000);
    // Five decimals always go in, as typed.
    t.digits("14652025");
    assert_eq!(t.app.slots[0].vfo, 14_652_025);
}

/// VFO-18: 2 m and 1.25 m default to 5 kHz in the US and Canada (15/20 kHz channel plans), 12.5 kHz elsewhere;
/// a step the user picked is kept when the region changes.
#[test]
fn region_steps_on_2m() {
    use crate::slot::{band_step, BAND_125M, BAND_VHF, STEP_12K5, STEP_5K};
    use ferrite_core::txplan::Region;
    assert_eq!(band_step(BAND_VHF, Some(Region::Us)), STEP_5K);
    assert_eq!(band_step(BAND_125M, Some(Region::Canada)), STEP_5K);
    assert_eq!(band_step(BAND_VHF, Some(Region::Uk)), STEP_12K5);
    assert_eq!(band_step(BAND_VHF, None), STEP_12K5);
    let mut t = T::new();
    t.app.settings_mut().set_region(Some(Region::Us));
    t.app.slots[0].vfo_mode = true;
    t.app.slots[0].vfo = 14_652_000;
    t.app.region_steps();
    assert_eq!(t.app.slots[0].cfg.step, STEP_5K, "US: 5 kHz");
    t.digits("146940");
    assert_eq!(t.app.slots[0].vfo, 14_694_000, "146.940 lands exactly");
    t.app.slots[0].cfg.step = crate::slot::STEPS.iter().position(|s| s.0 == 2500).unwrap() as u8; // the user's 25 kHz
    t.app.settings_mut().set_region(Some(Region::Europe));
    t.app.region_steps();
    assert_eq!(t.app.slots[0].cfg.step as usize, crate::slot::STEPS.iter().position(|s| s.0 == 2500).unwrap(), "kept");
}

/// VFO-20: the mode is the bandwidth. A saved FM + narrow reads as NFM; changing the mode sends the bandwidth
/// with it; quick settings row 3 is Mode.
#[test]
fn mode_carries_the_bandwidth() {
    let mut c = crate::slot::DEFAULT_CFG;
    (c.mode, c.narrow) = (0, true);
    c.fold_bw();
    assert_eq!((c.mode, c.narrow), (1, true), "FM + narrow is NFM");
    (c.mode, c.narrow) = (2, true);
    c.fold_bw();
    assert_eq!((c.mode, c.narrow), (2, false), "AM keeps the wide filter");
    assert_eq!(crate::slot::QUICK[2], Item::Mode);
    let mut t = T::new();
    t.app.change(Item::Mode, -1);
    let m = t.app.slot(0).cfg().mode;
    let cmds = t.cmds();
    assert!(cmds.contains(&RadioCmd::SlotParam { slot: 0, param: Param::Mode, value: m as i32 }), "{cmds:?}");
    assert!(cmds.contains(&RadioCmd::SlotParam { slot: 0, param: Param::Bandwidth, value: (m == 1) as i32 }), "{cmds:?}");
}

/// VFO-21: a frequency no chip receives (630-756 MHz, or above 1300) is refused with the ranges named, and the
/// entry stays open with its digits (decided Oct 7, 2026; the ranges widened to nicFW's the same day).
#[test]
fn unreceivable_frequency_is_refused() {
    let mut t = T::new();
    t.app.slots[0].vfo_mode = true;
    let before = t.app.slots[0].vfo;
    t.digits("650000");
    assert!(matches!(t.app.screen(), Screen::Entry(_)), "the entry stays: {:?}", t.app.screen());
    assert_eq!(t.app.slots[0].vfo, before, "nothing tuned");
    assert!(t.app.toast.is_some_and(|x| x.head.as_bytes() == b"Can't tune" && x.body.as_bytes() == b"18-630, 756-1300 MHz"));
    assert!(ferrite_core::freq::receivable(ferrite_core::freq::mhz(446)) && !ferrite_core::freq::receivable(ferrite_core::freq::mhz(700)));
}

/// VFO-23, KEY-23: hold 9 on a channel asks "Copy to VFO?"; BACK cancels, OK copies it to the slot's VFO
/// (nicFW's "Chan 2 VFO"); the channel is untouched.
#[test]
fn hold_9_copies_channel_to_vfo() {
    use crate::screens::confirm::{Ask, Confirm};
    let mut t = T::new();
    t.tap(Key::VfoMem);
    assert!(t.app.slot(0).on_channel());
    let ch = t.app.slot(0).ch.unwrap();
    t.hold(Key::Digit(9));
    assert!(matches!(t.app.screen(), Screen::Confirm(Confirm { ask: Ask::CopyToVfo, .. })), "{:?}", t.app.screen());
    assert_eq!(crate::screens::confirm::words(Ask::CopyToVfo).0, "Copy to VFO?");
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home, "BACK cancels");
    assert!(t.app.slot(0).on_channel(), "still on the channel");
    t.hold(Key::Digit(9));
    t.cmds();
    t.tap(Key::Menu);
    assert_eq!(t.app.screen(), Screen::Home);
    assert!(t.app.toast.is_some_and(|x| x.head.as_bytes() == b"Copied to VFO"));
    let s = t.app.slot(0);
    assert!(s.vfo_mode && s.vfo == ch.rx, "on the VFO at the channel's frequency");
    assert_eq!((s.cfg.offset, s.cfg.tx_tone, s.cfg.mode), (ch.cfg.offset, ch.cfg.tx_tone, ch.cfg.mode));
    assert!(t.cmds().contains(&RadioCmd::TuneSlot { slot: 0, freq: ch.rx }));
    t.tap(Key::VfoMem);
    assert_eq!(t.app.slot(0).ch.unwrap().rx, ch.rx, "V/M goes back to the same channel");
}

/// KEY-23: hold V/M opens the zone list in VFO and channel mode alike (one meaning); hold 9 no longer does.
#[test]
fn hold_vm_opens_zones_in_both_modes() {
    let mut t = T::new();
    assert!(!t.app.slot(0).on_channel());
    t.hold(Key::VfoMem);
    assert!(matches!(t.app.screen(), Screen::Zones(_)), "VFO: {:?}", t.app.screen());
    t.app.go_home();
    t.tap(Key::VfoMem);
    assert!(t.app.slot(0).on_channel());
    t.hold(Key::VfoMem);
    assert!(matches!(t.app.screen(), Screen::Zones(_)), "channel: {:?}", t.app.screen());
    assert!(t.app.slot(0).on_channel(), "the slot stays where it was");
    t.app.go_home();
    t.hold(Key::Digit(9));
    assert!(!matches!(t.app.screen(), Screen::Zones(_)), "hold 9 is copy now");
}

/// TEXT-12: ◀▶ move the cursor in a name; letters, spaces and BACK work at it; OK keeps the whole name.
#[test]
fn text_cursor_edits_a_name() {
    let mut t = T::new();
    t.tap(Key::VfoMem);
    t.hold(Key::Slot);
    t.tap(Key::Down); // Name
    t.tap(Key::Menu);
    while !t.app.text.is_empty() {
        t.tap(Key::Exit);
    }
    t.digits("2"); // A
    t.wait(1100);
    t.digits("222"); // C
    assert_eq!(t.app.text.as_bytes(), b"AC");
    t.tap(Key::Left);
    t.digits("22"); // B, before C
    assert_eq!(t.app.text.as_bytes(), b"ABC");
    t.tap(Key::Star);
    assert_eq!(t.app.text.as_bytes(), b"AB C");
    t.tap(Key::Exit);
    t.tap(Key::Exit);
    assert_eq!(t.app.text.as_bytes(), b"AC", "BACK deletes before the cursor");
    t.tap(Key::Left);
    t.tap(Key::Left);
    t.tap(Key::Exit);
    assert_eq!(t.app.text.as_bytes(), b"AC", "nothing before the cursor: no delete, no leaving");
    assert!(matches!(t.app.screen(), Screen::Text(_)));
    t.tap(Key::Right);
    t.tap(Key::Right);
    t.digits("3"); // D at the end
    assert_eq!(t.app.text.as_bytes(), b"ACD");
}

/// HOME-11: a frequency box redrawn in place (no blank first: tearing on the radio, Oct 12, 2026) ends up
/// exactly as a fresh full redraw, through frequencies of different widths and a name/frequency layout change.
#[test]
fn frequency_redrawn_in_place_matches_a_full_redraw() {
    let mut t = T::new();
    let mut fb = FrameBuffer::new();
    t.render(&mut fb);
    let check = |t: &mut T, fb: &mut FrameBuffer, what: &str| {
        t.render(fb);
        let mut fresh = FrameBuffer::new();
        t.app.redraw_all();
        t.render(&mut fresh);
        for y in 0..320 {
            for x in 0..240 {
                assert_eq!(fb.at(x, y), fresh.at(x, y), "{what}: stale pixel at {x},{y}");
            }
        }
    };
    for d in [1i8, 1, -3, 10, -10] {
        t.knob(d);
        check(&mut t, &mut fb, "knob");
    }
    t.digits("52525"); // 6 m: a narrower number
    check(&mut t, &mut fb, "52.525");
    t.digits("446100");
    check(&mut t, &mut fb, "446.100");
    t.tap(Key::Slot); // the large card moves: B has a channel with a name
    check(&mut t, &mut fb, "slot B");
    t.tap(Key::VfoMem);
    check(&mut t, &mut fb, "V/M");
}


/// HOME-11: home's redraws stay cheap on the radio's bus (each pixel written costs bus time): a slot change writes
/// at most 1.7 screens' worth, a V/M change half a screen. Oct 12, 2026: 3.2 screens (five passes over the cards)
/// took about 470 ms on the radio.
#[test]
fn home_redraws_dont_overdraw() {
    let mut t = T::new();
    let mut fb = FrameBuffer::new();
    t.render(&mut fb);
    for (what, key, most) in [("slot", Key::Slot, 1.7), ("V/M", Key::VfoMem, 0.5)] {
        fb.writes.iter_mut().for_each(|w| *w = 0);
        t.tap(key);
        t.render(&mut fb);
        let screens = fb.writes.iter().map(|&w| w as f64).sum::<f64>() / 76_800.0;
        assert!(screens <= most, "{what}: {screens:.2} screens written, at most {most}");
    }
}

/// VFO-25: choosing a region moves a slot still on its US factory frequency (A 146.520, B 443.475) into that
/// region's bands, so a UK radio can transmit after setup (users' reports, Nov 17, 2026); the US keeps them, and a
/// frequency the user tuned is never moved.
#[test]
fn region_start_off_us_frequencies() {
    use ferrite_core::txplan::{check, Region};
    for (r, a, b) in [
        (Region::Us, 14_652_000, 44_347_500),
        (Region::Canada, 14_652_000, 44_347_500),
        (Region::Australia, 14_652_000, 44_347_500),
        (Region::Uk, 14_550_000, 43_350_000),
        (Region::Europe, 14_550_000, 43_350_000),
        (Region::Japan, 14_550_000, 43_350_000),
        (Region::NewZealand, 14_652_000, 43_350_000),
        (Region::Other, 14_550_000, 43_350_000),
    ] {
        let mut t = T::new();
        t.app.settings_mut().set_region(Some(r));
        t.app.region_start();
        assert_eq!((t.app.slots[0].vfo, t.app.slots[1].vfo), (a, b), "{r:?}");
        for f in [a, b] {
            assert_eq!(check(f, r, false), Ok(()), "{r:?} {f}");
        }
    }
    let mut t = T::new();
    t.app.slots[0].vfo = 14_700_000; // tuned by the user, out of band in the UK: left alone
    t.app.settings_mut().set_region(Some(Region::Uk));
    t.app.region_start();
    assert_eq!(t.app.slots[0].vfo, 14_700_000);
}
