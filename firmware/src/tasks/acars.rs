//! ACARS's side of the UI task (chapter 27b, ACARS-11), beside the SSTV and decode tasks: the DSP task's decoder slot
//! is ACARS's while the Radio task listens for it ([`ferrite_app::App::acars_decoding`], [`DspCmd::Acars`]); each
//! frame it decodes comes back on [`ACARS_RX`] and goes into the list.

use super::dsp::ACARS_RX;
use crate::chan::DSP;
use crate::store::Store;
use ferrite_app::App;
use ferrite_core::msg::{DspCmd, LiveAll};

pub struct Acars {
    /// What the DSP task was last told (None: not yet).
    on: Option<bool>,
}

impl Acars {
    pub const fn new() -> Self {
        Acars { on: None }
    }

    /// Whether the next turn has anything to do.
    pub fn due(&self, app: &App, live: &LiveAll) -> bool {
        self.on != Some(app.acars_decoding(live)) || self.on == Some(true) || !ACARS_RX.is_empty()
    }

    /// One turn of the UI task's loop.
    pub fn poll(&mut self, app: &App, store: &Store, live: &LiveAll) {
        let want = app.acars_decoding(live);
        if self.on != Some(want) && DSP.try_send(DspCmd::Acars(want)).is_ok() {
            self.on = Some(want);
        }
        while let Ok(f) = ACARS_RX.try_receive() {
            app.acars_heard(store, &f.b[..f.len as usize]);
        }
        app.acars_tick(store);
    }
}
