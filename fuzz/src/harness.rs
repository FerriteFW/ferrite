//! The fuzz targets' bodies, one function each: cargo-fuzz runs them from `fuzz_targets/` (tools/fuzz.sh), and the
//! host tests run the same functions on seeded, mutated inputs in `cargo nextest run`
//! (`crates/ferrite-app/tests/fuzz_packets.rs`, which includes this file). Each panics on a broken invariant.
#![allow(dead_code)]

use ferrite_app::aprs::{AprsCfg, DIGI_ON};
use ferrite_app::aprs_live::{AprsLive, Sending, FRAME_MAX};
use ferrite_core::decoders::Found;
use std::fmt::Write;
use uafsk::hdlc::{ax25_addresses_ok, Deframer, MAX_FRAME};
use uaprs::ax25::{Address, Frame, Tnc2};
use uaprs::kiss::{encode, Command, Params};
use uaprs::parse::{parse, Packet};
use ufx25::Receiver;
use ufx25::{decode, rs::Rs, MODES};

/// uaprs::parse on any information field, with any destination (Mic-E reads its latitude from the destination).
/// Input: one byte for the destination's length (low 3 bits, 0 to 7), the destination, then the information field.
/// Every packet type is reached through the type byte; third-party packets are parsed again, a few levels deep.
pub fn aprs_parse(data: &[u8]) {
    let Some((&n, rest)) = data.split_first() else { return };
    let n = ((n & 7) as usize).min(rest.len());
    let (mut dest, mut info) = rest.split_at(n);
    let mut sink = String::new();
    for _ in 0..4 {
        let Ok(p) = parse(dest, info) else { return };
        let _ = write!(sink, "{p:?}");
        match p {
            Packet::Position { comment, .. } | Packet::Object { comment, .. } | Packet::Item { comment, .. } => {
                let v: Vec<u8> = comment.bytes().collect();
                assert_eq!(v.len(), comment.len());
                assert_eq!(v.is_empty(), comment.is_empty());
                assert!(v.len() <= comment.raw().len());
                if let Packet::Position { pos, .. } = p {
                    let _ = write!(sink, "{:?}", pos);
                }
                return;
            }
            Packet::ThirdParty(t) => {
                dest = t.dest;
                info = t.info;
            }
            _ => return,
        }
    }
}

/// AX.25 frame decode (address field, control and PID, information field), the radio's TNC2 text of it, and the
/// TNC2 splitter and callsign reader. A parsed frame must write back to the same bytes, apart from the H bits of the
/// destination and source (which `Frame::parse` clears) and the reserved bits.
pub fn ax25_frame(data: &[u8]) {
    if let Some(f) = Frame::parse(data) {
        assert!(f.digis().len() <= uaprs::ax25::MAX_DIGIS);
        let mut out = [0u8; 400];
        let mut big = vec![0u8; f.len()];
        let n = f.write(&mut big).expect("room for the frame");
        assert_eq!(n, data.len());
        // control, PID and info as received (address bytes may differ in their reserved and H bits)
        let at = 14 + 7 * f.digis().len();
        assert_eq!(&big[at..], &data[at..], "control, PID and info round trip");
        let _ = f.write(&mut out);
        let mut t = Vec::new();
        f.emit_tnc2(|c| t.push(c));
        let mut s = String::new();
        let _ = write!(s, "{f}");
        assert_eq!(s.as_bytes(), &t[..], "Display and emit_tnc2 agree");
        let _ = Frame::new(f.src, f.dest, f.digis(), f.info);
        for d in f.digis() {
            let _ = d.call();
            let mut e = Vec::new();
            d.emit(&mut |c| e.push(c));
            let back = Address::from_text(&e).expect("a parsed address reads back as text");
            assert!(back.call().eq_ignore_ascii_case(d.call()) && back.ssid == d.ssid);
        }
    }
    if let Some(t) = Tnc2::split(data) {
        let _ = Address::from_text(t.src);
        let _ = Address::from_text(t.dest);
        for p in t.path.split(|&c| c == b',') {
            let _ = Address::from_text(p);
        }
    }
    let _ = Address::from_text(data);
    let _ = Address::from_field(data);
}

/// The KISS decoder (cable and Bluetooth TNC), byte by byte, and the parameter commands it carries; data frames go
/// on to the AX.25 parser as the APRS task's KISS path does.
pub fn kiss(data: &[u8]) {
    let mut d: uaprs::kiss::Decoder = uaprs::kiss::Decoder::new();
    let mut p = Params::default();
    for &b in data {
        if let Some(r) = d.push(b) {
            let _ = r.for_single_port();
            if let Command::Data(f) = p.apply(&r) {
                let _ = Frame::parse(f);
                // a frame encodes and decodes back to itself
                let mut e = Vec::new();
                encode(0, f, |c| e.push(c));
                let mut d2: uaprs::kiss::Decoder = uaprs::kiss::Decoder::new();
                let mut got = None;
                for &c in &e {
                    if let Some(r2) = d2.push(c) {
                        got = Some(r2.data.to_vec());
                    }
                }
                if !f.is_empty() {
                    assert_eq!(got.as_deref(), Some(f));
                }
            }
            let _ = p.flags(1200);
        }
    }
}

/// FX.25 correction and deframing of a received block (ufx25::decode, the APRS task's Fx25Block::decode path):
/// any bytes, any of the eleven codes, any tag error count. Input: one byte picking the code, then the block (padded
/// or cut to the code's length).
pub fn fx25_block(data: &[u8]) {
    let Some((&sel, rest)) = data.split_first() else { return };
    let mode = MODES[sel as usize % MODES.len()];
    let n = mode.n as usize;
    let mut block = [0u8; 255];
    let m = rest.len().min(n);
    block[..m].copy_from_slice(&rest[..m]);
    let mut b = block;
    if let Some(d) = decode(&mut b[..n], mode, sel >> 4) {
        assert!(d.frame.len() + 2 <= mode.k as usize);
        assert!(d.corrected as usize <= mode.check() / 2);
        let _ = uaprs::ax25::Frame::parse(d.frame);
    }
    // the Reed-Solomon decoder on its own, and a correction must leave a valid code word
    let rs = Rs::new(mode.check());
    let mut c = block;
    if rs.decode(&mut c[..n], mode.k as usize).is_ok() {
        let mut again = c;
        assert_eq!(rs.decode(&mut again[..n], mode.k as usize).ok(), Some(0), "a corrected block has no errors left");
    }
}

/// The FX.25 receiver bit by bit (tag hunter, collector, correction, deframing): each input byte gives 8 bits, least
/// significant first. The first byte sets the tag errors accepted (0 to 15).
pub fn fx25_bits(data: &[u8]) {
    let Some((&t, rest)) = data.split_first() else { return };
    let mut rx = Receiver::with_tag_errors(t & 15);
    for &byte in rest {
        for i in 0..8 {
            if let Some(d) = rx.push((byte >> i) & 1) {
                assert!(d.frame.len() <= 255);
                let _ = uaprs::ax25::Frame::parse(d.frame);
            }
            let _ = rx.busy();
        }
    }
    let _ = rx.block();
}

/// The HDLC deframer (flags, bit stuffing, FCS, frame length) bit by bit, plain and with soft-decision repair. Each
/// input byte gives one bit (bit 0) and a confidence (the other 7 bits); the first byte sets the repair depth.
pub fn hdlc_bits(data: &[u8]) {
    let Some((&k, rest)) = data.split_first() else { return };
    let mut plain = Deframer::new();
    let mut soft = Deframer::new();
    soft.set_repair(k & 7);
    for &b in rest {
        let bit = b & 1;
        if let Some(f) = plain.push(bit) {
            assert!(f.len() <= MAX_FRAME);
            let _ = ax25_addresses_ok(f);
            let _ = uaprs::ax25::Frame::parse(f);
        }
        if let Some((f, _fcs)) = soft.push_soft(bit, (b >> 1) as u32 * 1000) {
            assert!(f.len() <= MAX_FRAME);
            let _ = ax25_addresses_ok(f);
            let _ = soft.repaired();
        }
    }
}

/// The radio's AFSK decoder as the DSP task runs it (ferrite_core::decoders::Decoder::afsk at 24 kHz: three
/// slicers, repair, FX.25), on any audio: input bytes are little-endian 16-bit samples, fed in blocks. FX.25 blocks
/// it hands over are corrected as the APRS task does.
pub fn afsk_audio(data: &[u8]) {
    let samples: Vec<i16> = data.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c)).collect();
    let mut d = ferrite_core::decoders::Decoder::afsk();
    for block in samples.chunks(96) {
        d.push(block, |f| match f {
            Found::Frame(f) => {
                assert!(f.len() <= uafsk::hdlc::MAX_FRAME);
                let _ = uaprs::ax25::Frame::parse(f);
            }
            Found::Fx25(b) => {
                let mut b = *b;
                if let Some(fr) = b.decode() {
                    let _ = uaprs::ax25::Frame::parse(fr.frame);
                }
            }
            _ => {}
        });
    }
}

/// The radio's receive path for a decoded frame: AprsLive::receive (the AX.25 parse, digipeating, the heard list,
/// messages and acks, our own echoes) then a tick (acks, digipeats and beacons queued), as the APRS task calls them.
/// Input: frames separated by 0xC0 (as KISS would delimit them); the digipeater and our callsign are on, so every
/// branch can run.
pub fn aprs_live(data: &[u8]) {
    let live = AprsLive::new();
    let mut cfg = AprsCfg::DEFAULT;
    cfg.digi = DIGI_ON | data.first().copied().unwrap_or(0) & 2;
    let me = b"VE7ABC-9";
    let mut now = 1000u32;
    let mut out = [0u8; FRAME_MAX];
    for f in data.split(|&b| b == 0xC0) {
        let rx = live.receive(f, now, me, &cfg);
        let _ = (rx.row, rx.inbound, rx.inbound_id, rx.state);
        let _ = live.tick(now, Sending::All, true, me, None, &cfg, &mut out);
        let _ = ferrite_app::aprs::wx(f);
        for i in 0..live.stations().min(4) {
            let _ = live.station(i);
        }
        now += 7;
    }
}

/// ACARS (chapter 27b): frames after their SOH through the parser, and into the RAM log as the Radio task will hand
/// them (ferrite_app::acars::AcarsLive: copies dropped, blocks joined, the text pool). Input: frames separated by
/// SOH (0x01, as each burst's sync ends); 7 s between frames, so copies, joins and overdue blocks all happen.
pub fn acars(data: &[u8]) {
    use ferrite_core::acars::{parse, JOIN_MAX, SOH, TEXT_MAX};
    let live = ferrite_app::acars::AcarsLive::new();
    let mut now = 1000u32;
    let mut text = [0u8; JOIN_MAX];
    for f in data.split(|&b| b == SOH) {
        if let Ok(p) = parse(f) {
            assert!(p.text().len() <= TEXT_MAX);
            let _ = (p.reg(), ferrite_core::acars::label(p.label), p.up());
        }
        let _ = live.frame(f, (now % 7) as u8, now);
        now += 7;
    }
    live.tick(now + 60);
    assert!(live.len() as usize <= ferrite_app::acars::KEEP);
    for i in 0..live.len() {
        let (m, n) = live.get(i, &mut text).expect("every message counted is there");
        assert!(n <= JOIN_MAX && m.blocks >= 1);
        let _ = (m.airline(), m.flight(), m.reg());
    }
}

// ---- ACARS: the receiver's audio and the screen (chapter 27b, ACARS-4, ACARS-7, ACARS-11) ----------------------

/// The ACARS screen on an App over the sample store, its frames handed in as the UI task does
/// ([`ferrite_app::App::acars_heard`]), drawn into a frame buffer with the readback's lines after each draw.
pub struct AcarsUi {
    app: Option<Box<ferrite_app::App>>,
    s: ferrite_app::sample::Sample,
    fb: ferrite_ui::fb::FrameBuffer,
    t: u32,
    live: ferrite_core::msg::LiveAll,
    dev: ferrite_core::msg::Device,
    set: *mut ferrite_app::settings::Settings,
    alert: *mut ferrite_app::screens::alert::AlertInfo,
}

impl AcarsUi {
    /// The screen open on an empty list (or the sample traffic), * on or off.
    pub fn new(samples: bool, only_text: bool) -> Self {
        let set = Box::into_raw(Box::new(ferrite_app::settings::Settings::DEFAULT));
        let alert = Box::into_raw(Box::new(ferrite_app::screens::alert::AlertInfo::NONE));
        // SAFETY: both live until Drop, after the App that borrows them is gone
        let app = Box::new(ferrite_app::App::new(unsafe { &mut *set }, unsafe { &mut *alert }));
        let mut u = AcarsUi {
            app: Some(app),
            s: ferrite_app::sample::Sample::new(),
            fb: ferrite_ui::fb::FrameBuffer::new(),
            t: 1000,
            live: Default::default(),
            dev: Default::default(),
            set,
            alert,
        };
        u.app.as_deref_mut().unwrap().settings_mut().backlight = 0;
        u.app.as_deref_mut().unwrap().settings_mut().acars_text = only_text;
        while u.app.as_deref_mut().unwrap().pop_cmd().is_some() {}
        if !samples {
            u.log().clear();
        }
        u.app.as_deref_mut().unwrap().open(ferrite_app::screens::nav::Dest::Acars);
        u.wait(200);
        u
    }

    fn log(&self) -> &ferrite_app::acars::AcarsLive {
        use ferrite_app::data::Data;
        self.s.acars().expect("the sample store keeps ACARS")
    }

    /// A frame's bytes after SOH, heard now.
    pub fn heard(&mut self, f: &[u8]) {
        self.app.as_deref_mut().unwrap().acars_heard(&self.s, f);
        assert!(self.log().len() as usize <= ferrite_app::acars::KEEP);
    }

    /// Time passing: the UI task's turns, 100 ms apart, or at most 20 of them over a long wait (a late turn).
    pub fn wait(&mut self, ms: u32) {
        let end = self.t.wrapping_add(ms);
        let step = (ms / 20).max(100);
        while self.t != end {
            self.t = self.t.wrapping_add(step.min(end.wrapping_sub(self.t)));
            self.app.as_deref_mut().unwrap().tick(self.t, &self.live, &self.s);
            self.app.as_deref_mut().unwrap().acars_tick(&self.s);
        }
    }

    /// A key pressed and held `ms` (repeats past the hold time).
    pub fn key(&mut self, k: ferrite_core::keys::Key, ms: u32) {
        self.app.as_deref_mut().unwrap().key_down(k, self.t, &self.s);
        self.wait(ms.max(50));
        self.app.as_deref_mut().unwrap().key_up(k, self.t, &self.s);
        self.wait(50);
    }

    pub fn knob(&mut self, d: i8) {
        self.app.as_deref_mut().unwrap().knob(d, self.t, &self.s);
        self.wait(50);
    }

    /// Draws the screen (back to ACARS when a key left it) and reads every readback line.
    pub fn render(&mut self) {
        use ferrite_app::Screen;
        if !matches!(self.app.as_deref_mut().unwrap().screen(), Screen::Acars(_)) {
            self.app.as_deref_mut().unwrap().open(ferrite_app::screens::nav::Dest::Acars);
            self.wait(50);
        }
        self.app.as_deref_mut().unwrap().render(&mut self.fb, &self.live, &self.dev, &self.s);
        let mut l = ferrite_app::term::cmd::Line::new();
        for i in 0..512 {
            if !ferrite_app::uistate::line(self.app.as_deref().unwrap(), &self.s, &self.dev, &self.live, i, &mut l) {
                break;
            }
        }
    }

    /// Every message's page: OK on the list, then each message drawn, scrolled to its end and stepped on.
    pub fn every_page(&mut self) {
        use ferrite_core::keys::Key;
        self.render();
        self.key(Key::Menu, 50);
        for _ in 0..=self.log().len() {
            self.render();
            for _ in 0..3 {
                self.knob(40);
                self.render();
            }
            self.key(Key::Right, 50);
        }
        self.key(Key::Exit, 50);
        self.render();
    }
}

impl Drop for AcarsUi {
    fn drop(&mut self) {
        // the App first (it borrows both), then what it borrowed
        drop(self.app.take());
        // SAFETY: from Box::into_raw in new; the App that borrowed them is dropped
        unsafe {
            drop(Box::from_raw(self.set));
            drop(Box::from_raw(self.alert));
        }
    }
}

/// The keys the ACARS screen takes, and a few it doesn't.
const ACARS_KEYS: [ferrite_core::keys::Key; 10] = {
    use ferrite_core::keys::Key::*;
    [Up, Down, Left, Right, Menu, Exit, Star, Hash, Digit(5), Slot]
};

/// ACARS-4, ACARS-7: the screen with anything heard while it's open. Input: a byte of flags (the sample traffic, *,
/// a clock), then operations: a frame's raw bytes (a length byte, then those), a frame built from fuzzed fields
/// with a good CRC (so blocks join and copies drop), a key (held up to 2.5 s), the knob, time passing (seconds to
/// days), a draw, the radio listening or a signal, the list cleared, the clock, the sample feed. A draw after each
/// operation; at the end every message's page.
pub fn acars_screen(data: &[u8]) {
    use ferrite_core::acars::{encode, Fields};
    use ferrite_core::msg::ModeLive;
    let Some((&flags, mut d)) = data.split_first() else { return };
    let mut u = AcarsUi::new(flags & 1 != 0, flags & 2 != 0);
    if flags & 4 != 0 {
        u.dev.clock_min = Some(19 * 60 + 6);
    }
    let take = |n: usize, d: &mut &[u8]| -> Vec<u8> {
        let k = n.min(d.len());
        let (a, b) = d.split_at(k);
        *d = b;
        a.to_vec()
    };
    let mut ops = 0;
    while let Some((&op, rest)) = d.split_first() {
        d = rest;
        ops += 1;
        if ops > 400 {
            break;
        }
        let arg = take(1, &mut d).first().copied().unwrap_or(0);
        match op % 11 {
            0 => {
                let f = take(arg as usize, &mut d);
                u.heard(&f);
            }
            1 => {
                // mode, reg 7, ack, label 2, block, msgno 4, flight 6: 22 bytes; `arg` the text's length and flags
                let h = take(22, &mut d);
                let mut h22 = [0u8; 22];
                h22[..h.len()].copy_from_slice(&h);
                let tl = (arg as usize & 0x7F) * 2;
                let text = take(tl, &mut d);
                let f = Fields {
                    mode: h22[0],
                    reg: &h22[1..8],
                    ack: h22[8],
                    label: [h22[9], h22[10]],
                    block: h22[11],
                    msgno: &h22[12..16],
                    flight: &h22[16..22],
                    text: &text,
                    more: arg & 0x80 != 0,
                };
                let mut b = [0u8; 600];
                let n = encode(&f, &mut b);
                u.heard(&b[..n.saturating_sub(1)]);
            }
            2 => u.key(ACARS_KEYS[arg as usize % ACARS_KEYS.len()], if arg >= 0xC0 { (arg as u32 - 0xC0) * 40 } else { 50 }),
            3 => u.knob(arg as i8),
            4 => u.wait(match arg >> 6 {
                0 => arg as u32 * 100,
                1 => (arg as u32 & 0x3F) * 10_000,
                2 => (arg as u32 & 0x3F) * 3_600_000,
                _ => u32::MAX - u.t % 1000, // the millisecond count's wrap
            }),
            5 => u.render(),
            6 => {
                u.live.mode = match arg % 3 {
                    0 => ModeLive::Off,
                    k => ModeLive::Acars { freq: u.app.as_deref_mut().unwrap().acars_freq(), rssi: -(arg as i16), open: k == 2 },
                }
            }
            7 if arg == 0x5A => u.log().clear(),
            8 => u.dev.clock_min = (arg & 1 != 0).then_some(arg as u16 * 7 % 1440),
            9 => {
                let now = u.t / 1000;
                let _ = ferrite_app::acars::sample::feed(u.log(), arg as usize % 12, arg % 9, now);
            }
            _ => {}
        }
        u.render();
    }
    u.every_page();
}

/// ACARS-11: any audio through the ACARS receiver (`ferrite_core::acarsrx`, as the DSP task runs it, in blocks),
/// then every frame it gives into the screen. Input: a byte picking the rate (the DSP task's 24 kHz; 12, 16 and
/// 48 kHz, the receiver's range, for recordings at their own rate) and the block size, then 16-bit samples. Each
/// frame fits the DSP task's queue slot ([`ferrite_core::acarsrx::FRAME_MAX`]) and parses. Returns the frames.
pub fn acars_audio(data: &[u8]) -> usize {
    use ferrite_core::acarsrx::{Demod, FRAME_MAX};
    let Some((&cfg, rest)) = data.split_first() else { return 0 };
    let rate = [24_000, 12_000, 16_000, 48_000][(cfg & 3) as usize];
    let block = 1 + (cfg >> 2) as usize * 16;
    let x: Vec<i16> = rest.as_chunks::<2>().0.iter().map(|&c| i16::from_le_bytes(c)).collect();
    let mut dm = Demod::new(rate);
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut got = |f: &[u8]| {
        assert!(f.len() <= FRAME_MAX, "a frame of {} bytes", f.len());
        assert!(ferrite_core::acars::parse(f).is_ok(), "a frame that doesn't parse: {f:02x?}");
        frames.push(f.to_vec());
    };
    for b in x.chunks(block) {
        dm.feed(b, &mut got);
    }
    // quiet after: a held copy goes out within HOLD_BITS
    dm.feed(&vec![0; (ferrite_core::acarsrx::HOLD_BITS as usize + 8) * rate as usize / 2400], &mut got);
    if frames.is_empty() {
        return 0;
    }
    let mut u = AcarsUi::new(false, cfg & 0x40 != 0);
    for f in &frames {
        u.heard(f);
        u.wait(3000);
        u.render();
    }
    u.every_page();
    frames.len()
}
