//! The terminal's dump and load (manual chapter 37, TERM-13) against the real store: a radio's channels, zones
//! and presets dumped, loaded through the terminal into a blank flash with the writes queued as the Storage task
//! takes them (a line runs only once the store has caught up), dumped again, and dumped once more after a
//! remount: the same text each time.

use ferrite_app::chan::{ChanWrite, Channel, Name, NO_ZONE};
use ferrite_app::data::{Data, NoData, Op, RangePreset, ZoneCfg};
use ferrite_app::screens::alert::AlertInfo;
use ferrite_app::settings::Settings;
use ferrite_app::slot::{SlotCfg, CTCSS, DCS, DCS_FLAG, DCS_INV};
use ferrite_app::term::{cmd, Sink, Term};
use ferrite_app::App;
use ferrite_core::freq::mhz;
use ferrite_core::msg::{Device, LiveAll};
use ferrite_store::sim::SimFlash;
use ferrite_store::{block, FlashStore};
use ferrite_ui::text::Buf;

type Store = FlashStore<SimFlash, NoData>;

fn app() -> App {
    let set: &'static mut Settings = Box::leak(Box::new(Settings::DEFAULT));
    let alert: &'static mut AlertInfo = Box::leak(Box::new(AlertInfo::NONE));
    App::new(set, alert)
}

struct Out(Vec<u8>);

impl Sink for Out {
    fn room(&self) -> usize {
        1 << 16
    }
    fn put(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
}

fn dump(app: &App, s: &Store) -> Vec<String> {
    let dev = Device::default();
    let (mut p, mut l, mut v) = (cmd::DumpPos::default(), cmd::Line::new(), Vec::new());
    while cmd::dump_line(app, s, &dev, &mut p, &mut l) {
        v.push(String::from_utf8_lossy(l.as_bytes()).into());
    }
    v
}

/// A radio with 300 channels using every field, 20 zones and 10 presets.
fn source() -> (App, Store) {
    let s = FlashStore::mount(SimFlash::new(), NoData);
    for z in 0..20u8 {
        let mut n = Name::new();
        n.push(b"Zone ");
        n.push(&[b'A' + z]);
        let cfg = ZoneCfg { scan_all: z % 2 == 0, shown: z % 3 != 0, resume: z % 13, dwell: if z % 4 == 0 { 255 } else { z }, pri: [Some(z as u16), None], ..ZoneCfg::default() };
        assert!(s.request(Op::PutZone(z as u16, n, cfg)));
    }
    for i in 0..10u8 {
        let p = RangePreset { name: Buf::from(b"Range"), start: mhz(140) + i as u32 * 100_000, end: mhz(150), step: i, mode: i % 3, narrow: i % 2 == 0, skips: 0, resume: i % 13 };
        assert!(s.request(Op::PutPreset(i, p)));
    }
    for n in 0..300u16 {
        let k = n as u32;
        let tone = |m: u32| match m % 4 {
            0 => 0,
            1 => CTCSS[(m % 50) as usize],
            2 => DCS_FLAG | DCS[(m % 104) as usize],
            _ => DCS_FLAG | DCS_INV | DCS[(m * 7 % 104) as usize],
        };
        let cfg = SlotCfg {
            mode: (n % 3) as u8,
            narrow: n % 2 == 0,
            rx_tone: tone(k / 3),
            tx_tone: tone(k / 2),
            offset: [0, 60_000, -500_000][n as usize % 3],
            power: (n % 3) as u8,
            step: (n % 14) as u8,
            // levels 0-9 and Global
            squelch: (n % 11) as u8,
            busy: (n % 3) as u8,
            reverse: n % 5 == 0,
            split: n % 7 == 3,
            open_beep: (n % 6) as u8,
        };
        // a split: TX on 70 cm while RX is on 2 m, low power (the 1.25 m cap goes by TX)
        let cfg = if cfg.split { SlotCfg { offset: mhz(446) as i32 + (n as i32 % 16) * 1250, ..cfg } } else { cfg };
        let mut c = Channel { num: n * 3, zone: if n % 21 == 20 { NO_ZONE } else { n % 21 }, rx: mhz(144) + (k * 1250) % 400_000, name: [0; 16], cfg, flags: (n % 8) as u8, tones: 0 };
        c.set_name(format!("{}\"{}", if n % 7 == 0 { "q\\" } else { "C" }, n).as_bytes());
        assert!(s.apply(ChanWrite::Put { ch: c, from: None }));
    }
    (app(), s)
}

/// One poll of the terminal; the writes it made go to the queue; every third poll the Storage task catches up.
fn poll(b: &mut App, term: &mut Term, s: &Store, out: &mut Out, now: &mut u32) {
    *now += 10;
    term.poll(b, s, &Device::default(), &LiveAll::default(), *now, out);
    while let Some(w) = b.pop_write() {
        assert!(s.apply(w), "the queue had room: a line waits for the store");
    }
    if (*now).is_multiple_of(30) {
        while let Some(j) = s.next_job() {
            assert!(block(s.run(j)));
        }
    }
}

#[test]
fn dump_load_through_the_terminal_into_flash() {
    let (a, src) = source();
    let text = dump(&a, &src);
    assert_eq!(text.iter().filter(|l| l.starts_with("ch ")).count(), 300);

    // A blank radio whose writes queue for the Storage task, run here every few polls.
    let s = FlashStore::mount(SimFlash::new(), NoData);
    s.set_deferred(true);
    let mut b = app();
    let mut term = Term::new();
    let mut out = Out(Vec::new());
    let mut now = 1000u32;
    let mut input: Vec<u8> = b"\r\r:load\r".to_vec();
    for l in &text {
        input.extend_from_slice(l.as_bytes());
        input.extend_from_slice(b"\r\n");
    }
    for &c in &input {
        while !term.ready() {
            poll(&mut b, &mut term, &s, &mut out, &mut now);
        }
        term.feed(&mut b, c, now, &s, &Default::default(), &Default::default());
        while let Some(w) = b.pop_write() {
            assert!(s.apply(w));
        }
        poll(&mut b, &mut term, &s, &mut out, &mut now);
    }
    for _ in 0..20 {
        poll(&mut b, &mut term, &s, &mut out, &mut now);
    }
    let shown = String::from_utf8_lossy(&out.0);
    assert!(shown.contains(" 0 refused."), "{}", &shown[shown.len().saturating_sub(600)..]);
    assert!(s.idle());
    assert_eq!(dump(&b, &s), text);
    // In the flash, not just in RAM: the same after a remount.
    let again = FlashStore::mount(s.into_parts().0, NoData);
    assert_eq!(dump(&b, &again), text);
}

/// SAT-3: `:sat add` through the terminal, the writes queued as on the radio: a pasted TLE adds the ISS (no
/// transponders), OMM CSV gives it new elements (its name and place kept) and adds SO-50; `sat` lists them; a TLE
/// with a broken line 2 is refused and named; `sat del 2` deletes one.
#[test]
fn sat_paste_through_the_terminal() {
    let s = FlashStore::mount(SimFlash::new(), NoData);
    s.set_deferred(true);
    let mut b = app();
    let mut term = Term::new();
    let mut out = Out(Vec::new());
    let mut now = 1000u32;
    let input = concat!(
        "\r\r:sat add\r",
        "ISS (ZARYA)\r\n",
        "1 25544U 98067A   26304.75703384  .00006360  00000+0  12291-3 0  9994\r\n",
        "2 25544  51.6332 344.2755 0007028 332.5119  27.5497 15.49131714592920\r\n",
        "OBJECT_NAME,OBJECT_ID,EPOCH,MEAN_MOTION,ECCENTRICITY,INCLINATION,RA_OF_ASC_NODE,ARG_OF_PERICENTER,MEAN_ANOMALY,EPHEMERIS_TYPE,CLASSIFICATION_TYPE,NORAD_CAT_ID,ELEMENT_SET_NO,REV_AT_EPOCH,BSTAR,MEAN_MOTION_DOT,MEAN_MOTION_DDOT\r\n",
        "ISS (ZARYA),1998-067A,2026-11-01T06:00:00.000000,15.49131714,.0007028,51.6332,344.2755,332.5119,27.5497,0,U,25544,999,59292,.12291E-3,.6360E-4,0\r\n",
        "SO-50,2002-058C,2026-10-31T19:53:55.491936,14.83296838,.0068979,64.5527,66.1378,237.1936,122.2515,0,U,27607,999,28472,.12403E-3,.904E-5,0\r\n",
        "1 99999U 98067A   26304.75703384  .00006360  00000+0  12291-3 0  9994\r\n",
        "2 junk\r\n",
        "end\r\n",
    );
    for &c in input.as_bytes() {
        while !term.ready() {
            poll(&mut b, &mut term, &s, &mut out, &mut now);
        }
        term.feed(&mut b, c, now, &s, &Default::default(), &Default::default());
        poll(&mut b, &mut term, &s, &mut out, &mut now);
    }
    for _ in 0..20 {
        poll(&mut b, &mut term, &s, &mut out, &mut now);
    }
    let shown = String::from_utf8_lossy(&out.0).to_string();
    assert!(shown.contains("Paste satellites"), "{shown}");
    assert!(shown.contains("3 applied, 1 refused"), "{}", &shown[shown.len().saturating_sub(400)..]);
    let iss = s.sat_rec(0).unwrap();
    assert_eq!((iss.id, iss.name(), iss.ntp), (25_544, &b"ISS (ZARYA)"[..], 0));
    assert_eq!(iss.el.epoch % 86_400, 6 * 3600, "the CSV's newer elements");
    assert_eq!(s.sat_rec(1).unwrap().id, 27_607);
    let dev = Device::default();
    let mut l = cmd::Line::new();
    let list = |i| {
        let mut l2 = cmd::Line::new();
        cmd::list_line(&b, &s, &dev, &LiveAll::default(), cmd::List::Sats, i, &mut l2).then(|| String::from_utf8_lossy(l2.as_bytes()).to_string())
    };
    assert!(list(0).unwrap().starts_with("1 ISS (ZARYA) #25544"), "{:?}", list(0));
    assert!(list(1).unwrap().starts_with("2 SO-50 #27607"));
    assert!(list(2).is_none());
    l.clear();
    let mut a = app();
    assert!(matches!(cmd::run(&mut a, &s, &dev, b"sat del 2", false), Ok(cmd::Done::Said(_))));
    while let Some(j) = s.next_job() {
        assert!(block(s.run(j)));
    }
    assert_eq!(s.sat_places(), 1);
    assert!(!cmd::writes(b"sat") && cmd::writes(b"sat add"));
}
