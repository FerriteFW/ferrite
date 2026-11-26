//! The decoders' side of the UI task (manual chapters 23 and 27; roadmap 6b), beside the APRS task:
//!
//! - **SAME:** tells the Radio task whether weather alerts are on (0 4 5 1, US and Canada only: WX-3); its BK4829
//!   receiver then listens on any slot tuned to a weather channel (DEC-19). A header it decodes comes back on
//!   `ALERTS`: the alert screen (WX-4) and an event log record (INBOX-6).
//! - **DTMF:** tells the Radio task whether the BK4829's DTMF detector runs (0 4 3, DEC-6); the digits come back
//!   in the live snapshot, for the slot's card.
//! - **POCSAG:** asks the Radio task for the pager on slot C's chip modem in Pager mode (DEC-23)
//!   (DEC-18: APRS's AFSK keeps the slot first); each page it decodes goes through the capcode filter into the
//!   event log, the inbox and, with popups on, a toast (DEC-4, DEC-7).

use super::dsp::PAGES;
use crate::chan::RADIO;
use crate::store::Store;
use ferrite_app::data::{Event, Src};
use ferrite_app::App;
use ferrite_core::msg::{LiveAll, RadioCmd, SameDecoded};
use ferrite_ui::text::Buf;

pub struct Decode {
    /// What the Radio task and the DSP task were last told (None: not yet).
    same: Option<bool>,
    pager: Option<u16>,
    dtmf: Option<bool>,
}

impl Decode {
    pub const fn new() -> Self {
        Decode { same: None, pager: None, dtmf: None }
    }

    /// One turn of the UI task's loop.
    pub fn poll(&mut self, app: &mut App, store: &Store, live: &LiveAll) {
        let same = app.same_wanted(); // PWR-24: off in Low Power Mode
        if self.same != Some(same) && RADIO.try_send(RadioCmd::Same(same)).is_ok() {
            self.same = Some(same);
        }
        let dtmf = app.dtmf_wanted(); // DEC-6
        if self.dtmf != Some(dtmf) && RADIO.try_send(RadioCmd::DtmfRx(dtmf)).is_ok() {
            self.dtmf = Some(dtmf);
        }
        // DEC-23: the pager is the Radio task's now, on slot C's chip modem, at 0 4 2 5's speed
        let pager = if app.pager_wanted(live) { app.pager_baud() } else { 0 };
        if self.pager != Some(pager) && RADIO.try_send(RadioCmd::Pager(pager)).is_ok() {
            self.pager = Some(pager);
        }
        while let Ok(p) = PAGES.try_receive() {
            if let Some(e) = app.pager_hit(&p, live) {
                store.log(&e);
            }
        }
    }

    /// The waiting SAME header (ALERTS), handled in this frame, not the UI task's (SBAR-23).
    #[inline(never)]
    pub fn take_alert(app: &mut App, store: &Store) {
        if let Ok(a) = crate::chan::ALERTS.try_receive() {
            Self::alert(app, store, &a);
        }
    }

    /// A SAME header from the Radio task (ALERTS): the alert, and its record in the event log.
    pub fn alert(app: &mut App, store: &Store, a: &SameDecoded) {
        app.same_alert(&a.header, a.slot, a.freq);
        if app.settings().same_on() {
            let name = ferrite_core::same::event_name(&a.header.event).unwrap_or("Alert");
            store.log(&Event { src: Src::Same, ago_s: 0, freq: a.freq, slot: a.slot, unread: true, id: 0, x: 0, y: 0, pos: None, name: Buf::from(name.as_bytes()), text: Buf::from(&a.header.event[..]) });
        }
    }
}
