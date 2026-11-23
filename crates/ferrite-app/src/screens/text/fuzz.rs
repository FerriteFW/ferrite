//! Text entry and predictive text fuzzed (ferrite-fuzzkit): any key sequence, taps and holds with any pauses, on
//! every text the radio asks for (KEY-12) and in Compose with predictive text on and off (TEXT-1 – TEXT-12), the
//! screen drawn after every key; and the word lookup on any keys.
//!
//! Invariants: no panic; the text never longer than what it is for takes, a digits-only text only digits; every
//! word a lookup gives has the keys asked for and the count agrees with the lookup.

use super::*;
use crate::sample::Sample;
use crate::screens::nav::Dest;
use crate::words;
use ferrite_core::msg::{Device, LiveAll};
use ferrite_fuzzkit::Fuzz;
use ferrite_ui::fb::FrameBuffer;
use std::vec;
use std::vec::Vec;

const FOR: [TextFor; 17] = [
    TextFor::ChannelName,
    TextFor::Channel(0),
    TextFor::SaveName,
    TextFor::Zone(0),
    TextFor::Preset(0),
    TextFor::Callsign,
    TextFor::PowerOn,
    TextFor::DtmfName(0),
    TextFor::GridFixed,
    TextFor::GridWaypoint,
    TextFor::Capcode,
    TextFor::Clock,
    TextFor::AprsPath,
    TextFor::ContactCall,
    TextFor::ContactName,
    TextFor::ReplyTo,
    TextFor::Callsign,
];

/// The keys text entry uses most, and the rest now and then.
fn key(b: u8) -> Key {
    match b % 24 {
        0..=9 => Key::Digit(b % 24),
        10..=12 => Key::Digit(2 + b % 8),
        13 => Key::Star,
        14 => Key::Hash,
        15 => Key::Menu,
        16 => Key::Exit,
        17 => Key::Left,
        18 => Key::Right,
        19 => Key::Up,
        20 => Key::Down,
        21 => Key::Slot,
        22 => Key::VfoMem,
        _ => Key::Side(1 + b / 24 % 2),
    }
}

#[test]
fn fuzz_text_entry() {
    Fuzz::new("text_entry", 0x7E47, 500).slow_ms(5_000).run(
        |r| {
            let mut v = vec![r.byte(), r.byte()];
            let n = r.len(60);
            for _ in 0..n {
                // a key, then its hold and the pause after it: short, a multi-tap's second, long
                v.push(r.byte());
                let any = r.byte();
                v.push(*r.pick(&[0, 0, 0, 1, 2, 3, 8, 12, any]));
            }
            v
        },
        |d| {
            let (which, flags) = (d[0], d[1]);
            let mut app = App::new(crate::tests::set(), crate::tests::alert());
            while app.pop_cmd().is_some() {}
            let st = Sample::new();
            let live = LiveAll::default();
            let dev = Device::default();
            let mut fb = FrameBuffer::new();
            app.set.predictive = flags & 1 != 0;
            let to = FOR[which as usize % FOR.len()];
            if flags & 2 != 0 {
                app.open(Dest::Compose);
            } else {
                open(&mut app, to, &st);
            }
            let mut t = 1000u32;
            for k in d[2..].chunks_exact(2) {
                let (key, hold) = (key(k[0]), k[1] as u32);
                app.key_down(key, t, &st);
                t += 40 + hold * 100;
                app.tick(t, &live, &st);
                app.key_up(key, t, &st);
                t += 30 + (k[0] as u32 & 0x60) * 12;
                app.tick(t, &live, &st);
                while app.pop_cmd().is_some() {}
                while let Some(w) = app.pop_write() {
                    st.apply(w);
                }
                if let Screen::Text(x) = app.screen() {
                    let to = x.to;
                    assert!(app.text.len() <= to.max(), "{:?}: {} bytes, takes {}", to, app.text.len(), to.max());
                    if matches!(to, TextFor::Capcode | TextFor::Clock) {
                        assert!(app.text.as_bytes().iter().all(u8::is_ascii_digit), "{to:?}: {:?}", app.text.as_bytes());
                    }
                }
                app.render(&mut fb, &live, &dev, &st);
            }
        },
    );
}

#[test]
fn fuzz_word_lookup() {
    Fuzz::new("word_lookup", 0x7E48, 20_000).run(
        |r| {
            let mut v = vec![r.below(8) as u8];
            if r.one_in(2) {
                // a real word's keys, whole, cut short or run on
                let w = *r.pick(&["the", "you", "thanks", "received", "repeater", "meet", "home", "qsl", "way", "on", "my", "are", "see", "later", "good", "morning", "net", "check"]);
                v.extend(w.bytes().map(words::key_of));
                v.truncate(1 + r.below(w.len() + 1));
                if r.one_in(3) {
                    v.push(b'2' + r.below(8) as u8);
                }
            } else {
                let n = r.len(words::WORD_MAX + 2);
                v.extend((0..n).map(|_| if r.one_in(12) { r.byte() } else { b'2' + r.below(8) as u8 }));
            }
            v
        },
        |d| {
            let (&n, keys) = d.split_first().unwrap();
            let count = words::count(keys);
            for i in 0..count.min(n as usize + 1) {
                let w = words::nth(keys, i).expect("as many words as the count");
                assert!(w.text.len() <= words::WORD_MAX);
                let typed: Vec<u8> = w.text.as_bytes().iter().map(|&c| words::key_of(c)).collect();
                assert_eq!(typed, keys, "{:?} isn't typed with {:?}", w.text.as_bytes(), keys);
                let _ = (words::short(w.text.as_bytes()), words::phrase(keys, w.text.as_bytes()));
            }
            assert!(words::nth(keys, count).is_none(), "a word past the count");
            let _ = words::quick(n as usize);
        },
    );
}
