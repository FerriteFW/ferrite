#!/usr/bin/env python3
"""The power bench: the radio on the DPS-150, each state set over the link, its current logged.

    tools/powerbench.py RADIO SUPPLY [--label 120MHz] [--seconds 60] [--settle 10] [--only "NAME;..."]
    tools/powerbench.py auto /dev/cu.usbmodemXXXX --label 240MHz

RADIO is a ferrite_link port (cable, or ble:ID); SUPPLY the DPS-150's. With --tx PORT --tx-slot a|b|c, a second
radio keys that slot in short bursts so the radio under test receives a signal (states marked rx): the second radio
must say `tx+dry` with Dry TX on (TX-48), and its slot must be on the frequency of the tested radio's active slot.
Bursts keep the bench's duty limit (5 s keyed counting the tail, a rest of three times it: 25 % of any window);
only the keyed seconds are measured. Needs a bench build (the `lcd` command,
TERM-40). Each state: `lcd reset`, the state's terminal lines, `--settle` seconds, then `--seconds` of readings.
Prints a table of mA per state against the baseline, and appends every state to target/power/LABEL.csv.

The supply's output is never touched: the radio stays powered (set it and turn it on by hand, or with dps150.py).
Nothing here transmits.
"""
import argparse
import csv
import statistics
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from dps150 import Dps150, T_ALL  # noqa: E402
from ferrite_link import Radio  # noqa: E402

# The display states of the low-power bench (ui-backlog, "Ultra low power mode"); each against the baseline. Every
# state starts from BASE, both lights held at 100 % over the settings', so the screen's timeout (which darkens both
# after a minute) can't change a state halfway; later lines override it.
# Battery Save pinned at 300/300 (the first 30 minutes after activity, where the 120 and 240 MHz runs sat), so a
# state can't cross into the level's own cycle halfway; the save states override it (TERM-41).
BASE = ["lcd bl 100", "lcd keys 100", "bk save 300 300"]
STATES = [
    ("baseline", []),
    ("bl 30", ["lcd bl 30"]),
    ("bl 0", ["lcd bl 0"]),
    ("keys 0", ["lcd keys 0"]),
    ("keys 60", ["lcd keys 60"]),
    ("keys 30", ["lcd keys 30"]),
    ("keys 15", ["lcd keys 15"]),
    ("fps 1", ["lcd fps 1"]),
    ("idle", ["lcd idle on"]),
    ("partial", ["lcd partial on"]),
    ("panel asleep", ["lcd sleep on", "lcd keys 0"]),
    ("low power set", ["lcd bl 30", "lcd keys 0", "lcd fps 1"]),
    ("save ~off", ["bk save 5000 10"]),
    ("save 1", ["bk save 300 500"]),
    ("save 2", ["bk save 180 700"]),
    ("save 3", ["bk save 180 800"]),
    ("low power state", ["lcd bl 30", "lcd keys 0", "lcd fps 1", "bk save 180 800"]),
    ("baseline again", []),
    ("rx signal", ["@rx"]),
    ("rx signal, low power set", ["lcd bl 30", "lcd keys 0", "lcd fps 1", "@rx"]),
]
APPROVED_HZ = (145_495_000, 446_475_000, 145_575_000)  # CLAUDE.md: the bench's keying frequencies
# `--suite lpm`: normal against Low Power Mode as a user runs them (PWR-24), from the factory defaults (brightness
# 7, keypad light with the screen, watch A+B, GPS on, Bluetooth off, Battery Save 1). Battery Save is pinned at each
# side's steady state (level 1's 300/500; the mode's 180/800), i.e. after its hold windows, so a 60 s state can't
# straddle one. "@wake" presses BACK first (lights the screen); "@dark" waits the 5 s timeout out.
LPM_SETUP = ["set 0 7 7 2 Off", "set 0 7 4 2 7", "set 0 7 4 5 With screen", "set 0 1 1 A+B", "set 0 6 1 On",
             "set 0 9 2 1 Off", "set 0 7 7 1 Level 1"]
LPM_RESTORE = ["set 0 7 7 2 Off", "bk save normal", "set 0 7 4 1 1 min"]
NORMAL = ["set 0 7 7 2 Off", "bk save 300 500"]
LOWPOWER = ["set 0 7 7 2 On", "bk save 180 800"]
LPM_STATES = [
    ("normal, screen lit", NORMAL + ["set 0 7 4 1 Never", "@wake"]),
    ("normal, screen dark", NORMAL + ["set 0 7 4 1 5 s", "@dark"]),
    ("low power, screen lit", LOWPOWER + ["set 0 7 4 1 Never", "@wake"]),
    ("low power, screen dark", LOWPOWER + ["set 0 7 4 1 5 s", "@dark"]),
    ("normal, receiving", NORMAL + ["set 0 7 4 1 Never", "@wake", "@rx"]),
    ("low power, receiving", LOWPOWER + ["set 0 7 4 1 Never", "@wake", "@rx"]),
]
BURST_S, TAIL_S, REST_S = 4.5, 0.5, 15.0  # keyed, the carrier after KEY off, then the rest
MEASURE = (1.0, 4.0)  # the part of each burst measured: after the squelch opens, before the unkey


def measure(d, seconds):
    a = d.tally()
    rows, last, t0 = [], None, time.monotonic()
    while time.monotonic() - t0 < seconds:
        d.poll(0.1)
        t = d.state.get("t")
        if t is None or time.monotonic() - t > 1.0:
            d.get(T_ALL)
        if t is not None and t != last:
            last = t
            rows.append((d.state["v"], d.state["i"]))
    m = summary(rows)
    b = d.tally()
    if a and b and b[0] > a[0]:
        # The supply's own tally over the span: the true mean, not ~2 readings a second of a 180/800 ms cycle.
        hours = (b[0] - a[0]) / 3600
        m["tally_ma"], m["tally_mw"] = (b[1] - a[1]) * 1000 / hours, (b[2] - a[2]) * 1000 / hours
    return m


def slot_freq(radio, slot):
    """A slot's frequency in Hz from `slot` (`A* ch 20 147.20000 MHz ...`); `slot` None: the active one."""
    for line in radio.cmd("slot").splitlines():
        w = line.split()
        if w and (w[0][0].lower() == slot if slot else w[0].endswith("*")):
            # `vfo 145.95000` or `ch 20 147.20000`: the first number with a point.
            return round(float(next(x for x in w[1:] if "." in x and x.replace(".", "").isdigit())) * 1e6)
    raise SystemExit(f"no slot {slot or 'active'} in: {radio.cmd('slot')}")


def check_dry(tx):
    if "Dry TX: On" not in tx.cmd("get 0 8 8 5"):
        sys.exit("the second radio's Dry TX is off: turn it on (set 0 8 8 5 on) before keying it")


def measure_rx(d, tx, slot, bursts):
    """`bursts` dry key-ups of the second radio; readings from inside each one's MEASURE window."""
    import threading
    from ferrite_link import remote_ptt
    rows = []
    for _ in range(bursts):
        check_dry(tx)
        th = threading.Thread(target=remote_ptt, args=(tx.link, BURST_S, slot))
        th.start()
        t0 = time.monotonic()
        last = d.state.get("t")
        while time.monotonic() - t0 < MEASURE[1]:
            d.poll(0.1)
            t = d.state.get("t")
            if time.monotonic() - (t or 0) > 1.0:
                d.get(T_ALL)
            if t and t != last:
                last = t
                if t - t0 >= MEASURE[0]:
                    rows.append((d.state["v"], d.state["i"]))
        th.join()
        time.sleep(REST_S)
    return rows


def summary(rows):
    if not rows:
        sys.exit("no readings from the supply")
    amps = [1000 * r[1] for r in rows]
    return {"n": len(rows), "volts": statistics.mean(r[0] for r in rows), "mean_ma": statistics.mean(amps),
            "median_ma": statistics.median(amps), "min_ma": min(amps), "max_ma": max(amps)}


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("radio")
    ap.add_argument("supply")
    ap.add_argument("--label", default="run")
    ap.add_argument("--seconds", type=float, default=60)
    ap.add_argument("--settle", type=float, default=10)
    ap.add_argument("--only", help="state names separated by ;")
    ap.add_argument("--tx", help="the second radio's port, keyed dry for the rx states")
    ap.add_argument("--tx-slot", choices=["a", "b", "c"])
    ap.add_argument("--bursts", type=int, default=8, help="key-ups per rx state")
    ap.add_argument("--suite", choices=["display", "lpm"], default="display",
                    help="display: the low-power bench's display states (default); lpm: normal against Low Power Mode")
    ap.add_argument("--note", help="the setup, for the record (volume, battery, antenna): target/power/notes.txt")
    a = ap.parse_args()
    states = [s for s in (LPM_STATES if a.suite == "lpm" else STATES) if not a.only or s[0] in a.only.split(";")]
    if not a.tx:
        states = [s for s in states if "@rx" not in s[1]]
    elif not a.tx_slot:
        sys.exit("--tx needs --tx-slot")
    out = Path(__file__).resolve().parents[1] / "target" / "power" / f"{a.label}.csv"
    out.parent.mkdir(parents=True, exist_ok=True)
    if a.note:
        with open(out.parent / "notes.txt", "a") as n:
            n.write(f"{time.strftime('%Y-%m-%d %H:%M')} {a.label}: {a.note}\n")
    new = not out.exists() or out.stat().st_size == 0
    results = []
    tx = Radio(a.tx) if a.tx else None
    if tx:
        if tx.build != "tx+dry":
            sys.exit(f"the second radio is {tx.build}, not tx+dry: turn its Dry TX on")
        check_dry(tx)
    with Radio(a.radio) as r, Dps150(a.supply) as d, open(out, "a", newline="") as f:
        # CLAUDE.md: PC keying only on the approved frequencies, Dry TX on (the 120-24 MHz runs of Oct 21 keyed
        # 145.950 before this check existed).
        if tx and slot_freq(tx, a.tx_slot) not in APPROVED_HZ:
            sys.exit(f"the second radio's slot {a.tx_slot.upper()} is on {slot_freq(tx, a.tx_slot)} Hz: PC keying only on "
                     f"{', '.join(f'{f / 1e6:.3f}' for f in APPROVED_HZ)} MHz")
        if tx and slot_freq(tx, a.tx_slot) != slot_freq(r, None):
            sys.exit(f"the second radio's slot {a.tx_slot.upper()} is on {slot_freq(tx, a.tx_slot)} Hz, the tested "
                     f"radio's active slot on {slot_freq(r, None)}: tune them together first")
        w = csv.writer(f)
        if new:
            w.writerow(["label", "state", "time", "samples", "volts", "mean_ma", "median_ma", "min_ma", "max_ma", "tally_ma", "tally_mw"])
        d.metering()
        if "lcd:" not in (r.cmd("lcd") or ""):
            sys.exit("the radio has no lcd command: flash a bench build (tools/mkbtf.sh tx-bench)")
        try:
            for name, lines in states:
                r.cmd("lcd reset")
                if a.suite == "lpm" and not results:
                    for line in LPM_SETUP:
                        r.cmd(line)
                for line in ([] if a.suite == "lpm" else BASE) + lines:
                    if line == "@wake":
                        r.press("BACK")
                    elif line == "@dark":
                        time.sleep(7)
                    elif line != "@rx":
                        said = r.cmd(line)
                        if "invalid" in said or "refused" in said:
                            sys.exit(f"{line}: {said.strip()}")
                rx = "@rx" in lines
                what = f"{a.bursts} dry bursts" if rx else f"{a.seconds:.0f} s"
                print(f"{name}: settling {a.settle:.0f} s, then {what}", flush=True)
                time.sleep(a.settle)
                d.poll()
                m = summary(measure_rx(d, tx, a.tx_slot, a.bursts)) if rx else measure(d, a.seconds)
                results.append((name, m))
                w.writerow([a.label, name, time.strftime("%Y-%m-%d %H:%M:%S"), m["n"], f"{m['volts']:.3f}",
                            *(f"{m[k]:.1f}" for k in ("mean_ma", "median_ma", "min_ma", "max_ma")),
                            *(f"{m[k]:.1f}" if k in m else "" for k in ("tally_ma", "tally_mw"))])
                f.flush()
                print(f"  {m['mean_ma']:.1f} mA mean, {m['median_ma']:.1f} median ({m['min_ma']:.0f}-{m['max_ma']:.0f})"
                      + (f"; the supply's tally {m['tally_ma']:.1f} mA" if "tally_ma" in m else ""))
        finally:
            r.cmd("lcd reset")
            r.cmd("bk save normal")
            if a.suite == "lpm":
                for line in LPM_RESTORE:
                    r.cmd(line)
            if tx:
                tx.close()
    base = results[0][1]["mean_ma"] if results and results[0][0] == "baseline" else None
    print(f"\n{a.label}: {results[0][1]['volts']:.2f} V" if results else "")
    print(f"{'state':16} {'mean mA':>8} {'median':>8} {'vs base':>8}")
    for name, m in results:
        diff = f"{m['mean_ma'] - base:+8.1f}" if base is not None else ""
        print(f"{name:16} {m['mean_ma']:8.1f} {m['median_ma']:8.1f} {diff}")
    print(f"(appended to {out})")


if __name__ == "__main__":
    main()
