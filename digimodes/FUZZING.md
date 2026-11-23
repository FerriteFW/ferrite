# Fuzzing the packet decoders

Everything the radio hears off the air goes through these decoders. A hostile or damaged packet must never crash, hang or misread memory: on the radio a panic is a fault (safe state, reboot).

The fuzz targets live in `ferrite/fuzz/`, a workspace of its own. They are not part of `cargo test`, clippy or the firmware build.

## In the normal tests

- The targets' bodies are in `fuzz/src/harness.rs`; `crates/ferrite-app/tests/fuzz_packets.rs` runs the same functions in `cargo nextest run`, seeded and mutated (`crates/ferrite-fuzzkit`).
- Every other untrusted input has its own `fuzz_*` test: the link, the terminal, the store, settings, updates, NMEA, the drivers, the other decoders, text entry.
- `FERRITE_FUZZ_SCALE=N` runs N times the cases. `FERRITE_FUZZ_CASE=SEED` runs one failing case alone.
- A failure prints its seed and input and saves it under `target/fuzz-found/`. Copy it into `fuzz/regress/TARGET/` to keep it: those run first, every time.

## Running

- Needs the pinned nightly (`rust-toolchain.toml`) and cargo-fuzz: `cargo install cargo-fuzz --locked`.
- From `ferrite/`: `tools/fuzz.sh 600` runs every target for 10 minutes, one after another.
- One target: `tools/fuzz.sh 600 aprs_parse`. List them: `tools/fuzz.sh list`.
- `JOBS=N` sets the workers per target (default: half the cores).
- Exit status 1 means a crash. The input is in `target/fuzz/artifacts/`, the log in `target/fuzz/logs/`.
- Everything stays under `target/fuzz/`. The corpora in `target/fuzz/corpus/` grow between runs.

## Seeds

- `tools/fuzz.sh seeds` rebuilds the corpora (the first run does it on its own).
- They come from the TNC2 test packets: `digimodes/uaprs/tests/spec_packets.txt`, and when present `../testdata/aprs/*.hex` (the WA8LMF CD and the APRS suite) and `../testdata/fx25/msgs.txt`.
- `fuzz/src/bin/seeds.rs` turns each packet into every target's format: AX.25 bytes, KISS, HDLC bits, FX.25 blocks and bit streams, and AFSK audio at 24 kHz.

## Targets

| Target | What it feeds |
|---|---|
| `aprs_parse` | `uaprs::parse`, every packet type; third-party packets parsed again |
| `ax25_frame` | `Frame::parse`, `write` round trip, TNC2 text, `Tnc2::split`, `Address` |
| `kiss` | the KISS decoder byte by byte, its commands, encode round trip |
| `fx25_block` | `ufx25::decode` on any block of any code, and the Reed-Solomon decoder alone |
| `fx25_bits` | the FX.25 receiver bit by bit (tag hunter, collector, correction) |
| `hdlc_bits` | the HDLC deframer bit by bit, plain and with soft-decision repair |
| `afsk_audio` | the radio's AFSK decoder (`ferrite_core::decoders`, 24 kHz) on any audio, FX.25 blocks corrected |
| `aprs_live` | `AprsLive::receive` and `tick`: the radio's whole receive path, digipeater on |

## A crash

1. Minimise it: `cargo fuzz tmin --target-dir target/fuzz TARGET target/fuzz/artifacts/FILE`.
2. Fix the bug.
3. Add a unit test with the input (or a hand-written packet showing the same thing) next to the code. Its doc comment says fuzzing found it.

Builds run with debug assertions and overflow checks, so an overflow that would wrap silently on the radio still shows up.
