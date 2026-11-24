//! FX.25 correction and deframing of a received block (ufx25::decode, the APRS task's Fx25Block::decode path):
//! any bytes, any of the eleven codes, any tag error count. Input: one byte picking the code, then the block (padded
//! or cut to the code's length).
#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../src/harness.rs"]
mod harness;

fuzz_target!(|data: &[u8]| harness::fx25_block(data));
