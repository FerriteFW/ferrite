//! ACARS frames (chapter 27b) through the parser and the RAM log that joins their blocks.
#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../src/harness.rs"]
mod harness;

fuzz_target!(|data: &[u8]| harness::acars(data));
