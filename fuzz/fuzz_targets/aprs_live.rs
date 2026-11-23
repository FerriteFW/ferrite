//! The radio's receive path for a decoded frame: AprsLive::receive (the AX.25 parse, digipeating, the heard list,
//! messages and acks, our own echoes) then a tick (acks, digipeats and beacons queued), as the APRS task calls them.
//! Input: frames separated by 0xC0 (as KISS would delimit them); the digipeater and our callsign are on, so every
//! branch can run.
#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../src/harness.rs"]
mod harness;

fuzz_target!(|data: &[u8]| harness::aprs_live(data));
