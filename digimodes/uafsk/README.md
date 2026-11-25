# uafsk

**A 1200-baud AFSK (Bell 202) packet modem for microcontrollers, with FX.25, in about 1.7 KB of RAM (1.4 KB without FX.25).**

uafsk turns audio into checked HDLC frames and frames into audio: the modulation APRS and VHF packet radio use (1200
Hz mark, 2200 Hz space, 1200 bit/s). It is `no_std`, allocates nothing, and uses integers only. What is inside a frame
(AX.25 addresses, APRS) belongs to a protocol crate, not the modem; the one exception is a sanity check on the
address field of a repaired frame.

Part of [udigimodes](../../README.md), digital-mode libraries for microcontrollers. Written for
[Ferrite](../../README.md), free firmware for the Radtel RT-950 Pro handheld. The radio's BK4829
has FFSK modems for commercial signalling (1200/1800 Hz and 1200/2400 Hz) but not Bell 202, so APRS needs a software
modem.

Status: **prototype, 0.1**. Measured on the WA8LMF TNC test CD and on off-air recordings; not yet run on a radio.

## At a glance

| | Dire Wolf 1.7 (`atest`, default) | uafsk `Demodulator<3, 1>` (default) |
| --- | --- | --- |
| RAM | | **1,668 B** (FX.25 included; 1,348 B before it) |
| CPU at 9600 S/s (Cortex-M4, emulator) | | **3.5 MIPS**: 2.2% at 240 MHz, 4.4% at 120 MHz (3.3 MIPS before FX.25) |
| WA8LMF track 1, 9600 S/s | 993 | **997** |
| WA8LMF track 1, 44.1 kS/s | 1005 | **1007** |
| WA8LMF track 2, 9600 S/s | 953 | **998** |
| WA8LMF track 2, 44.1 kS/s | 982 | **1009** |
| Off-air recording 1 (45 min), 12 kS/s | 353 | **393** (333 in common) |
| Off-air recording 2 (25 min), 12 kS/s | 367 | **386** (363 in common) |

Counts are frames with a correct FCS. Every frame uafsk decodes and Dire Wolf does not passes AX.25 address checks
and reads as ordinary APRS: real frames Dire Wolf missed. Full table:
[docs/measurements.md](docs/measurements.md).

### Off-air recordings

Two off-air recordings of a busy APRS channel (16 kS/s; not redistributed), against Dire Wolf 1.7 `atest` on the
same files; 12 kS/s is the recordings resampled to the radio's rate. Much of recording 1 is steeply tilted: the
1200 Hz tone 2.5 to 4 times the 2200 Hz one.

| Recording | Dire Wolf | uafsk 12 kS/s | uafsk 16 kS/s | Without repair, 12 kS/s | One-bit window, 12 kS/s |
| --- | --- | --- | --- | --- | --- |
| 1 (45 min) | 353 | **393** (333 also Dire Wolf's) | 406 (336) | 360 (319) | 205 (203) |
| 2 (25 min) | 367 | **386** (363) | 389 (365) | 379 (362) | 332 (332) |

The correlation window made the biggest difference: 1.4 bits instead of one narrows each tone's filter. Repair
(below) added the rest. A band-pass prefilter, smoothing the tone amplitudes, tapered windows, other slicer gains and
faster or slower normalisers each moved the counts by a few frames at most, or lost frames.

## Configurations

The type sets the cost: `Demodulator<S, C>` runs `S` slicers (1 to 3) in each of `C` chains (1 or 2).

| Type | RAM | Track 1 / track 2 at 9600 S/s | Use |
| --- | --- | --- | --- |
| `Demodulator<1, 1>` | about 0.9 KB | 990 / 981 | The smallest |
| **`Demodulator<3, 1>`** (default) | **1,668 B** | **997 / 998** | A radio |
| `Demodulator<3, 2>` | about 3.0 KB | 1006 / 999 | When RAM allows; helps on flat (discriminator) audio |

Most of the RAM is the frame buffers: one 330-byte buffer per slicer; FX.25 adds a 16-byte tag hunter per slicer
and one 264-byte block buffer they share (`Config::fx25: false` leaves them idle). `Config::repair: 0` turns repair off (60 bytes
and 3% of the CPU less, the counts of the previous release).

## Using it

```rust
use uafsk::demod::Config;
use uafsk::{Demodulator, Modulator, TxConfig};

// Receive: one call per audio sample (9600 to 48000 samples/s; 12000 on the radio).
let mut rx: Demodulator = Demodulator::new(Config::new(9600));
for &x in samples {
    rx.push(x, |frame| {
        // `frame` is the AX.25 frame without its FCS, already checked.
    });
}

// Transmit: an iterator of samples, phase-continuous, no buffer.
let cfg = TxConfig { rate_hz: 38400, ..TxConfig::default() };
for x in Modulator::new(frame, cfg) {
    dac_write(x);
}
```

### FX.25

`Config::fx25` (on by default) runs an FX.25 tag hunter ([ufx25](../ufx25/README.md)) beside each slicer's
deframer on the same bits, and the slicers share one block buffer: the first to find a tag claims it and collects
the Reed-Solomon block (a lower slicer finding the same tag within 16 bits takes over). The demodulator only
collects: correcting a block costs 0.27 to 1.03 M instructions, too much inside an audio callback. The complete
block waits in the buffer until released; correct it where there is time (another task), or call `decode_fx25` to
do it there and then. A tag that comes while the buffer is busy is skipped (`fx25_skipped` counts them):

```rust
for &x in block { rx.push(x, |frame| heard(frame)); }
if let Some(b) = rx.pending_fx25() {         // &mut Fx25Block (264 bytes, Copy): send a copy to another task
    queue.send(*b);                           // or decode it in place later and release then
    rx.release_fx25();                        // (`take_fx25` does both)
}
// elsewhere:
if let Some(f) = b.decode() {
    if !f.duplicate { heard(f.frame) }        // f.corrected bytes repaired; f.check: 16, 32 or 64
}
```

A clean FX.25 transmission is also a plain AX.25 frame: `push` reports it first, and the block carries that FCS so
the corrected copy says `duplicate`. Of the slicers that find the tag, the first one's block is kept: on Dire Wolf's
`gen_packets -n 100` files (16, 32, 64 check bytes) that gives 41, 44 and 46 frames, the same as decoding every
slicer's block (plain AX.25 alone, repair included: 36, 37, 38); a bitwise majority of the three blocks did worse
in white noise (126 against 135 of 150 frames). One shared buffer instead of
one per slicer (1,668 B instead of 2,460 B) decodes exactly the same on those files, with no tag skipped there.
Per-sample cost: 367 instructions instead of 345 (hunting for tags only behind flags); the off-air and WA8LMF counts
are unchanged.

To send FX.25, `Modulator::from_bits(ufx25::Encoder::new(frame, 16)?.bits(pre, post), cfg)`; `uafsk::ufx25` is
re-exported.

`rx.busy()` is carrier detect: true while 1200-baud data is on the channel, for a TNC deciding when it may
transmit. Each slicer scores whether signal transitions fall where its bit clock expects them (data: nearly always;
noise: one time in four), as Dire Wolf does. On the WA8LMF tracks it is busy for 99.9% of every decoded frame and
within 93 ms of a preamble starting; on noise, silence, steady tones and a voice-like sweep, 0%.
Away from any frame either decoder found it is busy 17-20% of the time, in stretches of about 100 ms to 1 s:
collisions and packets nobody decodes on that busy channel, as far as can be told.

`Config::new` turns the 1200 Hz high-pass on, which de-emphasised audio (a receiver's speaker output) needs. Set
`highpass: false` for flat audio.

## How it works

The demodulator follows Dire Wolf's (WB2OSZ), in integers:

1. **High-pass**: two first-order sections at 1200 Hz lift 2200 Hz against 1200 Hz and remove hum and CTCSS. On
   track 2 (de-emphasised) this alone raised decodes from 638 to 941.
2. **Correlate** each sample with 1200 Hz and 2200 Hz oscillators, in phase and quadrature, summed over 1.4 bits
   (`Config::window`). Running sums over a ring of the window's samples: each sample adds its products and takes
   away those of the sample leaving the ring, recomputed from the oscillators' phase a window ago. A window longer
   than a bit filters each tone more narrowly; 1.3 to 1.5 bits are close, beyond 1.6 the bits blur together.
3. **Normalise each tone separately** with a peak and valley tracker, so twist matters less.
4. **Slice** with up to three space gains (0, -3.5 and +3.5 dB). Each slicer has its own clock and deframer; a frame
   found by more than one is reported once.
5. **Recover the clock** with a digital PLL pulled to 85% at each tone change.
6. **NRZI, unstuffing, FCS** in [`hdlc::Deframer`](src/hdlc.rs).
7. **Repair** (`Config::repair`, 3 by default): each tone decision comes with its margin, how far the slicer was
   from the other tone. A frame that closes with a bad FCS most often has exactly one wrong tone decision (two
   adjacent wrong data bits), anywhere in the frame. The deframer keeps the 3 least confident decisions of each
   frame and, on a bad FCS, tries flipping them (7 combinations, fewest flips first; the CRC's linearity makes each
   try a 16-bit XOR). It takes the first combination that fixes the FCS, only if the bit stuffing stays as it was
   and the AX.25 address field is sane. Frames whose wrong tone was not among the 3 stay lost.

The modulator is an iterator: NRZI, bit stuffing and FCS from [`hdlc::Framer`](src/hdlc.rs) (or any bits, with
`Modulator::from_bits`), a phase-continuous oscillator from udsp's sine table.

## Testing

```sh
cargo test -p uafsk                                                   # round trips at 9.6-48 kS/s, noise, twist
../testdata/fetch/wa8lmf.sh                                           # WA8LMF CD tracks 1 and 2 (Docker)
cargo test --release -p uafsk --test report -- --ignored --nocapture  # the WA8LMF table, with Dire Wolf
```

CPU: `tools/cpu-bench` builds the demodulator for a Cortex-M4 and counts its instructions in an emulator (`run.py`).

## Licence

[PolyForm Noncommercial 1.0.0](../../LICENSE.md).
