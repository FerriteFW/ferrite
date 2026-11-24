//! The HDLC deframer (flags, bit stuffing, FCS, frame length) bit by bit, plain and with soft-decision repair. Each
//! input byte gives one bit (bit 0) and a confidence (the other 7 bits); the first byte sets the repair depth.
#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../src/harness.rs"]
mod harness;

fuzz_target!(|data: &[u8]| harness::hdlc_bits(data));
