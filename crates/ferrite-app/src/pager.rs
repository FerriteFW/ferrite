//! POCSAG pagers (manual chapter 27: DEC-4, DEC-7, DEC-18; 0 4 2): when the pager decoder is wanted, and what a
//! decoded page becomes. Shared by the firmware's UI task and the simulators.

use crate::data::{Event, Src};
use crate::App;
use ferrite_core::decoders::Page;
use ferrite_core::msg::LiveAll;
use ferrite_ui::text::Buf;
use ufmt::uwrite;

impl App {
    /// The pager slot: C in Pager mode (DEC-20; PWR-24: never in Low Power Mode).
    pub fn pager_slot(&self, _live: &LiveAll) -> Option<u8> {
        self.pager_on().then_some(crate::aprs::APRS_SLOT)
    }

    /// DEC-23: the speed 0 4 2 5 sets, in baud.
    pub fn pager_baud(&self) -> u16 {
        ferrite_core::pagerrx::BAUDS[self.set.pocsag_fast as usize % 2]
    }

    /// Whether the DSP task should run the pager decoder (DEC-18: it gets the slot when APRS doesn't want it).
    pub fn pager_wanted(&self, live: &LiveAll) -> bool {
        self.pager_slot(live).is_some()
    }

    /// A page the decoder heard: through the capcode filter (0 4 2 3), into the inbox with a toast if Popups is on
    /// (DEC-7),
    /// and the event log record to keep (INBOX-6). None when the filter drops it.
    pub fn pager_hit(&mut self, p: &Page, live: &LiveAll) -> Option<Event> {
        let s = &self.set;
        if s.pocsag_cap != 0 && p.capcode != s.pocsag_cap {
            return None;
        }
        let slot = self.pager_slot(live).unwrap_or(0);
        let freq = s.pocsag_freq;
        let mut head = Buf::<20>::new();
        let _ = uwrite!(head, "Pager {}", p.capcode);
        let text: &[u8] = if p.len == 0 { b"(tone only)" } else { p.text() };
        self.notify_decode(head.as_bytes(), text);
        self.pages_unread = self.pages_unread.saturating_add(1); // DEC-22: in the mail badge
        self.pages_boot = self.pages_boot.saturating_add(1); // DEC-20.1: the card's count and newest page
        self.last_page = (p.capcode, self.now);
        Some(Event { src: Src::Pocsag, ago_s: 0, freq, slot, unread: true, id: p.capcode, x: p.baud, y: p.function as u16, pos: None, name: Buf::new(), text: Buf::from(text) })
    }
}

#[cfg(test)]
mod tests {
    use crate::App;
    use ferrite_core::decoders::{Page, PAGE_TEXT};
    use ferrite_core::msg::LiveAll;

    struct Harness {
        app: App,
    }

    fn page(capcode: u32, text: &[u8]) -> Page {
        let mut p = Page { capcode, baud: 1200, function: 3, numeric: false, len: text.len() as u8, text: [0; PAGE_TEXT] };
        p.text[..text.len()].copy_from_slice(text);
        p
    }

    /// DEC-20: Decode (0 4 2 2) is Pager mode: slot C on the pager frequency, the decoder there; off puts back
    /// the APRS mode it replaced; none in Low Power Mode.
    #[test]
    fn pager_mode_holds_c() {
        let mut t = Harness { app: App::new(crate::tests::set(), crate::tests::alert()) };
        let live = LiveAll::default();
        assert!(!t.app.pager_wanted(&live), "POCSAG off by default");
        t.app.set_aprs_select(2); // APRS mode On
        t.app.set_pager(true);
        assert_eq!(t.app.aprs_holder(), Some(crate::aprs::Holder::Pager));
        assert_eq!(t.app.set.aprs_select(), 4, "0 5 3 says Pager");
        assert!(!t.app.set.aprs_mode && !t.app.aprs_on(), "APRS off meanwhile");
        assert_eq!(t.app.pager_slot(&live), Some(crate::aprs::APRS_SLOT));
        assert_eq!(t.app.tuned_freq(crate::aprs::APRS_SLOT), 43_998_750, "C on 0 4 2's frequency");
        let x = t.app.toast.expect("a toast");
        assert_eq!((x.head.as_bytes(), x.body.as_bytes()), (&b"Pager on"[..], &b"C on 439.987, APRS off"[..]));
        t.app.set_pager(false);
        assert_eq!(t.app.aprs_holder(), Some(crate::aprs::Holder::Mode), "APRS mode back");
        assert!(!t.app.pager_wanted(&live));
    }

    #[test]
    fn capcode_filter_and_the_record() {
        let mut t = Harness { app: App::new(crate::tests::set(), crate::tests::alert()) };
        t.app.set.pocsag_on = true;
        let mut live = LiveAll::default();
        live.slots[2].freq = 43_998_750;
        let before = (t.app.badge(), t.app.log_unread);
        let e = t.app.pager_hit(&page(1_234_567, b"MEETING MOVED TO 1930"), &live).expect("logged");
        assert_eq!((e.id, e.x, e.y, e.slot, e.freq), (1_234_567, 1200, 3, 2, 43_998_750));
        assert_eq!(e.text.as_bytes(), b"MEETING MOVED TO 1930");
        assert_eq!((t.app.badge(), t.app.log_unread), (before.0 + 1, before.1 + 1), "DEC-22: in the mail badge and the log's count");
        t.app.set.pocsag_cap = 42;
        assert!(t.app.pager_hit(&page(1_234_567, b"NOT FOR US"), &live).is_none(), "filtered");
        assert!(t.app.pager_hit(&page(42, b""), &live).is_some_and(|e| e.text.as_bytes() == b"(tone only)"));
    }

    /// DEC-7: with Popups (0 4 4) off a page still counts in the log's unread and is logged, but shows no toast.
    #[test]
    fn popups_off_logs_without_a_toast() {
        let mut t = Harness { app: App::new(crate::tests::set(), crate::tests::alert()) };
        t.app.set.pocsag_on = true;
        t.app.set.popups = false;
        let mut live = LiveAll::default();
        live.slots[0].freq = 43_998_750;
        let before = (t.app.badge(), t.app.log_unread);
        assert!(t.app.pager_hit(&page(7, b"QUIET"), &live).is_some(), "still logged");
        assert_eq!((t.app.badge(), t.app.log_unread), (before.0 + 1, before.1 + 1), "DEC-22: in the mail badge and the log's count");
        assert!(t.app.toast.is_none(), "no toast with Popups off");
        t.app.set.popups = true;
        t.app.pager_hit(&page(7, b"LOUD"), &live);
        assert!(t.app.toast.is_some(), "a toast with Popups on");
    }
}
