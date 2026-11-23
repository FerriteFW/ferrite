//! The FX.25 receiver bit by bit (tag hunter, collector, correction, deframing): each input byte gives 8 bits, least
//! significant first. The first byte sets the tag errors accepted (0 to 15).
#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../src/harness.rs"]
mod harness;

fuzz_target!(|data: &[u8]| harness::fx25_bits(data));
