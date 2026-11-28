//! ACARS audio (chapter 27b, ACARS-11) through the receiver, and every frame it gives into the screen.
#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../src/harness.rs"]
mod harness;

fuzz_target!(|data: &[u8]| {
    harness::acars_audio(data);
});
