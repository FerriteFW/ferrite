//! The ACARS screen (chapter 27b, ACARS-4, ACARS-7) with frames heard, keys, the knob, time and the radio's state.
#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../src/harness.rs"]
mod harness;

fuzz_target!(|data: &[u8]| harness::acars_screen(data));
