# Developing Ferrite

For people who want to build Ferrite, change it or read the code. Using it on a radio: [README.md](README.md).

Ferrite is Rust on [Embassy](https://embassy.dev), for the AT32F403A in the RT-950 Pro.

## Set up

- git, [rustup](https://rustup.rs) and Python 3.
- The compiler is a pinned nightly (`rust-toolchain.toml`). rustup installs it, its components and the radio's target on the first build.
- The ARM binutils, for the fast-flash check and the size tools (`arm-none-eabi-readelf`, `-nm`, `-objdump`, `-addr2line`):
  - Fedora: `sudo dnf install arm-none-eabi-binutils-cs`
  - Debian and Ubuntu: `sudo apt install binutils-arm-none-eabi`
  - Arch: `sudo pacman -S arm-none-eabi-binutils`
  - macOS: `brew install arm-none-eabi-binutils`
- Everything else is in this repo, the chip drivers and the digital modes included.

## Run it on the PC

```sh
cargo test        # the logic, the screens and the decoders
cargo simw        # the radio in a window, with a clickable front panel (-- --fresh starts at setup)
cargo sim         # every screen to target/sim/*.png
cargo simweb      # the simulator as wasm for a web page (sim-web/README.md)
```

`make test`, `make sim`, `make simw` and `make simweb` run those. `make` on its own lists every target. The Makefile only calls Cargo; the commands above are the build. Extra simulator flags: `make simw ARGS=--fresh`.

The terminal (manual chapter 37) runs in the simulator too:

- `cargo simw` opens a pseudo-terminal as the radio's cable and prints how to connect (`screen /dev/ttys006 115200`, then Enter).
- `cargo simw -- --term-tcp 7300`, then `stty raw -echo; nc 127.0.0.1 7300; stty sane`.
- `cargo run -p ferrite-sim -- --term-headless 60`: no window, for scripts.

## Build for the radio

```sh
python3 -m venv .venv && .venv/bin/pip install -r tools/requirements.txt
tools/mkbtf.sh tx          # target/ferrite-tx.ffw (and .btf): the user build
tools/mkbtf.sh tx-bench    # target/ferrite-txbench.ffw: plus bench commands, Dry TX on at power-on
tools/mkbtf.sh reader      # target/ferrite-reader.btf: the read-only backup reader Console loads
```

`make venv`, `make tx`, `make tx-bench` and `make reader` run those. `make fw` is the receive-only image. A clock build is `make tx-bench CLOCK=clock-240`.

- `mkbtf.sh` builds, checks what must sit in fast flash (`tools/slowcheck.py`), packs the image for the stock bootloader and the `.ffw` update file.
- Install your build with Ferrite Console (Install › Advanced, or Firmware Update), or over the cable: `.venv/bin/python tools/flash.py PORT target/ferrite-tx.ffw --backup backup.bin --dev-unsigned` (it refuses without a whole-chip backup).
- Your own builds aren't signed: Console takes them with "Allow unsigned files" ticked, `flash.py` with `--dev-unsigned`.
- Flash the `.ffw`, not the bare `.btf`: the `.btf` leaves out the text and other parts kept in SPI flash.

## Before a commit

`make check` runs the `cargo` lines below (tests and clippy, including the bench feature set). The UI test stays separate: `make uitest`.

```sh
cargo test
cargo test -p uradio-bus -p bk4829 -p si4732 -p st7789 -p w25q -p keypad -p nmea --all-features
cargo clippy --all-targets -- -D warnings
cargo fw-clippy -- -D warnings           # the radio's build (also --features tx, tx-bench, bench)
cargo fw-reader-clippy -- -D warnings    # the reader
python3 tools/uitest.py --sim auto       # the UI tests on the simulator
```

CI on Forgejo (`.forgejo/workflows/release.yml`) runs all of these on a release, then builds, signs and attaches the files.

## Layout

| Path | What |
| --- | --- |
| [`firmware/`](firmware) | The image: start-up and the safe state, the HAL, the board's pins, the tasks |
| [`crates/ferrite-core`](crates/ferrite-core) | Plain logic, tested on the PC: messages, frequencies, the transmit checks, the flash map, the link protocol |
| [`crates/ferrite-ui`](crates/ferrite-ui) | The drawing layer and the widgets screens are made of |
| [`crates/ferrite-app`](crates/ferrite-app) | The screens and the UI state machine, run by the firmware and the simulators; the terminal (`src/term/`) |
| [`crates/ferrite-radio`](crates/ferrite-radio) | The chips' recipes: tuning, modes, squelch, transmit sequences |
| [`crates/ferrite-store`](crates/ferrite-store) | The SPI-flash store, safe against a power cut at any byte |
| [`drivers/`](drivers) | Chip drivers: BK4829, SI4732, ST7789, W25Q, keypad, NMEA |
| [`digimodes/`](digimodes) | AFSK and FX.25, APRS, POCSAG, DSC and their DSP |
| [`reader/`](reader) | The read-only backup reader |
| [`sim/`](sim), [`sim-web/`](sim-web) | The radio on the PC, with the firmware's own code |
| [`tools/`](tools) | Packing and flashing, the size and fast-flash checks, the UI and bench test runners |
| [`assets/`](assets), [`fonts/`](fonts) | The logo and the fonts (OFL, licences beside them) |

## Tasks

Each piece of hardware has one owning task. Others send it commands; owners announce events on one bus (`firmware/src/chan.rs`, types in `ferrite_core::msg`).

| Task | Runs on | Owns |
| --- | --- | --- |
| Input sampler | interrupt, highest | The knob and the keys' sampling |
| DSP | interrupt | The decoders, audio in (ADC1) and out (DAC1) |
| Radio | interrupt | Both BK4829s, the SI4732, the RF path pins, the PA drive |
| UI | thread | The display and what the keys mean; runs the APRS and decoder sides too |
| Storage | thread | SPI flash (writes only inside Ferrite's own ranges) |
| Link | thread | The cable and Bluetooth: the link protocol, the terminal, KISS |
| GPS | thread | The GPS module, position and time |

## Rules worth knowing

- **Fast and slow flash.** Code that runs per sample or per pixel, or from interrupt context, must sit in the zero-wait part of flash (`firmware/memory.x`). `tools/slowcheck.py` fails the build if it doesn't.
- **Generics are copied into every caller.** A generic with a real body is a thin shim over one non-generic function.
- **Never write** calibration (SPI 0x00F000-0x00FFFF), the update area (0x300000 and up) or the MCU's option bytes.
- **The spec is the manual** ([FerriteFW/docs](https://github.com/FerriteFW/docs)). A behaviour change is a numbered rule there, with a test naming it.

## Licence

[PolyForm Noncommercial 1.0.0](LICENSE.md). Contributions: see the CLA once the repo is public.
