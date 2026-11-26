//! Renders of group D's screens (system and modes) into `target/sim/`: setup's eight pages, the Extend TX
//! warning, the PIN, About and its pages, the key test, the link screens, FM broadcast, HF mode, the alert and
//! the menu's groups. Each is reached the way the radio reaches it (keys, or the hook the firmware calls).

use ferrite_sim::fake;
use ferrite_app::screens::alert::AlertInfo;
use ferrite_app::screens::nav::Dest;
use ferrite_app::App;
use ferrite_core::keys::Key;
use ferrite_core::msg::Device;
use ferrite_ui::fb::FrameBuffer;

struct Shot {
    app: App,
    dev: Device,
    radio: fake::FakeRadio,
    t: u32,
    scale: u32,
    fb: FrameBuffer,
}

impl Shot {
    fn run(&mut self, ms: u32) {
        let store = fake::store();
        let end = self.t + ms;
        while self.t <= end {
            while let Some(c) = self.app.pop_cmd() {
                self.radio.cmd(c);
            }
            self.radio.step(self.t as f32 / 1000.0, self.app.active());
            self.app.tick(self.t, &self.radio.live, &store);
            self.app.render(&mut self.fb, &self.radio.live, &self.dev, &store);
            self.t += 20;
        }
    }

    fn press(&mut self, key: Key, hold: u32) {
        let store = fake::store();
        self.app.key_down(key, self.t, &store);
        self.t += hold;
        self.app.tick(self.t, &self.radio.live, &store);
        self.app.key_up(key, self.t, &store);
        self.run(40);
    }

    fn keys(&mut self, keys: &[Key]) {
        for &k in keys {
            self.press(k, 80);
        }
    }

    fn home(&mut self) {
        self.press(Key::Exit, 800);
    }

    /// MENU 0 and a path.
    fn menu0(&mut self, path: &[u8]) {
        self.home();
        self.keys(&[Key::Menu, Key::Digit(0)]);
        for &d in path {
            self.press(Key::Digit(d), 80);
        }
    }

    fn open(&mut self, d: Dest) {
        self.app.open(d);
        self.run(40);
    }

    fn save(&mut self, name: &str) {
        self.run(40);
        if self.app.toast_up() {
            self.run(3100); // no stale toast in the picture
        }
        crate::save(&self.fb, name, self.scale);
    }
}

pub fn run(scale: u32) {
    let store = fake::store();
    let mut dev = fake::device();
    dev.clock_min = Some(14 * 60 + 32);
    dev.utc = false;
    let mut app = fake::app();
    app.settings_mut().backlight = 0; // shots: the screen never goes dark (lights tested in ferrite-app)
    let mut radio = fake::FakeRadio::new();
    fake::sample(&mut app, &store, &mut radio);
    let mut s = Shot { app, dev, radio, t: 0, scale, fb: FrameBuffer::new() };
    s.run(500);

    // Setup, from the first-boot hook (BOOT-12), with the mock-ups' answers: from nothing set.
    s.app.settings_mut().callsign = ferrite_app::settings::Text::EMPTY;
    s.app.settings_mut().set_region(None);
    s.app.first_boot();
    s.save("setup-1-welcome");
    s.press(Key::Menu, 80);
    use Key::Digit as D;
    s.keys(&[D(8), D(8), D(8), D(2), Key::Star, D(7), Key::Star, D(3), D(3), D(3)]);
    s.run(1100);
    s.keys(&[D(3), D(3)]);
    s.run(1100);
    s.keys(&[D(7), D(7), D(7)]);
    s.save("setup-2-callsign");
    s.press(Key::Menu, 80);
    s.press(Key::Down, 80); // Canada
    s.save("setup-3-region");
    s.press(Key::Menu, 80);
    s.keys(&[D(2), Key::Star]);
    s.keys(&[D(5), D(5), D(5), D(3), D(3), D(9), D(9)]);
    s.save("setup-4-owner");
    s.press(Key::Menu, 80);
    s.save("setup-5-time");
    s.press(Key::Menu, 80);
    s.save("setup-6-look");
    s.press(Key::Menu, 80);
    s.save("setup-7-battery");
    s.press(Key::Menu, 80);
    s.save("setup-8-summary");
    s.press(Key::Menu, 80);

    // The menu's groups (MENU 0 5: the MenuGroup mock-up), a list, the Extend TX warning.
    s.menu0(&[5]);
    s.press(Key::Down, 80);
    s.save("group-5");
    s.menu0(&[9]);
    s.save("group-9");
    s.menu0(&[9, 2]);
    s.save("list-9-2");
    s.menu0(&[8, 8]);
    s.app.knob(1, s.t, &store);
    s.save("extend-tx");
    s.home();

    // PIN: setting one, then asked at power on.
    s.menu0(&[9, 3]);
    s.app.knob(1, s.t, &store);
    s.keys(&[D(1), D(9), D(5)]);
    s.save("pin-set");
    s.keys(&[D(0), Key::Menu]);
    s.app.ask_pin();
    s.keys(&[D(1), D(9)]);
    s.save("pin");
    s.keys(&[D(5), D(0), Key::Menu]);
    // With an owner card (PIN-2.1).
    let keep = (s.app.settings().owner, s.app.settings().boot_card);
    ferrite_app::sample::owner_card(s.app.settings_mut());
    s.app.ask_pin();
    s.save("pin-owner");
    s.keys(&[D(1), D(9), D(5), D(0), Key::Menu]);
    (s.app.settings_mut().owner, s.app.settings_mut().boot_card) = keep;

    // About and the read-only pages.
    s.menu0(&[9, 8]);
    s.save("about");
    s.press(Key::Right, 80);
    s.save("about-selftest");
    for (d, name) in [
        (Dest::Calibration, "calibration"),
        (Dest::AdvSquelch, "adv-squelch"),
        (Dest::SsbPatch, "ssb-patch"),
        (Dest::SsbConfirm, "ssb-confirm"),
        (Dest::Mailbox, "mailbox"),
        (Dest::LinkStatus, "link-status"),
        (Dest::PowerOff, "power-off"),
    ] {
        s.home();
        s.open(d);
        s.save(name);
    }
    // Power off, a write session and the empty battery are modal: setup from the first-boot hook leaves them
    // (and # then OK finish it, keeping the answers).
    let leave = |s: &mut Shot| {
        s.app.first_boot();
        s.keys(&[Key::Hash, Key::Menu]);
    };
    leave(&mut s);
    s.dev.link.write_pct = Some(62);
    s.open(Dest::WriteProgress);
    s.save("write-progress");
    s.dev.link.write_pct = None;
    leave(&mut s);
    let mv = s.dev.battery_mv;
    s.dev.battery_mv = 6_420;
    s.open(Dest::EmptyBattery);
    s.save("empty-battery");
    s.dev.battery_mv = mv;
    leave(&mut s);

    // A8, A13, A14, A15: the fixed position (a latitude half typed), SAME location codes (one being typed),
    // the DTMF dialer with digits, and the memories.
    s.home();
    s.menu0(&[6, 9]);
    s.keys(&[D(4), D(5), D(2), D(5)]);
    s.save("position-entry");
    s.home();
    s.menu0(&[4, 5, 2]);
    s.keys(&[Key::Down, Key::Menu, D(0), D(5), D(3)]);
    s.save("location-codes");
    s.home();
    s.menu0(&[8, 5, 1]);
    s.keys(&[D(1), D(2), D(3), Key::Hash]);
    s.save("dtmf-dialer");
    s.home();
    s.menu0(&[8, 5, 2]);
    s.save("dtmf-memories");
    s.home();

    // The key test after a few keys; MENU 0 9 1 2.
    s.menu0(&[9, 1, 2]);
    s.keys(&[Key::Up, D(5), Key::Exit]);
    s.app.knob(1, s.t, &store);
    s.press(Key::Star, 900);
    s.save("key-test");
    s.home();

    // FM broadcast's settings (MENU 0 2 6; decision 352: a slot in the band is the receiver).
    s.menu0(&[2, 6]);
    s.save("fm-menu");
    s.home();

    // HF mode: the patch loading, the screen, its options, the recent list; and the Status page meanwhile.
    s.menu0(&[3, 3]);
    s.run(700); // the fake radio streams the patch in about 2 s
    s.save("hf-loading");
    s.run(2000);
    s.keys(&[D(7), D(0), D(7), D(4), Key::Menu, Key::Star]);
    s.run(10_500);
    s.press(Key::Up, 80);
    s.save("hf");
    s.press(Key::Menu, 800);
    s.save("hf-menu");
    s.press(Key::Exit, 80);
    s.press(Key::Hash, 80); // the recent list (C18): group C's shot is "hf-recent"
    s.press(Key::Exit, 80);
    s.press(Key::Menu, 800);
    s.keys(&[D(0), D(0)]); // the full menu from HF options, then Status: HF keeps running underneath
    s.save("status-hf");
    s.home();

    // A weather alert as in the Alert mock-up.
    let mut a = AlertInfo::NONE;
    a.event.set(b"Severe thunderstorm warning");
    a.areas.set("Ottawa \u{b7} Gatineau".as_bytes());
    a.from.set(b"Env. Canada");
    (a.issued, a.until, a.slot, a.freq) = (14 * 60 + 28, 15 * 60 + 15, 2, 16_255_000);
    s.app.raise_alert(&a);
    s.run(100);
    s.save("alert");
}
