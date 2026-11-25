# upocsag

**POCSAG pager messages for microcontrollers, in about 0.6 KB of RAM.**

upocsag receives POCSAG at 512, 1200 and 2400 baud at the same time, from FM discriminator audio. POCSAG is the
paging format used by hospitals and fire services, and by the amateur [DAPNET](https://hampager.de) network. The
crate corrects up to two bit errors per codeword and gives back addresses, function bits and message text, numeric
or alphanumeric. It also encodes messages, for transmitting and for tests. It is `no_std`, allocates nothing, and
uses integers only.

Part of [udigimodes](../../README.md), digital-mode libraries for microcontrollers. Written for
[Ferrite](../../README.md), free firmware for the Radtel RT-950 Pro handheld. The radio's BK4829
has 1200 and 2400 baud FSK packet modes but no 512 baud and no POCSAG batch framing, so the receiver is software.
[`Decoder`](src/decode.rs) also takes codewords directly, from any source that already has them.

Status: **prototype, 0.1**. Measured on 7 real recordings and synthetic signals; not yet run on a radio.

## At a glance

| | multimon-ng 1.3.1 | upocsag `Receiver` |
| --- | --- | --- |
| RAM | | **564 B** (256 of it the message buffer) |
| Flash (Cortex-M4F, size-optimised) | | about 3.5 KB |
| CPU at 9600 S/s (Cortex-M4, emulator) | | **2.0 MIPS**: 1.3% at 240 MHz, 2.5% at 120 MHz |
| Polarity | one at a time (`-i` to invert) | both, automatically |
| Rates | one demodulator each | all three at once |

## Real recordings

Messages decoded; in brackets, how many are identical to multimon-ng's (address, function and text). multimon-ng
runs at 22050 S/s, both polarities; the row shows the one that decodes more.

| Recording | multimon-ng | upocsag, 22050 S/s | upocsag, 9600 S/s |
| --- | --- | --- | --- |
| Wikimedia Commons, 512 baud | 1 | 1 (1) | 1 (1) |
| sigidwiki, 512 baud | 1 | 1 (1) | 1 (1) |
| sigidwiki, 1200 baud | 1 | 1 (1) | 1 (1) |
| sigidwiki, 2400 baud | 1 | 1 (1) | 1 (0)\* |
| sigidwiki, NZ fire and courier traffic, 1200 baud | 24 | **24 (24)** | **24 (24)** |
| sigidwiki, US fire dispatch, 1200 baud | 16 | 9 (7) | 9 (7) |
| sigidwiki "148", mostly noise | 0 | 1 (0) | 1 (0) |

\* 2400 baud at 9600 S/s is 4 samples a bit: one codeword of the 2400-baud sample decodes wrongly (marked in the
message's `errors`).

The US fire dispatch recording is spliced: a batch repeats partway through, which shifts every codeword after it out
of its slot. Both decoders get its real page. multimon-ng's 9 extra messages, and our 2 extra, come from the
misaligned codewords after the splice. On the noise-only recording, 1 message is noise that passed the error
correction. Codewords with more than two errors are counted in `Message::errors`, so firmware can hide such
messages.

## Using it

```rust
use upocsag::Receiver;

let mut rx: Receiver = Receiver::new(9600);   // 9600 to 32768 samples/s of discriminator audio
for &x in samples {
    rx.push(x, |m| {
        // m.address, m.function, m.baud, m.errors; m.alpha() or m.numeric() for the text.
    });
}
rx.flush(|m| { /* squelch closed: the message in progress, if any */ });
```

Which kind of text an address carries is set per pager, not sent. Function 3 is usually alphanumeric and function
0 numeric, but systems differ, so `Message` offers both readings.

Transmitting: `encode::encode(address, function, Content::Alpha(b"..."), |word| ...)` gives the codewords after
the 576-bit preamble. Each one is sent most significant bit first, as two-level FSK.

## How it works

For each rate, from discriminator audio, in integers:

1. **Average over one bit**: a running sum over a shared 64-sample ring. This is the matched filter for square bits.
2. **Threshold** half-way between peak and valley trackers, so a frequency offset does not matter.
3. **Recover the clock** with a digital PLL. Each level crossing pulls it a quarter of the way into line; the
   crossing's time is interpolated between samples, which matters at 4 samples a bit (2400 baud at 9600 S/s). The
   PLL samples half-way between crossings.
4. **Find the sync word** in either polarity, with up to two bit errors. Each rate locks on its own. A lock drops at
   two uncorrectable codewords in a row, or when a batch's sync word is missing. Noise passes the correction about
   one time in four, so a false lock lasts only a few codewords.
5. **Decode** ([`Decoder`](src/decode.rs)): BCH(31,21) correction of up to two errors, with the parity bit rejecting
   most triple errors ([`bch`](src/bch.rs)). An address codeword starts a message; its slot in the batch gives the
   address's low three bits. Message codewords add 20 bits each; the next address, an idle codeword or the end of
   the transmission ends the message.

The first rate to lock owns the decoder. If it drops, another locked rate takes over at its next batch.

Discriminator audio is needed: a speaker output's high-pass and de-emphasis distort 512-baud bits. Ferrite's
discriminator mode provides it.

## Testing

```sh
cargo test -p upocsag                                                   # BCH, round trips, synthetic audio
# the recordings and multimon-ng decodes: FerriteFW/testdata beside ferrite (fetch/all.sh adds the sigid_* set)
cargo test --release -p upocsag --test report -- --ignored --nocapture  # the table above
```

The synthetic tests cover every rate at 9600, 22050 and 32000 S/s, both polarities, a DC offset, noise, and
silence. CPU: `tools/cpu-bench` (`run.py` counts the instructions in an emulator).

## Licence

[PolyForm Noncommercial 1.0.0](../../LICENSE.md).
