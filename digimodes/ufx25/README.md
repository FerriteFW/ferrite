# ufx25

**FX.25 forward error correction for AX.25 packet radio, for microcontrollers: 280 bytes of RAM per receiver.**

FX.25 puts an ordinary AX.25 frame (flags, bit stuffing, FCS) in a Reed-Solomon block behind a 64-bit correlation
tag. A plain AX.25 receiver still decodes the frame; an FX.25 receiver also repairs up to 8, 16 or 32 wrong bytes
(16, 32 or 64 check bytes). `no_std`, no allocation, integer only.

Part of Ferrite's digital modes ([`digimodes/`](..)).

Status: **prototype, 0.1**. Interoperates with Dire Wolf both ways; not yet run on a radio.

## Use

Transmit: wrap a frame (without FCS, as `uafsk::hdlc::Framer` takes it) and send the bits through NRZI and the
modulator.

```rust
let enc = ufx25::Encoder::new(&frame, 16)?;   // smallest block with 16 check bytes; Err(TooLong) -> send plain AX.25
for bit in enc.bits(30, 3) { /* NRZI + AFSK: a 0 changes the tone */ }
```

Receive: feed the same data bits the plain deframer gets (after NRZI), one `Receiver` per slicer.

```rust
let mut rx = ufx25::Receiver::new();
if let Some(d) = rx.push(bit) {
    // d.frame (no FCS), d.fcs, d.corrected (bytes repaired), d.tag_errors, d.mode
}
```

`push` corrects the block in the call that completes it. To do that elsewhere (an audio interrupt has no time for
it), use `push_raw`, which only collects, and `ufx25::decode` on the block later:

```rust
if let Some(mode) = rx.push_raw(bit) {
    let mut copy = [0u8; ufx25::MAX_BLOCK];
    copy[..mode.n as usize].copy_from_slice(rx.block());     // valid until the next tag, 64+ bits away
    // ... later, in another task:
    let d = ufx25::decode(&mut copy[..mode.n as usize], mode, rx.tag_errors());
}
```

Several slicers can share one block buffer: a `Hunter` per slicer (16 bytes: the tag hunt) and one `Collector`
(260 bytes: the block), claimed by the first slicer to find a tag. uafsk's `Demodulator` does this for you
(`Config::fx25`, `pending_fx25`).

Run it next to `uafsk::hdlc::Deframer`: a clean FX.25 transmission comes out of both (deduplicate by FCS; the FX.25
copy arrives after the check bytes, up to about 2,000 bits after the plain one), a damaged one only from ufx25.

`ufx25::rs::Rs` is the Reed-Solomon code on its own.

## The format

From the specification, "FX.25 Forward Error Correction Extension to AX.25 Link Protocol for Amateur Packet Radio"
(Stensat Group, draft 0.01.06, 1 September 2006; Table 1):

| Tag | Value | Block (n, k) | Check bytes |
| --- | --- | --- | --- |
| 0x01 | `B74DB7DF8A532F3E` | 255, 239 | 16 |
| 0x02 | `26FF60A600CC8FDE` | 144, 128 | 16 |
| 0x03 | `C7DC0508F3D9B09E` | 80, 64 | 16 |
| 0x04 | `8F056EB4369660EE` | 48, 32 | 16 |
| 0x05 | `6E260B1AC5835FAE` | 255, 223 | 32 |
| 0x06 | `FF94DC634F1CFF4E` | 160, 128 | 32 |
| 0x07 | `1EB7B9CDBC09C00E` | 96, 64 | 32 |
| 0x08 | `DBF869BD2DBB1776` | 64, 32 | 32 |
| 0x09 | `3ADB0C13DEAE2836` | 255, 191 | 64 |
| 0x0A | `AB69DB6A543188D6` | 192, 128 | 64 |
| 0x0B | `4A4ABEC4A724B796` | 128, 64 | 64 |

- On air: preamble flags, the tag (least significant bit first), the block (bytes least significant bit first:
  information, then check bytes), postamble flags; NRZI over all of it. The information part holds a flag, the
  bit-stuffed frame and FCS, a flag, and pad.
- The tags are the specification's Gold codes; a test regenerates all of them (and the reserved 0x00 and 0x40) from
  its LFSR description.

The specification does not give the Reed-Solomon details. Measured on Dire Wolf's `gen_packets` output (only its
programs were used, as black boxes) and confirmed by Dire Wolf decoding ours:

- Field polynomial x^8 + x^4 + x^3 + x^2 + 1 (0x11D), generator roots α^1 … α^r (first consecutive root 1, α = 2),
  first block byte = highest-degree coefficient.
- **Shortened blocks put the missing zeros between the data and the check bytes**, not in front: a (48, 32) block
  is the 255-byte codeword with 207 zero information bytes after the 32 data bytes. (Only this layout gives Dire
  Wolf's shortened blocks zero syndromes, and Dire Wolf reports corrected positions 239-254 in a 144-byte block.)
- Pad: Dire Wolf continues the flag bit pattern to the end of the information part (not byte-aligned 0x7E as the
  specification recommends); ufx25 does the same. Receivers ignore it either way.
- Tag tolerance: Dire Wolf accepts a tag with up to 8 wrong bits (8 decodes as FX.25, 9 does not); ufx25's default
  is the same (`Receiver::with_tag_errors` to change it). The tags are at least 32 bits apart.

## Interoperability with Dire Wolf

`tools/interop.sh [user@host]` makes the WAVs (12 kS/s) and runs both directions; the WAVs are in
the test data's `fx25/` (FerriteFW/testdata).

Dire Wolf to ufx25 (`gen_packets -X`, uafsk's tone detector with 3 slicers, NRZI, one `Receiver` per slicer):

| File | FX.25 frames decoded | With noise written over 3 bytes of each block |
| --- | --- | --- |
| 5 frames, `-X 16` | 5 of 5, all FCS good | 5 repaired (3-4 bytes each); plain AX.25: 0 |
| 5 frames, `-X 32` | 4 of 4 (the fifth is too long, sent plain) | 4 repaired; plain: 1 (the plain frame) |
| 5 frames, `-X 64` | 4 of 4 (likewise) | 4 repaired; plain: 1 |
| 5 frames, `-X 1` (Dire Wolf picks) | 5 of 5 | 5 repaired; plain: 0 |

`gen_packets -n 100` (one frame 100 times with rising noise), frames with a good FCS:

| Check bytes | Dire Wolf `atest` | ufx25 + uafsk | of which repaired by FX.25 | plain uafsk alone |
| --- | --- | --- | --- | --- |
| 16 | 44 | 41 | 14 | 31 |
| 32 | 42 | 44 | 16 | 36 |
| 64 | 41 | 46 | 18 | 37 |

ufx25 to Dire Wolf (`Encoder`, modulated in the test, `atest -d x`), 4 frames per file using 3 or 4 block sizes:

| Check bytes | Clean | t wrong bytes per block | t + 1 wrong bytes |
| --- | --- | --- | --- |
| 16 | 4 FX.25 decodes | 4, "fixed 8 errors" each | 0 (we reject them too) |
| 32 | 4 | 4, "fixed 16 errors" each | 0 |
| 64 | 4 | 4, "fixed 32 errors" each | 0 |

## Cost

Cortex-M4, `opt-level = "z"`, fat LTO, instruction counts from the emulator in `tools/cpu-bench` (`run.py`):

| | |
| --- | --- |
| RAM | `Receiver` 280 B: a `Hunter` (16 B: 64 + 32 bits of history, state) and a `Collector` (260 B: the 255-byte block); several slicers can share one `Collector`; `Encoder` 272 B (only while sending); decoding uses up to about 0.5 KB of stack, nothing in between |
| Flash | receive about 3.0 KB (1.9 KB code, 0.77 KB GF(256) tables, 0.3 KB generator and tag tables); transmit adds about 1.3 KB |
| Hunting for a tag | 53 instructions per bit on noise (243 before the flag gate): with 3 slicers at 1200 bit/s 0.19 MIPS |
| End of a 255-byte block | 0.27 M (16 check bytes, no errors) to 1.03 M instructions (64 check bytes, 32 errors): 1.7 to 6.4 ms at 240 MHz, all in the `push` call that completes the block |
| Encoding a 255-byte block | 0.32 M (16) to 0.57 M (64) instructions |

The block decode runs inside one `push`; from an audio path use `push_raw` and `decode` elsewhere.

Hunting looks for a tag only behind a flag: a tag follows the preamble's flags (or the previous block), so the 64
newest bits are compared with the tags only when one of the three bytes before them is a flag (two damaged flags
still leave one). On noise that is about one bit in 85. Dire Wolf's files decode exactly as before (41, 44, 46).

## Tests

```sh
cargo test -p ufx25                                                        # RS and framing (no files needed)
crates/ufx25/tools/interop.sh USER@HOST                                  # Dire Wolf both ways
cargo test --release -p ufx25 --test interop -- --ignored --nocapture      # the decode side, once the WAVs exist
```

## Licence

[PolyForm Noncommercial 1.0.0](../../LICENSE.md). Written from the FX.25 specification and general Reed-Solomon
theory; no FX.25 implementation's source was read.
