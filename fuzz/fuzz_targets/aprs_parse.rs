//! uaprs::parse on any information field, with any destination (Mic-E reads its latitude from the destination).
//! Input: one byte for the destination's length (low 3 bits, 0 to 7), the destination, then the information field.
//! Every packet type is reached through the type byte; third-party packets are parsed again, a few levels deep.
#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../src/harness.rs"]
mod harness;

fuzz_target!(|data: &[u8]| harness::aprs_parse(data));
