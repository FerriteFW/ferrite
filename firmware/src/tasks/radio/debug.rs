//! The terminal's debug requests.

use super::*;

impl Radio {
    /// The terminal's debug requests (TERM-21 to TERM-24): registers read and written as asked, but never a
    /// transmit or PA bit, nor the chip's GPIO outputs (`bk_write_refused`, checked here as well as in the
    /// terminal). Memory requests are the UI task's.
    pub(super) fn debug(&mut self, d: DebugReq) -> DebugAns {
        match d {
            DebugReq::BkRead { chip, .. } | DebugReq::BkWrite { chip, .. } if chip > 1 || self.chips & 1 << chip == 0 => {
                DebugAns::Refused("that BK4829 didn't answer at start-up")
            }
            DebugReq::BkRead { chip, reg } if reg < 0x80 => DebugAns::Reg { chip, reg, val: self.chip(chip).read(reg) },
            DebugReq::BkWrite { chip, reg, val } => {
                if let Some(why) = bk_write_refused(reg, val) {
                    return DebugAns::Refused(why);
                }
                self.chip(chip).write(reg, val);
                DebugAns::Reg { chip, reg, val: self.chip(chip).read(reg) }
            }
            // TERM-31, PWR-21: the settle waits, set or asked; from the next wake or sleep on.
            DebugReq::Settle(set) => {
                if let Some(set) = set {
                    self.settle = set;
                }
                DebugAns::Settle(self.settle)
            }
            // TERM-53: the bus speed test at one half period for reads and writes, then the speeds as they were;
            // or the speeds in force set (RAM only).
            DebugReq::Spi { half, reads, set } => {
                if let Some([rd, wr]) = set {
                    self.wire.set_speeds(rd as u32 * 100, wr as u32 * 100);
                }
                let before = self.wire.speeds();
                // Each chip's REG_48 (the receive AF gain), the register the writes test, read at stock's speed: it
                // and the patterns' read-backs are what the test is judged by.
                let stock = bk4829::wire::HALF_US * 1000;
                let half = half.map_or(before.0, |h| h as u32 * 100);
                self.wire.set_speeds(stock, stock);
                let was = [0u8, 1].map(|n| if self.chips & 1 << n != 0 { self.chip(n).read(bk4829::reg::AF_GAIN) } else { 0 });
                // Reads (the chip's ID) and writes (REG_48 in four bit patterns) at the half period tested; each write
                // read back at stock's, so a write is judged apart from the reads. REG_48 ends as it was.
                let (mut rd, mut wr) = ([u16::MAX; 2], [u16::MAX; 2]);
                let c0 = crate::hal::time::cycles();
                for n in 0..2u8 {
                    if self.chips & 1 << n != 0 {
                        let v = was[n as usize];
                        // Four bit patterns on REG_48 (its own value and three flips of it); what each reads back
                        // when written at stock's speed is what a fast write must read back.
                        let pats = [v, v ^ 0x5555, v ^ 0xAAAA, !v];
                        let want = pats.map(|p| {
                            let mut c = self.chip(n);
                            c.write(bk4829::reg::AF_GAIN, p);
                            c.read(bk4829::reg::AF_GAIN)
                        });
                        let (mut r, mut w) = (0u16, 0u16);
                        for i in 0..reads as usize {
                            self.wire.set_speeds(half, half);
                            let mut c = self.chip(n);
                            r += (c.read(0) != bk4829::wire::ID) as u16;
                            c.write(bk4829::reg::AF_GAIN, pats[i % 4]);
                            self.wire.set_speeds(stock, stock);
                            w += (self.chip(n).read(bk4829::reg::AF_GAIN) != want[i % 4]) as u16;
                        }
                        self.chip(n).write(bk4829::reg::AF_GAIN, v);
                        (rd[n as usize], wr[n as usize]) = (r, w);
                    }
                }
                let ops = reads as u32 * self.chips.count_ones() * 3;
                let us = (crate::hal::time::cycles().wrapping_sub(c0) / crate::hal::clock::mhz()).checked_div(ops).unwrap_or(0);
                self.wire.set_speeds(before.0, before.1);
                // A write at a failing speed may have hit another register: every slot's receive set-up again.
                if reads > 0 {
                    (0..SLOTS as u8).for_each(|s| self.retune(s));
                }
                let now = self.wire.speeds();
                DebugAns::Spi { half: (half / 100) as u8, reads, rd, wr, us, now: [(now.0 / 100) as u8, (now.1 / 100) as u8] }
            }
            // WATCH-17.1 (decision 262): the turns the chips took, read and maybe zeroed.
            DebugReq::Turns { reset } => DebugAns::Turns(self.turns.take(reset)),
            // TERM-42, TERM-43 (decision 305): the receive filter override and the FSK modem test.
            DebugReq::RxAf(set) => self.rxaf_ask(set),
            DebugReq::RxIf(set) => self.rxif_ask(set),
            DebugReq::Hold(set) => self.hold_ask(set),
            // TERM-29.4: a slot on the other band's chip (the fox-hunt measurement); every slot retunes on a change
            DebugReq::Swap(set) => {
                if let Some(m) = set.filter(|&m| m != self.swap) {
                    self.swap = m;
                    (0..SLOTS as u8).for_each(|s| self.retune(s));
                }
                DebugAns::Swap(self.swap)
            }
            // TERM-44 (decision 315): the DSP task runs the meter; the answer is its last report (zeroed here on a
            // start, so a stale one never shows)
            DebugReq::Meter(set) => {
                if let Some(on) = set {
                    crate::tasks::dsp::meter_reset(on);
                    if crate::chan::DSP.try_send(ferrite_core::msg::DspCmd::Meter(on)).is_err() {
                        return DebugAns::Refused("the DSP task is busy; try again");
                    }
                }
                DebugAns::Meter(crate::tasks::dsp::meter_report())
            }
            DebugReq::Fsk(a) => self.fsk_ask(a),
            // TERM-46 (decision 319): an audio switch pin, now
            DebugReq::Pin { which, mode } => {
                use crate::board::{ACC_ROUTE, AMP_EN, MIC_INJECT, PC9_AUDIO, REPEAT_AUDIO};
                use ferrite_core::msg::PinMode;
                let p = [REPEAT_AUDIO, MIC_INJECT, PC9_AUDIO, AMP_EN[0], ACC_ROUTE, AMP_EN[1]][which as usize % 6];
                match mode {
                    PinMode::Low => p.output(false, Mode::OutPp),
                    PinMode::High => p.output(true, Mode::OutPp),
                    PinMode::PullUp => p.config(Mode::InPu),
                    PinMode::PullDown => p.config(Mode::InPd),
                }
                crate::hal::time::delay_us(50); // the level settles (a pull through a few k into a few nF)
                DebugAns::Pin { which, mode, high: p.read() }
            }
            // TERM-54: DAC1's tone now, wherever the pins route it
            DebugReq::Dac { hz, level } => {
                crate::hal::tone::start(hz as u32, level);
                DebugAns::Dac { hz: crate::hal::tone::hz() as u16, level: if hz == 0 { 0 } else { level } }
            }
            // TERM-45 (decision 318): tone-generator 4FSK while keyed (transmit builds)
            #[cfg(feature = "tx")]
            DebugReq::SynthFsk(c) => self.synth_fsk(c),
            #[cfg(not(feature = "tx"))]
            DebugReq::SynthFsk(_) => DebugAns::Refused("synthfsk needs a transmit build"),
            // TERM-32 (a bench test): PA0, the mic level tap VOX reads, at 8 kHz for 32 ms, to tell raw audio from an
            // envelope. Through the ADC's preempted group, as VOX; the Radio task waits through it. TERM-32.1 (`af`):
            // PA2, the receive audio (both BK4829s summed, and the SI4732's: bench Oct 25, 2026, decision 290).
            DebugReq::Mic(pa2) => {
                use crate::hal::{adc, clock, time::cycles};
                use ferrite_core::vox::{MicCapture, MIC_PERIOD_US, MIC_SAMPLES};
                adc::init();
                let mut s = [0u16; MIC_SAMPLES];
                let per = MIC_PERIOD_US * clock::mhz();
                let start = cycles();
                for (i, v) in s.iter_mut().enumerate() {
                    while cycles().wrapping_sub(start) < i as u32 * per {}
                    *v = adc::read(if pa2 { adc::CH_PA2 } else { adc::CH_PA0 });
                }
                let us = cycles().wrapping_sub(start) / clock::mhz();
                DebugAns::Mic(MicCapture::from_samples(&s, us))
            }
            // TERM-33 (decision 132, a bench test): armed for the next PTT key-up, once; the key-up plays it.
            // TERM-34: the suite needs a TX timeout it fits in; asked while it runs, its progress.
            #[cfg(feature = "tx")]
            DebugReq::TxTest(ask) => {
                use ferrite_core::txtest::{Ask, Signal};
                match ask {
                    Ask::Set(Some(a)) if a.signal == Signal::Suite && self.tx.timeout_s < ferrite_core::txsuite::TIMEOUT_MIN_S => {
                        return DebugAns::Refused("the suite runs 60 s: set the TX timeout (0 8 1) to 90 s or more first");
                    }
                    Ask::Set(a) => {
                        if a.is_some() && self.fsk.as_ref().is_some_and(|r| r.tx_armed()) {
                            self.fsk = None; // one test signal per key-up: the last armed
                        }
                        self.tx.test = a;
                        log(b"txtest armed ", a.map_or(0, |a| a.signal as u32 + 1));
                    }
                    Ask::Show => {
                        if let Some((section, left_ms)) = self.suite_status() {
                            return DebugAns::TxSuite { section, left_ms };
                        }
                    }
                }
                DebugAns::TxTest(self.tx.test)
            }
            #[cfg(not(feature = "tx"))]
            DebugReq::TxTest(_) => DebugAns::Refused("the receive-only build has no transmit: txtest needs the tx-bench build"),
            DebugReq::Si { cmd, n, reply } => {
                let (n, rn) = ((n as usize).min(8), (reply as usize).clamp(1, 16));
                // TERM-24: powered down (and in reset) outside FM broadcast and HF mode: nothing is sent then.
                if self.si.mode().is_none() {
                    return DebugAns::Refused("the SI4732 is powered down: FM broadcast or HF mode powers it up");
                }
                let mut data = [0u8; 16];
                match self.si.command(&cmd[..n], &mut data[..rn], &mut BusyDelay) {
                    Ok(()) => DebugAns::Si { data, n: rn as u8 },
                    Err(_) => DebugAns::Refused("the SI4732 didn't answer the command"),
                }
            }
            _ => DebugAns::Refused("not a Radio task request"),
        }
    }

    /// TERM-29, SAFE-24 (decision 114, a bench test): hold both band receive selects on (VHF RX PB0 and UHF RX
    /// PE12), back to one, or ask. Refused while keyed, in HF mode (its path and the relay) and while the crossband
    /// repeater runs (it owns both chips' paths). RAM only: off at boot.
    pub(super) async fn rx_both(&mut self, on: Option<bool>) -> DebugAns {
        let Some(on) = on else { return DebugAns::RxBoth(self.path.both()) };
        if self.hf.is_some() {
            return DebugAns::Refused("HF mode is on: it has the HF path and the relay; leave HF mode first");
        }
        #[cfg(feature = "tx")]
        if self.xb.is_some() {
            return DebugAns::Refused("the crossband repeater is running: it owns both chips' paths");
        }
        let mut s = Steps::default();
        match self.path.set_both(on, &mut s) {
            Err(Blocked::Keyed) => DebugAns::Refused("transmitting: try again after the unkey"),
            Err(_) => DebugAns::Refused("an HF slot has the relay energised: tune VHF or UHF first"),
            Ok(()) => {
                self.run(&s).await;
                DebugAns::RxBoth(on)
            }
        }
    }
}
