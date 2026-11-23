//! What survives a power cycle, loaded from any bytes (ferrite-fuzzkit): the settings record as the store loads it
//! (an older firmware's shorter record, a newer one's longer, a damaged one) and the slots' state. Seeds are real
//! records, the defaults and settings stepped through the menu's own changes, then mutated.
//!
//! Invariants: no panic; after `sanitize` every setting the menu shows from a list shows one of its choices
//! (`screens::groups`: the knob's values), every menu value draws; a loaded record saved and loaded again is the
//! same; the slots' state restored, saved and restored again is the same, and the home screen draws.

use super::*;
use crate::screens::groups::{all_values, change, settable, wired};
use crate::sample::Sample;
use ferrite_core::msg::{Device, LiveAll};
use ferrite_fuzzkit::{mutate, Fuzz, Rng};
use ferrite_ui::text::Buf;
use std::string::String;
use std::sync::OnceLock;
use std::vec::Vec;

struct V(Vec<u8>);

impl Sink for V {
    fn put(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
}

fn bytes(p: &dyn Fn(&mut dyn Sink)) -> Vec<u8> {
    let mut v = V(Vec::new());
    p(&mut v);
    v.0
}

fn app_with(set: Settings) -> App {
    let s = crate::tests::set();
    *s = set;
    let mut app = App::new(s, crate::tests::alert());
    while app.pop_cmd().is_some() {}
    app
}

/// The settings as the store loads them (`FlashStore::load_settings`).
fn load(b: &[u8]) -> Settings {
    let mut set = Settings::DEFAULT;
    set.mic_v2 = false;
    set.load(&mut SliceSource(b));
    set.mic_upgrade();
    set.sanitize();
    set
}

fn path(p: &[u8; 3]) -> &[u8] {
    &p[..if p[2] == 0 { 2 } else { 3 }]
}

/// Settings records: the defaults, and settings stepped through the menu (each a real value the knob gives).
fn seeds() -> &'static Vec<Vec<u8>> {
    static S: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    S.get_or_init(|| {
        let mut v = std::vec![bytes(&|s| Settings::DEFAULT.save(s))];
        let paths: Vec<[u8; 3]> = all_values().into_iter().map(|x| x.0).collect();
        let mut r = Rng::new(0x5E7);
        for _ in 0..12 {
            let mut app = app_with(Settings::DEFAULT);
            for _ in 0..40 {
                let p = *r.pick(&paths);
                let _ = change(&mut app, path(&p), r.range(1, 5) as i32 * if r.one_in(2) { -1 } else { 1 });
                while app.pop_cmd().is_some() {}
            }
            let set = *app.set;
            v.push(bytes(&|s| set.save(s)));
        }
        v
    })
}

fn check_settings(b: &[u8]) {
    let set = load(b);
    let app = app_with(set);
    let dev = Device::default();
    let mut shown = Buf::<24>::new();
    for (p, values) in all_values() {
        shown.clear();
        let Some(_) = wired(&app, &dev, path(&p), &mut shown) else { continue };
        let text = String::from_utf8_lossy(shown.as_bytes()).into_owned();
        // DTMF digit length and gap (0 8 5 3, 0 8 5 5): a value between the knob's steps is a real duration,
        // clamped to the range where it is shown and used; 0 7 3 2 with its text shows the text
        let off_grid_ok = matches!(path(&p), [8, 5, 3] | [8, 5, 5]) || p == [7, 3, 2] && set.pon == 2;
        assert!(values.is_empty() || values.contains(&text) || off_grid_ok, "0 {:?} shows {text:?}, not one of its choices {values:?}", path(&p));
    }
    for p in settable() {
        shown.clear();
        let _ = wired(&app, &dev, p, &mut shown);
    }
    // stable: saved and loaded again, the same
    let again = load(&bytes(&|s| set.save(s)));
    assert!(again == set, "a loaded record saved and loaded again differs");
}

fn mutated(r: &mut Rng, corpus: &[Vec<u8>], max: usize) -> Vec<u8> {
    if r.one_in(8) {
        let n = r.len(max);
        return r.bytes(n);
    }
    let mut d = r.pick(corpus).clone();
    mutate(r, &mut d, max, corpus);
    if r.one_in(4) {
        d.truncate(r.below(d.len() + 1)); // an older firmware's shorter record
    }
    d
}

#[test]
fn fuzz_settings_record() {
    let s = seeds();
    assert!(s.len() > 5 && s[0].len() > 100);
    check_settings(&s[0]);
    Fuzz::new("settings_record", 0x5E70, 20_000).run(|r| mutated(r, s, 1024), check_settings);
}

/// A byte for each knob step on random settings rows, from the defaults: the menu itself never leaves a setting
/// out of its choices either.
#[test]
fn fuzz_settings_knob() {
    let paths: Vec<[u8; 3]> = all_values().into_iter().map(|x| x.0).collect();
    Fuzz::new("settings_knob", 0x5E71, 3_000).run(
        |r| {
            let n = r.len(64);
            r.bytes(n * 2)
        },
        |steps| {
            let mut app = app_with(Settings::DEFAULT);
            for s in steps.chunks_exact(2) {
                let p = paths[s[0] as usize % paths.len()];
                let _ = change(&mut app, path(&p), s[1] as i8 as i32);
                while app.pop_cmd().is_some() {}
            }
            let set = *app.set;
            let b = bytes(&|s| set.save(s));
            assert!(load(&b) == set, "a setting the menu chose was changed by loading it");
            check_settings(&b);
        },
    );
}

#[test]
fn fuzz_slot_state() {
    let st = Sample::new();
    let base = {
        let app = app_with(Settings::DEFAULT);
        bytes(&|s| app.save_state(s))
    };
    let corpus = std::vec![base];
    Fuzz::new("slot_state", 0x5107, 1_000).run(
        |r| mutated(r, &corpus, 600),
        |b| {
            let mut app = app_with(Settings::DEFAULT);
            app.restore_state(&mut SliceSource(b), &st);
            while app.pop_cmd().is_some() {}
            // a slot on a channel takes the store's copy of it as it is restored: settled after one pass
            let restore = |b: &[u8]| {
                let mut a = app_with(Settings::DEFAULT);
                a.restore_state(&mut SliceSource(b), &st);
                bytes(&|s| a.save_state(s))
            };
            let once = restore(&bytes(&|s| app.save_state(s)));
            assert_eq!(restore(&once), once, "restored, saved and restored again differs");
            let live = LiveAll::default();
            app.tick(2000, &live, &st);
            let mut fb = ferrite_ui::fb::FrameBuffer::new();
            app.render(&mut fb, &live, &Device::default(), &st);
        },
    );
}
