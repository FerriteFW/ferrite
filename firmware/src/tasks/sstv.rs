//! SSTV's side of the UI task (chapter 27a), beside the APRS and decode tasks:
//!
//! - **Receive:** while the SSTV screen is open the DSP task's decoder slot is SSTV's ([`DspCmd::Sstv`]); a mode
//!   started by hand goes as [`DspCmd::SstvForce`]. Each line it decodes comes back through `SSTV_EV` (the row in
//!   `SSTV_ROWS`) and goes to the App, which writes it to the picture's slot and draws it.
//! - **Send:** the App's picture to send keys the active slot as a data transmission ([`transmit::sstv_key`]);
//!   once the Radio task says it's keyed, the picture goes to the DSP task ([`SSTV_TX`]), and when it's out
//!   ([`SSTV_DONE`]) the slot unkeys. BACK stops it ([`SSTV_STOP`]).

use super::dsp::{SSTV_DONE, SSTV_EV, SSTV_FRAME, SSTV_ROWS, SSTV_STOP, SSTV_TX};
use crate::chan::DSP;
use crate::store::Store;
use core::sync::atomic::Ordering::Relaxed;
use ferrite_app::screens::{transmit, Screen};
use ferrite_app::App;
use ferrite_core::msg::{DspCmd, LiveAll};

/// How long a key-up may take before it counts as refused.
const KEY_WAIT_MS: u32 = 3_000;
/// The longest picture (Martin M1, 115 s) and some: a DSP task that never answers unkeys anyway.
const PLAY_WAIT_MS: u32 = 140_000;
/// SSTV-5: how long a send waits for the store's queued work (a slot's erase as the screen opens, about 2 s) before
/// keying, so the picture's rows can be read as it goes; after this it goes anyway.
const STORE_WAIT_MS: u32 = 5_000;

enum Tx {
    Idle,
    Keying { at_ms: u32, send: ferrite_core::sstv::Send, slot: u8 },
    Playing { at_ms: u32, slot: u8 },
}

pub struct Sstv {
    /// What the DSP task was last told (None: not yet).
    on: Option<bool>,
    tx: Tx,
    /// When a send started waiting for the store (SSTV-5), if it is.
    wait_ms: Option<u32>,
}

impl Sstv {
    pub const fn new() -> Self {
        Sstv { on: None, tx: Tx::Idle, wait_ms: None }
    }

    /// Whether the next turn has anything to do (the screen open, a picture going out, news waiting).
    pub fn due(&self, app: &App) -> bool {
        app.sstv.open || !matches!(self.tx, Tx::Idle) || app.sstv.send.is_some() || !SSTV_EV.is_empty()
    }

    /// One turn of the UI task's loop.
    pub fn poll(&mut self, app: &mut App, store: &Store, live: &LiveAll, now_ms: u32) {
        // SSTV-1: the screen gone (BACK, or something took its place) and nothing going out: the decoders back
        if app.sstv.open && !matches!(app.screen(), Screen::Sstv(_)) && matches!(self.tx, Tx::Idle) && app.sstv.send.is_none() {
            app.sstv_close(store);
        }
        app.sstv_prepare(store); // SSTV-1: the slots read once the screen opened
        let want = app.sstv.open;
        if self.on != Some(want) && DSP.try_send(DspCmd::Sstv(want)).is_ok() {
            self.on = Some(want);
        }
        if let Some(v) = app.sstv.force {
            if DSP.try_send(DspCmd::SstvForce(v)).is_ok() {
                app.sstv.force = None;
            }
        }
        while let Ok(ev) = SSTV_EV.try_receive() {
            let row = match ev {
                ferrite_core::sstv::Event::Line(_, i) => SSTV_ROWS.lock(|r| r.borrow()[i as usize % 4]),
                _ => [0; ferrite_core::sstv::WIDTH],
            };
            app.sstv_event(ev, &row, store);
        }
        match self.tx {
            Tx::Idle => {
                if let Some(send) = app.sstv.send {
                    // the store busy (an erase): its reads fail meanwhile and the rows would go out grey
                    let since = *self.wait_ms.get_or_insert(now_ms);
                    if store.has_jobs() && !app.sstv.stop && now_ms.wrapping_sub(since) < STORE_WAIT_MS {
                        return;
                    }
                    self.wait_ms = None;
                    let slot = app.active();
                    let power = app.slot(slot).cfg().power;
                    if app.sstv.stop || !transmit::sstv_key(app, slot, power) {
                        app.sstv.send = None;
                    } else {
                        self.tx = Tx::Keying { at_ms: now_ms, send, slot };
                    }
                }
            }
            Tx::Keying { at_ms, send, slot } => {
                if live.tx.keyed && live.tx.slot == slot {
                    SSTV_DONE.reset(); // an earlier picture's late end isn't this one's
                    SSTV_TX.signal(send);
                    app.sstv.send = None;
                    app.sstv_sending(&send);
                    self.tx = Tx::Playing { at_ms: now_ms, slot };
                } else if app.sstv.stop || now_ms.wrapping_sub(at_ms) > KEY_WAIT_MS {
                    transmit::data_unkey(app, slot); // refused by the Radio task (its toast says why)
                    app.sstv.send = None;
                    self.tx = Tx::Idle;
                }
            }
            Tx::Playing { at_ms, slot } => {
                if app.sstv.stop {
                    SSTV_STOP.store(true, Relaxed);
                }
                app.sstv.frame = SSTV_FRAME.load(Relaxed) as u16;
                // SSTV-5.4: the timeout or the battery unkeyed it: the picture stops too
                if !live.tx.keyed {
                    SSTV_STOP.store(true, Relaxed);
                }
                let done = SSTV_DONE.try_take();
                if done.is_none() && now_ms.wrapping_sub(at_ms) > PLAY_WAIT_MS {
                    SSTV_STOP.store(true, Relaxed); // the DSP task stops playing too, not only the key
                }
                if done.is_some() || now_ms.wrapping_sub(at_ms) > PLAY_WAIT_MS {
                    transmit::data_unkey(app, slot);
                    app.sstv_sent(done.unwrap_or(false));
                    app.sstv.stop = false;
                    self.tx = Tx::Idle;
                }
            }
        }
    }
}
