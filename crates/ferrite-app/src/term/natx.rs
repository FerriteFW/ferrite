//! The natives behind the pages that match the radio's own screens (TERM-48 to TERM-52; the gap list is
//! docs/research/vt100-parity.md): a slot changed as the keypad would, the active slot's quick settings (MENU), the
//! APRS lists and a message sent, the GPS position as the radio's page has it, device values, and the radio's
//! screen as text with its keys (the remote head). Each is a thin call into what the radio already has.

use super::tpl::*;
use super::{cmd_trim, Cx, Term, W};
use crate::chan::ChannelStore;
use crate::data::{Draft, MsgState, Op};
use crate::slot::QUICK;
use crate::slotctl::SlotChange;
use crate::App;
use ferrite_core::freq::Freq10;
use ferrite_ui::text::Buf;

/// Why slot `s` can't be changed from here now (the keypad's checks), as the program's refusal codes: 2
/// transmitting, 1 the APRS slot (APRS-19: the note says whose), 4 scanning; 0 it can.
fn slot_held(t: &mut Term, app: &App, s: u8) -> i32 {
    if app.radio.keyed.is_some() {
        2
    } else if app.aprs_slot(s) {
        t.no(app.aprs_holds().as_bytes())
    } else {
        (app.scan_mask & 1 << s != 0) as i32 * 4
    }
}

impl Term {
    /// A refusal the firmware words (the note says why, a warning); 1. The program words its own refusals from
    /// the codes the natives give back, so their text is in the SPI flash.
    fn no(&mut self, why: &[u8]) -> i32 {
        self.note.set(why);
        (self.warn, self.bnote) = (true, false);
        1
    }

    #[inline(never)]
    pub(super) fn more<S: ChannelStore>(&mut self, id: u8, x: [i32; 4], cx: &mut Cx<S>, w: &mut W) -> i32 {
        let (app, st, dev) = (&mut *cx.app, cx.store, cx.dev);
        let d = st.data();
        let a = x[0];
        match id {
            // the typed line as MHz (-1: not a frequency)
            N_LMHZ => return ferrite_core::freq::parse(cmd_trim(self.pg.line.as_bytes())).map_or(-1, |f| f as i32),
            // SIDE2 on home (SCAN-1): a scan on the active slot starts, or the one running stops
            N_SCAN => crate::screens::scan::side2(app, st),
            // slot a: 0 its VFO tuned to x[2], 1 V/M, 2 the active slot, 3 on channel x[2] (0-based). 0 done; 1
            // refused (the note says why), 2 transmitting, 4 scanning, 5 no channel on the slot yet, 6 no such channel
            N_SAPPLY => {
                let s = (a as u8).min(2);
                let sl = *app.slot(s);
                let mut c = SlotChange::default();
                match x[1] {
                    0 => (c.freq, c.vfo) = (Some(x[2] as Freq10), true),
                    1 if sl.on_channel() => c.vfo = true,
                    2 => c.active = true,
                    k => {
                        // V/M back to the slot's channel, or another channel by number (the keypad's entry)
                        let held = slot_held(self, app, s);
                        if held != 0 {
                            return held;
                        }
                        let to = if k == 1 { sl.ch.map(|c| c.num) } else { Some(x[2] as u16) };
                        let Some((ch, pos)) = to.and_then(|n| st.by_num(n)) else { return if k == 1 { 5 } else { 6 } };
                        app.set_channel(s, ch.zone, pos, st);
                        return 0;
                    }
                }
                return match app.slot_apply(s, &c, st) {
                    Ok(()) => 0,
                    Err(why) => self.no(why.as_bytes()),
                };
            }
            // the active slot's quick setting a (MENU 1-9): 0 its name, 1 its value, 2 not the default, 3 greyed,
            // 4 its help (or why it is greyed), 5 back to the default, 6 and 7 a step up or down
            N_QUICK => {
                let it = QUICK[(a as usize).min(8)];
                let why = app.greyed(it);
                match x[1] {
                    0 => w.s(it.name().as_bytes()),
                    1 => w.s(crate::screens::quick::row_value(app, it).as_bytes()),
                    2 => return !app.is_default(it) as i32,
                    3 => return why.is_some() as i32,
                    4 => match why {
                        Some(y) => w.s(y.as_bytes()),
                        None => w.s(crate::text::load(it.help(!app.slot(app.active).on_channel())).as_bytes()),
                    },
                    _ if why.is_some() => return self.no(why.unwrap_or_default().as_bytes()),
                    5 => app.reset(it),
                    k => app.change(it, if k == 6 { 1 } else { -1 }),
                }
            }
            // a list's length: 0 APRS heard, 1 messages, 2 the GPS satellites in view
            N_NLIST => {
                return match a {
                    0 => d.stations(),
                    1 => d.messages(),
                    _ => d.sats() as u16,
                } as i32
            }
            // list a's row x[1], field x[2]: text is printed, numbers given back
            N_LITEM => {
                let (i, f) = (x[1] as u16, x[2]);
                match a {
                    0 => {
                        let Some(s) = d.station(i) else { return 0 };
                        match f {
                            0 => w.s(s.call.as_bytes()),
                            1 => return s.ago_s as i32,
                            2 => w.s(s.via.as_bytes()),
                            3 => w.s(s.comment.as_bytes()),
                            _ => return s.times as i32,
                        }
                    }
                    1 => {
                        let Some(m) = d.message(i) else { return 0 };
                        match f {
                            0 => return m.inbound as i32,
                            1 => w.s(m.peer.as_bytes()),
                            2 => w.s(m.text.as_bytes()),
                            3 => return m.ago_s as i32,
                            _ => {
                                return match m.state {
                                    MsgState::Unread => 0,
                                    MsgState::Read => 1,
                                    MsgState::Pending => 2,
                                    MsgState::Acked => 3,
                                    MsgState::Retrying(n) => 8 + n as i32,
                                    MsgState::Failed => 4,
                                    MsgState::Rejected => 5,
                                    MsgState::Cancelled => 6,
                                }
                            }
                        }
                    }
                    _ => {
                        let Some(s) = d.sat(i as u8) else { return 0 };
                        return match f {
                            0 => s.prn as i32,
                            1 => s.snr as i32,
                            2 => s.used as i32,
                            3 => s.elev as i32,
                            _ => s.azim as i32,
                        };
                    }
                }
            }
            // APRS: 0 the draft's recipient typed, 2 and 3 the draft's recipient and text printed, 4 the typed text
            // sent to it (Compose's checks), 5 a beacon now, 6 message x[1] read, 7 every message read, 8 the
            // captured text as the typed line (a callsign from a list). A send or beacon: 0 done; 1 refused (the
            // note says why), 2 no callsign, 3 no recipient, 4 no text, 5 four being tried, 6 callsign over 6,
            // 7 APRS and beacons off
            N_APRSDO => {
                let (dr, line) = (d.draft(), cmd_trim(self.pg.line.as_bytes()));
                match a {
                    0 | 4 => {
                        let dr = match a {
                            0 => {
                                let mut to = [0u8; 9];
                                for (t, c) in to.iter_mut().zip(line) {
                                    *t = c.to_ascii_uppercase();
                                }
                                Draft { to: Buf::from(&to[..line.len().min(9)]), ..dr }
                            }
                            _ => Draft { text: Buf::from(&line[..line.len().min(67)]), ..dr },
                        };
                        d.request(Op::Draft(dr));
                        if a == 4 {
                            return self.send(app, d, &dr);
                        }
                    }
                    2 => w.s(dr.to.as_bytes()),
                    3 => w.s(dr.text.as_bytes()),
                    5 => {
                        // as the APRS card's beacon now (APRS-17.9, APRS-8)
                        if let Some(why) = app.kiss_holds_aprs() {
                            return self.no(why.as_bytes());
                        }
                        if app.aprs_holder().is_none() {
                            return 7;
                        }
                        if !d.request(Op::BeaconNow) {
                            return if app.set.callsign.as_bytes().is_empty() { 2 } else { 6 };
                        }
                    }
                    6 => {
                        // INBOX-3.1, INBOX-7.1: one message fewer, its log entry read with it
                        let unread = d.message(x[1] as u16).is_some_and(|m| m.inbound && m.state == MsgState::Unread);
                        if d.request(Op::MessageRead(x[1] as u16)) && unread {
                            (app.inbox, app.log_unread) = (app.inbox.saturating_sub(1), app.log_unread.saturating_sub(1));
                        }
                    }
                    7 => {
                        if d.request(Op::MessagesReadAll) {
                            (app.log_unread, app.inbox, app.pages_unread) = (app.log_unread.saturating_sub(app.badge()), 0, 0); // INBOX-3.2, DEC-22
                        }
                    }
                    // what was captured, as the typed line (a reply's callsign, from the list)
                    _ => self.pg.line = Buf::from(w.cap(false).as_bytes()),
                }
            }
            // GPS line a, as the radio's position page (0 6 2): 0 the status, 1 the fix's age, 2 and 3 the position,
            // 4-9 the tiles' names, 10-15 their values, 16 the nearest waypoint; 0 when there is none
            N_GPSL => {
                let p = crate::screens::gps::position_shown(app, d, dev);
                let i = a as usize;
                match (i, &p.fix, &p.near) {
                    (0, ..) => w.s(p.status.as_bytes()),
                    (1, ..) => w.s(p.age.as_bytes()),
                    (2, Some(f), _) => w.s(f.0.as_bytes()),
                    (3, Some(f), _) => w.s(f.1.as_bytes()),
                    (4..=9, Some(_), _) => w.s(crate::screens::gps::tile_names().nth(i - 4).unwrap_or_default()),
                    (10..=15, Some(f), _) => w.s(f.2[i - 10].as_bytes()),
                    (16, _, Some(n)) => {
                        w.s(n.0.as_bytes());
                        w.s(b"  ");
                        w.s(n.1.as_bytes());
                    }
                    _ => return 0,
                }
                return 1;
            }
            // a device value: 0 battery %, 1 the MCU's temperature (-999 unknown), 2 CPU % (-1 unknown), 3 the MCU
            // clock (MHz), 4 the stack never used (bytes)
            _ => {
                return match a {
                    0 => dev.battery_pct as i32,
                    1 => dev.mcu_temp.map_or(-999, i32::from),
                    2 => dev.cpu_pct.map_or(-1, i32::from),
                    3 => dev.mcu.mhz as i32,
                    _ => dev.stack_free as i32,
                }
            }
        }
        0
    }

    /// A message sent as Compose sends it (APRS-18, APRS-8, APRS-13.3, KISS-10): 0, or why not (N_APRSDO's codes).
    fn send(&mut self, app: &App, d: &dyn crate::data::Data, dr: &Draft) -> i32 {
        if let Some(w) = app.kiss_holds_aprs() {
            return self.no(w.as_bytes());
        }
        match () {
            _ if d.callsign().is_none() => 2,
            _ if dr.to.is_empty() => 3,
            _ if dr.text.is_empty() => 4,
            _ if !d.send_room() => 5,
            _ if !d.request(Op::Send) => 6,
            _ => 0,
        }
    }
}
