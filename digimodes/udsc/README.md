# udsc

**Marine VHF DSC (Digital Selective Calling) for microcontrollers: distress alerts and calls on channel 70.**

udsc receives ITU-R M.493 DSC as sent on VHF channel 70 (156.525 MHz): 1200-baud FSK at 1300 and 2100 Hz.
Every call comes out as its symbols and, decoded, its fields:

- the format: distress, all ships, individual, group or geographic area;
- the calling station's MMSI and the address;
- the category: routine, safety, urgency or distress;
- the telecommands and the proposed working channel;
- for a distress alert, or its acknowledgement or relay: the vessel, the nature of distress, the position and the
  time.

It is `no_std`, allocates nothing, and uses integers only.

Part of [udigimodes](../../README.md), digital-mode libraries for microcontrollers. Written for
[Ferrite](../../README.md), free firmware for the Radtel RT-950 Pro handheld. The radio's BK4829
has FFSK modems at 1200/1800 and 1200/2400 Hz, but none at DSC's 1300/2100, so the receiver is software. It
shares [uafsk](../uafsk)'s FSK front end.

Status: **prototype, 0.1**. Tested on calls made by an independent encoder, in noise; not yet run on a radio or
on off-air recordings (none are published that we could find).

## At a glance

| | |
| --- | --- |
| RAM | **1,208 B** (`Receiver<4>`: four slicers, each with its own framer) |
| CPU at 9600 S/s (Cortex-M4, emulator) | 5.4 MIPS: 3.4% at 240 MHz |
| Calls from an independent encoder, clean | **11 of 11** exact (symbols and fields), and 3 of 3 back to back |
| In noise (SNR in 4.8 kHz), calls decoded exactly | 3 dB: 215 of 220 flat, 218 de-emphasised; 6 dB: 220 of 220 |

## How it works

1. **FSK to bits:** uafsk's front end at 1300/2100 Hz: correlators, per-tone normalisation, slicers, a clock
   each. A fourth slicer compares raw amplitudes. In white noise it has a quarter of the others' bit errors, and
   it is what brings 6 dB from 144 to 200 calls.
2. **Phasing:** the DX 125 and RX 111 to 104 phasing pattern, found by correlation. Locking needs the last four
   codewords within 3 bit errors of it; noise matches that by chance about once in 10^11 bits.
3. **Two copies, one symbol:** every symbol is sent twice, five codewords apart (DX and RX). Each symbol is
   taken as the value whose 10-bit codeword is nearest both copies.
4. **ECC:** the XOR of the call's symbols confirms the result, or stands in for the least certain symbol.
5. **End:** the EOS, followed by the ECC. If the signal fades before the end is recognised, the most likely
   EOS is found looking back, and the ECC decides.

`Message::parse` reads the fields.

## Using it

```rust
use uafsk::fsk::Config;
use udsc::{Message, Receiver};

let mut rx: Receiver = Receiver::new(Config::new(9600));   // high-pass on for speaker audio; off for flat audio
for &x in samples {
    rx.push(x, |call| {
        if call.ecc_ok {
            if let Some(m) = Message::parse(call.symbols) {
                // m.format, m.self_id, m.category, m.distress, m.channel ...
            }
        }
    });
}
```

## Testing

```sh
cargo test -p udsc                                                   # codewords, damaged copies, fields
# the calls from dsc_generator: FerriteFW/testdata beside ferrite (its tools/make-dsc.sh remakes them, Docker)
cargo test --release -p udsc --test report -- --ignored --nocapture  # exact decodes, then in noise
```

The calls come from [dsc_generator](https://github.com/kywalda/dsc_generator) (CC0), run unchanged apart from
its sound-card output. It encodes 11 kinds of call:

- individual: routine, acknowledgement, test;
- all ships: safety, urgency;
- group;
- geographic area;
- distress: with a position, with the position unknown, and in the southern hemisphere;
- distress acknowledgement.

## Licence

[PolyForm Noncommercial 1.0.0](../../LICENSE.md).
