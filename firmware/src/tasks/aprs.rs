//! The APRS task (manual chapter 25, roadmap 5b-5d), run by the UI task beside the App and the store it feeds,
//! as the simulators run theirs: frames from the DSP task's decoder go to [`AprsLive`] (the heard list, messages
//! to us into the inbox and the log, acks), and once a second it may give a frame to send (an ack, a digipeat,
//! one of ours, a beacon), which goes out as a data transmission on slot C:
//!
//! 0. APRS-10, APRS-29 (decision 181): the frame waits for a clear moment on the APRS frequency (p-persistence on
//!    C's squelch, [`ClearWait`]) for up to 5 s, else it isn't sent;
//! 1. [`transmit::data_key`]: the automatic transmission's checks (TX-35, APRS-8, APRS-10), then the data key-up
//!    at the APRS power (0 5 6 3, default Low);
//! 2. once the Radio task says slot C is keyed, the frame to the DSP task ([`AFSK_TX`]), which plays it from the TX
//!    delay's flags (the key-up's ramp fits in them);
//! 3. when it's out ([`AFSK_DONE`]), the unkey. A key-up the Radio task refuses, or a frame that never finishes,
//!    unkeys after a timeout.
//!
//! The decoder runs while APRS mode is on (APRS-16), and without it while a slot on a channel flagged APRS is
//! the active one (APRS-22: decode only, nothing sent). With APRS mode off and automatic beacons on (APRS-29),
//! beacons alone go out, on C. Bluetooth KISS waits for roadmap 3d. The digipeater
//! follows 0 5 7 (off by default, APRS-12, APRS-26).

use super::dsp::{Frame, Heard, AFSK_DONE, AFSK_TX, APRS_RX};
use super::kissq;
use crate::chan::DSP;
use crate::store::Store;
use ferrite_app::aprs_live::uaprs::kiss;
use ferrite_app::aprs_live::{Clear, ClearWait, CLEAR_WAIT_MS, FRAME_MAX};
use ferrite_app::data::{Data, Event, MsgState, Src};
use ferrite_app::screens::{msgs, transmit};
use ferrite_app::App;
use ferrite_core::msg::{DecoderKind, DspCmd, LiveAll};
use ferrite_core::txplan;
use ferrite_ui::text::Buf;

/// APRS mode's slot (APRS-15).
const SLOT_C: u8 = 2;
/// The Radio task keys within this long, or it refused.
const KEY_WAIT_MS: u32 = 1_500;
/// The longest frame (330 bytes at 1200 Bd, about 2.2 s) after up to 2.55 s of flags, with margin.
const PLAY_WAIT_MS: u32 = 6_000;
/// KISS-2's TX delay default, 300 ms (the app may change it); the APRS task's own frames use it too.
const TXDELAY_10MS: u8 = 30;

enum Tx {
    Idle,
    /// [`Aprs::frame`] (a `what`) waits for a clear moment (APRS-10, APRS-29).
    Clearing { what: &'static str },
    /// Keyed for [`Aprs::frame`], waiting for the Radio task; whether it is a beacon (APRS-53's Sent page).
    Keying { at_ms: u32, beacon: bool },
    /// On the air. The deadline is [`Aprs::play_since`]: a KISS frame finishing moves it.
    Playing,
}

pub struct Aprs {
    on: bool,
    last_s: u32,
    tx: Tx,
    /// The frame being sent.
    frame: Frame,
    /// The KISS TNC (chapter 26): the cable's bytes in, the app's settings, and channel access. Frames wait in
    /// [`kissq`], eight deep.
    kiss: kiss::Decoder,
    params: kiss::Params,
    /// The oldest queued frame has started its 5 s to find a clear channel (KISS-13).
    kiss_waiting: bool,
    kiss_at: u32,
    /// The key-up in progress is a KISS window, not one of our own frames.
    kiss_tx: bool,
    /// When the transmission on air last made progress: a KISS frame finishing extends it.
    play_since: u32,
    /// [`kissq::finished`] last seen, so each frame that finishes moves `play_since`.
    kiss_done_seen: u8,
    access: kiss::Access,
    clear: ClearWait,
    seed: u32,
}

impl Aprs {
    pub fn new() -> Self {
        let params = kiss::Params { tx_delay: TXDELAY_10MS, ..kiss::Params::default() };
        Aprs { on: false, last_s: u32::MAX, tx: Tx::Idle, frame: Frame { len: 0, txdelay_10ms: 0, fx25: 0, level: 1, b: [0; FRAME_MAX] }, kiss: kiss::Decoder::new(), params, kiss_waiting: false, kiss_at: 0, kiss_tx: false, play_since: 0, kiss_done_seen: 0, access: kiss::Access::default(), clear: ClearWait::default(), seed: 0x2545_F491 }
    }

    /// A byte from the KISS port, the cable or Bluetooth (KISS-6; TERM-2's route on the cable).
    pub fn kiss_in(&mut self, b: u8) {
        let Some(r) = self.kiss.push(b) else { return };
        if !r.for_single_port() {
            return; // KISS-11: one port, port 0
        }
        // KISS-11: the shortest AX.25 frame is two addresses and a control byte (15). KISS-12: only data acts;
        // settings change `params`, and hardware commands (PTT tests, a frequency set, a save) do nothing.
        // A full window drops the frame; the host retries it.
        match self.params.apply(&r) {
            kiss::Command::Data(d) if (15..=FRAME_MAX).contains(&d.len()) => {
                let mut f = Frame { len: d.len() as u16, txdelay_10ms: self.params.tx_delay, fx25: 0, level: 1, b: [0; FRAME_MAX] };
                f.b[..d.len()].copy_from_slice(d);
                kissq::push(f);
            }
            kiss::Command::Set => kissq::set_tail(self.params.closing_flags()),
            _ => {}
        }
    }

    /// ACKMODE: a frame whose audio has finished is echoed to the host, command 12, on the port it came in on.
    fn ack(&mut self, kiss_cable: bool) {
        if !self.params.ack_mode {
            while kissq::pop_done().is_some() {}
            return;
        }
        while let Some(f) = kissq::pop_done() {
            let out = if kiss_cable { &crate::chan::TERM_OUT } else { &crate::chan::BT_OUT };
            crate::chan::term_out_locked(|| {
                kiss::encode_cmd(0, kiss::ACKMODE, f.bytes(), |b| {
                    out.put(b);
                });
            });
            if kiss_cable {
                crate::hal::uart::tx_ring(crate::report::UART_CPS, &crate::chan::TERM_OUT);
            }
            crate::chan::TERM_WAKE.signal(());
        }
    }

    /// A random byte for p-persistence (xorshift).
    fn random(&mut self) -> u8 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed as u8
    }

    /// PWR-26.15: something waits for this turn now (a frame heard, a KISS frame, one being sent), not only on the
    /// once-a-second turn.
    pub fn due(&self) -> bool {
        self.busy() || kissq::pending() || !APRS_RX.is_empty()
    }

    /// A frame is being keyed or played: the UI task turns faster meanwhile.
    pub fn busy(&self) -> bool {
        !matches!(self.tx, Tx::Idle)
    }

    /// One turn of the UI task's loop.
    #[inline(never)] // its frames and buffers in its own stack frame, not the UI task's (SBAR-23)
    pub fn poll(&mut self, app: &mut App, store: &Store, live: &LiveAll, dev: &ferrite_core::msg::Device, now_ms: u32) {
        let extra = &store.extra;
        extra.aprs.readings(dev.battery_mv, dev.mcu_temp, app.aprs_freq()); // APRS-54: the values in our position reports
        let me = app.aprs_call(); // APRS-30: callsign and SSID
        extra.set_callsign(me.as_bytes());
        // KISS-6: the cable (0 9 2 3 on KISS, or 0 3 7 on Cable) or Bluetooth (0 3 7, with Bluetooth on).
        let kiss_cable = app.settings().kiss_on_cable();
        let kiss_on = app.kiss_on(); // PWR-24: none in Low Power Mode. KISS-9: C is the APRS slot meanwhile (`App::aprs_c`)
        let heard_at = if kiss_on { app.kiss_freq() } else { app.aprs_freq() };
        let on = app.aprs_on();
        let send = app.aprs_sending();
        let decode = on || kiss_on || app.aprs_flag_decode(); // APRS-22
        let n = kissq::take_extra();
        for _ in 0..n {
            extra.kiss_sent(); // a continuation frame of the window, once its audio starts
        }
        // KISS-13: a client's frame doesn't outlive KISS (nor wait in Low Power Mode, PWR-24). One already on
        // air finishes; the ones still queued do not.
        if kiss_on {
            self.ack(kiss_cable);
        } else if matches!(self.tx, Tx::Idle) {
            kissq::clear();
            self.kiss_waiting = false;
        } else {
            kissq::drop_queued();
        }
        if decode != self.on && DSP.try_send(DspCmd::Decoder(if decode { DecoderKind::Afsk } else { DecoderKind::Off })).is_ok() {
            self.on = decode;
        }
        let now_s = now_ms / 1000;
        // Frames heard (APRS-3, APRS-6), and FX.25 blocks corrected here (APRS-44: 3 to 13 ms of this task's
        // time, not the DSP task's).
        while let Ok(h) = APRS_RX.try_receive() {
            let mut block;
            let (f, fx) = match h {
                Heard::Frame(ref f) => (f.bytes(), None),
                // APRS-56: heard, not decoded; logged only with 0 5 8 8 on, read (no badge: a diagnosis, not news)
                Heard::Miss(m, peak) => {
                    if app.settings().decode_log && ferrite_app::aprs_live::miss_logged(&m) {
                        let mut e = log_event(heard_at, b"", b"");
                        ferrite_app::aprs_live::miss_text(&m, peak, &mut e.name, &mut e.text);
                        e.unread = false;
                        store.log(&e);
                    }
                    continue;
                }
                Heard::Fx25(b) => {
                    block = b;
                    let Some(d) = block.decode() else {
                        super::dsp::FX25_FAILED.fetch_add(1, core::sync::atomic::Ordering::Relaxed); // APRS-56
                        if app.settings().decode_log {
                            let mut e = log_event(heard_at, b"?", b"Not decoded: FX.25 block, too many errors");
                            e.unread = false;
                            store.log(&e);
                        }
                        continue;
                    };
                    if d.duplicate {
                        extra.aprs.note_fx25(d.frame, d.corrected); // heard plain already
                        continue;
                    }
                    (d.frame, Some(d.corrected))
                }
            };
            if kiss_on {
                extra.kiss_heard(); // APRS-17.2: the KISS card's count
                let out = if kiss_cable { &crate::chan::TERM_OUT } else { &crate::chan::BT_OUT };
                // decision 290: held against the DSP task's audio frames, as every thread-mode writer of TERM_OUT
                crate::chan::term_out_locked(|| {
                    kiss::encode(0, f, |b| {
                        out.put(b); // KISS-4; a full ring drops the rest of the frame
                    })
                });
                if kiss_cable {
                    crate::hal::uart::tx_ring(crate::report::UART_CPS, &crate::chan::TERM_OUT);
                }
                crate::chan::TERM_WAKE.signal(());
            }
            let rx = extra.aprs.receive(f, now_s, me.as_bytes(), &extra.cfg());
            if let Some(n) = fx {
                extra.aprs.note_fx25(f, n);
            }
            // APRS-4: the station's row, when its rule says one is due (the front of the heard list); through the
            // store, so details RAM no longer holds come from its last row (APRS-4.5).
            if let Some(s) = ferrite_app::data::Data::station(store, 0).filter(|_| rx.row) {
                store.heard_row(&s);
            }
            // APRS-32: a retry heard again after a power cycle is in the store already (acked all the same).
            if let Some((from, text)) = rx.inbound.filter(|(f, t)| !store.message_heard(f.as_bytes(), rx.inbound_id.as_bytes(), t.as_bytes())) {
                app.notify_message(from.as_bytes(), text.as_bytes()); // INBOX-3.1
                store.message_to_us(from.as_bytes(), text.as_bytes(), rx.inbound_id.as_bytes());
                store.log(&log_event(heard_at, from.as_bytes(), text.as_bytes())); // INBOX-6
            }
            if let Some((peer, text, st)) = rx.state {
                store.message_state(peer.as_bytes(), text.as_bytes(), st);
                if st == MsgState::Rejected {
                    msgs::rejected(app, peer.as_bytes()); // APRS-34
                }
            }
            if let Some(by) = rx.echo {
                msgs::digipeated(app, by.as_bytes()); // APRS-36
                store.log(&log_event(heard_at, by.as_bytes(), b"digipeated our frame")); // APRS-36.1: findable later
            }
            if rx.changed || rx.state.is_some() {
                extra.changed();
            }
        }
        // APRS-57: the auto reply, once its delay is up: one of our messages, stored and tried as any (APRS-13);
        // none without room for it (APRS-13.3) or a callsign to send as (APRS-8).
        if let Some(to) = extra.aprs.reply_now(now_s) {
            let cfg = extra.cfg();
            if !me.is_empty() && extra.aprs.room() && store.message_in(false, to.as_bytes(), cfg.reply_text(), 0) {
                extra.aprs.send(to.as_bytes(), cfg.reply_text());
            }
        }
        match &self.tx {
            // APRS-10.1: APRS mode or beacons turned off (or Low Power Mode, or KISS) while it waited: dropped,
            // never keyed on C's own channel.
            &Tx::Clearing { what } if !send.may_send(what) => self.tx = Tx::Idle,
            &Tx::Clearing { what } => {
                let busy = live.slots[SLOT_C as usize].carrier; // SQL-13.1
                let r = self.random();
                match self.clear.poll(now_ms, busy, r) {
                    Clear::Wait => {}
                    Clear::Send => {
                        self.kiss_tx = false;
                        self.tx = if transmit::data_key(app, SLOT_C, extra.cfg().power, what.as_bytes()) { Tx::Keying { at_ms: now_ms, beacon: what == "Beacon" } } else { Tx::Idle };
                    }
                    Clear::GiveUp => {
                        let w = what.as_bytes();
                        transmit::not_sent(app, w, b"channel busy");
                        store.log(&log_event(app.aprs_freq(), w, b"not sent: channel busy"));
                        self.tx = Tx::Idle;
                    }
                }
            }
            &Tx::Keying { at_ms, beacon } => {
                if live.tx.keyed && live.tx.slot == SLOT_C {
                    if self.kiss_tx {
                        kissq::arm_burst(); // the DSP task sends whatever is queued behind this frame before it unkeys
                    }
                    AFSK_TX.signal(self.frame);
                    if beacon {
                        extra.aprs.beacon_sent(now_s); // APRS-53: on the Sent page
                        extra.changed();
                    }
                    self.play_since = now_ms;
                    self.kiss_done_seen = kissq::finished();
                    self.tx = Tx::Playing;
                } else if now_ms.wrapping_sub(at_ms) > KEY_WAIT_MS {
                    transmit::data_unkey(app, SLOT_C); // refused by the Radio task (its toast says why)
                    if self.kiss_tx {
                        kissq::abandon_on_air();
                        self.kiss_tx = false;
                    }
                    self.tx = Tx::Idle;
                }
            }
            Tx::Playing => {
                let n = kissq::finished();
                if n != self.kiss_done_seen {
                    self.kiss_done_seen = n; // that frame is out: the next one has its own 6 s
                    self.play_since = now_ms;
                }
                if AFSK_DONE.try_take().is_some() || now_ms.wrapping_sub(self.play_since) > PLAY_WAIT_MS {
                    transmit::data_unkey(app, SLOT_C);
                    self.kiss_tx = false;
                    self.tx = Tx::Idle;
                }
            }
            // KISS-2: the host's frame, exempt from the 30 s spacing, once the channel is clear (p-persistence,
            // once per key-up). KISS-13: a frame the channel stays busy for gives up after APRS-10's 5 s (the
            // client resends as it sees fit), and only that frame, so the one behind it gets its own wait.
            Tx::Idle if kiss_on && kissq::has_queued() => {
                if !self.kiss_waiting {
                    self.kiss_waiting = true;
                    self.kiss_at = now_ms;
                }
                if now_ms.wrapping_sub(self.kiss_at) >= CLEAR_WAIT_MS {
                    kissq::drop_oldest_queued();
                    self.kiss_waiting = false;
                } else {
                    let busy = live.slots[SLOT_C as usize].carrier; // SQL-13.1
                    let r = self.random();
                    if self.access.may_send(&self.params, now_ms, busy, r) {
                        if let Some(f) = kissq::take_for_air() {
                            self.frame = f;
                            self.kiss_waiting = false;
                            if transmit::data_key(app, SLOT_C, extra.cfg().power, b"KISS") {
                                extra.kiss_sent(); // APRS-17.2: the first frame of the key-up; the rest as they start
                                kissq::set_tail(self.params.closing_flags());
                                self.kiss_tx = true;
                                self.tx = Tx::Keying { at_ms: now_ms, beacon: false };
                            } else {
                                kissq::abandon_on_air();
                            }
                        }
                    }
                }
            }
            Tx::Idle if now_s != self.last_s => {
                self.last_s = now_s;
                let relay_ok = txplan::relay_ok(app.tuned_freq(app.active()), app.aprs_freq());
                let cfg = extra.cfg();
                let mut out = [0u8; FRAME_MAX];
                let t = extra.aprs.tick(now_s, send, relay_ok, me.as_bytes(), app.beacon_fix(extra.fix()), &cfg, &mut out);
                for (peer, text, st) in t.states.into_iter().flatten() {
                    store.message_state(peer.as_bytes(), text.as_bytes(), st);
                }
                if t.changed {
                    extra.changed();
                }
                if t.len > 0 {
                    // APRS-45: our own frames as FX.25 when 0 5 8 5 says so (a digipeat goes out plain).
                    // APRS-48, APRS-49: 0 5 8 3's delay and 0 5 8 4's level for our own frames (KISS's frames keep the host's)
                    self.frame = Frame { len: t.len as u16, txdelay_10ms: cfg.txdelay_10ms(), fx25: cfg.fx25_for(t.what), level: cfg.tx_level.min(2), b: [0; FRAME_MAX] };
                    self.frame.b[..t.len].copy_from_slice(&out[..t.len]);
                    self.clear.start(now_ms);
                    self.tx = Tx::Clearing { what: t.what };
                } else if let Some(why) = match t.what {
                    "Beacon: no position" => Some(&b"no position"[..]),
                    "Beacon: old fix" => Some(&b"fix too old"[..]),
                    _ => None,
                } {
                    // GPS-33: a beacon not sent is said and logged (one waiting for a fresh fix says nothing yet).
                    transmit::not_sent(app, b"Beacon", why);
                    store.log(&log_event(app.aprs_freq(), b"Beacon not sent", why));
                }
            }
            Tx::Idle => {}
        }
    }
}

fn log_event(freq: ferrite_core::freq::Freq10, name: &[u8], text: &[u8]) -> Event {
    Event { src: Src::Aprs, ago_s: 0, freq, slot: SLOT_C, unread: true, id: 0, x: 0, y: 0, pos: None, name: Buf::from(name), text: Buf::from(text) }
}
