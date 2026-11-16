//! # usstv
//!
//! Slow-scan television for microcontrollers: `no_std`, no allocation, f32 (a Cortex-M4F's FPU).
//!
//! - [`rx::Decoder`]: audio in (any rate from 8 kHz; Ferrite gives 24 kHz), the VIS read, image lines out as RGB565,
//!   [`mode::WIDTH`] wide, with the sender's clock error (slant) corrected from the sync pulses.
//! - [`tx::Encoder`]: a picture to audio, VIS first, phase-continuous.
//! - [`mode`]: Robot 36, Martin M1, Scottie S1, PD90 and PD120. Every image is given as 320 wide and at most 256
//!   lines: PD120's 640 x 496 is averaged to 320 x 248.

#![no_std]

pub mod color;
pub mod math;
pub mod mode;
pub mod rx;
pub mod tx;

pub use mode::{Mode, MODES};
pub use rx::{Decoder, Sink};
pub use tx::{Encoder, Picture};
