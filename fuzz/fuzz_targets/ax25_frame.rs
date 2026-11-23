//! AX.25 frame decode (address field, control and PID, information field), the radio's TNC2 text of it, and the
//! TNC2 splitter and callsign reader. A parsed frame must write back to the same bytes, apart from the H bits of the
//! destination and source (which `Frame::parse` clears) and the reserved bits.
#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../src/harness.rs"]
mod harness;

fuzz_target!(|data: &[u8]| harness::ax25_frame(data));
