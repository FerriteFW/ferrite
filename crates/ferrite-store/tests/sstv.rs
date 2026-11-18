//! SSTV (chapter 27a) through the App and a real store: a picture decoded by the codec goes into its slot (header,
//! rows, end), the next slot gets ready, the screen shows it, and a picture written over the link lands in a
//! "to send" slot without stopping the store (SSTV-4.5).

use ferrite_app::chan::ChannelStore;
use ferrite_app::data::NoData;
use ferrite_app::settings::Settings;
use ferrite_app::sstv::{headers, VIEW_CARD};
use ferrite_app::App;
use ferrite_core::sstv::{self, Event, Header, HEADER_LEN, ROW_LEN, WIDTH};
use ferrite_core::usstv;
use ferrite_store::sim::SimFlash;
use ferrite_store::FlashStore;

type Store = FlashStore<SimFlash, NoData>;

fn app() -> App {
    let set: &'static mut Settings = Box::leak(Box::new(Settings::DEFAULT));
    let alert: &'static mut ferrite_app::screens::alert::AlertInfo = Box::leak(Box::new(ferrite_app::screens::alert::AlertInfo::NONE));
    App::new(set, alert)
}

fn mount() -> Store {
    FlashStore::mount(SimFlash::new(), NoData)
}

/// The codec's lines, as the DSP task would hand them over, for a Martin M1 picture of `card`.
struct Collect(Vec<Event>, Vec<[u16; WIDTH]>);
impl usstv::Sink for Collect {
    fn start(&mut self, m: &'static usstv::Mode) {
        self.0.push(Event::Start(m.vis));
        self.1.push([0; WIDTH]);
    }
    fn line(&mut self, y: u16, px: &[u16; WIDTH]) {
        self.0.push(Event::Line(y, 0));
        self.1.push(*px);
    }
    fn end(&mut self, whole: bool) {
        self.0.push(Event::End(whole));
        self.1.push([0; WIDTH]);
    }
}

struct Card;
impl usstv::Picture for Card {
    fn rgb(&mut self, x: usize, y: usize) -> (u8, u8, u8) {
        sstv::test_card(x, y, b"VA7TST\0\0\0\0")
    }
}

fn decoded(vis: u8) -> Collect {
    let mode = usstv::Mode::by_vis(vis).unwrap();
    let mut e = usstv::Encoder::new(mode, 24_000, 8000);
    let mut audio = vec![0i16; 6000];
    while let Some(s) = e.next(&mut Card) {
        audio.push(s);
    }
    audio.extend([0i16; 6000]);
    let mut d = usstv::Decoder::new(24_000);
    let mut c = Collect(Vec::new(), Vec::new());
    d.feed(&audio, &mut c);
    c
}

fn row(st: &Store, slot: u8, y: u16) -> [u8; ROW_LEN] {
    let mut b = [0u8; ROW_LEN];
    assert!(st.data().data_read(sstv::row_addr(slot, y), &mut b));
    b
}

/// SSTV-2, SSTV-4.1, SSTV-4.4: a received picture into the first received slot, its header finished at the end.
#[test]
fn received_picture_is_saved() {
    let st = mount();
    let mut a = app();
    a.sstv_open(&st);
    assert_eq!(a.sstv.view, VIEW_CARD, "nothing received yet: the test card");
    let c = decoded(44);
    for (ev, px) in c.0.iter().zip(&c.1) {
        a.sstv_event(*ev, px, &st);
    }
    let hs = headers(&st);
    let h = hs[1].expect("slot 1 (the first received) holds it");
    assert_eq!((h.kind, h.vis, h.lines, h.complete, h.seq), (sstv::KIND_RX, 44, Some(256), true, 1));
    // the rows are the decoder's, little-endian RGB565
    let (_, px) = c.0.iter().zip(&c.1).find(|(e, _)| matches!(e, Event::Line(100, _))).unwrap();
    let r = row(&st, 1, 100);
    assert_eq!(u16::from_le_bytes([r[0], r[1]]), px[0]);
    assert_eq!(u16::from_le_bytes([r[638], r[639]]), px[319]);
    // a second picture goes into the next slot, numbered after
    for (ev, px) in c.0.iter().zip(&c.1) {
        a.sstv_event(*ev, px, &st);
    }
    let hs = headers(&st);
    assert_eq!(hs[2].map(|h| h.seq), Some(2));
    assert_eq!(a.sstv.view, 2, "the screen shows the newest");
}

/// SSTV-4.1: with all four received slots full, the oldest is erased ready for the next picture.
#[test]
fn oldest_received_is_written_over() {
    let st = mount();
    let mut a = app();
    let c = decoded(8);
    a.sstv_open(&st);
    for _ in 0..5 {
        for (ev, px) in c.0.iter().zip(&c.1) {
            a.sstv_event(*ev, px, &st);
            }
    }
    let seqs: Vec<_> = headers(&st)[1..].iter().map(|h| h.map(|h| h.seq)).collect();
    // three received slots: the fourth over the first, the fifth over the second; the oldest left (the third)
    // already erased for the next one
    assert_eq!(seqs, vec![Some(4), Some(5), None]);
}

/// SSTV-4.5: a link write inside the picture slots is a picture's: the store keeps running (no update state).
#[test]
fn console_picture_write_leaves_the_store_running() {
    use ferrite_app::data::DataWrite;
    let st = mount();
    let a0 = sstv::slot_addr(0);
    assert!(st.data().picture_write(DataWrite::Erase(a0)));
    let h = Header { kind: sstv::KIND_SEND, vis: 0, complete: true, lines: Some(256), utc: 0, freq: 0, name: *b"photo\0\0\0\0\0\0\0\0\0\0\0", seq: 0 };
    let mut bytes = [0u8; 64];
    bytes.copy_from_slice(&h.encode()[..HEADER_LEN]);
    assert!(st.data().picture_write(DataWrite::Program { addr: a0, len: 64, bytes }));
    assert_eq!(headers(&st)[0], Some(h));
    assert_eq!(st.data().layout_found(), 0, "not an update");
}

/// SSTV-4.1: a slot with no picture but old bytes in it (the range was free before) is erased before a picture
/// goes in, so the picture isn't programmed over them.
#[test]
fn old_bytes_in_a_slot_are_erased_first() {
    use ferrite_app::data::DataWrite;
    let st = mount();
    // old bytes in slot 1, header sector and pixels, no magic
    for off in [0u32, 4096, 4096 + 100 * 640] {
        let mut bytes = [0x5Au8; 64];
        bytes[0] = 0;
        assert!(st.data().picture_write(DataWrite::Program { addr: sstv::slot_addr(1) + off, len: 64, bytes }));
    }
    let mut a = app();
    a.sstv_open(&st);
    let c = decoded(8);
    for (ev, px) in c.0.iter().zip(&c.1) {
        a.sstv_event(*ev, px, &st);
    }
    let h = headers(&st)[1].expect("the picture in slot 1");
    assert_eq!((h.vis, h.lines, h.complete), (8, Some(240), true));
    let (_, px) = c.0.iter().zip(&c.1).find(|(e, _)| matches!(e, Event::Line(100, _))).unwrap();
    let r = row(&st, 1, 100);
    assert_eq!(u16::from_le_bytes([r[0], r[1]]), px[0], "row 100 is the picture's, not old bytes ANDed in");
}

/// SSTV-2.6: a picture holds the active slot's squelch open (its monitor) until it ends; a monitor the user latched
/// is left alone.
#[test]
fn a_picture_holds_the_squelch_open() {
    use ferrite_core::msg::RadioCmd;
    let st = mount();
    let mut a = app();
    a.sstv_open(&st);
    while a.pop_cmd().is_some() {}
    let c = decoded(8);
    let mut on = false;
    let mut off = false;
    for (ev, px) in c.0.iter().zip(&c.1) {
        a.sstv_event(*ev, px, &st);
        while let Some(cmd) = a.pop_cmd() {
            match cmd {
                RadioCmd::Monitor { slot: 0, on: true } => on = true,
                RadioCmd::Monitor { slot: 0, on: false } => off = on,
                _ => {}
            }
        }
    }
    assert!(on && off, "held open at the start, closed at the end");
}
