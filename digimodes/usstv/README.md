# usstv

Slow-scan television for microcontrollers: `no_std`, no allocation, f32.

- **Receive** (`Decoder`): audio in, the VIS read, image lines out as RGB565, 320 wide.
  - Slant (the sender's clock error) corrected from the sync pulses.
  - A mode can be forced when the VIS was missed: the image starts at the next sync.
- **Transmit** (`Encoder`): a picture to audio, VIS first, phase-continuous.
- **Modes:** Robot 36, Martin M1, Scottie S1, PD90, PD120.
  - Every image is 320 wide and at most 256 lines: PD120's 640 x 496 is averaged to 320 x 248.
- **Tests:** every mode through the encoder and back, clean, noisy, with a 300 ppm clock error, and forced.
  `cargo test -p usstv --test roundtrip pictures -- --ignored` writes decoded pictures to `target/sstv/`.

RAM: about 5 KB for the decoder. Part of Ferrite (`digimodes/`); same licence.
