//! Narrow AM and the 8.33 kHz grid (VFO-4.2, VFO-18.1; ferrite#189, ferrite#190).

use super::*;
use crate::slot::{band_mode, mode_am, mode_narrow, on_833, step_833, Item, NAM, SLOT_MODE_IDS, STEP_833, STEPS};
use ferrite_core::msg::Param;
use ferrite_core::txplan::Region;
use std::vec::Vec;

const MHZ118: Freq10 = 11_800_000;

/// VFO-18.1: three 8.33 kHz steps are exactly 25 kHz, thirty are 250 kHz, and down retraces up; each lands on the
/// grid (+0, +8.333, +16.667 kHz in every 25 kHz block, to the chip's 10 Hz).
#[test]
fn step_833_stays_on_the_grid() {
    assert_eq!(STEPS[STEP_833 as usize].1, "8.33 kHz");
    let mut f = MHZ118;
    let mut seen = Vec::new();
    for _ in 0..30 {
        f = step_833(f, 1);
        seen.push(f);
    }
    assert_eq!(&seen[..3], [MHZ118 + 833, MHZ118 + 1667, MHZ118 + 2500]);
    assert_eq!(f, MHZ118 + 25_000, "30 steps: 250 kHz, no drift");
    for g in &seen {
        assert!([0, 833, 1667].contains(&(g % 2500)), "{g} off the grid");
    }
    for _ in 0..30 {
        f = step_833(f, -1);
    }
    assert_eq!(f, MHZ118);
    assert_eq!(step_833(MHZ118 + 840, 1), MHZ118 + 1667, "from off the grid: the nearest channel, then on");
    assert_eq!(step_833(MHZ118 + 840, -1), MHZ118);
    assert_eq!((on_833(MHZ118 + 800), on_833(MHZ118 + 1700), on_833(MHZ118 + 2200)), (MHZ118 + 833, MHZ118 + 1667, MHZ118 + 2500));
}

/// VFO-18.1 on home: with the 8.33 kHz step the knob moves channel to channel; three turns are 118.025.
#[test]
fn knob_on_833_lands_on_channels() {
    let mut t = T::new();
    t.app.slots[0].vfo = MHZ118;
    t.app.slots[0].cfg_mut().step = STEP_833;
    for _ in 0..3 {
        t.knob(1);
    }
    assert_eq!(t.app.slots[0].vfo, MHZ118 + 2500);
    t.knob(-1);
    assert_eq!(t.app.slots[0].vfo, MHZ118 + 1667);
    assert!(t.cmds().contains(&RadioCmd::TuneSlot { slot: 0, freq: MHZ118 + 1667 }));
}

/// VFO-4.2: NAM is AM through the narrow filter; the knob's modes are FM, NFM, AM, NAM, WFM; the airband defaults
/// to NAM in every region (and before one is chosen); other bands are unchanged.
#[test]
fn nam_mode_and_airband_default() {
    assert_eq!(SLOT_MODE_IDS, [0, 1, 2, NAM, crate::slot::WFM]);
    assert_eq!(crate::slot::MODES[NAM as usize], "NAM");
    assert!(mode_am(NAM) && mode_narrow(NAM));
    assert!(mode_am(2) && !mode_narrow(2), "AM keeps the wide filter");
    assert!(mode_narrow(1) && !mode_am(1));
    for r in [None, Some(Region::Us), Some(Region::Canada), Some(Region::Uk), Some(Region::Europe), Some(Region::Japan), Some(Region::Australia), Some(Region::NewZealand), Some(Region::Other)] {
        assert_eq!(band_mode(crate::slot::BAND_AIR, r), NAM, "{r:?}");
        assert_eq!(band_mode(0, r), 2, "HF stays AM");
    }
}

/// VFO-4.2: the mode item steps through NAM and sends it with the narrow bandwidth; choosing the 8.33 kHz step on an
/// AM slot turns it to NAM (the step alone on any other mode is the UI's own).
#[test]
fn mode_item_and_step_833_pick_nam() {
    let mut c = crate::slot::SlotCfg { mode: 2, ..Default::default() };
    assert_eq!(Item::Mode.set(&mut c, 3, 0), Some((Param::Mode, NAM as i32)));
    assert!(c.narrow);
    c.mode = 2;
    c.narrow = false;
    assert_eq!(Item::Step.set(&mut c, STEP_833 as i32, 0), Some((Param::Mode, NAM as i32)));
    assert_eq!((c.mode, c.narrow, c.step), (NAM, true, STEP_833));
    c.mode = 0;
    assert_eq!(Item::Step.set(&mut c, STEP_833 as i32, 0), None, "FM keeps FM");
    assert_eq!(c.mode, 0);
}

/// VFO-4.2 at the App: a mode change to NAM sends the mode, then the narrow bandwidth (VFO-20).
#[test]
fn nam_reaches_the_radio_narrow() {
    let mut t = T::new();
    t.app.slots[0].vfo = MHZ118;
    t.app.slots[0].cfg_mut().mode = 2;
    t.cmds();
    t.app.change(Item::Mode, 1); // AM -> NAM
    let c = t.cmds();
    let i = c.iter().position(|c| *c == RadioCmd::SlotParam { slot: 0, param: Param::Mode, value: NAM as i32 }).expect("mode sent");
    assert!(c[i..].contains(&RadioCmd::SlotParam { slot: 0, param: Param::Bandwidth, value: 1 }), "{c:?}");
}

/// TX-20: NAM is receive only, as AM.
#[test]
fn nam_is_receive_only() {
    let mut t = T::new();
    t.app.radio.tx_built = true;
    t.app.slots[0].cfg_mut().mode = NAM;
    assert_eq!(crate::screens::transmit::refusal(&t.app, 0), Some(ferrite_core::txplan::Refusal::AmRxOnly));
}

/// VFO-18.1: on the 8.33 kHz step, typing three decimals within 50 Hz of a channel (118.008, 118.017) tunes that
/// channel; 118.025 is one as typed.
#[test]
fn typed_833_channels_snap() {
    for (keys, want) in [(&[1, 1, 8, 0, 0, 8][..], MHZ118 + 833), (&[1, 1, 8, 0, 1, 7][..], MHZ118 + 1667), (&[1, 1, 8, 0, 2, 5][..], MHZ118 + 2500)] {
        let mut t = T::new();
        t.app.slots[0].vfo = MHZ118 + 5000;
        t.app.slots[0].cfg_mut().step = STEP_833;
        for (i, &d) in keys.iter().enumerate() {
            if i == 3 {
                t.tap(Key::Star); // the point
            }
            t.tap(Key::Digit(d));
        }
        assert_eq!(t.app.slots[0].vfo, want, "{keys:?}");
    }
}
