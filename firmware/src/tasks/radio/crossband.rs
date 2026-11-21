//! The crossband repeater (the `tx` build).

use super::*;

/// The crossband repeater (manual chapter 19, decision 93), in the `tx` build only.
#[cfg(feature = "tx")]
impl Radio {
    /// Start or stop. XB-2 on the radio: two sides on different chips, neither on HF, the relay released as it is
    /// now (the repeater never moves it); a refusal goes back in `XbLive::stopped` for the UI to show.
    pub(super) async fn crossband(&mut self, c: Option<ferrite_core::msg::XbCfg>) {
        let now = now_ms() as u32;
        let Some(cfg) = c else {
            if let Some(x) = self.xb.take() {
                if self.path.keyed().is_some() {
                    self.xb_unkey(&x, false, now).await;
                }
            }
            self.tuned = [None; 2];
            self.live.mode = ModeLive::Off;
            speaker(false);
            log(b"xb off", 0);
            return;
        };
        let sides = [cfg.side1 % SLOTS as u8, cfg.side2 % SLOTS as u8];
        let f = sides.map(|s| self.slots[s as usize].freq);
        if let Err(why) = xb::sides_ok(f, self.path.relay()) {
            self.live.mode = ModeLive::Xb(ferrite_core::msg::XbLive { stopped: why as u8, ..Default::default() });
            log(b"xb refused ", why as u32);
            return;
        }
        // XB-13: Battery Save's receivers awake; watch and scans simply don't run while it does.
        let a = self.save.activity(now_ms());
        self.apply(a).await;
        self.tuned = [None; 2];
        speaker(false);
        let rep = Repeater::new(cfg, now);
        self.live.mode = ModeLive::Xb(rep.live);
        self.xb = Some(XbRun { rep, sides, hop: 0, hop_t: now });
        log(b"xb on, side 1 khz ", f[0] / 100);
    }

    /// XB-14: local PTT holds the repeater, unkeying a pass first; released, it carries on.
    pub(super) async fn xb_hold(&mut self, on: bool) {
        let now = now_ms() as u32;
        let Some(mut x) = self.xb.take() else { return };
        if on && x.rep.from().is_some() && self.path.keyed().is_some() {
            self.xb_unkey(&x, false, now).await;
        }
        x.rep.hold(on, now);
        if !on {
            self.tuned = [None; 2]; // back on the sides on the next tick
        }
        self.xb = Some(x);
    }

    /// The repeater's tick (10 ms): while ready the receive path takes turns on the two sides; a pass keeps its
    /// side's. Each listened side's squelch, with its tone when the tone gate is on (XB-4), goes to the engine; its
    /// key-ups and unkeys are carried out; the sequencer's duty limit, battery and backstop run while keyed.
    pub(super) async fn xb_tick(&mut self, now: u32) {
        let Some(mut x) = self.xb.take() else { return };
        let keyed = self.path.keyed().is_some();
        if keyed {
            let batt = self.battery_ok();
            let cfg = self.xb_seq_cfg(&x, false);
            let mut st = Steps::default();
            if let Some(e) = self.tx.seq.tick(&cfg, batt, &mut self.path, now, &mut st) {
                self.run(&st).await;
                if e != txseq::Event::PreAlert {
                    x.rep.cut(now);
                    self.xb_after_unkey(&x);
                    log(b"xb cut by the sequencer ", e as u32);
                }
            }
        }
        let keyed_chip = self.path.keyed().map(|s| s.chip());
        // Each side's chip on its slot (the transmitting one stays as keyed: simplex, the same frequency).
        // By index: `for s in x.sides` left core's array IntoIter::next in slow flash in the tx build (slowcheck).
        #[allow(clippy::needless_range_loop)]
        for i in 0..2 {
            let s = x.sides[i];
            let n = Sel::rx(self.slots[s as usize].freq).chip();
            if keyed_chip != Some(n) {
                self.tune_chip(n, Some((s, self.slots[s as usize].rx_freq()))).await;
            }
        }
        let listen = match x.rep.from() {
            Some(from) => from,
            None => {
                if now.wrapping_sub(x.hop_t) >= XB_TURN_MS {
                    (x.hop, x.hop_t) = (x.hop ^ 1, now);
                }
                x.hop
            }
        };
        let ls = x.sides[listen as usize];
        let lf = self.slots[ls as usize].freq;
        if keyed_chip.is_none() {
            self.select_rx(lf, Cause::Auto).await; // never the relay: refused instead (checked at start)
        }
        let mut heard = [None; 2];
        let n = Sel::rx(lf).chip();
        if self.path.rx_now() == Rx::Bk(Sel::rx(lf)) && self.has(n) && keyed_chip != Some(n) {
            let mut bk = self.chip(n);
            let (rssi, st) = (bk.rssi_dbm(), bk.status());
            let sl = self.slots[ls as usize];
            let gate = x.rep.cfg.flags & ferrite_core::msg::XbCfg::TONE_GATE != 0 && sl.rx_tone != 0;
            let open = st.squelch_open && (!gate || radio_chip::tone_heard(sl.rx_tone, &st));
            heard[listen as usize] = Some(open);
            self.live.slots[ls as usize].rssi = rssi;
            if x.rep.from() == Some(listen) || x.rep.from().is_none() {
                x.rep.set_rssi(rssi);
            }
        }
        match x.rep.tick(now, heard) {
            Some(XbCmd::Key { from }) => self.xb_key(&mut x, from, now).await,
            Some(XbCmd::Unkey { id }) => self.xb_unkey(&x, id, now).await,
            // Decision 95: stock's RX end while keyed (the mic back), and the repeat audio again on a return.
            Some(XbCmd::Drop) => {
                let mut st = Steps::default();
                self.path.xb_drop(&mut st);
                self.run(&st).await;
            }
            Some(XbCmd::Reopen) => {
                let mut st = Steps::default();
                self.path.xb_reopen(&mut st);
                self.run(&st).await;
            }
            None => {}
        }
        // Audio (XB-15): the passing side's chip always (its audio is what goes out); the speaker per the
        // setting, or SIDE1's monitor on that side.
        let from = x.rep.from();
        let spk_set = x.rep.cfg.flags & ferrite_core::msg::XbCfg::SPEAKER != 0;
        let mut heard_slot = None;
        for i in 0..2u8 {
            let s = x.sides[i as usize];
            let n = Sel::rx(self.slots[s as usize].freq).chip();
            let open = from == Some(i) || heard[i as usize] == Some(true);
            if open {
                heard_slot = Some(s);
            }
            if self.has(n) && keyed_chip != Some(n) {
                self.chip(n).af_out(if open { AfOut::Normal } else { AfOut::Mute });
            }
            let sl = &mut self.live.slots[s as usize];
            sl.squelch_open = open;
            sl.carrier = open; // the chip's own decision (SQL-13.1)
            if !open {
                sl.rssi = sl.rssi.min(-121);
            }
        }
        let spk = heard_slot.is_some_and(|s| spk_set || self.monitor & 1 << s != 0);
        if self.path.keyed().is_some() {
            AMP_EN[0].write(true); // the audio stage carries the repeat audio (stock: PB8 on)
            AMP_EN[1].write(spk);
        } else {
            speaker(spk);
        }
        self.live.mode = ModeLive::Xb(x.rep.live);
        self.xb = Some(x);
    }

    /// The sequencer's settings for a pass: its timeout a little after the repeater's own (XB-9: the repeater
    /// unkeys first), the CW ID when the repeater asks for it (XB-12), the end tone and tail elimination as the
    /// transmitting slot's (decision 95: stock plays its ROGER and TAIL at a repeat's unkey as for any TX).
    fn xb_seq_cfg(&self, x: &XbRun, id: bool) -> txseq::Cfg {
        let to = x.rep.from().map_or(0, |f| x.sides[1 - f as usize]);
        txseq::Cfg {
            timeout_s: (x.rep.timeout_ms() / 1000 + 5).min(600) as u16,
            prealert_s: 0,
            end_tone: self.tx.end[to as usize % 4] != 0,
            cw_id: id && self.tx.cw_len != 0,
            cw_each: true,
            tail: self.tx.tail[to as usize % 4],
        }
    }

    /// A pass: the other side's chip keyed on its own receive frequency (XB-18) with its tone, at the repeater's
    /// power (XB-3: Low or Mid), through the sequencer's checks; the receiving chip stays in receive (XB-6). The
    /// engine may key on an open it heard on `from` while the hop has the receive path on the other side (whose
    /// chip transmits): the key-up selects `from`'s receiver first (XB-5, decision 163), and the tick listens
    /// there until the pass ends, then hops again.
    async fn xb_key(&mut self, x: &mut XbRun, from: u8, now: u32) {
        if let Some(c) = crate::chan::TX_CAL.try_take() {
            (self.tx.cal, self.tx.gauge) = c;
        }
        let to = x.sides[1 - from as usize];
        let rx = self.slots[x.sides[from as usize] as usize].freq;
        let f = self.slots[to as usize].freq;
        let power = x.rep.cfg.power.min(1);
        let Some((region, extend)) = self.tx.region else {
            x.rep.refused(now);
            return log(b"xb refused: no region from the UI", 0);
        };
        let req = Req {
            tx: f,
            rx,
            kind: TxKind::Voice,
            person: false,
            confirmed: false,
            am: self.slots[to as usize].am,
            rx_only: self.slots[to as usize].rx_only,
            busy: false, // the repeater relays what it hears; its own tone gate decides (XB-*)
            hf_si: false,
            si_on: self.si.mode().is_some(),
            region,
            extend,
            cal_ok: self.tx.cal.is_some(),
            cal_window: self.tx.cal.is_some_and(|c| c.in_window(f)),
            batt_ok: self.battery_ok(),
            power,
        };
        if let Err(r) = self.tx.seq.check(&req, &self.path, now) {
            x.rep.refused(now);
            return log(b"xb refused ", r as u32);
        }
        // TX-11 (TX audit F16): the PA lookup refuses outside its rows.
        // CAL-6.1: a band whose curves failed a check refuses as one without a curve does
        let Some(pa) = self.tx.cal.and_then(|c| c.pa(f, power).ok()) else {
            x.rep.refused(now);
            return log(b"xb refused: no PA curve there", 0);
        };
        let n = Sel::tx(f).chip();
        if !self.has(n) {
            x.rep.refused(now);
            return;
        }
        let tone = self.tx.tone[to as usize % 4];
        // TX-43 (TX audit F9): the cable stops before the pass keys; the snapshot after this tick corrects it.
        crate::chan::RF_STATE.fetch_or(crate::chan::RF_BUSY_KEYED, core::sync::atomic::Ordering::Relaxed);
        self.ldo_on(n);
        // Decision 95: the TX side's own bandwidth (stock takes the receiving channel's).
        let narrow = self.slots[to as usize].narrow;
        let setup = self.tx_chip_setup(f, narrow, tone, false); // the user's mic gain, as stock's repeater
        radio_chip::tx_setup(&mut self.chip(n), &setup);
        self.tuned[n as usize] = Some((to, f));
        self.maybe_awake |= 1 << n;
        self.tx.pa = pa;
        (self.tx.fault, self.tx.data, self.tx.slot, self.tx.dialled) = (false, Some(power), to, false);
        // Decision 95: stock's TX setup clears PB8 unless the accessory detect (PC15) is low; its PTT state 1
        // sets it again with the repeat audio (`Act::RepeatAudio`).
        pb8(false);
        let mut st = Steps::default();
        if let Err(r) = self.tx.seq.key_xb(&req, &mut self.path, now, &mut st) {
            x.rep.refused(now);
            return log(b"xb refused ", r as u32);
        }
        self.run(&st).await;
        if self.tx.fault {
            let mut st = Steps::default();
            self.tx.seq.safe(&mut self.path, now_ms() as u32, &mut st);
            self.run(&st).await;
            x.rep.cut(now);
            self.xb_after_unkey(x);
            return log(b"xb read-back failed", 0);
        }
        self.tx.chip = Some(n);
        self.live.tx = TxLive { keyed: true, slot: to, mic: 0, refused: None, refusals: self.live.tx.refusals, dtmf_sent: self.live.tx.dtmf_sent, voice: self.live.tx.voice, ptt_stuck: self.live.tx.ptt_stuck };
        log(b"xb pass to khz ", f / 100);
    }

    /// The end of a pass (XB-8): the CW ID first when due (XB-12) and tail elimination as the slot's, then the path
    /// machine's crossband unkey (PA 0 with the repeat audio on, the unkey wait, the TX path off, the mic back).
    async fn xb_unkey(&mut self, x: &XbRun, id: bool, now: u32) {
        let cfg = self.xb_seq_cfg(x, id);
        let mut st = Steps::default();
        self.tx.seq.release(&cfg, &mut self.path, now, &mut st);
        self.run(&st).await;
        self.xb_after_unkey(x);
        log(b"xb unkey, id ", id as u32);
    }

    /// After any unkey of a pass: the transmitting chip back to its side's receive settings on the next tick.
    fn xb_after_unkey(&mut self, _x: &XbRun) {
        if let Some(n) = self.tx.chip.take() {
            self.tuned[n as usize] = None;
        }
        self.tx.data = None;
        self.live.tx.keyed = false;
    }
}
