//! The two remote ports, the programming cable and Bluetooth (manual chapters 26, 29 and 37, roadmap 3d), and
//! what runs on them: the terminal ([`Term`]), the link protocol ([`Session`]) and the KISS TNC. The firmware
//! and the simulators feed both ports' bytes through here, so both run the same rules.
//!
//! - **A link session on each port, at the same time** (LINK-28, decision 152): the cable's and Bluetooth's sessions
//!   are independent (their frames, `HELLO`, PIN, running `CMD`, write session). What they share is made safe here:
//!   writes are serialised (a `WRITE`, `DELETE` or `BEGIN` from one port while the other is mid-write answers
//!   `busy`), remote PTT has one keyer at a time (the other port's key-up and keepalives are refused; either may
//!   send `KEY` off), and the firmware's debug requests go one at a time.
//! - **One terminal**, held by one port at a time (RAM: a second VT100 screen state isn't worth it; the link's `CMD`
//!   runs the same command lines on either port). The cable always wins: physical access (LINK-7), and the way
//!   back if a phone left it open. Bluetooth takes it when the cable's terminal is closed; while the cable has
//!   it, Bluetooth's terminal bytes are dropped. A link session never takes or blocks the terminal.
//! - **KISS** needs neither, so it runs on its port alongside the other one (KISS-6): the cable by 0 9 2 3 on
//!   KISS or 0 3 7 on Cable, Bluetooth by 0 3 7 on Bluetooth. Its bytes go back to the caller.
//! - **Trust:** the cable is trusted (TERM-3, LINK-7). On Bluetooth, while the radio's PIN lock is on, the
//!   terminal and the link each ask for the PIN first (LINK-30); `BAUD` takes only its two rates (LINK-15.1, LINK-25).
//! - **A Bluetooth connection** ends when it has been quiet for [`BT_IDLE_MS`]: the module tells the radio
//!   nothing about connects (it is a transparent bridge, LINK-25), so silence is the only sign the phone left.
//!   Its terminal closes, its link session starts over, and the PIN it gave ends with it.

use crate::chan::ChannelStore;
use crate::link::{self, Route, Session};
use crate::term::{Sink, Term};
use crate::App;
use ferrite_core::msg::{DebugAns, DebugReq, Device, LiveAll};

/// Bluetooth's only rate (LINK-25, decision 152): the module's resting 9600.
pub use ferrite_core::link::BT_BAUD;
/// A Bluetooth connection is taken as over after this long with no byte from it.
pub const BT_IDLE_MS: u32 = 120_000;
/// A debug request waits this long for the firmware's answer before another may go (the asking command gives up
/// after [`crate::term::debug::ANSWER_MS`]).
const DEBUG_WAIT_MS: u32 = 2_500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Port {
    Cable,
    Bt,
}

impl Port {
    /// The port's index in [`Ports::links`]: 0 the cable, 1 Bluetooth.
    pub const fn i(self) -> usize {
        match self {
            Port::Cable => 0,
            Port::Bt => 1,
        }
    }
}

/// Where a byte from a port goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum To {
    Term,
    /// That port's link session.
    Link(Port),
    /// The KISS TNC has this port (KISS-6): the caller's KISS decoder.
    Kiss,
    /// The cable holds the terminal: a Bluetooth terminal byte, dropped.
    Drop,
}

/// Who asked the debug request in flight (TERM-21, LINK-34).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asker {
    Term,
    Link(usize),
}

pub struct Ports {
    pub term: Term,
    /// One link session per port (LINK-28): the cable's (trusted) and Bluetooth's.
    pub links: [Session; 2],
    /// Which port holds the terminal now.
    owner: Port,
    /// When Bluetooth last sent a byte, and whether it has since the connection started.
    bt_last: u32,
    bt_seen: bool,
    /// PWR-26.2: when the cable last sent a byte (None: never).
    cable_last: Option<u32>,
    /// Bluetooth terminal bytes dropped while the cable held the terminal.
    pub dropped: u32,
    /// The debug request in flight: who asked, and when.
    dbg: Option<(Asker, u32)>,
    now: u32,
    /// Live View (LINK-47): what the PC has of the UI, for the one session that has it (the newest subscription;
    /// one at a time, 1.4 KB).
    ui: ferrite_core::liveview::Track,
    ui_owner: Option<usize>,
    /// LINK-51: the one compression block, for the first session that asks (2.6 KB of windows, 0.5 KB of frames).
    z: link::z::Zip,
    /// LINK-13.4: when the last receive-audio frame (a block or a keep-alive) went, and blocks not sent for want of
    /// room in the cable's send ring.
    audio_at: u32,
    audio_noroom: u32,
}

/// LINK-13.4: a keep-alive this often while nothing is heard (the PC knows the stream is still there).
pub const AUDIO_KEEPALIVE_MS: u32 = 1_000;

impl Default for Ports {
    fn default() -> Self {
        Self::new()
    }
}

impl Ports {
    pub const fn new() -> Self {
        let mut bt = Session::new(false);
        bt.set_max_baud(0); // LINK-15.1: Bluetooth's `BAUD` takes only the module's two rates
        Ports { term: Term::new(), links: [Session::new(true), bt], owner: Port::Cable, bt_last: 0, bt_seen: false, cable_last: None, dropped: 0, dbg: None, now: 0, ui: ferrite_core::liveview::Track::new(), ui_owner: None, z: link::z::Zip::new(), audio_at: 0, audio_noroom: 0 }
    }

    /// LINK-13.4: blocks not sent for want of room in the cable's send ring (the bench's readback).
    pub fn audio_noroom(&self) -> u32 {
        self.audio_noroom
    }

    /// LINK-13.4: the cable's session asked for receive audio (the caller's tick is short enough to take its blocks).
    pub fn listening(&self) -> bool {
        self.links[0].listening()
    }

    /// LINK-13.4, LINK-13.5: what the cable's session listens to (the `AUDIO` mode: 0 nothing; asked for, and still
    /// active): the caller runs the capture and hands its µ-law blocks to [`Ports::audio`] only
    /// while this is 1.
    pub fn audio_wanted(&mut self, now: u32) -> u8 {
        self.links[0].audio_on(now)
    }

    /// LINK-13.4: one block of receive audio for the cable's session: sent while a slot's squelch is open
    /// (with the slot heard), else only a keep-alive each [`AUDIO_KEEPALIVE_MS`]; nothing unless it listens.
    pub fn audio(&mut self, blk: &ferrite_core::rxaudio::Block, live: &LiveAll, now: u32, cable: &mut impl Sink) {
        use ferrite_core::rxaudio::{NO_SLOT, SQ_OPEN};
        if self.links[0].audio_on(now) != 1 {
            return; // LINK-13.9: the raw stream is the DSP task's alone
        }
        match live.slots.iter().position(|s| s.squelch_open) {
            Some(slot) => {
                if !self.links[0].audio_out(slot as u8, SQ_OPEN, Some(blk), cable) {
                    self.audio_noroom += 1;
                }
                self.audio_at = now;
            }
            None if now.wrapping_sub(self.audio_at) >= AUDIO_KEEPALIVE_MS => {
                self.links[0].audio_out(NO_SLOT, 0, None, cable);
                self.audio_at = now;
            }
            None => {}
        }
    }

    /// Bluetooth runs at its unlocked rate now (LINK-25.1): the Live View's rate cap follows (LINK-19.5).
    pub fn set_bt_fast(&mut self, fast: bool) {
        self.links[1].fast = fast;
    }

    /// The fastest cable rate the caller's UART runs within 2 % (LINK-11).
    pub fn set_cable_max_baud(&mut self, b: u32) {
        self.links[0].set_max_baud(b);
    }

    /// Which port holds the terminal now.
    pub fn owner(&self) -> Port {
        self.owner
    }

    /// `port` takes another byte now (TERM-4, TERM-13: a whole line or frame waits for the store).
    pub fn ready_for(&self, port: Port) -> bool {
        self.links[port.i()].ready() && (self.owner != port || self.term.ready())
    }

    /// Both ports take another byte now.
    pub fn ready(&self) -> bool {
        self.term.ready() && self.links.iter().all(|l| l.ready())
    }

    /// The terminal or a session has work waiting: poll again soon.
    pub fn busy(&self) -> bool {
        self.term.busy() || !self.ready() || self.links.iter().any(|l| l.events_waiting())
    }

    /// The terminal moves to `port`: a fresh one (the other port's closes).
    fn switch(&mut self, port: Port) {
        self.owner = port;
        self.term = Term::new();
        self.term.set_bluetooth(port == Port::Bt);
    }

    /// Where byte `b` from `port` goes (TERM-2 on the cable; auto on Bluetooth). A terminal byte may move the
    /// terminal to `port`: always for the cable, for Bluetooth when the cable's terminal is closed. Link frames go
    /// to the port's own session (LINK-28).
    pub fn route(&mut self, app: &App, port: Port, b: u8, now: u32) -> To {
        if port == Port::Bt {
            (self.bt_last, self.bt_seen) = (now, true);
        } else {
            self.cable_last = Some(now);
        }
        let kiss = app.kiss_on()
            && match port {
                Port::Cable => app.set.kiss_on_cable(),
                Port::Bt => app.set.kiss_on_bt(),
            };
        if kiss {
            return To::Kiss;
        }
        let mode = if port == Port::Cable { app.set.cable_port } else { 0 };
        match link::route(mode, self.links[port.i()].active(now), b) {
            Route::Link => To::Link(port),
            Route::Term => {
                if port != self.owner {
                    if port == Port::Bt && self.term.on() {
                        self.dropped = self.dropped.wrapping_add(1);
                        return To::Drop;
                    }
                    self.switch(port);
                }
                To::Term
            }
        }
    }

    /// A routed byte to the terminal or a link session (call the App's `pop_write` after each: a channel edit
    /// writes one channel per key).
    pub fn feed<S: ChannelStore>(&mut self, app: &mut App, store: &S, dev: &Device, live: &LiveAll, to: To, b: u8, now: u32) {
        match to {
            To::Term => self.term.feed(app, b, now, store, dev, live),
            To::Link(p) if self.z.owner == Some(p.i()) => {
                let s = &mut self.links[p.i()];
                self.z.take(b, &mut |c| s.feed(c, now)); // LINK-51: decoded, then the session's as ever
            }
            To::Link(p) => self.links[p.i()].feed(b, now),
            To::Kiss | To::Drop => {}
        }
    }

    /// The terminal redrawn into its port's output and each session's waiting frame answered into its own. Ends a
    /// Bluetooth connection gone quiet; a Bluetooth terminal asks for the PIN while the PIN lock is on (LINK-30).
    #[allow(clippy::too_many_arguments)]
    pub fn poll<S: ChannelStore>(&mut self, app: &mut App, store: &S, dev: &Device, live: &LiveAll, now: u32, cable: &mut impl Sink, bt: &mut impl Sink) {
        self.now = now;
        if self.bt_seen && (now.wrapping_sub(self.bt_last) >= BT_IDLE_MS || !app.set.bt) {
            self.bt_seen = false;
            if self.owner == Port::Bt {
                self.switch(Port::Cable);
            }
            self.links[1].set_port(false, 0); // LINK-29: the next connection starts over (its PIN too)
        }
        self.term.set_pin_needed(self.owner == Port::Bt && app.pin_required());
        match self.owner {
            Port::Cable => self.term.poll(app, store, dev, live, now, cable),
            Port::Bt => self.term.poll(app, store, dev, live, now, bt),
        }
        // LINK-28: writes one port at a time; each session hears whether the other is mid-write.
        let writing = [self.links[0].writing(), self.links[1].writing()];
        self.links[0].set_other_writing(writing[1]);
        self.links[1].set_other_writing(writing[0]);
        // LINK-51: the block's owner gives it back when its session ends or goes quiet; it is free for whoever asks.
        if let Some(o) = self.z.owner.filter(|&o| !self.links[o].lz || !self.links[o].active(now)) {
            self.links[o].lz = false;
            self.z.owner = None;
            self.z.reset();
        }
        for i in 0..2 {
            self.links[i].lz_free = self.z.owner.is_none_or(|o| o == i);
        }
        let z = &mut self.z;
        if z.owner == Some(0) {
            self.links[0].poll(app, store, dev, live, now, &mut link::z::ZOut { z, inner: cable });
        } else {
            self.links[0].poll(app, store, dev, live, now, cable);
        }
        let z = &mut self.z;
        if z.owner == Some(1) {
            self.links[1].poll(app, store, dev, live, now, &mut link::z::ZOut { z, inner: bt });
        } else {
            self.links[1].poll(app, store, dev, live, now, bt);
        }
        for i in 0..2 {
            if self.links[i].lz && self.z.owner.is_none() {
                self.z.owner = Some(i); // its HELLO answer has gone, plain: both ends start from here
                self.z.reset();
            }
        }
        // LINK-46, LINK-47: events for subscribed sessions; the Live View to the newest subscription's port.
        for i in 0..2 {
            if self.links[i].ui_starting() && self.ui_owner != Some(i) {
                if let Some(o) = self.ui_owner {
                    self.links[o].ui_drop();
                }
                self.ui_owner = Some(i);
            }
        }
        if self.ui_owner.is_some_and(|o| !self.links[o].ui_wanted()) {
            self.ui_owner = None;
        }
        let ui = &mut self.ui;
        let owner = self.ui_owner;
        let [c, b] = &mut self.links;
        let z = &mut self.z;
        let zo = z.owner;
        if zo == Some(0) {
            c.events(app, store, dev, live, now, if owner == Some(0) { Some(&mut *ui) } else { None }, &mut link::z::ZOut { z, inner: cable });
        } else {
            c.events(app, store, dev, live, now, if owner == Some(0) { Some(&mut *ui) } else { None }, cable);
        }
        let z = &mut self.z;
        if zo == Some(1) {
            b.events(app, store, dev, live, now, if owner == Some(1) { Some(ui) } else { None }, &mut link::z::ZOut { z, inner: bt });
        } else {
            b.events(app, store, dev, live, now, if owner == Some(1) { Some(ui) } else { None }, bt);
        }
    }

    /// A debug command's next request for the firmware (TERM-21), from the terminal or from a command run over
    /// either link session (LINK-34): one in flight at a time. Answer with [`Ports::debug_answer`].
    pub fn debug_req(&mut self) -> Option<DebugReq> {
        if self.dbg.is_some_and(|(_, at)| self.now.wrapping_sub(at) < DEBUG_WAIT_MS) {
            return None;
        }
        self.dbg = None;
        let (who, r) = if let Some(r) = self.term.debug_req() {
            (Asker::Term, r)
        } else if let Some(r) = self.links[0].debug_req() {
            (Asker::Link(0), r)
        } else {
            (Asker::Link(1), self.links[1].debug_req()?)
        };
        self.dbg = Some((who, self.now));
        Some(r)
    }

    /// The firmware's answer to the last [`Ports::debug_req`], to whichever asked.
    pub fn debug_answer(&mut self, a: DebugAns) {
        match self.dbg.take() {
            Some((Asker::Term, _)) => self.term.debug_answer(a),
            Some((Asker::Link(i), _)) => self.links[i].debug_answer(a),
            None => {}
        }
    }

    /// A rate agreed by the cable's `BAUD` for its UART, once (LINK-11).
    pub fn take_baud(&mut self) -> Option<u32> {
        self.links[0].take_baud()
    }

    /// A rate agreed by Bluetooth's `BAUD` (LINK-15.1: 9600 or 115200), once: USART1 switches after the answer has
    /// gone, and goes back to 9600 unless a `HELLO` arrives at the new rate within 3 s.
    pub fn take_bt_baud(&mut self) -> Option<u32> {
        self.links[1].take_baud()
    }

    /// Bluetooth's session's `HELLO`s (LINK-15.1's proof).
    pub fn bt_hellos(&self) -> u16 {
        self.links[1].hellos()
    }

    /// `REBOOT` accepted on either port, once (LINK-24).
    pub fn take_reboot(&mut self) -> bool {
        let c = self.links[0].take_reboot();
        self.links[1].take_reboot() || c
    }

    /// `STOCK_RESTORE` accepted on the cable, once, with its seq (BACKUP-7, LINK-8; Bluetooth's session refuses it).
    pub fn take_restore(&mut self) -> Option<u8> {
        self.links[0].take_restore()
    }

    /// The cable session's `HELLO`s (LINK-11: a new rate stays once one arrives at it).
    pub fn hellos(&self) -> u16 {
        self.links[0].hellos()
    }

    /// A write session's progress on either port (only one writes at a time), for the Link page (LINK-9).
    pub fn progress(&self) -> Option<u8> {
        self.links.iter().find_map(|l| l.progress())
    }

    /// The link protocol is running on `port` now (TERM-2: for its flow control and the Link status page).
    pub fn link_on(&self, port: Port, now: u32) -> bool {
        self.links[port.i()].active(now)
    }

    /// A Bluetooth device is connected, as far as the radio can tell: Bluetooth is on and the device has sent
    /// something within [`BT_IDLE_MS`] (the status bar's icon, SBAR).
    pub fn bt_connected(&self, app: &App, now: u32) -> bool {
        app.set.bt && self.bt_seen && now.wrapping_sub(self.bt_last) < BT_IDLE_MS
    }

    /// PWR-26.2: a live session, for the MCU clock: a link session active on either port (a frame within
    /// `link::ACTIVE_MS`), or a byte (terminal, link or KISS) from the cable, or from a connected Bluetooth device,
    /// within `clocktree::LINK_MS`.
    pub fn live(&self, app: &App, now: u32) -> bool {
        use ferrite_core::clocktree::LINK_MS;
        self.links.iter().any(|l| l.active(now))
            || self.cable_last.is_some_and(|t| now.wrapping_sub(t) < LINK_MS)
            || self.bt_connected(app, now) && now.wrapping_sub(self.bt_last) < LINK_MS
    }
}
