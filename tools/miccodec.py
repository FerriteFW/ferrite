#!/usr/bin/env python3
"""The mic-codec bench (Nov 24, 2026): can the MCU hear the microphone through the idle BK4829?

The idea (docs/archive/mic-audio-features.md, "Mic loopback on the idle BK4829"): the chip that isn't receiving gets
REG_30 = 0x0306 (transmit DSP, audio DAC and mic ADC on; no PLL, no PA) and full mic gain, so it plays the mic out
on its audio pin, onto the net the MCU samples on PA2 (`af`, TERM-32.1). Nothing is keyed.

Phases (a bench build: tx-bench or bringup; the radio over the cable or Bluetooth):
  floor   `af` with the codec off: the flat line to compare with
  codec   the codec on while you hold a steady tone at the mic (a whistle, or a phone playing 1 kHz)
  bits    the same with bit 8 off (0x0206: undocumented in the register table) and with the audio DAC off (0x0104)
  --dac   DAC1 plays 1500 Hz (TERM-54) with the codec on and the mic quiet: PC12 low, then high (DAC1 onto the mic
          line, where the codec chip hears it too): does it reach PA2, and can the two share the net?
  --sdr   RF: rtl_power on the build box over the idle chip's band with the codec off, then on (no key-up)

Everything is put back at the end: the chip's registers, Watch (which also lets the radio's next path step reset
the pins), DAC1 off.

  ../.venv/bin/python tools/miccodec.py ble:E5A6A6FF-F37C-29A8-1864-A35DDD29EEAF [--chip 0|1] [--dac] [--sdr]
"""
import argparse
import re
import statistics
import subprocess
import sys
import time

sys.path.insert(0, __file__.rsplit("/", 1)[0])
import ferrite_link as fl  # noqa: E402

CODEC = 0x0306  # their value: AF DAC (9), bit 8, mic ADC (2), TX DSP (1)
VARIANTS = [(0x0206, "bit 8 off"), (0x0104, "audio DAC off (expect nothing)")]
RX_BITS = 0x3C01  # REG_30: receive chain (13:10) and receive DSP (0)
BANDS = {0: ("VHF", "136M:174M:25k"), 1: ("UHF", "400M:480M:25k")}


def reg(r, chip, n, val=None):
    line = f"bk {chip} {n:x}" + ("" if val is None else f" {val:x}")
    m = re.search(r"(?:= |reads back )0x([0-9A-F]{4})", r.cmd(line))  # a read, or a write's read-back
    if not m:
        sys.exit(f"`{line}` gave no register value")
    return int(m.group(1), 16)


def af(r):
    t = r.cmd("af")
    mn, mx, mean = map(int, re.search(r"min (\d+), max (\d+), mean (\d+)", t).groups())
    hz = int(re.search(r"(\d+) Hz if", t).group(1))
    rails = int(re.search(r"(\d+) at a rail", t).group(1))
    return dict(swing=mx - mn, mean=mean, hz=hz, rails=rails)


def bursts(r, label, n=4):
    rs = [af(r) for _ in range(n)]
    sw = statistics.median(x["swing"] for x in rs)
    hz = statistics.median(x["hz"] for x in rs)
    rails = sum(x["rails"] for x in rs)
    print(f"  {label:<34} swing {sw:>5.0f} counts   {hz:>5.0f} Hz   mean {rs[0]['mean']}" + (f"   {rails} at a rail" if rails else ""))
    return sw, hz


def sweep(host, band, gain):
    out = subprocess.run(["ssh", host, f"rtl_power -f {band} -g {gain} -i 3 -1 - 2>/dev/null"],
                         capture_output=True, text=True, timeout=120).stdout
    bins = []
    for line in out.splitlines():
        f = [x.strip() for x in line.split(",")]
        if len(f) < 7:
            continue
        lo, step = float(f[2]), float(f[4])
        bins += [(lo + i * step, float(db)) for i, db in enumerate(f[6:])]
    if not bins:
        sys.exit(f"rtl_power on {host} gave nothing (is the dongle there and free?)")
    return bins


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("port", help="the radio: /dev/cu.usbserial-* or ble:ID")
    ap.add_argument("--chip", type=int, choices=(0, 1), help="the codec chip (default: the one asleep)")
    ap.add_argument("--dac", action="store_true", help="also DAC1 beside the codec (TERM-54)")
    ap.add_argument("--sdr", action="store_true", help="also an RF sweep with the codec off and on")
    ap.add_argument("--retune", metavar="SLOT=MHZ", help="move a VFO slot for the test (put back after), e.g. "
                    "A=145.575 to free the UHF chip when A and the APRS slot share the two bands")
    ap.add_argument("--rf-only", action="store_true", help="only the RF sweep (no one at the mic needed)")
    ap.add_argument("--sdr-host", default="xeon", help="where the RTL-SDR is (ssh host)")
    ap.add_argument("--sdr-gain", default="20")
    a = ap.parse_args()

    with fl.Radio(a.port, name=b"miccodec.py") as r:
        if not r.bench:
            sys.exit(f"{r.build}: not a bench build (tx-bench or bringup), so no bk, pin, af or dac")
        watch = re.search(r"Watch: (\S+)", r.cmd("get 0 1 1")).group(1)
        r.cmd("set 0 1 1 Off")
        moved = None
        if a.retune:
            slot, mhz = a.retune.upper().split("=")
            line = next((x for x in r.cmd("slot").splitlines() if x.startswith(slot)), "")
            m = re.match(rf"{slot}\*?\s+vfo\s+([\d.]+)", line)
            if not m:
                r.cmd(f"set 0 1 1 {watch}")
                sys.exit(f"--retune: slot {slot} isn't on the VFO ({line.strip()!r})")
            moved = (slot, m.group(1))
            r.cmd(f"slot {slot} vfo {mhz}")
        time.sleep(1.5)
        chip = a.chip
        if chip is None:
            # idle = no receive bits in any of ten reads over 2 s (Battery Save sleeps a busy chip between wakes)
            busy = [False, False]
            for _ in range(10):
                for c in (0, 1):
                    busy[c] |= reg(r, c, 0x30) & RX_BITS != 0
                time.sleep(0.2)
            idle = [c for c in (0, 1) if not busy[c]]
            if not idle:
                for step in ([f"slot {moved[0]} vfo {moved[1]}"] if moved else []) + [f"set 0 1 1 {watch}"]:
                    r.cmd(step)
                sys.exit("both chips receive (the active slot and the APRS slot on different bands): put them on one "
                         "band, e.g. --retune A=145.575")
            chip = idle[0]
        saved = {n: reg(r, chip, n) for n in (0x30, 0x7D)}
        print(f"{r.build}: codec chip {chip} ({BANDS[chip][0]}), REG_30 {saved[0x30]:04X}, REG_7D {saved[0x7D]:04X}; "
              f"Watch {watch} -> Off for the test")

        def codec(val):
            reg(r, chip, 0x30, 0)
            if val:
                reg(r, chip, 0x7D, saved[0x7D] | 0x3F)  # full mic gain: the level at the MCU is small
                reg(r, chip, 0x30, val)
                r.cmd("pin pb8 0")  # the shared audio stage: the mic stays on its line
                r.cmd("pin pc12 0")  # DAC1 off the mic line

        def held():
            now = reg(r, chip, 0x30)
            return "" if now in (CODEC, 0x0206, 0x0104) else f"   (REG_30 now {now:04X}: the radio rewrote it)"

        try:
            if not a.rf_only:
                print("floor (codec off):")
                bursts(r, "no codec")
                codec(CODEC)
                input("\nHold a steady tone at the mic (whistle, or a phone on 1 kHz) and press Enter; keep it going... ")
                print("codec on:")
                bursts(r, f"REG_30 {CODEC:04X}")
                for v, what in VARIANTS:
                    codec(v)
                    bursts(r, f"REG_30 {v:04X} ({what})")
                codec(CODEC)
                bursts(r, f"REG_30 {CODEC:04X} again")
                print(held() or "  the codec chip kept its REG_30 throughout")
                if a.dac:
                    input("\nStop the tone (mic quiet) and press Enter... ")
                    bursts(r, "codec on, mic quiet")
                    r.cmd("dac 1500 4")
                    bursts(r, "dac 1500 Hz, PC12 low")
                    r.cmd("pin pc12 1")
                    bursts(r, "dac 1500 Hz, PC12 high (mic line)")
                    r.cmd("dac off")
                    r.cmd("pin pc12 0")
                    bursts(r, "dac off")
                    input("Whistle again with nothing else on, then Enter (the mic with DAC1 idle on its line)... ")
                    bursts(r, "codec on, tone, DAC1 off")
            if a.sdr or a.rf_only:
                band = BANDS[chip][1]
                print(f"\nRF over {band} on {a.sdr_host} (no key-up; about 10 s a sweep):")
                codec(0)
                off = sweep(a.sdr_host, band, a.sdr_gain)
                codec(CODEC)
                on = sweep(a.sdr_host, band, a.sdr_gain)
                n = min(len(off), len(on))
                floor = statistics.median(db for _, db in off[:n])
                d = sorted(((on[i][1] - off[i][1], on[i][0]) for i in range(n)), reverse=True)[:5]
                print(f"  floor {floor:.1f} dB; biggest rises with the codec on: "
                      + ", ".join(f"{hz / 1e6:.3f} MHz +{dd:.1f} dB" for dd, hz in d))
                print("  (a few dB here and there is the noise; a real carrier stands 10 dB or more over the floor)")
                print(held() or "  the codec chip kept its REG_30 through the sweep")
        finally:
            # Watch first: its retune and path step put the pins back even if a register write below fails
            for step in [f"slot {moved[0]} vfo {moved[1]}" if moved else None, f"set 0 1 1 {watch}", "dac off" if a.dac else None, f"bk {chip} 30 0", f"bk {chip} 7d {saved[0x7D]:x}",
                         f"bk {chip} 30 {saved[0x30]:x}"]:
                if step:
                    try:
                        r.cmd(step)
                    except fl.RadioError as e:
                        print(f"  restore `{step}` failed: {e}")
            print(f"\nput back: {'slot ' + moved[0] + ' ' + moved[1] + ', ' if moved else ''}Watch {watch}, chip {chip}'s REG_30 {saved[0x30]:04X} and REG_7D {saved[0x7D]:04X}")

if __name__ == "__main__":
    main()
