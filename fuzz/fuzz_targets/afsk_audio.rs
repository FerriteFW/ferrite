//! The radio's AFSK decoder as the DSP task runs it (ferrite_core::decoders::Decoder::afsk at 24 kHz: three
//! slicers, repair, FX.25), on any audio: input bytes are little-endian 16-bit samples, fed in blocks. FX.25 blocks
//! it hands over are corrected as the APRS task does.
#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../src/harness.rs"]
mod harness;

fuzz_target!(|data: &[u8]| harness::afsk_audio(data));
