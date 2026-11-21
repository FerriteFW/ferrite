//! What the radio is commanded to do, checked on the PC: real plans from `ferrite-core` carried out by [`run`]
//! on the recording mock, and the chip set-ups on a recording BK4829 bus. The physical side (RF, audio, timing on
//! a scope) stays the bench's.

extern crate std;

use crate::bt::BtPower;
use crate::chip::{self, RxSetup};
use crate::gps::{GpsPower, QUIET_MS};
use crate::mock::{block_on, Ev, RecBus, RecHw, State};
use crate::run::{run, Pin, PA_RAMP_STEPS};
use bk4829::{reg, Bk4829};
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::path::{Cause, Path, Rx, Sel, Steps, UNKEY_WAIT_MS, XB_AUDIO_DELAY_MS};
use ferrite_core::txplan::Region;
use ferrite_core::txseq::{Cfg, Kind, Req, TxSeq};
use std::vec::Vec;

const VHF: Freq10 = mhz(146) + 52_000;
const UHF: Freq10 = mhz(446);
const TEN_M: Freq10 = mhz(28) + 40_000;

fn req(tx: Freq10, rx: Freq10) -> Req {
    Req {
        tx,
        rx,
        kind: Kind::Voice,
        person: true,
        confirmed: false,
        am: false,
        rx_only: false,
        busy: false,
        hf_si: false,
        si_on: false,
        region: Region::Us,
        extend: false,
        cal_ok: true,
        cal_window: true,
        batt_ok: true,
        power: 2,
    }
}

/// A radio at its safe state, then receiving on `f` as the active slot.
fn receiving(f: Freq10) -> (Path, TxSeq, RecHw, State) {
    let (mut path, seq, mut hw) = (Path::new(), TxSeq::new(), RecHw::new());
    let mut s = Steps::default();
    path.safe(0, &mut s);
    path.rx(Rx::Bk(Sel::rx(f)), Cause::ActiveSlot, 0, &mut s).unwrap();
    block_on(run(&mut hw, &s));
    let st = hw.check(&State::default());
    hw.log.clear();
    (path, seq, hw, st)
}

fn go(hw: &mut RecHw, s: &Steps, st: &State) -> State {
    hw.log.clear();
    block_on(run(hw, s));
    hw.check(st)
}

fn pa_up(e: &Ev) -> bool {
    matches!(e, Ev::Pa(v) if *v > 0)
}

/// TX-23.1, TX-23.2, TX-27, SAFE-11: a key-up carried out on the pins in the planned order: PA drive 0, the receive
/// select cleared, the TX select, the chip keyed (its read-back), then the PA ramped up to the calibrated level in
/// equal steps 1 ms apart; never two selects on, never the PA up without its chip keyed.
#[test]
fn key_up_order_on_the_pins() {
    let (mut path, mut seq, mut hw, st) = receiving(VHF);
    let mut s = Steps::default();
    seq.key(&req(VHF, VHF), &mut path, 0, &mut s).unwrap();
    go(&mut hw, &s, &st);
    let zero = hw.at(|e| *e == Ev::Pa(0)).unwrap();
    let rx_off = hw.at(|e| *e == Ev::Pin(Pin::Sel(Sel::VhfRx), false)).unwrap();
    let tx_on = hw.at(|e| *e == Ev::Pin(Pin::Sel(Sel::VhfTx), true)).unwrap();
    let key = hw.at(|e| *e == Ev::Key(0)).unwrap();
    let ramp = hw.at(pa_up).unwrap();
    assert!(zero < rx_off && rx_off < tx_on && tx_on < key && key < ramp, "{:?}", hw.evs());
    let steps: Vec<u8> = hw.evs().into_iter().filter_map(|e| match e { Ev::Pa(v) if v > 0 => Some(v), _ => None }).collect();
    assert_eq!(steps.len(), PA_RAMP_STEPS as usize);
    assert!(steps.windows(2).all(|w| w[0] < w[1]) && *steps.last().unwrap() == hw.pa_level, "{steps:?}");
    assert!(hw.at(|e| *e == Ev::Pin(Pin::Amp(0), false)).is_some(), "muted while keyed: the speaker off");
}

/// TX-26, TX-47.2, TONE-8, TX-28: the unkey on the pins in stock's order: end tone, CW ID, the audio stage (PB8) on
/// and tail elimination while still keyed, then PA drive 0, at least 100 ms, the chip out of transmit, then the
/// TX select clear and the receive path back (TX audit F4, F6).
#[test]
fn unkey_order_on_the_pins() {
    let (mut path, mut seq, mut hw, st) = receiving(VHF);
    let mut s = Steps::default();
    seq.key(&req(VHF, VHF), &mut path, 0, &mut s).unwrap();
    let st = go(&mut hw, &s, &st);
    let cfg = Cfg { end_tone: true, cw_id: true, tail: true, ..Cfg::default() };
    let mut s = Steps::default();
    seq.release(&cfg, &mut path, 5_000, &mut s);
    go(&mut hw, &s, &st);
    let tone = hw.at(|e| *e == Ev::EndTone).unwrap();
    let cw = hw.at(|e| *e == Ev::CwId).unwrap();
    let tail = hw.at(|e| *e == Ev::Tail).unwrap();
    let zero = hw.at(|e| *e == Ev::Pa(0)).unwrap();
    let unkey = hw.at(|e| *e == Ev::Unkey(0)).unwrap();
    let tx_off = hw.at(|e| *e == Ev::Pin(Pin::Sel(Sel::VhfTx), false)).unwrap();
    let rx_on = hw.at(|e| *e == Ev::Pin(Pin::Sel(Sel::VhfRx), true)).unwrap();
    let stage = hw.at(|e| *e == Ev::Pin(Pin::Amp(0), true)).unwrap();
    assert!(tone < cw && cw < stage && stage < tail && tail < zero && zero < unkey && unkey < tx_off && tx_off < rx_on, "{:?}", hw.evs());
    assert!(hw.time(unkey) - hw.time(zero) >= UNKEY_WAIT_MS as u32, "the unkey wait before the chip leaves transmit");
}

/// TX-7.2, TX-23.1 (the read-back before the PA ramp), SAFE-11: a key-up whose read-back fails stops the plan there: no
/// PA ramp, the fault reported for the caller's safe state.
#[test]
fn failed_read_back_never_ramps() {
    let (mut path, mut seq, mut hw, st) = receiving(UHF);
    hw.key_ok = false;
    let mut s = Steps::default();
    seq.key(&req(UHF, UHF), &mut path, 0, &mut s).unwrap();
    go(&mut hw, &s, &st);
    assert!(hw.at(|e| *e == Ev::Fault(1)).is_some(), "{:?}", hw.evs());
    assert!(hw.at(pa_up).is_none(), "no PA drive after a failed read-back: {:?}", hw.evs());
}

/// TX-33 (the Radio task's `Shutdown` while keyed, TX audit M6): the sequencer's safe state, run before any chip
/// sleeps, takes the PA drive to 0 first, with no end tone, CW ID or tail; then the chip out of transmit, the unkey
/// wait, every select off. The same from a crossband pass, its repeat audio off before the chip unkeys.
#[test]
fn power_off_while_keyed_zeroes_the_pa_first() {
    let (mut path, mut seq, mut hw, st) = receiving(UHF);
    let mut s = Steps::default();
    seq.key(&req(UHF, UHF), &mut path, 0, &mut s).unwrap();
    let st = go(&mut hw, &s, &st);
    let mut s = Steps::default();
    seq.safe(&mut path, 1_000, &mut s);
    let end = go(&mut hw, &s, &st);
    assert_eq!(hw.evs()[0], Ev::Pa(0), "PA drive 0 before anything else: {:?}", hw.evs());
    assert!(hw.at(|e| matches!(e, Ev::EndTone | Ev::CwId | Ev::Tail)).is_none(), "no tones at a power off");
    let unkey = hw.at(|e| *e == Ev::Unkey(1)).unwrap();
    let tx_off = hw.at(|e| *e == Ev::Pin(Pin::Sel(Sel::UhfTx), false)).unwrap();
    assert!(unkey < tx_off && hw.time(tx_off) >= UNKEY_WAIT_MS as u32, "{:?}", hw.evs());
    assert!(end.sel.iter().all(|&x| !x) && end.pa == 0 && end.keyed.is_none());
    assert!(!seq.keyed() && path.keyed().is_none());

    // A crossband pass: RX on UHF, TX on VHF.
    let (mut path, mut seq, mut hw, st) = receiving(UHF);
    let mut s = Steps::default();
    seq.key_xb(&Req { person: false, ..req(VHF, UHF) }, &mut path, 0, &mut s).unwrap();
    let st = go(&mut hw, &s, &st);
    let mut s = Steps::default();
    seq.safe(&mut path, 1_000, &mut s);
    let end = go(&mut hw, &s, &st);
    assert_eq!(hw.evs()[0], Ev::Pa(0), "{:?}", hw.evs());
    assert!(end.sel.iter().all(|&x| !x) && end.pa == 0 && end.keyed.is_none() && !end.repeat);
}

/// SAFE-20, SAFE-21, HF-11, TX-27: tuning the active slot from 2 m to 10 m moves the relay with no RF: muted,
/// every select clear, the PA at zero, then the relay, its settle time (10 ms), then the new select. Watch and scan
/// (`Cause::Auto`) never move it.
#[test]
fn relay_moves_only_quiet_and_settled() {
    let (mut path, _, mut hw, st) = receiving(VHF);
    let mut s = Steps::default();
    assert!(path.rx(Rx::Bk(Sel::rx(TEN_M)), Cause::Auto, 0, &mut s).is_err());
    go(&mut hw, &s, &st);
    assert!(hw.at(|e| matches!(e, Ev::Pin(Pin::Relay, _))).is_none(), "Auto never moves the relay");
    let mut s = Steps::default();
    path.rx(Rx::Bk(Sel::rx(TEN_M)), Cause::ActiveSlot, 1_000, &mut s).unwrap();
    go(&mut hw, &s, &st);
    let mute = hw.at(|e| *e == Ev::Muted(true)).unwrap();
    let off = hw.at(|e| *e == Ev::Pin(Pin::Sel(Sel::VhfRx), false)).unwrap();
    let relay = hw.at(|e| *e == Ev::Pin(Pin::Relay, true)).unwrap();
    let hf = hw.at(|e| *e == Ev::Pin(Pin::Sel(Sel::HfRx), true)).unwrap();
    assert!(mute < relay && off < relay && relay < hf, "{:?}", hw.evs());
    assert!(hw.time(hf) - hw.time(relay) >= ferrite_core::path::RELAY_SETTLE_MS as u32, "the relay's settle time");
}

/// SAFE-22: the eleventh relay switch inside a minute is refused: the pins see ten moves, not eleven.
#[test]
fn relay_guard_on_the_pins() {
    let (mut path, _, mut hw, mut st) = receiving(VHF);
    let mut moves = 0;
    for i in 0..12u32 {
        let f = if i % 2 == 0 { TEN_M } else { VHF };
        let mut s = Steps::default();
        let _ = path.rx(Rx::Bk(Sel::rx(f)), Cause::ActiveSlot, 1_000 + i * 1_000, &mut s);
        st = go(&mut hw, &s, &st);
        moves += hw.evs().iter().filter(|e| matches!(e, Ev::Pin(Pin::Relay, _))).count();
    }
    assert_eq!(moves, ferrite_core::path::GUARD_SWITCHES, "the guard's limit reaches the pins");
    assert!(path.take_alarm(), "the event log's alarm");
}

/// TX-37, TX-28, WATCH-12 (TX audit F11): a key-up on a frequency behind the other relay state (a 10 m slot keyed
/// while 2 m is heard, or HF mode with the SI4732 on the antenna) is refused by the Radio task's own sequencer,
/// with no pin moved: the relay never switches at key-up.
#[test]
fn key_up_across_the_relay_is_refused() {
    let (mut path, mut seq, mut hw, st) = receiving(VHF);
    let mut s = Steps::default();
    assert_eq!(seq.key(&req(TEN_M, TEN_M), &mut path, 0, &mut s), Err(ferrite_core::txplan::Refusal::RelayMismatch));
    go(&mut hw, &s, &st);
    assert!(hw.evs().is_empty(), "{:?}", hw.evs());
    assert!(!seq.keyed() && path.keyed().is_none() && !path.relay());
}

/// XB-6, XB-7, XB-8 (decision 95: stock's crossband audio switching): the other chip keys with the receiving
/// one's select left on; the PA ramps, then 15 ms later PA15 goes low (the receiving chip's audio into the mic
/// input) with PB8 on; at the receive drop PA15 is high again while still keyed; the unkey clears the receive
/// select while keyed, then PA 0, the unkey wait, the chip back, the TX select clear.
#[test]
fn crossband_audio_switching_as_stock() {
    let (mut path, mut seq, mut hw, st) = receiving(VHF);
    let mut s = Steps::default();
    seq.key_xb(&req(UHF, VHF), &mut path, 0, &mut s).unwrap();
    let st = go(&mut hw, &s, &st);
    assert!(st.sel[Sel::VhfRx as usize] && st.sel[Sel::UhfTx as usize], "receiving side stays selected");
    let last_ramp = hw.log.iter().rposition(|(_, e)| pa_up(e)).unwrap();
    let audio = hw.at(|e| *e == Ev::Pin(Pin::RepeatAudio, false)).unwrap();
    let pb8 = hw.at(|e| *e == Ev::Pin(Pin::Amp(0), true)).unwrap();
    assert!(last_ramp < audio && audio < pb8, "{:?}", hw.evs());
    assert!(hw.time(audio) - hw.time(last_ramp) >= XB_AUDIO_DELAY_MS as u32, "stock's 10-20 ms after the PA");
    // The receive drop: the mic again, still keyed.
    let mut s = Steps::default();
    path.xb_drop(&mut s);
    let st = go(&mut hw, &s, &st);
    assert_eq!(hw.evs().first(), Some(&Ev::Pin(Pin::RepeatAudio, true)));
    assert!(st.pa > 0 && st.keyed == Some(1), "still keyed through the hang");
    // The unkey, as stock's PTT state 4.
    let mut s = Steps::default();
    seq.release(&Cfg::default(), &mut path, 3_000, &mut s);
    let end = go(&mut hw, &s, &st);
    let rx_off = hw.at(|e| *e == Ev::Pin(Pin::Sel(Sel::VhfRx), false)).unwrap();
    let zero = hw.at(|e| *e == Ev::Pa(0)).unwrap();
    let unkey = hw.at(|e| *e == Ev::Unkey(1)).unwrap();
    let tx_off = hw.at(|e| *e == Ev::Pin(Pin::Sel(Sel::UhfTx), false)).unwrap();
    assert!(rx_off < zero && zero < unkey && unkey < tx_off, "{:?}", hw.evs());
    assert!(hw.time(unkey) - hw.time(zero) >= UNKEY_WAIT_MS as u32);
    assert!(end.keyed.is_none() && end.pa == 0 && !end.repeat);
}

/// BCR-20: the SI4732 input pins as stock: FM broadcast 0/0, AM and the rest 1/1; leaving the SI4732 sets 1/0,
/// but the boot and safe state before any broadcast leave stock's 0/0 alone.
#[test]
fn si4732_input_pins() {
    assert_eq!(chip::si_input(false), [false, false]);
    assert_eq!(chip::si_input(true), [true, true]);
    let mut hw = RecHw::new();
    let mut s = Steps::default();
    Path::new().safe(0, &mut s);
    block_on(run(&mut hw, &s));
    assert!(hw.at(|e| matches!(e, Ev::Pin(Pin::SiIn(_), _))).is_none(), "untouched before the SI4732 was used");
    hw.si_used = true;
    hw.log.clear();
    block_on(run(&mut hw, &s));
    let a = hw.at(|e| *e == Ev::Pin(Pin::SiIn(0), true)).unwrap();
    let b = hw.at(|e| *e == Ev::Pin(Pin::SiIn(1), false)).unwrap();
    assert!(a < b);
    // Receiving through the SI4732 is what marks it used: leaving it afterwards sets 1/0.
    let (mut path, mut hw) = (Path::new(), RecHw::new());
    let mut s = Steps::default();
    path.rx(Rx::Si, Cause::HfReceiver, 0, &mut s).unwrap();
    block_on(run(&mut hw, &s));
    assert!(hw.si_used && hw.at(|e| matches!(e, Ev::Pin(Pin::SiIn(_), _))).is_none(), "its own state comes with the start");
    let mut s = Steps::default();
    path.safe(0, &mut s);
    block_on(run(&mut hw, &s));
    assert!(hw.at(|e| *e == Ev::Pin(Pin::SiIn(0), true)).is_some() && hw.at(|e| *e == Ev::Pin(Pin::SiIn(1), false)).is_some());
}

/// BCR-19, PWR-6 (decision 113): Battery Save's sleep step during broadcast FM, on the pins: nothing at all (the
/// receive select stays on: switching it clicked in the shared amp); the speaker amp, the receiver mute and the
/// SI4732's input pins are never touched, so the broadcast plays on. Without a broadcast only the BK4829's
/// receive select goes off, as stock.
#[test]
fn sleep_step_keeps_the_broadcast_playing() {
    let (mut path, _, mut hw, _) = receiving(VHF);
    hw.si_used = true; // broadcast FM running
    let mut s = Steps::default();
    path.rx_off(true, &mut s).unwrap();
    block_on(run(&mut hw, &s));
    assert_eq!(hw.evs(), []);
    let (mut path, _, mut hw, _) = receiving(VHF);
    let mut s = Steps::default();
    path.rx_off(false, &mut s).unwrap();
    block_on(run(&mut hw, &s));
    assert_eq!(hw.evs(), [Ev::Pin(Pin::Sel(Sel::VhfRx), false)]);
}

/// TX-27, TX-28, SAFE-21, SAFE-11: random sequences of everything that plans (tuning, watch, key-up and unkey,
/// the sequencer's tick, the safe state) carried out by `run` on the mock: the pin-level rules hold after every
/// plan (the same sequences `ferrite-core`'s rf_invariants checks at the plan level).
#[test]
fn random_plans_keep_the_pin_rules() {
    struct Rng(u64);
    impl Rng {
        fn next(&mut self, n: u32) -> u32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 33) % n as u64) as u32
        }
    }
    let freqs = [VHF, mhz(147), mhz(223) + 50_000, UHF, TEN_M, mhz(29) + 60_000, mhz(243)];
    let cfg = Cfg { end_tone: true, cw_id: true, tail: true, timeout_s: 30, ..Cfg::default() };
    let mut rng = Rng(950);
    let mut keyed = 0;
    for _ in 0..100 {
        let (mut path, mut seq, mut hw) = (Path::new(), TxSeq::new(), RecHw::new());
        let mut st = State::default();
        let mut now = 0u32;
        for _ in 0..200 {
            now += 1 + rng.next(5_000);
            let f = freqs[rng.next(freqs.len() as u32) as usize];
            let mut s = Steps::default();
            match rng.next(6) {
                0 | 1 => {
                    let cause = [Cause::ActiveSlot, Cause::Auto, Cause::HfReceiver][rng.next(3) as usize];
                    let _ = path.rx(Rx::Bk(Sel::rx(f)), cause, now, &mut s);
                }
                2 => {
                    let mut r = req(f, freqs[rng.next(freqs.len() as u32) as usize]);
                    r.power = rng.next(3) as u8;
                    if seq.key(&r, &mut path, now, &mut s).is_ok() {
                        keyed += 1;
                    }
                }
                3 => seq.release(&cfg, &mut path, now, &mut s),
                4 => {
                    let _ = seq.tick(&cfg, rng.next(20) != 0, &mut path, now, &mut s);
                }
                _ => {
                    if rng.next(8) == 0 {
                        seq.safe(&mut path, now, &mut s);
                    }
                }
            }
            hw.key_ok = rng.next(40) != 0;
            st = go(&mut hw, &s, &st);
            if hw.at(|e| matches!(e, Ev::Fault(_))).is_some() {
                // The Radio task goes to the safe state next.
                let mut s = Steps::default();
                seq.safe(&mut path, now, &mut s);
                st = go(&mut hw, &s, &st);
                st.keyed = None;
            }
        }
    }
    assert!(keyed > 300, "the sequences reach key-ups: {keyed}");
}

fn bk() -> Bk4829<RecBus> {
    Bk4829::new(RecBus::new())
}

/// VFO-20 (AM keeps the wide filter): tuning a slot in AM writes stock's mode 6
/// (REG_31 bit 0, REG_42 0x6F5C, REG_2A 0x7434, REG_2B 0x0500), FM stock's mode 7 (0x6B5A, 0x7400, 0x0000), the
/// wide filter for AM, the audio muted before receive comes on last.
#[test]
fn tune_writes_stocks_am_and_fm() {
    let mut b = bk();
    chip::tune(&mut b, &RxSetup { freq: mhz(118) + 95_000, narrow: false, am: true, rx_tone: 0, rx_tail: false, data: false });
    let bus = b.release();
    assert_eq!((bus.last(0x42), bus.last(0x2A), bus.last(reg::AF_FILTERS)), (Some(0x6F5C), Some(0x7434), Some(0x0500)));
    assert_eq!(bus.regs[reg::FUNC2 as usize] & 1, 1, "REG_31 bit 0");
    assert_eq!(bus.last(reg::RF_FILTER), Some(0x3028), "AM keeps the wide filter");
    assert_eq!(bus.writes.last(), Some(&(reg::POWER, 0xBFF1)), "receive on last");
    assert!(bus.writes.iter().rposition(|w| w.0 == reg::AF_OUT).unwrap() < bus.writes.len() - 2, "muted first");
    let mut b = bk();
    chip::tune(&mut b, &RxSetup { freq: VHF, narrow: true, am: false, rx_tone: 0, rx_tail: false, data: false });
    let bus = b.release();
    assert_eq!((bus.last(0x42), bus.last(0x2A), bus.last(reg::AF_FILTERS)), (Some(0x6B5A), Some(0x7400), Some(0)));
    assert_eq!(bus.regs[reg::FUNC2 as usize] & 1, 0);
    assert_eq!(bus.last(reg::RF_FILTER), Some(0x4008), "NFM narrow");
    assert_eq!(bus.last(reg::FREQ_LO).map(|lo| lo as u32 | (bus.regs[reg::FREQ_HI as usize] as u32) << 16), Some(VHF));
}

/// SND-2, RX audio level: every receive setup writes stock's REG_48 0xB2BF (Gain2 -4.5 dB, AF DAC 15), FM wide,
/// narrow and AM alike; the start-up 0xB3FF (+3.5 dB) distorted receive audio on the radio (Oct 12, 2026). The wide
/// filter is stock's 0x3028, narrow 0x4008, and the AF route is written whole (0x6042 muted).
#[test]
fn tune_writes_stocks_receive_level() {
    for (narrow, am, filter) in [(false, false, 0x3028), (true, false, 0x4008), (false, true, 0x3028)] {
        let mut b = bk();
        chip::tune(&mut b, &RxSetup { freq: if am { mhz(118) + 95_000 } else { VHF }, narrow, am, rx_tone: 0, rx_tail: false, data: false });
        let bus = b.release();
        assert_eq!(bus.last(reg::AF_GAIN), Some(bk4829::AF_GAIN_RX), "REG_48 (narrow {narrow}, am {am})");
        assert_eq!(bk4829::AF_GAIN_RX, 0xB2BF);
        assert_eq!(bus.last(reg::RF_FILTER), Some(filter));
        assert_eq!(bus.last(reg::AF_OUT), Some(0x6042), "muted, whole register");
    }
}

/// CHAN-3, TONE-6: the receive tone word picks the detector: 0 none (REG_51 0), CTCSS in 0.1 Hz, bit 15 DCS (the
/// 134.4 Hz word in REG_07), and no tone with tail elimination on listens for the 55 Hz tail (stock's REG_07 0x21CD).
#[test]
fn tune_sets_the_subtone_detector() {
    let tuned = |rx_tone: u16, rx_tail: bool| {
        let mut b = bk();
        chip::tune(&mut b, &RxSetup { freq: VHF, narrow: false, am: false, rx_tone, rx_tail, data: false });
        b.release()
    };
    assert_eq!(tuned(0, false).last(reg::SUBTONE), Some(0));
    let ctcss = tuned(885, false);
    assert_eq!(ctcss.last(reg::CTC), Some(bk4829::ctcss_word(885) & 0x1FFF));
    assert_eq!(ctcss.last(reg::SUBTONE).map(|v| v >> 12 & 1), Some(1), "CTCSS mode");
    let dcs = tuned(0x8000 | 0o23, false);
    assert_eq!(dcs.first(reg::CTC).map(|i| dcs.writes[i].1), Some(0x0AD7));
    assert!(dcs.first(reg::CDCSS).is_some());
    let tail = tuned(0, true);
    assert_eq!(tail.last(reg::CTC), Some(0x21CD), "stock's 55 Hz tail word");
}

/// TSRCH-12: the tone hint's probe switches the detectors to scan (REG_51 51, CTC1 the DCS bit rate) without
/// touching the frequency, filters or audio; putting the slot's tone back writes what tuning wrote (CTCSS, DCS,
/// none with REG_07 then unused, the 55 Hz tail).
#[test]
fn tone_probe_and_restore() {
    let mut b = bk();
    chip::tone_probe(&mut b);
    let bus = b.release();
    assert_eq!(bus.writes, [(reg::SUBTONE, 51), (reg::CTC, 0x0AD7)], "only the detector registers");
    for (rx_tone, rx_tail) in [(1000, false), (0x8000 | 0o23, false), (0x8000 | 0x4000 | 0o754, false), (0, false), (0, true)] {
        let mut t = bk();
        chip::tune(&mut t, &RxSetup { freq: VHF, narrow: false, am: false, rx_tone, rx_tail, data: false });
        let tuned = t.release();
        let mut r = bk();
        chip::tone_probe(&mut r);
        chip::subtone(&mut r, rx_tone, rx_tail);
        let restored = r.release();
        for reg_ in [reg::SUBTONE, reg::CTC, reg::CDCSS, reg::CTCSS_THRESH] {
            if reg_ == reg::CTC && rx_tone == 0 && !rx_tail {
                continue; // detector off (REG_51 0): REG_07 is unused
            }
            assert_eq!(restored.regs[reg_ as usize], tuned.regs[reg_ as usize], "REG_{reg_:02X} for {rx_tone:#x}");
        }
    }
}

/// TSRCH-3, TSRCH-13: what the detectors read becomes a tone word: CTCSS snapped to the standard tone within 1 Hz or
/// 1 % (REG_68 at the BK4829's scale, rounded), else kept to 0.1 Hz; out of 60-260 Hz ignored; DCS in any rotation,
/// with its polarity (the complement is inverted), and a DCS reading wins over a CTCSS one.
#[test]
fn heard_tone_decodes() {
    const CT: [u16; 4] = [670, 885, 1000, 1622];
    const DC: [u16; 3] = [0o023, 0o114, 0o754];
    assert_eq!(chip::heard_tone(Some(884), None, &CT, &DC), Some(885));
    assert_eq!(chip::heard_tone(Some(1003), None, &CT, &DC), Some(1000));
    assert_eq!(chip::heard_tone(Some(1300), None, &CT, &DC), None, "midway between 100.0 and 162.2: no tone");
    assert_eq!(chip::heard_tone(Some(1607), None, &CT, &DC), Some(1622));
    assert_eq!(chip::heard_tone(Some(bk4829::scan_dhz(1803)), None, &CT, &DC), Some(885));
    assert_eq!(chip::heard_tone(Some(550), None, &CT, &DC), None, "the 55 Hz tail is no tone");
    assert_eq!(chip::heard_tone(None, None, &CT, &DC), None);
    let w = bk4829::dcs_codeword(0o114, false);
    assert_eq!(chip::heard_tone(None, Some(w), &CT, &DC), Some(0x8000 | 0o114));
    let rot = (w >> 5) | (w << 18) & 0x7F_FFFF;
    assert_eq!(chip::heard_tone(None, Some(rot), &CT, &DC), Some(0x8000 | 0o114), "rotated");
    let inv = bk4829::dcs_codeword(0o023, true);
    assert_eq!(chip::heard_tone(Some(885), Some(inv), &CT, &DC), Some(0x8000 | 0x4000 | 0o023), "D023I");
}

/// TSRCH-3, TSRCH-13 (decision 179): the reading wanders about ±1.4 % carrier to carrier, so a CTCSS reading snaps
/// to the nearest table tone: the clear radio's 88.5 read "T 87.3" and "T 89.6", both 88.5; refused only in the
/// middle fifth between two tones. Past the ends the inner gap counts.
#[test]
fn heard_ctcss_snaps_to_the_nearest_tone() {
    let ct = &ferrite_app_ctcss();
    let snap = |d: u16| chip::heard_tone(Some(d), None, ct, &[]);
    assert_eq!(snap(873), Some(885));
    assert_eq!(snap(896), Some(885));
    assert_eq!(snap(662), Some(670));
    assert_eq!(snap(2470), Some(2503), "250.3 read 1.3 % low");
    assert_eq!(snap(2535), Some(2541), "253.5 is nearest 254.1 (the table's last tone)");
    assert_eq!(snap(900), None, "midway between 88.5 and 91.5");
    assert_eq!(snap(2590), None, "past 254.1 by more than 2/5 of the gap to 250.3");
    assert_eq!(snap(608), None, "60.8: 6.2 Hz under 67.0, past 2/5 of the 2.3 Hz gap");
    for &t in ct.iter() {
        assert_eq!(snap(t), Some(t), "{t} exact");
    }
}

/// The 50 standard CTCSS tones (ferrite-app's `slot::CTCSS`, which this crate doesn't depend on).
fn ferrite_app_ctcss() -> [u16; 50] {
    [
        670, 693, 719, 744, 770, 797, 825, 854, 885, 915, 948, 974, 1000, 1035, 1072, 1109, 1148, 1188, 1230, 1273, 1318,
        1365, 1413, 1462, 1514, 1567, 1598, 1622, 1655, 1679, 1713, 1738, 1773, 1799, 1835, 1862, 1899, 1928, 1966, 1995,
        2035, 2065, 2107, 2181, 2257, 2291, 2336, 2418, 2503, 2541,
    ]
}

/// TSRCH-13: the DCS match reads the code out of each rotation instead of building every code's word; for every
/// 9-bit code, every rotation and both polarities it gives what the one-codeword-per-code search gave.
#[test]
fn heard_dcs_matches_the_full_search() {
    let all: Vec<u16> = (0..0o1000).collect();
    let reference = |w: u32| {
        for inverted in [false, true] {
            let mut x = if inverted { !w & 0x7F_FFFF } else { w & 0x7F_FFFF };
            for _ in 0..23 {
                if let Some(&c) = all.iter().find(|&&c| bk4829::dcs_codeword(c, false) == x) {
                    return Some(0x8000 | if inverted { 0x4000 } else { 0 } | c);
                }
                x = (x >> 1) | (x & 1) << 22;
            }
        }
        None
    };
    for &c in &all {
        for inv in [false, true] {
            let mut w = bk4829::dcs_codeword(c, inv);
            for _ in 0..23 {
                assert_eq!(chip::heard_tone(None, Some(w), &[], &all), reference(w), "code {c:o} inv {inv} word {w:06x}");
                w = (w >> 1) | (w & 1) << 22;
            }
        }
    }
}

/// SND-11: a local beep on a chip: REG_30 to stock's beep value 0x0302 (AF and the tone DSP only: no PLL, no PA,
/// no mic), the tone generator at stock's beep gain; a sleeping chip is woken first and put back to sleep after;
/// a receiving one goes back to receive with its REG_48.
#[test]
fn beep_never_keys_and_restores() {
    let mut b = Bk4829::new({
        let mut r = RecBus::new();
        r.regs[reg::AF_GAIN as usize] = 0x1234;
        r
    });
    let gain = chip::beep_start(&mut b, 1000, 0x17, true);
    assert_eq!(gain, 0x1234);
    chip::beep_end(&mut b, gain, true);
    let bus = b.release();
    assert_eq!(bus.writes.first().map(|w| w.0), Some(reg::LDO), "woken first");
    let p = bus.writes.iter().find(|w| w.0 == reg::POWER).unwrap().1;
    assert_eq!(p, bk4829::POWER_BEEP);
    assert_eq!(p & 0x80F8 & !0x0300, 0, "no PLL, PA or mic bits in 0x{p:04X}");
    assert_eq!(bus.last(reg::AF_GAIN), Some(0x1234), "REG_48 back");
    assert_eq!(bus.writes.last().map(|w| w.0), Some(reg::LDO), "asleep again last");
    let mut b = bk();
    let g = chip::beep_start(&mut b, 1000, 0x17, false);
    chip::beep_end(&mut b, g, false);
    assert_eq!(b.release().writes.last(), Some(&(reg::POWER, 0xBFF1)), "receive again");
}

/// SQL-15, SQL-16.1, SQL-16.3: the squelch thresholds go to REG_78 and REG_4F as computed (stock's 0x0801E64C), and
/// nothing else.
#[test]
fn squelch_writes_stocks_registers() {
    let mut b = bk();
    chip::squelch(&mut b, (0x5955, 0x2827));
    assert_eq!(b.release().writes, [(reg::SQ_RSSI, 0x5955), (reg::SQ_NOISE, 0x2827)]);
}

/// SQL-17, SQL-17.1: a scan step or the strip's look sets REG_4E's open delay (bits 15:12) to 0, ordinary receive
/// back to stock's 3 (0x3815, its start-up value); the close delay (11:8) and the glitch threshold stay.
#[test]
fn open_delay_fast_for_scans_stock_otherwise() {
    let mut b = bk();
    b.write(reg::SQ_GLITCH_OPEN, 0x3815);
    chip::open_delay(&mut b, true);
    let mut r = b.release();
    assert_eq!(r.last(reg::SQ_GLITCH_OPEN), Some(0x0815), "the close delay kept");
    r.writes.clear();
    let mut b = Bk4829::new(r);
    chip::open_delay(&mut b, false);
    assert_eq!(b.release().writes, [(reg::SQ_GLITCH_OPEN, 0x3815)], "stock's start-up value again");
}

/// TSRCH-10.1: the counter's chips sit mid-band, chip 0 below 300 MHz and chip 1 above, each in its own front end,
/// and use stock's frequency scan (REG_32 0x8245 on, 0x8244 off; the reading from REG_0D/0E in 10 Hz units).
#[test]
fn counter_uses_stocks_frequency_scan() {
    assert_eq!(Sel::rx(chip::COUNTER_TUNE[0]).chip(), 0);
    assert_eq!(Sel::rx(chip::COUNTER_TUNE[1]).chip(), 1);
    assert!(!Sel::rx(chip::COUNTER_TUNE[0]).relay() && !Sel::rx(chip::COUNTER_TUNE[1]).relay(), "never the relay");
    let mut b = bk();
    b.freq_scan(true);
    b.freq_scan(false);
    let mut bus = b.release();
    assert_eq!(bus.writes, [(reg::FREQ_SCAN, 0x8245), (reg::FREQ_SCAN, 0x8244)]);
    bus.regs[reg::SCAN_HI as usize] = 0x0001;
    bus.regs[reg::SCAN_LO as usize] = 0x6550;
    let mut b = Bk4829::new(bus);
    assert_eq!(b.freq_scan_result(), Some(((1 << 16) | 0x6550) * 10));
}

/// DEC-19: SAME on the chip's own modem: REG_58 0x03C4 (receive mode 000, bandwidth 010, gain 3), the
/// 520.83 baud rate word, the sync bytes `ZCZC`, set up on a chip tuned to the weather channels (162.400-162.550).
#[test]
fn same_modem_setup() {
    let mut b = bk();
    chip::same_arm(&mut b);
    let bus = b.release();
    assert_eq!(bus.first(reg::FSK_MODE).map(|i| bus.writes[i].1), Some(0x03C4 | 1), "0x03C4 with the modem enable");
    assert_eq!((chip::SAME_FSK.tone_word, bus.last(reg::TONE2_FREQ)), (5377, Some(5377)));
    let sync = [bus.last(reg::FSK_SYNC01), bus.last(reg::FSK_SYNC23)];
    assert_eq!(sync, [Some(0x5AC2), Some(0x5AC2)], "ZCZC as the chip wants it");
    assert!(chip::WX.contains(&(mhz(162) + 55_000)) && !chip::WX.contains(&(mhz(162) + 56_000)));
}

/// TX-7.1, TX-23.1 (the chip keyed with its read-back before the PA): key with the microphone (REG_30 0xC1FE), read
/// back REG_30, the frequency (REG_38/39) and REG_3B, then stock's REG_36 (0x7F High, 0x57 otherwise) and its
/// read-back; all match: keyed.
#[test]
fn key_reads_back_before_the_pa() {
    for (high, r36) in [(true, 0x7F), (false, 0x57)] {
        let (mut b, want) = keyed_ready();
        assert_eq!(chip::key_mic(&mut b, high, &want), Ok(()));
        let bus = b.release();
        assert_eq!(bus.last(reg::POWER), Some(chip::REG30_TX_MIC));
        assert_eq!(bus.last(reg::PA), Some(r36));
        assert!(bus.first(reg::PA).unwrap() > bus.writes.iter().rposition(|w| w.0 == reg::POWER).unwrap());
    }
}

fn setup(freq: Freq10, narrow: bool, tone: u16) -> chip::TxSetup {
    chip::TxSetup { freq, narrow, tone, tone_dev: 0x46, mic: 0x24 }
}

/// TX-45 (TX audit M7): the TX set-up writes stock's REG_43 with every TX frequency (0x3028 wide, 0x4008 narrow), over
/// whatever the chip held (here the reset value 0x4048, as a cross-band split's TX chip may), out of receive
/// first (REG_30 0) and the sub-tone after.
#[test]
fn tx_setup_writes_the_bandwidth() {
    for (narrow, r43) in [(false, 0x3028), (true, 0x4008)] {
        let mut b = bk();
        b.write(reg::RF_FILTER, 0x4048);
        chip::tx_setup(&mut b, &setup(VHF, narrow, 885));
        let bus = b.release();
        assert_eq!(bus.last(reg::RF_FILTER), Some(r43));
        assert_eq!(bus.regs[reg::FREQ_LO as usize] as u32 | (bus.regs[reg::FREQ_HI as usize] as u32) << 16, VHF);
        assert!(bus.first(reg::POWER).unwrap() < bus.writes.iter().rposition(|w| w.0 == reg::RF_FILTER).unwrap(), "out of receive first");
        assert_eq!(bus.regs[reg::SUBTONE as usize] & 0x8000, 0x8000, "CTCSS sent");
    }
    let mut b = bk();
    chip::tx_setup(&mut b, &setup(UHF, false, 0));
    assert_eq!(b.release().last(reg::RF_FILTER), Some(0x3028));
}

/// TX-46, VOX-6, VOX-15, TONE-1.2, TX audit F1, F2, F7, F14: the key-up writes stock's registers from the start-up
/// values: REG_7D 0xE912 to stock's default 0xE924 (bit 5 is the 6-bit field's top bit; Mic gain 5), REG_40 0x34D2 to
/// stock's 0x350A, REG_47 0x6142 (stock's TX route), REG_51 with the calibration's CTCSS byte (0x9046 on the black
/// radio) or its DCS byte in REG_51's low 7 bits, and REG_30 0 before any of them.
#[test]
fn tx_setup_writes_stocks_registers() {
    let mut b = bk();
    b.write(reg::MIC, 0xE912);
    b.write(reg::DEVIATION, 0x34D2);
    chip::tx_setup(&mut b, &setup(VHF, false, 885));
    let bus = b.release();
    assert_eq!(bus.last(reg::MIC), Some(0xE924));
    assert_eq!(bus.last(reg::DEVIATION), Some(0x350A));
    assert_eq!(bus.last(reg::AF_OUT), Some(0x6142));
    assert_eq!(bus.last(reg::SUBTONE), Some(0x9046));
    assert_eq!(bus.first(reg::POWER), bus.writes.iter().position(|w| w.0 == reg::POWER));
    let p0 = bus.writes.iter().position(|w| *w == (reg::POWER, 0)).unwrap();
    assert!(p0 < bus.first(reg::AF_OUT).unwrap() && p0 < bus.first(reg::FREQ_LO).unwrap(), "REG_30 0 first, as stock");
    let mut b = bk();
    b.write(reg::MIC, 0xE912);
    chip::tx_setup(&mut b, &chip::TxSetup { tone: 0x8000 | 0o23, tone_dev: 0x3E, mic: ferrite_core::vox::MIC_DATA, ..setup(UHF, false, 0) });
    let bus = b.release();
    assert_eq!(bus.last(reg::SUBTONE).map(|v| v & 0x7F), Some(0x3E), "the DCS byte");
    assert_eq!(bus.last(reg::MIC), Some(0xE934), "a data key-up: V0.29's 0x34");
}

/// TONE-20.1, TONE-20.2 (decision 140): a DCS transmission as stock V0.27's 0x0800D3CC, on the recording bus: REG_51
/// 0xA000 for a normal code and 0x8000 for an inverted one (bit 13, "transmit negative code", set for normal), with the
/// calibration's DCS byte, REG_07 the 134.4 Hz bit rate, and the same normal codeword in both (D023: 0x0813, then
/// 0x8763). Bit 13 clear with the normal codeword went out as 047N, 023 inverted, on the bench.
#[test]
fn dcs_tx_polarity_is_stocks() {
    for (inverted, r51) in [(false, 0xA03E), (true, 0x803E)] {
        let mut b = bk();
        let tone = 0x8000 | if inverted { 0x4000 } else { 0 } | 0o23;
        chip::tx_setup(&mut b, &chip::TxSetup { tone, tone_dev: 0x3E, ..setup(VHF, false, 0) });
        let bus = b.release();
        assert_eq!(bus.last(reg::SUBTONE), Some(r51), "inverted {inverted}");
        let at = bus.first(reg::SUBTONE).unwrap();
        assert_eq!(bus.writes[at + 1..at + 4], [(0x07, 0x0AD7), (0x08, 0x0813), (0x08, 0x8763)], "inverted {inverted}");
    }
}

/// TONE-20.3, TONE-20.4 (decision 142): DCS receive as stock V0.27 (0x0800A50C -> 0x0800D3CC): tuning a D023N slot
/// writes REG_51 0xA000 (bit 13 set), D023I 0x8000, both with the normal codeword (0x0813, 0x8763); and the squelch
/// decision, as stock's 0x0801EAC4, opens a normal code only on REG_0C bit 14 and an inverted one only on bit 15 (on
/// air Ferrite's D023N opened to a stock radio's D023I too). CTCSS opens on bit 10 alone, no tone always.
#[test]
fn dcs_rx_polarity_is_stocks() {
    for (inverted, r51) in [(false, 0xA000 | 51), (true, 0x8000 | 51)] {
        let mut b = bk();
        let tone = 0x8000 | if inverted { 0x4000 } else { 0 } | 0o23;
        chip::tune(&mut b, &RxSetup { freq: VHF, narrow: false, am: false, rx_tone: tone, rx_tail: false, data: false });
        let bus = b.release();
        assert_eq!(bus.last(reg::SUBTONE), Some(r51), "inverted {inverted}");
        let at = bus.first(reg::SUBTONE).unwrap();
        assert_eq!(bus.writes[at + 1..at + 4], [(0x07, 0x0AD7), (0x08, 0x0813), (0x08, 0x8763)], "inverted {inverted}");
    }
    let st = |r0c: u16| {
        let mut b = bk();
        b.write(reg::STATUS, r0c);
        b.status()
    };
    let (n, i) = (0x8000 | 0o23, 0xC000 | 0o23);
    let (pos, neg, ctc, none) = (st(1 << 14), st(1 << 15), st(1 << 10), st(0));
    assert!(chip::tone_heard(n, &pos) && !chip::tone_heard(n, &neg), "N opens on bit 14 only");
    assert!(chip::tone_heard(i, &neg) && !chip::tone_heard(i, &pos), "I opens on bit 15 only");
    assert!(!chip::tone_heard(n, &ctc) && !chip::tone_heard(i, &none));
    assert!(chip::tone_heard(885, &ctc) && !chip::tone_heard(885, &pos) && !chip::tone_heard(885, &neg));
    assert!(chip::tone_heard(0, &none));
}

/// TONE-10.4, TONE-19.3 (decision 141): a keyed tone sequence (end tone, CW ID) starts silent: the tone off, REG_40 at
/// the tones' 0x43C (top four bits kept), then tone transmit through stock's REG_30 step; the caller then waits
/// `TONE_LEAD_MS` for the generator to settle. The end puts the tone off and REG_40 back to stock's 0x50A.
#[test]
fn keyed_tone_sequence_lead_in_and_deviation() {
    let mut b = bk();
    b.write(reg::DEVIATION, 0x350A);
    let n0 = b.release().writes.len();
    let mut b = bk();
    b.write(reg::DEVIATION, 0x350A);
    chip::tones_begin(&mut b, false);
    let bus = b.release();
    assert_eq!(
        bus.writes[n0..],
        [(reg::TONE, 0), (reg::DEVIATION, 0x343C), (reg::POWER, 0x0200), (reg::POWER, 0xC3FA)],
        "silent, the tones' REG_40, then tone transmit"
    );
    assert_eq!(ferrite_core::tonegen::TONE_LEAD_MS, 200, "past the swell's 90 % at about 190 ms");
    let mut b = Bk4829::new(bus);
    chip::tones_end(&mut b);
    let bus = b.release();
    let n = bus.writes.len();
    assert_eq!(bus.writes[n - 2..], [(reg::TONE, 0), (reg::DEVIATION, 0x350A)]);
}

/// TERM-33.9, TERM-33.11, TERM-33.12 (decision 132): a `flat` test key-up turns off the TX HPF/LPF/pre-emphasis
/// (REG_2B[2:0] = 111), a `data` one only the TX HPF (REG_2B[2:0] = 100 exactly, whatever was there; the other bits
/// kept); both turn off MIC AGC (REG_19[15]) and the ALC (REG_7D[6]) on top of the data key-up's REG_7D 0x34, clear a
/// TX mute (REG_50[15]), touch nothing else, and the unkey's restore puts all four back as they were.
#[test]
fn tx_test_filters_and_restore() {
    use ferrite_core::txfilters::Filters;
    for (f, r2b, want2b) in [(Filters::Flat, 0x0000, 0x0007), (Filters::Data, 0x0000, 0x0004), (Filters::Data, 0x0703, 0x0704), (Filters::Flat, 0x0702, 0x0707)] {
        let mut b = bk();
        for (r, v) in [(reg::AF_FILTERS, r2b), (0x19, 0x1041), (reg::MIC, 0xE912), (0x50, 0xBB18)] {
            b.write(r, v);
        }
        chip::tx_setup(&mut b, &chip::TxSetup { mic: ferrite_core::vox::MIC_DATA, ..setup(VHF, false, 0) });
        let n0 = b.release();
        let n = n0.writes.len();
        let mut b = Bk4829::new(n0);
        let was = chip::tx_test_filters(&mut b, f);
        assert_eq!(was, chip::TestRegs { r2b, r19: 0x1041, r7d: 0xE934, r50: 0xBB18 });
        let bus = b.release();
        assert_eq!(bus.last(reg::AF_FILTERS), Some(want2b), "{f:?} from {r2b:#06x}");
        assert_eq!(bus.last(0x19), Some(0x9041));
        assert_eq!(bus.last(reg::MIC), Some(0xE974), "ALC off, 0x34 kept");
        assert_eq!(bus.last(0x50), Some(0x3B18), "never muted: the DAC rides the mic input");
        let touched: Vec<u8> = bus.writes[n..].iter().map(|w| w.0).collect();
        assert!(touched.iter().all(|r| [reg::AF_FILTERS, 0x19, reg::MIC, 0x50].contains(r)), "{touched:?}");
        let mut b = Bk4829::new(bus);
        chip::tx_test_restore(&mut b, &was);
        let bus = b.release();
        assert_eq!([bus.last(reg::AF_FILTERS), bus.last(0x19), bus.last(reg::MIC), bus.last(0x50)], [Some(r2b), Some(0x1041), Some(0xE934), Some(0xBB18)]);
    }
}

/// TERM-33.15, TERM-33.16: after the `data` set-up, `bypass` sets REG_47<0> on the key-up's 0x6142 and clears the
/// soft limiter (REG_2F<7:5>, the de-emphasis gain <13:8> kept); an equaliser raises REG_7D's mic sensitivity by
/// its steps (0x34 + 11 = 0x3F, ALC off kept; never past it) and sets REG_40's deviation field (top bits kept);
/// nothing else is written, and the restore puts the four back.
#[cfg(feature = "bench")]
#[test]
fn tx_test_extra_and_restore() {
    use ferrite_core::txeq::Eq;
    for (bypass, eq) in [(true, Eq::Off), (true, Eq::Eq), (false, Eq::Eq), (true, Eq::Eq8)] {
        let mut b = bk();
        b.write(0x2F, 0x9890);
        chip::tx_setup(&mut b, &chip::TxSetup { mic: ferrite_core::vox::MIC_DATA, ..setup(VHF, false, 0) });
        chip::tx_test_filters(&mut b, ferrite_core::txfilters::Filters::Data);
        let bus = b.release();
        let n = bus.writes.len();
        let (r47, r40, r7d) = (bus.last(reg::AF_OUT).unwrap(), bus.last(reg::DEVIATION).unwrap(), bus.last(reg::MIC).unwrap());
        assert_eq!((r47, r40 & 0x0FFF, r7d & 0x7F), (0x6142, 0x50A, 0x74));
        let mut b = Bk4829::new(bus);
        let was = chip::tx_test_extra(&mut b, bypass, eq.mic_steps(), (eq != Eq::Off).then(|| eq.deviation()));
        assert_eq!(was, chip::ExtraRegs { r47, r2f: 0x9890, r40, r7d });
        let bus = b.release();
        assert_eq!(bus.last(reg::AF_OUT), Some(if bypass { 0x6143 } else { 0x6142 }));
        assert_eq!(bus.last(0x2F), Some(if bypass { 0x9810 } else { 0x9890 }));
        let mic = match eq { Eq::Off => 0x34, Eq::Eq | Eq::Eq8 => 0x3F };
        assert_eq!(bus.last(reg::MIC), Some((r7d & !0x3F) | mic), "{eq:?}");
        assert_eq!(bus.last(reg::DEVIATION), Some((r40 & 0xF000) | if eq == Eq::Eq8 { 0x57A } else { 0x50A }));
        assert!(bus.writes[n..].iter().all(|w| [reg::AF_OUT, 0x2F, reg::MIC, reg::DEVIATION].contains(&w.0)), "{:?}", &bus.writes[n..]);
        let mut b = Bk4829::new(bus);
        chip::tx_test_extra_restore(&mut b, &was);
        let bus = b.release();
        assert_eq!([bus.last(reg::AF_OUT), bus.last(0x2F), bus.last(reg::DEVIATION), bus.last(reg::MIC)], [Some(r47), Some(0x9890), Some(r40), Some(r7d)]);
    }
}

/// APRS-14.3 (decision 135): an APRS burst's key-up, on the recording bus: the data key-up's REG_7D 0x34, then the
/// `data` set-up (`aprs_filters` for a data key-up, in every build): REG_2B[2:0] = 100, MIC AGC off (REG_19[15]), ALC off
/// (REG_7D[6]), REG_50's TX mute clear; the unkey's restore puts the four back as the key-up found them.
#[test]
fn aprs_burst_takes_the_data_set_up() {
    let f = ferrite_core::txfilters::aprs_filters(true).expect("an APRS burst changes the set-up");
    let mut b = bk();
    for (r, v) in [(reg::AF_FILTERS, 0x0000), (0x19, 0x1041), (reg::MIC, 0xE924), (0x50, 0x3B18)] {
        b.write(r, v);
    }
    chip::tx_setup(&mut b, &chip::TxSetup { mic: ferrite_core::vox::MIC_DATA, ..setup(VHF, false, 0) });
    let was = chip::tx_test_filters(&mut b, f);
    let bus = b.release();
    assert_eq!((bus.last(reg::AF_FILTERS), bus.last(0x19), bus.last(reg::MIC)), (Some(0x0004), Some(0x9041), Some(0xE974)));
    assert_eq!(bus.last(0x50), Some(0x3B18));
    let mut b = Bk4829::new(bus);
    chip::tx_test_restore(&mut b, &was);
    let bus = b.release();
    assert_eq!([bus.last(reg::AF_FILTERS), bus.last(0x19), bus.last(reg::MIC), bus.last(0x50)], [Some(0), Some(0x1041), Some(0xE934), Some(0x3B18)]);
    // A voice key-up changes none of it.
    assert_eq!(ferrite_core::txfilters::aprs_filters(false), None);
}

#[cfg(feature = "bench")]
/// TERM-33.5 `afsk-bk`: the begin switches the keyed chip to tone transmit through stock's step (REG_30 0x0200 wide
/// or 0 narrow, then 0xC3FA: the mic ADC off) and only then sets tone 1 to the space tone at the gain (REG_71,
/// REG_72 0, REG_70 0x8000 | gain << 8); a change writes REG_71 alone; the end turns the tones off (REG_70 0) and
/// leaves REG_30 at tone transmit for the unkey; REG_47 (the key-up's AF route), REG_50, REG_2B, REG_7D and
/// REG_19 are never written.
#[test]
fn afsk_bk_registers() {
    use ferrite_core::txtest::{bk_gain, BK_MARK_HZ, BK_SPACE_HZ};
    for narrow in [false, true] {
        let mut b = bk();
        chip::afsk_bk_begin(&mut b, BK_SPACE_HZ, bk_gain(100), narrow);
        let bus = b.release();
        let step = if narrow { 0 } else { 0x0200 };
        assert_eq!(
            bus.writes,
            [(reg::POWER, step), (reg::POWER, 0xC3FA), (reg::TONE1_FREQ, bk4829::tone_word(2200)), (reg::TONE2_FREQ, 0), (reg::TONE, 0x8000 | 66 << 8)],
            "narrow {narrow}"
        );
        assert_eq!(bk4829::tone_word(2200), 22714);
        let mut b = Bk4829::new(bus);
        chip::afsk_bk_tone(&mut b, BK_MARK_HZ);
        chip::afsk_bk_tone(&mut b, BK_SPACE_HZ);
        let bus = b.release();
        assert_eq!(bus.writes[5..], [(reg::TONE1_FREQ, 12389), (reg::TONE1_FREQ, 22714)]);
        let mut b = Bk4829::new(bus);
        chip::afsk_bk_end(&mut b);
        let bus = b.release();
        assert_eq!(bus.writes[7..], [(reg::TONE, 0)]);
        assert_eq!(bus.last(reg::POWER), Some(0xC3FA), "the mic stays off until the unkey");
        assert!(bus.writes.iter().all(|w| [reg::POWER, reg::TONE, reg::TONE1_FREQ, reg::TONE2_FREQ].contains(&w.0)), "{:?}", bus.writes);
    }
    // LEVEL scales the gain into REG_70[14:8].
    let mut b = bk();
    chip::afsk_bk_begin(&mut b, BK_SPACE_HZ, bk_gain(150), false);
    assert_eq!(b.release().last(reg::TONE), Some(0x8000 | 99 << 8));
}

/// A chip tuned to 146.520 MHz with a REG_3B of its own, and what TX-7 expects of it.
fn keyed_ready() -> (Bk4829<RecBus>, chip::TxExpect) {
    let f = mhz(146) + 52_000;
    let mut b = bk();
    b.set_freq(f * 10);
    b.write(0x3B, 0x1234);
    (b, chip::TxExpect { freq: f, reg3b: 0x1234, narrow: false })
}

/// TX-7.2: any one register wrong refuses the key-up, naming it, before REG_36's level is left standing: REG_30 not
/// keyed, the synthesized frequency not the one written, REG_3B changed since start-up, REG_36 not taking. The
/// caller then goes to the safe state with the PA at zero (`failed_read_back_never_ramps`).
#[test]
fn any_wrong_read_back_refuses() {
    use chip::ReadBack;
    let stuck = |r: u8, v: u16| {
        let (b, want) = keyed_ready();
        let mut bus = b.release();
        bus.stuck = Some((r, v));
        (Bk4829::new(bus), want)
    };
    let (mut b, want) = stuck(reg::POWER, 0x0200);
    assert_eq!(chip::key_mic(&mut b, true, &want), Err(ReadBack::Power));
    assert!(b.release().last(reg::PA).is_none(), "no REG_36 after a failed read-back");
    let (mut b, want) = stuck(reg::FREQ_LO, 0x0001);
    assert_eq!(chip::key_mic(&mut b, true, &want), Err(ReadBack::Freq));
    assert!(b.release().last(reg::PA).is_none());
    let (mut b, mut want) = keyed_ready();
    want.freq += 1; // the word in the chip isn't the one asked for
    assert_eq!(chip::key_mic(&mut b, true, &want), Err(ReadBack::Freq));
    let (mut b, want) = stuck(0x3B, 0x1235);
    assert_eq!(chip::key_mic(&mut b, true, &want), Err(ReadBack::Xtal));
    assert!(b.release().last(reg::PA).is_none());
    let (mut b, want) = stuck(reg::PA, 0x0000);
    assert_eq!(chip::key_mic(&mut b, false, &want), Err(ReadBack::Bias));
}

/// TONE-13, TONE-19.1, TONE-19.3, DTMF-7, DTMF-8, DTMF-11: while keyed a tone burst or a DTMF symbol (A-D too) switches
/// REG_30 to tone transmit (stock's mode 4, the microphone off) with the sidetone as asked and the tones' REG_40;
/// letting go returns to the microphone with its read-back and stock's REG_40.
#[test]
fn keyed_tones_switch_the_source() {
    let mut b = bk();
    assert_eq!(chip::keyed_tone(&mut b, Some(1750), None, true, false), (true, true));
    let bus = b.release();
    let tone = bus.last(reg::POWER).unwrap();
    assert_ne!(tone, chip::REG30_TX_MIC, "tone transmit, not the microphone");
    assert_eq!(bus.last(reg::TONE1_FREQ), Some(bk4829::tone_word(1750)));
    // TX audit M8: the burst (and the end tone and CW ID, through the same tone_tx) at V0.29's gain 0x17, REG_70
    // 0x9700, not V0.27's 0xBC00; DTMF pairs keep 0xC5C5 (both versions).
    assert_eq!(bus.last(reg::TONE), Some(0x9700));
    assert_eq!(bus.last(reg::DEVIATION).map(|v| v & 0xFFF), Some(bk4829::tx::TONE_DEVIATION), "TONE-19: the tones' REG_40");
    for d in ['5', 'A', 'D', '#'] {
        let mut b = bk();
        assert_eq!(chip::keyed_tone(&mut b, None, Some(d), false, false), (true, true), "{d}");
        let bus = b.release();
        assert_eq!(bus.last(reg::POWER), Some(tone));
        assert_eq!(bus.last(reg::TONE), Some(0xC5C5));
        let pair = bk4829::tx::dtmf_pair(d).unwrap();
        assert_eq!((bus.last(reg::TONE1_FREQ), bus.last(reg::TONE2_FREQ)), (Some(bk4829::tone_word(pair.0)), Some(bk4829::tone_word(pair.1))));
    }
    let mut b = bk();
    assert_eq!(chip::keyed_tone(&mut b, None, None, false, true), (false, true));
    let bus = b.release();
    assert_eq!((bus.last(reg::TONE), bus.last(reg::POWER)), (Some(0), Some(chip::REG30_TX_MIC)));
    assert_eq!(bus.last(reg::DEVIATION).map(|v| v & 0xFFF), Some(bk4829::tx::TX_DEVIATION), "REG_40 back to stock's 0x50A");
    assert_eq!(bus.last(reg::AF_OUT), Some(0x6142), "back to the key-up's AF route (F14)");
    let n = bus.writes.len();
    assert_eq!(bus.writes[n - 2..], [(reg::POWER, 0), (reg::POWER, chip::REG30_TX_MIC)], "narrow: through 0, as stock (F3)");
}

/// TONE-6, TONE-8 (tail elimination at the unkey): no tone sends the 55 Hz tail, DCS the 134.4 Hz turn-off code
/// (REG_52 phase 0), CTCSS the chosen phase shift; on the chip, REG_52 bit 15 with the phase in bits 14:13.
#[test]
fn tail_follows_the_tone() {
    use bk4829::tx::Tail;
    // TONE-5, TX audit F7: the 55 Hz tone at the calibration's CTCSS byte, as stock (the black radio's 0x46).
    assert_eq!(chip::tail(0, 2, 0x46), Tail::Tone55 { dev: 0x46 });
    assert_eq!(chip::tail(0x8000 | 0o23, 2, 0x46), Tail::Shift { phase: 0 });
    assert_eq!(chip::tail(885, 2, 0x46), Tail::Shift { phase: 2 });
    assert_eq!(chip::tail(885, 0, 0x46), Tail::Tone55 { dev: 0x46 });
    let mut b = bk();
    b.tail_tx(Some(chip::tail(885, 3, 0x46)));
    let r52 = b.release().last(reg::CTCSS_THRESH).unwrap();
    assert_eq!((r52 >> 15, r52 >> 13 & 3), (1, 3));
}

/// GPS-5, GPS-22: PB10 driven low while any chip transmits and handed back to the UART after unkey, once each.
#[test]
fn gps_pb10_while_keyed() {
    let mut g = GpsPower::default();
    assert_eq!(g.step(0, true, true, false).pb10_low, None);
    assert_eq!(g.step(10, true, true, true).pb10_low, Some(true));
    assert_eq!(g.step(20, true, true, true).pb10_low, None);
    assert_eq!(g.step(30, true, true, false).pb10_low, Some(false));
}

/// GPS-22, GPS-5 (TX audit F15): the Radio task drives PB10 low before the key-up, as stock's PTT press; the GPS
/// task's next look keeps it low if keyed, and hands it back to the UART if the key-up was refused.
#[test]
fn gps_pb10_before_the_key_up() {
    let mut g = GpsPower::default();
    g.forced_low();
    assert_eq!(g.step(0, true, true, true).pb10_low, None, "keyed: already low");
    assert_eq!(g.step(10, true, true, false).pb10_low, Some(false), "back after the unkey");
    g.forced_low();
    assert_eq!(g.step(20, true, true, false).pb10_low, Some(false), "a refused key-up: back at once");
    assert_eq!(g.step(30, true, true, false).pb10_low, None);
}

/// TX-43.1, TX-43.2 (TX audit F9): the cable sends nothing while keyed, except in the `tx-dry` build (the bench reads
/// the chips over it with the PA drive at 0). SND-14: nothing at all with a speaker mic connected (PC10 is its bias).
#[test]
fn cable_held_while_keyed() {
    use crate::cable::may_send;
    assert!(may_send(false, false, false) && !may_send(true, false, false));
    assert!(may_send(true, true, false) && may_send(false, true, false));
    // SND-14 (decision 146): a speaker mic connected: nothing, keyed or not, dry or not.
    for (k, d) in [(false, false), (true, false), (false, true), (true, true)] {
        assert!(!may_send(k, d, true), "keyed {k} dry {d}");
    }
}

/// GPS-1.1, GPS-1.2, GPS-4, GPS-25: powered with 0 6 1; off at once for a signal 0 6 8 covers, and back on only after
/// `QUIET_MS` (2 s) without one, so a burst of short signals doesn't cycle it; 0 6 1 off keeps it off.
#[test]
fn gps_power_around_signals() {
    let mut g = GpsPower::default();
    assert_eq!(g.step(5_000, true, true, false).power, Some(true));
    assert_eq!(g.step(5_100, true, false, false).power, Some(false));
    assert_eq!(g.step(5_200, true, true, false).power, None, "not straight back");
    assert_eq!(g.step(5_100 + QUIET_MS - 1, true, true, false).power, None);
    assert_eq!(g.step(5_100 + QUIET_MS, true, true, false).power, Some(true));
    assert_eq!(g.step(9_000, false, true, false).power, Some(false));
    assert_eq!(g.step(20_000, false, true, false).power, None);
}

/// GPS-1.1, GPS-1.2: 0 6 1 Off then On powers the module at once; the firmware's `allowed` (`GpsCfg::powered`) is
/// false while GPS is off, which isn't a signal, so it doesn't start GPS-4's 2 s wait (the bench: PA8 still
/// high 1 s after On).
#[test]
fn gps_on_powers_at_once() {
    let mut g = GpsPower::default();
    assert_eq!(g.step(5_000, true, true, false).power, Some(true));
    assert_eq!(g.step(6_000, false, false, false).power, Some(false));
    assert_eq!(g.step(9_000, false, false, false).power, None);
    assert_eq!(g.step(9_010, true, true, false).power, Some(true), "On: powered on the first look");
}

/// LINK-14, LINK-25 (decision 76): PB9 follows 0 9 2 1, and while Bluetooth is on it is set again after each
/// transmission, as stock; never while it is off.
#[test]
fn bluetooth_pb9() {
    let mut b = BtPower::default();
    assert_eq!(b.step(false, false).power, None, "off by default");
    assert_eq!(b.step(true, false).power, Some(true));
    assert!(!b.step(true, true).reassert);
    assert!(b.step(true, false).reassert, "after the unkey");
    assert!(!b.step(true, false).reassert, "once");
    assert_eq!(b.step(false, true).power, Some(false));
    assert!(!b.step(false, false).reassert, "never while off");
}

/// SAFE-1, CAL-1, CAL-2, INST-7, INST-8, SAFE-2, UPD-7: the SPI driver's guard refuses any program or erase that
/// touches calibration (0x00F000-0x00FFFF) or the update area (0x300000 up), checks the erase unit (a whole 4 KB
/// sector, aligned) rather than the range asked for, and keeps programs inside one page; everything else below the
/// update area is writable (layout 3, decision 333); the bring-up build writes nowhere.
#[test]
fn spi_flash_guard() {
    use crate::flash::{erase_ok, program_ok, PAGE, SECTOR};
    use ferrite_core::layout::{CALIBRATION, UPDATE_AREA, WRITABLE};
    assert_eq!((PAGE as usize, SECTOR as usize), (ferrite_store::PAGE as usize, ferrite_store::SECTOR as usize));
    let ok = &WRITABLE[..];
    for a in [CALIBRATION.start, CALIBRATION.start + 0x800, UPDATE_AREA.start, UPDATE_AREA.end - SECTOR] {
        assert!(!program_ok(ok, a, 16), "program at 0x{a:06X}");
        assert!(!erase_ok(ok, a & !(SECTOR - 1)), "erase at 0x{a:06X}");
    }
    assert!(erase_ok(ok, CALIBRATION.start - SECTOR) && erase_ok(ok, CALIBRATION.end), "the sectors either side of calibration are layout 3's");
    assert!(!erase_ok(ok, 0x02_D800), "unaligned: refused, not rounded");
    assert!(!program_ok(ok, 0x02_D0F8, 16), "across a page boundary");
    assert!(!program_ok(ok, 0x02_D000, 0) && !program_ok(ok, 0x02_D000, PAGE + 1));
    assert!(!program_ok(ok, u32::MAX - 4, 8), "no wrap-around");
    for r in ok {
        assert!(erase_ok(ok, r.start) && erase_ok(ok, r.end - SECTOR) && program_ok(ok, r.start, PAGE));
        assert!(!erase_ok(ok, r.end) || ok.iter().any(|o| o.start == r.end));
    }
    assert!(!program_ok(&[], 0x02_D000, 16) && !erase_ok(&[], 0x02_D000), "bring-up: nowhere");
}

/// SAFE-14.1, BOOT-3, KEY-8, TX-19, TX-42 (the released-once part): a PTT held at power-on keeps transmit shut until
/// every PTT input has read released once; after that it stays open for the power-on.
#[test]
fn ptt_held_at_power_on() {
    use crate::ptt::{names, PttGuard, PTT_A, PTT_B, PTT_EXT, STUCK_MS};
    let mut g = PttGuard::default();
    assert!(!g.look(PTT_A) && !g.look(PTT_A), "held since boot");
    assert!(g.look(0), "released once");
    assert!(g.look(PTT_A), "pressed again later: that's a real PTT");
    assert_eq!(g.stuck(STUCK_MS + 10), 0, "let go before 1 s: never stuck");
    // Held past 1 s: stuck, by name, until each is let go; a press after boot is never stuck.
    let mut g = PttGuard::default();
    g.look(PTT_A | PTT_EXT);
    assert_eq!(g.stuck(STUCK_MS - 1), 0, "not yet");
    assert!(!g.look(PTT_A | PTT_EXT));
    assert_eq!(g.stuck(STUCK_MS), PTT_A | PTT_EXT);
    g.look(PTT_A | PTT_B);
    assert_eq!(g.stuck(1500), PTT_A, "the side PTT let go; PTT B pressed after boot isn't stuck");
    assert!(!g.open(), "PTT A still held since boot");
    g.look(0);
    assert_eq!((g.stuck(2000), g.open()), (0, true));
    g.look(PTT_A);
    assert_eq!(g.stuck(2500), 0, "pressed again: a real PTT, not stuck");
    let mut b = [0u8; 40];
    let n = names(PTT_A | PTT_EXT, &mut b);
    assert_eq!(&b[..n], b"PTT A + side PTT");
}

/// PWR-5: Battery Save's sleep and wake on a chip, as stock: sleep writes REG_30 = 0 then REG_37 = 0x1D00; wake
/// REG_37 = 0x9D1F, then the receive sequence (`chip::tune` wakes first and turns receive on last).
#[test]
fn sleep_and_wake_as_stock() {
    let mut b = bk();
    b.sleep();
    let bus = b.release();
    assert_eq!(bus.writes, [(reg::POWER, 0), (reg::LDO, 0x1D00)]);
    let mut b = bk();
    chip::tune(&mut b, &RxSetup { freq: VHF, narrow: false, am: false, rx_tone: 0, rx_tail: false, data: false });
    let bus = b.release();
    assert_eq!(bus.writes.first(), Some(&(reg::LDO, 0x9D1F)));
    assert_eq!(bus.writes.last(), Some(&(reg::POWER, 0xBFF1)));
}

/// A BK4829 bus and a delay writing to one log, so a wait's place among the register writes shows: a wait is
/// logged as (`WAIT`, ms).
#[derive(Clone, Default)]
struct Log(std::rc::Rc<core::cell::RefCell<Vec<(u8, u16)>>>);

const WAIT: u8 = 0xFF;

impl bk4829::Bus for Log {
    fn write(&mut self, reg: u8, value: u16) {
        self.0.borrow_mut().push((reg, value));
    }
    fn read(&mut self, _reg: u8) -> u16 {
        0
    }
    fn delay_us(&mut self, us: u32) {
        self.0.borrow_mut().push((WAIT_US, us as u16));
    }
}

/// The driver's busy waits (us), as (`WAIT_US`, us).
const WAIT_US: u8 = 0xFE;

impl chip::Delay for Log {
    async fn ms(&mut self, ms: u32) {
        self.0.borrow_mut().push((WAIT, ms as u16));
    }
}

impl Log {
    fn take(&self) -> Vec<(u8, u16)> {
        core::mem::take(&mut *self.0.borrow_mut())
    }
}

const RX: RxSetup = RxSetup { freq: VHF, narrow: false, am: false, rx_tone: 0, rx_tail: false, data: false };

/// PWR-21.1, PWR-21.5 (decision 119), SQL-18.9: from sleep, REG_37's supplies on, the wake settle, then the receive
/// set-up in stock's order: the AF route muted, REG_37 for receive (0x9F1F), receive off, ..., receive on last; the
/// wake time is kept.
#[test]
fn wake_settles_before_the_receive_setup() {
    use ferrite_core::settle::Settle;
    let log = Log::default();
    let (mut bk, mut d) = (Bk4829::new(log.clone()), log.clone());
    let at = block_on(chip::tune_settled(&mut bk, &mut d, &RX, Settle { wake_ms: 2, sleep_ms: 1 }, None, 100));
    assert_eq!(at, 100);
    let w = log.take();
    assert_eq!(w[..5], [(reg::LDO, 0x9D1F), (WAIT, 2), (reg::AF_OUT, 0x6042), (reg::LDO, 0x9F1F), (reg::POWER, 0)], "{w:x?}");
    assert_eq!(w.iter().rposition(|e| e.0 == reg::POWER && e.1 == 0), Some(4), "receive on only at the end");
    // Stock's 50 us between the two REG_30 writes (decision 119, the TX audit).
    assert_eq!(w[w.len() - 3..], [(reg::POWER, 0x0200), (WAIT_US, 50), (reg::POWER, 0xBFF1)]);
    assert_eq!(w.iter().filter(|e| e.0 == WAIT).count(), 1);
}

/// PWR-21.2, PWR-21.4: a chip whose supplies came on a tick early (the hop's `due`), or that is long awake, is tuned
/// with no wait and no wake write (REG_37 gets only the receive value, SQL-18.9); one woken 1 ms ago waits the rest;
/// 0 is stock's: no wait at all.
#[test]
fn wake_settle_counts_from_the_early_wake() {
    use ferrite_core::settle::Settle;
    let s = Settle { wake_ms: 2, sleep_ms: 1 };
    let log = Log::default();
    let (mut bk, mut d) = (Bk4829::new(log.clone()), log.clone());
    assert_eq!(block_on(chip::tune_settled(&mut bk, &mut d, &RX, s, Some(90), 100)), 90);
    let w = log.take();
    assert!(!w.iter().any(|e| e.0 == WAIT || *e == (reg::LDO, 0x9D1F)), "{w:x?}");
    assert!(w.contains(&(reg::LDO, 0x9F1F)), "receive's REG_37");
    block_on(chip::tune_settled(&mut bk, &mut d, &RX, s, Some(99), 100));
    assert_eq!(log.take().iter().filter(|e| e.0 == WAIT).collect::<Vec<_>>(), [&(WAIT, 1)]);
    block_on(chip::tune_settled(&mut bk, &mut d, &RX, Settle::STOCK, None, 100));
    let w = log.take();
    assert_eq!(w[0], (reg::LDO, 0x9D1F));
    assert!(!w.iter().any(|e| e.0 == WAIT), "stock's: back to back");
}

/// PWR-21.1: to sleep, the AF route muted, REG_30 off, the sleep settle, then REG_37's supplies off; with 0 the same
/// writes with no wait (stock's REG_30 0 then REG_37 0x1D00, after the mute).
#[test]
fn sleep_mutes_then_settles() {
    use ferrite_core::settle::Settle;
    let log = Log::default();
    let (mut bk, mut d) = (Bk4829::new(log.clone()), log.clone());
    block_on(chip::sleep_settled(&mut bk, &mut d, Settle { wake_ms: 2, sleep_ms: 1 }));
    assert_eq!(log.take(), [(reg::AF_OUT, 0x6042), (reg::POWER, 0), (WAIT, 1), (reg::LDO, 0x1D00)]);
    block_on(chip::sleep_settled(&mut bk, &mut d, Settle::STOCK));
    assert_eq!(log.take(), [(reg::AF_OUT, 0x6042), (reg::POWER, 0), (reg::LDO, 0x1D00)]);
}

/// PWR-21.1: a receiving chip retuned (scan, a new frequency) is muted before its frequency changes.
#[test]
fn retune_mutes_first() {
    let log = Log::default();
    let mut bk = Bk4829::new(log.clone());
    chip::tune_rest(&mut bk, &RX);
    assert_eq!(log.take()[0], (reg::AF_OUT, 0x6042));
}

#[cfg(feature = "bench")]
/// TERM-34 (decision 139) `txtest suite` on the recording bus, for a key-up with no sub-tone, CTCSS and DCS: every
/// step of the table in turn, then the hold, then the end. At each step the chip is on the mic (REG_30 0xC1FE) for
/// DAC1's and the sub-tones' steps and in tone transmit (0xC3FA) for the generator's; REG_2B[2:0], MIC AGC and the
/// ALC as the step's set-up; REG_40's low bits the step's, its top four the key-up's; a sub-tone only in section 6
/// (REG_51 bit 15, with the calibration's gain for its kind), the tone generator on only for its own steps
/// (REG_70). Only those registers (and the tone words, REG_07/08 and REG_52 the sub-tones rewrite) are written.
/// After the end every register is back as the key-up left it but the tone words (REG_71/72, unused until the next
/// tone sets them) and, with no DCS at the key-up, REG_08 (DCS's codeword, unused with REG_51 off; the receive
/// tune after the unkey writes it).
#[test]
fn suite_steps_and_restore() {
    use ferrite_core::txsuite::{Src, HOLD, REG40_STOCK, STEPS};
    use ferrite_core::txtest::Filters;
    assert_eq!(REG40_STOCK, bk4829::tx::TX_DEVIATION);
    let (ctcss_dev, dcs_dev) = (0x3C, 0x2E);
    for tone in [0, 885, 0x8000 | 0o152] {
        let mut b = bk();
        for (r, v) in [(reg::AF_FILTERS, 0x0700), (0x19, 0x1041), (reg::MIC, 0xE912), (0x50, 0xBB18), (reg::DEVIATION, 0x7123), (reg::CTCSS_THRESH, 0x0292)] {
            b.write(r, v);
        }
        let s = chip::TxSetup { tone, tone_dev: 0x46, mic: ferrite_core::vox::MIC_DATA, ..setup(VHF, false, 0) };
        chip::tx_setup(&mut b, &s);
        b.tx_on(bk4829::tx::Source::Mic, true);
        let before = b.release();
        let keyed = before.regs;
        let n0 = before.writes.len();
        let mut b = Bk4829::new(before);
        let was = chip::suite_begin(&mut b, s.tone, s.tone_dev);
        assert_eq!(was.r40, 0x750A);
        assert_eq!(b.read(reg::SUBTONE), 0, "the slot's sub-tone off for the suite");
        let mut prev = None;
        for (i, st) in STEPS.iter().chain([HOLD].iter()).enumerate() {
            let dev = if st.src.subtone() & 0x8000 != 0 { dcs_dev } else { ctcss_dev };
            chip::suite_step(&mut b, &was, prev.as_ref(), st, false, dev);
            let r = |b: &mut Bk4829<RecBus>, x| b.read(x);
            let want30 = if st.src.tones() { 0xC3FA } else { chip::REG30_TX_MIC };
            assert_eq!(r(&mut b, reg::POWER), want30, "step {i}");
            let low = r(&mut b, reg::AF_FILTERS);
            match st.filters {
                Filters::Voice => assert_eq!((low, r(&mut b, 0x19), r(&mut b, reg::MIC), r(&mut b, 0x50)), (0x0700, 0x1041, 0xE934, 0xBB18), "step {i}"),
                f => {
                    assert_eq!(low, if f == Filters::Flat { 0x0707 } else { 0x0704 }, "step {i}");
                    assert_eq!((r(&mut b, 0x19), r(&mut b, reg::MIC), r(&mut b, 0x50)), (0x9041, 0xE974, 0x3B18), "step {i}");
                }
            }
            assert_eq!(r(&mut b, reg::DEVIATION), 0x7000 | st.reg40, "step {i}");
            let r51 = r(&mut b, reg::SUBTONE);
            match st.src {
                Src::Sub(t) => {
                    assert_eq!(r51 & 0x8000, 0x8000, "step {i}");
                    assert_eq!(r51 & 0x7F, if t & 0x8000 != 0 { dcs_dev } else { ctcss_dev } as u16, "step {i}");
                }
                _ => assert_eq!(r51 & 0x8000, 0, "step {i}: a sub-tone outside section 6"),
            }
            let r70 = r(&mut b, reg::TONE);
            match st.src {
                Src::BkFlags => assert_eq!(r70, 0x8000 | 66 << 8, "step {i}"),
                Src::BkTone(hz) => {
                    assert_eq!(r70, 0x8000 | (bk4829::tx::TONE_TX_GAIN as u16) << 8, "step {i}");
                    assert_eq!(r(&mut b, reg::TONE1_FREQ), bk4829::tone_word(hz as u32), "step {i}");
                }
                _ => assert_eq!(r70 & 0x8000, 0, "step {i}: the generator on"),
            }
            prev = Some(*st);
        }
        chip::suite_end(&mut b, &was);
        let bus = b.release();
        let allowed = [reg::AF_FILTERS, 0x19, reg::MIC, 0x50, reg::DEVIATION, reg::POWER, reg::SUBTONE, reg::CTC, reg::CDCSS, reg::CTCSS_THRESH, reg::TONE, reg::TONE1_FREQ, reg::TONE2_FREQ];
        assert!(bus.writes[n0..].iter().all(|w| allowed.contains(&w.0)), "{:x?}", bus.writes[n0..].iter().find(|w| !allowed.contains(&w.0)));
        for r in 0..128u8 {
            if r != reg::TONE1_FREQ && r != reg::TONE2_FREQ && !(r == reg::CDCSS && tone & 0x8000 == 0) {
                assert_eq!(bus.regs[r as usize], keyed[r as usize], "tone {tone:#x}: register {r:#04x} not back");
            }
        }
    }
}

/// APRS-55, APRS-55.1, SQL-19.5 (decision 309): the APRS slot's FM tune turns the receive 300 Hz high-pass and de-emphasis off (REG_2B
/// 0x0500: the 3 kHz low-pass stays on), and `rx_data` the noise gate (REG_28<8>) and AFC (REG_73<4> = 1) off, the
/// registers' other bits kept; a voice slot's tune writes REG_2B 0 and puts both back, and nothing at all when they
/// are already as voice wants them. AM never gets the data set-up.
#[test]
fn aprs_slot_tune_writes_the_data_rx_setup() {
    let fresh = || {
        let mut bus = RecBus::new();
        (bus.regs[0x28], bus.regs[0x73]) = (0x0B40, 0x4682); // the start-up's REG_28, REG_73's reset value
        Bk4829::new(bus)
    };
    let fm = |data| RxSetup { freq: mhz(144) + 39_000, narrow: false, am: false, rx_tone: 0, rx_tail: false, data };
    let mut b = fresh();
    chip::tune(&mut b, &fm(true));
    chip::rx_data(&mut b, true);
    let bus = b.release();
    assert_eq!((bus.last(reg::AF_FILTERS), bus.last(0x28), bus.last(0x73)), (Some(0x0500), Some(0x0A40), Some(0x4692)));
    // Back to a voice slot on the same chip: everything as before.
    let mut b = Bk4829::new(bus);
    chip::tune(&mut b, &fm(false));
    chip::rx_data(&mut b, false);
    let bus = b.release();
    assert_eq!((bus.last(reg::AF_FILTERS), bus.regs[0x28], bus.regs[0x73]), (Some(0), 0x0B40, 0x4682));
    // A voice tune on a chip already set for voice writes neither REG_28 nor REG_73.
    let mut b = fresh();
    chip::tune(&mut b, &fm(false));
    chip::rx_data(&mut b, false);
    let bus = b.release();
    assert_eq!((bus.last(reg::AF_FILTERS), bus.last(0x28), bus.last(0x73)), (Some(0), None, None));
    let mut b = fresh();
    chip::tune(&mut b, &RxSetup { am: true, ..fm(true) });
    let bus = b.release();
    let r2b: Vec<u16> = bus.writes.iter().filter(|w| w.0 == reg::AF_FILTERS).map(|w| w.1).collect();
    assert_eq!(r2b, [0x0500], "AM: stock's mode 6 only");
}

#[cfg(feature = "bench")]
/// TERM-42: `bk rxaf` after an FM tune sets REG_2B bits 10:8 over the tune's 0x0000 and keeps the other bits.
#[test]
fn rx_af_override_after_fm_tune() {
    let mut b = bk();
    chip::tune(&mut b, &RxSetup { freq: mhz(145) + 49_500, narrow: false, am: false, rx_tone: 0, rx_tail: false, data: false });
    assert_eq!(b.read(reg::AF_FILTERS), 0x0000);
    b.write(reg::AF_FILTERS, 0x0005);
    chip::rx_af_override(&mut b, 0x0700);
    assert_eq!(b.read(reg::AF_FILTERS), 0x0705);
    chip::rx_af_override(&mut b, 0x0400);
    assert_eq!(b.read(reg::AF_FILTERS), 0x0405);
}

#[cfg(feature = "bench")]
/// TERM-43: `fsk rx 2400` sets the modem up as stock's FSK 2400 (REG_58 0x00C9, REG_72 0x60CA, REG_70 0x00AC, REG_5C
/// 0x5665) with the bench frame's length (32 bytes: REG_5D 0x1F00), 8 preamble bytes and the 4-byte reset sync word,
/// no scramble (REG_59 0x0078), then clears the FIFO and turns receive on; `nocrc` clears REG_5C bit 6.
#[test]
fn fsk_bench_rx_registers() {
    use ferrite_core::fskbench::{Cfg, Speed};
    let c = Cfg { speed: Speed::parse(b"2400").unwrap(), crc: true };
    let mut b = bk();
    chip::fsk_bench_rx(&mut b, &c);
    let bus = b.release();
    for (r, v) in [(reg::FSK_MODE, 0x00C9), (reg::TONE2_FREQ, 0x60CA), (reg::TONE, 0x00AC), (reg::FSK_CRC, 0x5665), (reg::FSK_LEN, 0x1F00), (reg::FSK_SYNC01, 0x85CF), (reg::FSK_SYNC23, 0xAB45)] {
        assert_eq!(bus.last(r), Some(v), "REG_{r:02X}");
    }
    let n = bus.writes.len();
    assert_eq!(bus.writes[n - 3..n - 1], [(reg::FSK_CTRL, 0x4078), (reg::FSK_CTRL, 0x1078)], "clear, then receive");
    let mut b = bk();
    chip::fsk_bench_rx(&mut b, &Cfg { crc: false, ..c });
    assert_eq!(b.release().last(reg::FSK_CRC), Some(0x5625));
}

#[cfg(all(feature = "bench", feature = "tx"))]
/// TERM-43 `fsk tx`: tone transmit through stock's step, the modem set up, frame 0's 16 words into REG_5F after the
/// FIFO clear and before transmit on (REG_59 bit 11); the end leaves transmit off in the modem and the modem off.
#[test]
fn fsk_bench_tx_registers() {
    use ferrite_core::fskbench::{payload, words, Cfg, Speed};
    let c = Cfg { speed: Speed::parse(b"ffsk1800").unwrap(), crc: true };
    let mut b = bk();
    chip::fsk_bench_tx_begin(&mut b, &c, false, 0);
    let bus = b.release();
    assert_eq!(bus.writes[..2], [(reg::POWER, 0x0200), (reg::POWER, 0xC3FA)]);
    assert_eq!(bus.last(reg::FSK_MODE), Some(0x3FC3));
    let fifo: Vec<u16> = bus.writes.iter().filter(|w| w.0 == reg::FSK_FIFO).map(|w| w.1).collect();
    assert_eq!(fifo, words(&payload(0)));
    let n = bus.writes.len();
    assert_eq!(bus.writes[n - 1], (reg::FSK_CTRL, 0x0878), "transmit on last");
    let mut b = Bk4829::new(bus);
    chip::fsk_bench_tx_end(&mut b, &c);
    let bus = b.release();
    assert_eq!((bus.last(reg::FSK_CTRL), bus.last(reg::TONE), bus.last(reg::FSK_MODE)), (Some(0x0078), Some(0), Some(0)));
}
