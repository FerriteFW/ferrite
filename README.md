<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo/ferrite-lockup-dark.svg">
    <img alt="Ferrite" src="assets/logo/ferrite-lockup-light.svg" width="420">
  </picture>
</p>

<p align="center"><b>Free firmware for the Radtel RT-950 Pro</b>, source public.</p>
<p align="center"><a href="https://www.radtels.com/products/radtel-rt-950-pro-27mhz-tx-rx-10w-walkie-talkie-am-fm-usb-lsb-cw-receive-gps-aprs-wireless-bluetooth-phone-programming-noaa-two-way-radio?ref=FERRITEFW&variant=44669996794064">Get the RT-950 Pro from Radtel</a> · <a href="https://discord.gg/fYygBVD2Kx">Discord</a></p>

> **Status: 0.9, the first public release.** Ferrite runs every day on our own radios, transmit included. Expect
> rough edges, and keep your backup.

## Start here: use Ferrite on your radio

1. **Get Ferrite Console**, the companion app, from its [Releases page](https://github.com/FerriteFW/console/releases): the macOS `.dmg`, the Linux `.AppImage` or the Windows portable `.exe`.
2. **Connect the radio** with its programming cable.
3. **Open Install.** Console backs up the whole radio first and won't go on until you've saved that file. Then it installs Ferrite.
4. **Read the user guide** for first-run setup, the keys and every feature.

Going back to stock: Console's Back to stock page puts your backup and the stock firmware back.

Help: the [Ferrite Discord](https://discord.gg/fYygBVD2Kx). Bugs: [GitHub issues](https://github.com/FerriteFW/ferrite/issues) (see CONTRIBUTING.md for what to send).

## What it does

- Three slots, A, B and C: receive on two or three at once, each with its own frequency or channel.
- Transmit on the tested bands: repeater offsets, tones, a crossband repeater, a timeout warning.
- Channels and zones, scanning and a band scope.
- APRS on the radio: beacons, messages with acks, stations heard, digipeating, FX.25, KISS over Bluetooth or the cable.
- FM broadcast on any slot, HF receive (AM, SSB and CW), weather alerts (US and Canada), GPS.
- Battery: the processor slows down when nothing needs it (idle current about 19 mA, down from 64).

## Releases

Each release is one update file, `ferrite-VERSION.ffw`: the firmware and its data, signed. Console checks the signature before installing anything.

Releases also carry the build report (`build-report.html`: tests, flash and memory use), `SHA256SUMS` and its signature. To check by hand:

```sh
minisign -Vm SHA256SUMS -P RWQbhRdY9EcsW+Has6GdiIJMPCVPZltho1Lv4lkOpBYls0fxgxZ6Tvai
```

Ferrite's public key (ID `5B2C47F45817851B`) is also in [`minisign.pub`](minisign.pub).

## Safety

- At power-on the transmitter is held off before anything else runs. A fault returns the radio to that safe state.
- Factory calibration and the bootloader's update area are never written.
- Transmit limits (bands, power, timeout, battery) are checked in the firmware itself.

## For developers

Building, testing, the code's layout and how to put your own build on a radio: [DEVELOPING.md](DEVELOPING.md). The commands: `just --list`; first time: `just setup`.

## Contributing

Issues are what help most: bugs, ideas, design suggestions and fixes. We'd rather have an issue than an AI-generated pull request: [CONTRIBUTING.md](CONTRIBUTING.md).

## Licence

[PolyForm Noncommercial 1.0.0](LICENSE.md): free for any noncommercial use. Selling Ferrite, shipping it on radios for sale or selling a service built on it needs a commercial licence. Copyright 2026 Alex Harvey.

The firmware includes others' work under their own licences (Rust crates, fonts): [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md).

The names "Ferrite", "Ferrite Console" and "Ferrite Pocket" and the logo are for FerriteFW's own releases: a changed build takes another name ([TRADEMARKS.md](TRADEMARKS.md)).
