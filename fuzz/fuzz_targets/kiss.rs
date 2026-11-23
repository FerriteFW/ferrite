//! The KISS decoder (cable and Bluetooth TNC), byte by byte, and the parameter commands it carries; data frames go
//! on to the AX.25 parser as the APRS task's KISS path does.
#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../src/harness.rs"]
mod harness;

fuzz_target!(|data: &[u8]| harness::kiss(data));
