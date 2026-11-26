//! PIN entry (chapter 34): at power on with its attempts and wait, changing the PIN (digits shown), turning
//! the lock off, unlock's PTT, and the readback.

use super::*;
use crate::screens::pin::Pin;

fn with_pin(pin: u32) -> T {
    let t = T::ready();
    (t.app.set.pin, t.app.set.pin_len, t.app.set.pin_mode) = (pin, 4, 1);
    t
}

fn digits(t: &mut T, ds: &[u8]) {
    for &d in ds {
        t.tap(Key::Digit(d));
    }
}

fn show_pin(t: &mut T, p: Pin) {
    t.app.open(Dest::Pin);
    t.app.screen = Screen::Pin(p);
    t.draw();
}

/// PIN-1, PIN-2, PIN-3: at power on: masked digits, \u{25c0} deletes, the attempts left in amber after a miss, the
/// 30 s wait after three; PTT does nothing; the readback has the heading, stars and the line.
#[test]
fn power_on_misses_and_wait() {
    let mut t = with_pin(1234);
    show_pin(&mut t, Pin::power_on());
    assert_eq!(t.name(), "PIN entry");
    t.reads("title=\"PIN\"");
    t.shows("Enter PIN");
    t.shows("This radio has a PIN.");
    t.reads("hints \"0-9\" \"Type\" \"OK\" \"Unlock\" \"BACK\" \"Delete\"");
    digits(&mut t, &[9, 9]);
    t.reads("pin \"Enter PIN\" typed=\"**\"");
    t.tap(Key::Left);
    t.reads("typed=\"*\"");
    t.tap(Key::Left);
    t.ptt(true);
    t.ptt(false);
    assert!(matches!(t.screen(), Screen::Pin(_)), "PTT waits for the PIN");
    for k in 0..3 {
        digits(&mut t, &[9, 9, 9, 9]);
        t.tap(Key::Menu);
        if k == 0 {
            t.shows("2 attempts left");
            assert!(t.px(150, 200, ferrite_ui::gfx::AMBER) > 0);
        }
        if k == 1 {
            t.shows("1 attempt left");
        }
    }
    t.shows("Wait 30 s");
    t.reads("note=\"Wait 30 s\"");
    digits(&mut t, &[1, 2, 3, 4]);
    t.tap(Key::Menu);
    assert!(matches!(t.screen(), Screen::Pin(_)), "nothing while waiting");
    t.wait(30_500);
    digits(&mut t, &[1, 2, 3, 4]);
    t.tap(Key::Menu);
    assert_eq!(t.screen(), Screen::Home);
}

/// PIN-2.1: at power on the owner card shows under the boxes (the callsign, then the lines set); not at
/// unlock or for changing the PIN, and not when the card is empty.
#[test]
fn power_on_shows_the_owner_card() {
    let mut t = with_pin(1234);
    t.app.set.set_callsign(b"va7fer");
    show_pin(&mut t, Pin::power_on());
    t.hides("IF FOUND"); // the callsign alone isn't a card
    crate::sample::owner_card(t.app.set);
    t.app.set.boot_card = false; // shown whatever the boot screen is set to
    show_pin(&mut t, Pin::power_on());
    t.shows("IF FOUND, PLEASE CONTACT");
    t.shows("VA7FER");
    t.shows("Sam Fernandez");
    t.shows("+1 604 555 0142");
    t.shows("va7fer@example.org");
    show_pin(&mut t, Pin::unlock());
    t.hides("Sam Fernandez"); // only at power on
    show_pin(&mut t, Pin::off());
    t.hides("Sam Fernandez");
}

/// PIN-4: changing the PIN: the current one, then the new one with its digits shown, then again; a second
/// entry that differs starts the new one over.
#[test]
fn change_shows_new_digits() {
    let mut t = with_pin(1234);
    let p = Pin::change(&t.app);
    show_pin(&mut t, p);
    t.shows("Current PIN");
    t.shows("The PIN set on this radio.");
    t.reads("pin \"Current PIN\"");
    digits(&mut t, &[1, 2, 3, 4]);
    t.tap(Key::Menu);
    t.shows("New PIN");
    digits(&mut t, &[5, 6, 7, 8]);
    t.shows("5");
    t.shows("8");
    t.tap(Key::Menu);
    t.shows("New PIN again");
    t.shows("The same digits once more.");
    t.reads("pin \"New PIN again\"");
    t.reads("hints \"0-9\" \"Type\" \"OK\" \"Save\"");
    digits(&mut t, &[5, 6, 7, 9]);
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "PINs differ / type it again");
    t.reads("pin \"New PIN\"");
}

/// PIN-1: turning PIN lock off asks for the PIN first ("Turn off"); away from power on PTT still transmits.
#[test]
fn off_and_ptt() {
    let mut t = with_pin(1234);
    show_pin(&mut t, Pin::off());
    t.shows("Current PIN");
    t.reads("\"OK\" \"Turn off\"");
    show_pin(&mut t, Pin::unlock());
    t.reads("\"OK\" \"Unlock\"");
    t.ptt(true);
    assert!(matches!(t.screen(), Screen::Transmit(_)), "{}", t.name());
    t.ptt(false);
}
