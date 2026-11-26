#!/usr/bin/env python3
"""The transmit bench in one command: the on-air checks against the black radio (a `tx-dry` build) and an RTL-SDR,
each run's numbers stored, compared with the previous run of the same configuration, and one summary page.

    tools/bench.py --dry-plan                          # what it would do; touches nothing
    tools/bench.py --i-know-its-tx-dry                 # the default matrix: VHF wide, VHF NFM, UHF wide
    tools/bench.py --flash target/ferrite-txbench.btf    # flash first (flash.py --backup), wait for the boot, then run
    tools/bench.py --runs vhf-wide --refusals --keepalive --timeout --i-know-its-tx-dry
    tools/bench.py --sweep ctcss,dcs [--from D365N] [--count N] [--items A,B] [--resume SESSION] [--dry-plan]
                                                       # sweeps (below): one short key-up per value, duty-limited
    tools/bench.py --load DIR [--build LABEL]          # re-analyse saved captures (DIR searched for <run>.u8)
    tools/bench.py --suite tx-behaviour [--tests A,B] [--dry-plan]
                                                       # a suite of checks by name (tools/benchtests/): around a
                                                       # key-up, by readback, terminal and SDR (its docstring)
    tools/bench.py --selftest                          # a fake radio and synthetic IQ: the whole flow and the gates
    tools/bench.py --suite modes-on-air [--tests vox,dtmf] [--kiss-ble ble:NAME] [--slow] [--no-audio] [--dry-plan]
                                                       # VOX by the Mac's speaker, DTMF, APRS, KISS, GPS, watch, HF,
                                                       # broadcast, end tone (tools/benchtests/modes_on_air.py)
    tools/bench.py --selftest --sim tcp:127.0.0.1:7300 # the same against the simulator's link (fake SDR)
    tools/bench.py --auto [--dry-plan]                 # what changed since the last on-radio runs decides: the UI
                                                       # tests (tools/uitest.py), the SDR suite, both + a reboot check;
                                                       # only screen files (screens/<page>.rs): uitest.py --page <page>
    tools/bench.py --auto --all | --ui | --rf          # everything, or one side, forced
    tools/bench.py --auto --dry-plan --diff-files crates/ferrite-app/src/screens/home.rs   # a made-up change

Per run (TERM-34, LINK-20, LINK-33, LINK-34; everything goes over the link, nothing through the keypad):
  1. save slot A as it is (`slot A`; its VFO's if it is on a channel), the active slot, the end tone (0 8 4) and
     the TX timeout (0 8 1);
  2. `slot A vfo FREQ MODE low tone off`, `slot A active`, `set 0 8 4 Off`, the TX timeout raised to 120 s if it
     is under 90 s, `txtest suite`;
  3. start rtl_sdr, key over the link (LINK-20 keepalives every 0.5 s) for the suite's 60 s and the 1 kHz hold, unkey;
  4. after all the runs: everything put back (in `finally`, whatever happened), then each capture measured with
     sdrsuite.py's analysis, the numbers stored and compared.

Safety gates: nothing is keyed unless all of these hold, checked right before every KEY:
  - the build is `tx-dry` (PA drive held at 0): HELLO's build byte says tx-dry with the bench bit (LINK-35: 0x82,
    decision 151; a radio that says receive or tx is refused outright, --i-know-its-tx-dry or not), or its version
    string says so ("dry"/"TX DRY"), or --flash flashed an image with the "TX DRY" boot line in it in this same
    invocation. Firmware from before the build byte doesn't say it, so without one of these the bench refuses,
    unless --i-know-its-tx-dry is given (a warning is printed and recorded). A full-power `tx` build is accepted
    only while it says tx+dry (Dry TX on, MENU 0 8 8 5, or --dry-on, which turns it on from the PC: TX-48.2; TX-48:
    build byte 0x03, no bench bit): its bar must still
    say dry=1 before every KEY on, and what needs txtest (the suites, the AFSK level sweep) is skipped, said why;
  - the build transmits at all (`txtest` isn't refused as a receive-only build);
  - the slot is on its VFO at one of the approved test frequencies (config `test_freqs`), with no offset and no
    split (TX on the same frequency), power low, not scanning, not APRS, and it is the active slot;
  - on the cable, the KEY on is answered at once: a `tx` build holds every answer while keyed (TX-43) and a `tx-dry`
    build doesn't; no answer within 0.6 s means a real transmitter, so it unkeys at once and stops.
  The refusal checks (--refusals, --sweep refusals-wide) key on frequencies outside the band plan on purpose: those
  must be refused by the radio (each with its reason); if one is not, the bench unkeys at once and fails the check.
  Each is gated on its own frequency read back; the other gates still hold.
  KEY off is always sent in `finally` (and at exit), refused or not; Ctrl-C and SIGTERM end there too.

Extra checks: --refusals (remote PTT refused out of band, with the expected reason; no carrier on the SDR),
--keepalive (keepalives for 1 s, then none: the carrier must drop within 2.5 s of the last), --timeout (TX timeout
30 s, keyed up to 36 s with keepalives: the carrier ~30 s; the timeout put back).

Sweeps (--sweep A,B or all; alone they run no suite): many values of one thing, one short key-up each on the sweep
frequency (config sweeps.mhz, 145.495; --sweep-mhz 446.475), measured on the SDR as it goes:
  refusals-wide  remote KEY on 43 frequencies at the band plan's edges and gaps (TX-1, TX-35): refused with
                 the reason the radio's region gives (ones it allows aren't probed), no carrier; never keyed (a key-up
                 the radio accepts is unkeyed at once and fails); Extend TX must be off
  ctcss          every CTCSS tone of slot.rs and typed 60.0/260.0 Hz: frequency within 0.2 Hz, one deviation
  dcs            every DCS code, N and I, and typed D000N/D777I: decoded with its own polarity and never the other
  endtones       each end tone (0 8 4), FM and NFM, on a real unkey: steps, levels (TONE-19.4), lead-in, hold
  sublevel       Subtone level Low/Normal/High on 100.0 Hz and D023N: the deviation steps (TONE-16)
  afsk-level     `txtest afsk data L`, L 10-150 %: deviation in proportion (TERM-33.6)
One tone per key-up: the slot's tone can't change while keyed (TX-22), so a burst holds one item. Each item's
result goes to <session>/sweeps.json as it comes: --resume SESSION takes over those that passed and runs the rest
(the failures again); --from ITEM starts there (sweeps listed before its own are left out), --items A,B runs only
those, --count N at most N a sweep. A sweep that didn't cover all its items records its rules as part (◐).

Duty limit (config duty; DutyLimiter, the one place every key-up passes; a longer key-up is refused): a key-up at
most 5 s with its 0.5 s unkey tail; a rest of 2x after it; at most 25 % of any 5 min keyed, else the next key-up
waits and says so. The 60 s suite and the 30 s timeout test are exceptions (one continuous key-up each by what they
measure): over 5 s, still rested 2x after and counted in the 5 min. Refusal probes don't wait (nothing goes out
unless the radio is broken, and then it counts). Every key-up and key-down is in session.json's `duty` with its
time; the totals print at the end and show on the page.

Results: under --results (default ../bench-results, beside the ferrite checkout) one directory per session,
<date-time>_<build>, holding session.json, one <run>.json per run (key numbers and curves), each run's sdrsuite
page, txt and charts in <run>/, the capture (<run>.u8, unless --no-keep-captures) and summary.html, a single
self-contained page. The build is the radio's version plus --build LABEL, or the flashed image's hash, or the host
checkout's git hash (marked "host"). Each run is compared with the newest earlier session holding the same
configuration (run name, frequency, mode); changes beyond the tolerances (config `tolerances`: [glob, abs|rel|exact,
value], first match wins) are flagged; an `info` tolerance shows the change but never flags it. --config FILE.json overrides any default (see DEFAULTS).

Rules: bench/results.json (uitest's shape) holds each check tagged with the manual rules whose stated numbers it
measures (rule_tests(): "all" or "part" of a rule); --record puts them (and a --auto run's UI results) into the
test plan with tools/record_results.py; --rule-results SESSION writes the file for an older session.

Needs numpy and matplotlib (the project's .venv), pyserial for the cable, rtl_sdr for a capture.
"""
import argparse
import atexit
import contextlib
import random
import statistics
import datetime
import fnmatch
import hashlib
import html
import json
import math
import os
import re
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import time

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
FERRITE = os.path.dirname(HERE)
ROOT = os.path.dirname(FERRITE)
sys.path.insert(0, HERE)
import bench_rules  # noqa: E402
import benchtests  # noqa: E402
import ferrite_link as fl  # noqa: E402
import sdrsuite  # noqa: E402
import sdrtx  # noqa: E402

TX_TIMEOUT = "0 8 1"
END_TONE = "0 8 4"
SUBTONE_LEVEL = "0 8 8 1"
EXTEND_TX = "0 8 8 4"
KEY_ANSWER_S = 0.6  # TX-43: a tx-dry build answers KEY on at once; a tx build holds it while keyed

DEFAULTS = {
    "port": None,  # --port, else ferrite_link.resolve_port: FERRITE_PORT, radios.toml's default, the only cable
    "slot": "A",
    # MHz; the only frequencies the bench keys on (user approved Oct 13 2026); first and last: the VHF and UHF picks
    "test_freqs": [145.495, 446.475],
    # MHz; approved only as a same-band split's TX (VFO-16, Oct 15 2026): only tx_behaviour's split_tx keys here
    "split_freqs": [145.575],
    "runs": [
        {"name": "vhf-wide", "mhz": 145.495, "mode": "fm"},
        {"name": "vhf-nfm", "mhz": 145.495, "mode": "nfm"},
        {"name": "uhf-wide", "mhz": 446.475, "mode": "fm"},
    ],
    "aliases": {"wide": "vhf-wide", "nfm": "vhf-nfm", "uhf": "uhf-wide"},  # --load: older capture names
    "refusals": [
        {"mhz": 160.000, "reason": "outside the band plan"},
        {"mhz": 243.000, "reason": "emergency"},
    ],
    "timeout_min_s": 90,
    "timeout_raise": "120 s",
    "suite_key_s": 63.5,  # the suite's 59.95 s and ~3.5 s of the 1 kHz hold
    "sdr_lead_s": 2.0,
    "sdr_tail_s": 3.0,
    "sdr_gain": 0.9,  # the R820T's lowest fixed gain: rtl_sdr -g 0 means AUTOMATIC, which saturated a UHF run (Oct 14, 2026)
    "sdr_ppm": 0,
    # Keepalives for alive_s, then none: the carrier drops 2 s after the last one; KEY off at alive_s + wait_s (4 s:
    # inside the duty limit's 5 s burst).
    "keepalive": {"alive_s": 1.0, "wait_s": 3.0, "max_drop_s": 2.5},
    # One key-up past the 30 s setting (a duty exception): the radio ends it at 30 s, KEY off at 36 s at the latest.
    "timeout_test": {"set": "30 s", "expect_s": 30.0, "key_s": 36.0, "tol_s": 1.5},
    # The bench's own transmit duty limit (Alex, Oct 14 2026), enforced in one place (DutyLimiter, which every
    # key-up goes through): a key-up at most max_burst_s (its key time plus unkey_tail_s, the carrier hold after
    # KEY off); then a rest of rest_factor times it; and at most window_frac of any window_s keyed, else the next
    # key-up waits (printed). `exceptions` are the key-ups that can't be split by what they measure (the 60 s suite,
    # the 30 s timeout): over max_burst_s, still rested after and counted. A refusal probe (a key-up the radio must
    # refuse) waits for no rest unless probes_rest; it counts only if the radio keyed.
    "duty": {"max_burst_s": 5.0, "rest_factor": 2.0, "window_s": 300.0, "window_frac": 0.25, "unkey_tail_s": 0.5,
             "exceptions": ["suite", "timeout"], "probes_rest": False},
    # --sweep: many values of one thing, a short key-up each on `mhz` (an approved test frequency), the SDR around
    # each (lead_s before the key-up, tail_s after KEY off: the end tone and the 400 ms hold). Captures are kept
    # only for items that fail unless keep_captures. `only`: a list of item labels to run (a partial sweep).
    "sweeps": {
        "mhz": 145.495, "lead_s": 1.0, "tail_s": 1.5, "keep_captures": False,
        # every CTCSS tone of ferrite-app's slot.rs, then typed ones at TONE-15's ends
        "ctcss": {"spot": 4, "key_s": 2.0, "typed": [60.0, 260.0], "tol_hz": 0.2, "spread": 0.05},
        # every DCS code of slot.rs, N then I, then typed ones (TONE-15)
        "dcs": {"spot": 4, "key_s": 2.0, "typed": ["D000N", "D777I"], "min_windows": 2, "spread": 0.10},
        # remote KEY on each, expected refused with the reason the band plan (TX-1, TX-35) gives for the
        # radio's region; ones the region allows are not probed. Never keys: the radio refuses (or is unkeyed at once).
        "refusals-wide": {"lead_s": 0.6, "tail_s": 0.8, "freqs": [
            70.0, 121.475, 121.5, 121.525, 121.55, 136.0, 143.995, 146.0, 148.0, 150.0, 160.0, 174.0, 199.995, 200.0,
            219.0, 219.995, 220.0, 222.0, 223.5, 224.995, 225.0, 242.975, 243.0, 243.025, 243.05, 260.0, 300.0, 380.0,
            405.975, 406.0, 406.05, 406.1, 406.125, 406.15, 419.995, 420.0, 429.995, 440.0, 449.995, 450.0, 470.0,
            520.0, 900.0]},
        # each end tone (0 8 4) on a real unkey, wide and NFM: steps, levels (TONE-19.4), lead-in, hold
        "endtones": {"key_s": 1.5, "modes": ["fm", "nfm"], "extra_tail_s": 0.5, "level_tol": 0.12,
                     # TX-47.2: 400 ms hold, PA drive 0, then the 100 ms unkey wait before the chip leaves transmit.
                     # On tx-dry the PA drive is 0 throughout, so the SDR sees the chip's leak to the chip's exit:
                     # 500 ms (measured 0.495-0.505 s on all 10 end tones, Oct 14, 2026).
                     "hold_s": 0.50, "hold_tol_s": 0.05},
        # Subtone level (0 8 8 1) Low, Normal, High on a CTCSS tone and a DCS code (TONE-16)
        "sublevel": {"key_s": 2.0, "tones": ["100.0", "D023N"], "min_step": 0.05},
        # `txtest afsk data LEVEL` at each level (TERM-33.6): deviation in proportion up to 100 %, rising above
        "afsk-level": {"key_s": 3.0, "levels": [10, 25, 50, 75, 100, 125, 150], "tol": 0.05, "tol_hz": 30},
    },
    "refusal_capture_s": 4.0,
    "results": os.path.join(ROOT, "bench-results"),
    "backup": os.path.join(ROOT, "backups", "blackradio", "flash.bin"),
    "tolerances": [
        ["markers_found", "exact", 0],
        ["hold", "exact", 0],
        ["dcs.code", "exact", 0],
        # The tone generator's swell and timing per REG_40 burst vary run to run on an unchanged build (start_frac
        # swung -2.2 to +0.7, jitter 14 to 41 us): shown, never flagged. Their deviation and twist stay strict.
        ["reg40.*.start_frac", "info", 0],
        ["reg40.*.tau_ms", "info", 0],
        ["reg40.*.jitter_rms_us", "info", 0],
        # Exact at 0 errors; a run already full of errors (6855 against 6878) isn't a change.
        ["fsk4.errors", "rel", 0.05],
        ["fsk4.symbols", "rel", 0.2],
        # Absolute, so a UHF carrier offset of ~70 Hz (the SDR's ppm) moves all four together; fsk4.ratio (strict)
        # catches a real change of scale, a flip moves them by thousands.
        ["fsk4.levels.*", "abs", 100],
        ["fsk4.ratio", "abs", 0.1],
        ["sweep.*.low_hz", "rel", 0.05],
        ["sweep.*.high_hz", "abs", 30],
        ["*.twist_db", "abs", 0.5],
        ["snr_db", "abs", 6],
        ["*_db", "abs", 0.5],
        ["*.jitter_rms_us", "abs", 5],
        ["*.tau_ms", "abs", 10],
        ["*.t90_ms", "abs", 20],
        ["*.start_frac", "abs", 0.08],
        ["endtone.*.hz", "abs", 1.0],
        ["endtone.*.ms", "abs", 10],
        ["ctcss.*.hz", "abs", 0.1],
        ["dcs.baud", "abs", 0.5],
        ["dcs.windows_frac", "abs", 0.1],
        ["reg40.x3000", "abs", 8],
        ["reg40.lin3000", "abs", 8],
        ["carrier.offset_hz", "abs", 150],
        ["timing_ppm", "abs", 5],
        ["keyed_s", "abs", 1.0],
        ["*", "rel", 0.05],  # deviations and the rest
    ],
}


class BenchError(Exception):
    pass


class SafetyError(BenchError):
    """A gate refused: nothing is keyed (or it unkeyed at once)."""


class TermError(BenchError):
    pass


VERBOSE = False  # --verbose: every line; by default say() drops the per-step chatter matched by DETAIL
# Per-step detail: the per-key-up duty lines, test/step headers, calibration, save/restore notes. Safety messages
# (refusals, "KEY ... not answered", "KEYED a key-up it must refuse", stopped, errors) never match these.
DETAIL = re.compile(r"^(duty: (keyed|rest|[\d.]+ s keyed in the last)|== |calibrat|saved:|restored:|radio: |"
                    r"analysing |re-analysing |answer: |sent \$|snapshot: |battery: [a-z]+ \d+%$)")


def say(*a):
    if not VERBOSE and a and isinstance(a[0], str) and DETAIL.match(a[0].lstrip()):
        return
    print(*a, flush=True)


# ---- Clocks (the selftest's is virtual, so a 60 s key-up takes no time) ----


class Clock:
    @staticmethod
    def time():
        return time.time()

    @staticmethod
    def sleep(s):
        time.sleep(max(s, 0))


class FakeClock:
    def __init__(self):
        self.t = 1_000_000.0

    def time(self):
        return self.t

    def sleep(self, s):
        self.t += max(s, 0)


# ---- The duty limit: every key-up goes through it ----


class DutyLimiter:
    """The bench's transmit duty limit (config `duty`), so the tx-dry radio is never worked hard: a key-up is at most
    `max_burst_s` counted (its key time plus `unkey_tail_s`, the carrier after KEY off); after it a rest of at least
    `rest_factor` times what it counted, before the next key-up; and no window of `window_s` holds more than
    `window_frac` of it keyed: the next key-up waits until it fits, and says so. `exceptions` (by kind: the 60 s
    suite, the 30 s timeout test, each one continuous key-up by what it measures) may pass `max_burst_s`, never the
    window's cap, and still rest after and count. A refusal probe (kind `probe`) waits for nothing unless
    `probes_rest`: the radio must refuse it, and it counts only if the radio keyed. Every key-up and key-down is
    logged (the session's `duty`). Times on the bench's clock (virtual in the selftest)."""

    def __init__(self, dcfg, clock, out=say):
        self.c = dict(DEFAULTS["duty"], **(dcfg or {}))
        self.clock, self.out = clock, out
        self.log, self.waits = [], []
        self.t_start = clock.time()

    @property
    def cap_s(self):
        return self.c["window_frac"] * self.c["window_s"]

    def planned(self, key_s, extra_tail=0.0):
        return key_s + self.c["unkey_tail_s"] + extra_tail

    def spans(self):
        return [(e["t_on"], e["t_on"] + e["counted_s"]) for e in self.log if e["counted_s"] > 0]

    def keyed_in(self, a, b):
        return sum(max(0.0, min(b, y) - max(a, x)) for x, y in self.spans())

    def check_burst(self, planned_s, kind, purpose):
        if planned_s > self.cap_s + 1e-9:
            raise BenchError(f"duty: {purpose} would key {planned_s:.1f} s, over the window's cap of {self.cap_s:.0f} s "
                             f"in {self.c['window_s'] / 60:g} min")
        if planned_s > self.c["max_burst_s"] + 1e-9 and kind not in self.c["exceptions"]:
            raise BenchError(f"duty: {purpose} would key {planned_s:.1f} s with the unkey tail, over the "
                             f"{self.c['max_burst_s']:g} s burst limit (only {', '.join(self.c['exceptions'])} may: "
                             "config duty.exceptions)")

    def wait_needed(self, planned_s, t_key, kind):
        """(seconds to wait, why) so a key-up of `planned_s` at `t_key` keeps the rest and the window."""
        if kind == "probe" and not self.c["probes_rest"]:
            return 0.0, ""
        W, cap = self.c["window_s"], self.cap_s
        last = next((e for e in reversed(self.log) if e["counted_s"] > 0), None)
        rest = 0.0
        if last:
            rest = max(0.0, last["t_on"] + last["counted_s"] * (1 + self.c["rest_factor"]) - t_key)

        def over(w):  # the window ending at this key-up's end is the fullest one holding it
            return self.keyed_in(t_key + w + planned_s - W, t_key + w) + planned_s > cap + 1e-9

        if not over(rest):
            return rest, ("rest" if rest > 0 else "")
        lo, hi = rest, rest + W
        while hi - lo > 0.05:
            mid = (lo + hi) / 2
            lo, hi = (mid, hi) if over(mid) else (lo, mid)
        return hi, "window"

    def _sleep(self, w, why, purpose, planned_s, t_key):
        if w <= 0:
            return
        if why == "window":
            used = self.keyed_in(t_key + planned_s - self.c["window_s"], t_key)
            self.out(f"  duty: {used:.1f} s keyed in the last {self.c['window_s'] / 60:g} min and {planned_s:.1f} s "
                     f"more would pass the {self.cap_s:.0f} s cap ({self.c['window_frac']:.0%}): waiting {w:.0f} s")
        elif w >= 1:
            self.out(f"  duty: rest {w:.1f} s ({self.c['rest_factor']:g}x the last key-up)")
        self.waits.append(dict(t=self.clock.time(), s=w, why=why, before=purpose))
        self.clock.sleep(w)

    def wait(self, planned_s, kind, purpose, lead=0.0):
        """Before a capture starts: wait so the key-up `lead` s later fits (the SDR's lead overlaps the rest)."""
        self.check_burst(planned_s, kind, purpose)
        w, why = self.wait_needed(planned_s, self.clock.time() + lead, kind)
        self._sleep(w, why, purpose, planned_s, self.clock.time() + lead)

    def admit(self, planned_s, kind, purpose):
        """Right before KEY on (the one place every key-up passes): the limits again, waiting if still needed."""
        self.check_burst(planned_s, kind, purpose)
        w, why = self.wait_needed(planned_s, self.clock.time(), kind)
        self._sleep(w, why, purpose, planned_s, self.clock.time())

    def record(self, purpose, kind, ev, t_sent, extra_tail=0.0):
        """A key-up's log line: sent at t_sent, down at the radio's unkey or KEY off; counted with the unkey tail."""
        t_end = ev.get("t_unkeyed") or ev.get("t_off") or self.clock.time()
        keyed = ev.get("accepted") is not False  # refused: nothing went out; else from KEY on sent to the unkey
        keyed_s = max(t_end - t_sent, 0.0) if keyed else 0.0
        counted = keyed_s + self.c["unkey_tail_s"] + extra_tail if keyed_s > 0 else 0.0
        e = dict(purpose=purpose, kind=kind, t_on=t_sent, t_off=ev.get("t_off"), t_end=t_end, keyed_s=keyed_s,
                 counted_s=counted, refused=ev.get("refused"), unkeyed=ev.get("unkeyed"),
                 key_up=datetime.datetime.fromtimestamp(t_sent).isoformat(timespec="milliseconds"),
                 key_down=datetime.datetime.fromtimestamp(t_end).isoformat(timespec="milliseconds"))
        self.log.append(e)
        if counted > 0:
            now = t_sent + counted
            self.out(f"  duty: keyed {keyed_s:.1f} s (+{counted - keyed_s:.1f} s tail); "
                     f"{self.keyed_in(now - self.c['window_s'], now):.1f} of {self.cap_s:.0f} s in the last "
                     f"{self.c['window_s'] / 60:g} min; {sum(x['keyed_s'] for x in self.log):.0f} s keyed in all")
        return e

    def summary(self):
        W = self.c["window_s"]
        spans = self.spans()
        peak = max((self.keyed_in(b - W, b) for _, b in spans), default=0.0)
        wall = max(self.clock.time() - self.t_start, 1e-9)
        counted = sum(e["counted_s"] for e in self.log)
        return dict(limits=self.c, log=self.log, waits=self.waits, keyups=sum(1 for e in self.log if e["keyed_s"] > 0),
                    keyed_s=sum(e["keyed_s"] for e in self.log), counted_s=counted, wall_s=wall, duty=counted / wall,
                    peak_window_s=peak, peak_window_frac=peak / W, waited_s=sum(w["s"] for w in self.waits),
                    longest_s=max((e["counted_s"] for e in self.log), default=0.0))

    def report(self):
        s = self.summary()
        return (f"duty: {s['keyups']} key-ups, {s['keyed_s']:.0f} s keyed ({s['counted_s']:.0f} s with the unkey tails) "
                f"over {s['wall_s'] / 60:.1f} min ({s['duty']:.0%}); fullest {W_txt(self.c)}: {s['peak_window_s']:.0f} s "
                f"({s['peak_window_frac']:.0%}, cap {self.c['window_frac']:.0%}); waited {s['waited_s']:.0f} s")


def W_txt(c):
    return f"{c['window_s'] / 60:g} min"


# ---- The radio over the link ----


def mhz10(mhz):
    return int(round(mhz * 100000))


def parse_slot_line(line):
    """`slot A`'s line: "A* vfo 146.52000 mode=FM power=low tone=off offset=-0.60000" or
    "B  ch 12 "Club" 443.47500 mode=NFM ... txtone=100.0 rxtone=off"."""
    line = line.strip()
    m = re.match(r'([ABC])(\*?)\s+(?:(vfo|aprs)|ch (\d+)(?: "[^"]*")?)\s+(\d+\.\d+)\s*(.*)$', line)
    if not m:
        raise BenchError(f"can't read the slot line {line!r}")
    s = dict(slot=m.group(1), active=m.group(2) == "*", vfo=m.group(3) == "vfo", aprs=m.group(3) == "aprs",
             channel=int(m.group(4)) if m.group(4) else None, freq=m.group(5), line=line)
    for k, v in re.findall(r"(\w+)=(\S+)", m.group(6)):
        s[k] = v
    if "tone" in s:
        s["txtone"] = s["rxtone"] = s["tone"]
    s["scanning"] = " scanning" in line
    return s


class Radio:
    """The radio's link (ferrite_link.py's framing): HELLO, INFO, SLOT, CMD/MORE (terminal lines), KEY."""

    def __init__(self, spec, clock=None):
        self.spec = spec
        self.cable = not spec.startswith("tcp:")
        self.port = fl.Port(spec)
        self.link = fl.Link(self.port)
        self.last = time.time()
        self.hello_ans = self.hello()

    def close(self):
        with contextlib.suppress(Exception):
            self.port.close()

    def _ask(self, ty, payload=b"", timeout=2.0):
        if time.time() - self.last > 8:  # the link lets the cable go 10 s after the last frame (TERM-2)
            self.port.write(b"\x00")
            time.sleep(0.05)
        try:
            s, rest = self.link.ask(ty, payload, timeout=timeout)
        finally:
            self.last = time.time()
        return s, rest

    def hello(self):
        s, r = self._ask(fl.HELLO, bytes([fl.PROTOCOL]) + bytes(8) + b"bench.py")
        if s != 0:
            raise BenchError(f"HELLO: {fl.why(s, r)}")
        if not r[2]:
            raise BenchError("the radio's PIN lock is on (LINK-30); the bench uses the cable")
        ver, rest = fl.text(r[3:])
        return dict(protocol=r[0], version=ver, build=fl.build_name(rest), bench=fl.build_bench(rest))

    def info(self):
        s, r = self._ask(fl.INFO)
        if s != 0:
            raise BenchError(f"INFO: {fl.why(s, r)}")
        ver, r = fl.text(r)
        chans, zones, presets, slen, region = struct.unpack("<HHBHB", r[:8])  # protocol 3: zones are 2 bytes
        call, _ = fl.text(r[8:])
        return dict(version=ver, channels=chans, zones=zones, presets=presets, region=region, callsign=call)

    def term(self, line):
        """One terminal command line (LINK-34); its output. A refusal raises TermError with the radio's reason."""
        s, rest = self._ask(fl.CMD, line.encode("ascii"))
        out = []
        for _ in range(400):
            if s != 0:
                raise TermError(f"{line}: {fl.why(s, rest)}")
            out.append(rest[1:].decode("ascii", "replace"))
            if rest[0] == 0:
                return "".join(out)
            if len(rest) == 1:
                time.sleep(0.05)
            s, rest = self._ask(fl.MORE)
        raise TermError(f"{line}: no end to its output")

    def slot(self, n):
        """LINK-33: the slot's working fields, binary."""
        s, r = self._ask(fl.SLOT, bytes([n]))
        if s != 0:
            raise BenchError(f"SLOT: {fl.why(s, r)}")
        sl, act, ch, f, mode, pw, rxt, txt, off, flags = struct.unpack("<BBHIBBHHiB", r[:19])
        return dict(active=bool(act), channel=None if ch == 0xFFFF else ch, freq10=f, mode=mode, power=pw, rx_tone=rxt,
                    tx_tone=txt, offset=off, split=bool(flags & 1), aprs=bool(flags & 2), scanning=bool(flags & 4))

    # KEY (LINK-20), a step at a time so the bench's loop owns the timing.
    def key_on(self, slot_code):
        """(status, reason) of KEY on, or None when no answer came within KEY_ANSWER_S."""
        if time.time() - self.last > 10:
            # After a long quiet spell (a duty wait) the first frame went unanswered twice on the bench (Oct 14,
            # 2026): wake the link with an INFO, answered, before the KEY on.
            with contextlib.suppress(Exception):
                self.link.ask(fl.INFO, timeout=2.0)
        self.last = time.time()
        seq = self.link.send(fl.KEY, bytes([fl.PTT, fl.PTT_ON, slot_code]))
        end = time.time() + KEY_ANSWER_S
        while time.time() < end:
            for t, s, status, rest in self.link.answers():
                if t == fl.KEY | 0x80 and s == seq:
                    return status, ("" if status == 0 else fl.why(status, rest))
        return None

    def key_alive(self):
        self.last = time.time()
        self.link.send(fl.KEY, bytes([fl.PTT, fl.PTT_ALIVE]))

    def poll_unkey(self):
        """A KEY answer with a failure: the radio unkeyed (its reason), else None."""
        for t, s, status, rest in self.link.answers():
            if t == fl.KEY | 0x80 and status != 0:
                return fl.why(status, rest)
        return None

    def key_off(self):
        """KEY off; True when answered within 3 s (held answers come out first, TX-43)."""
        seq = self.link.send(fl.KEY, bytes([fl.PTT, fl.PTT_OFF]))
        end = time.time() + 3
        while time.time() < end:
            for t, s, status, rest in self.link.answers():
                if t == fl.KEY | 0x80 and s == seq:
                    self.last = time.time()
                    return True
        return False


# ---- The SDR ----


class Sdr:
    """rtl_sdr writing a capture file in the background while the bench keys."""

    def __init__(self, cfg, clock):
        self.cfg, self.clock, self.p = cfg, clock, None

    def start(self, mhz, seconds, path):
        if not shutil.which("rtl_sdr"):
            raise BenchError("rtl_sdr not found (brew install librtlsdr)")
        n = int(seconds * sdrtx.FS)
        cmd = ["rtl_sdr", "-f", str(int(mhz * 1e6 + sdrtx.OFF)), "-s", str(sdrtx.FS), "-g", str(self.cfg["sdr_gain"]),
               "-p", str(self.cfg["sdr_ppm"]), "-n", str(n), path]
        self.path, self.seconds, self.t0 = path, seconds, self.clock.time()
        self.p = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)

    def finish(self):
        left = self.t0 + self.seconds - self.clock.time()
        try:
            self.p.wait(timeout=max(left, 0) + 20)
        except subprocess.TimeoutExpired:
            self.p.kill()
            err = self.p.stderr.read().decode(errors="replace").strip().replace("\n", " | ")[-300:]
            raise BenchError(f"rtl_sdr didn't finish ({os.path.getsize(self.path) if os.path.exists(self.path) else 0}"
                             f" bytes written): {err}")
        if not os.path.exists(self.path) or os.path.getsize(self.path) < 1000:
            raise BenchError(f"rtl_sdr wrote nothing: {self.p.stderr.read().decode(errors='replace')[-300:]}")
        self.p = None
        return np.fromfile(self.path, np.uint8)

    def stop(self):
        if self.p and self.p.poll() is None:
            self.p.kill()


# ---- Measurements ----


def carrier_runs(raw, min_s=0.03, gap_s=0.05):
    """The keyed spans (s from the capture's start): power 20 dB over the floor in 10 ms blocks, gaps under `gap_s`
    joined. The capture must be mostly unkeyed (the floor is its 5th percentile)."""
    x = sdrtx.baseband(raw)
    fs = sdrtx.FS / sdrtx.DEC
    blk = int(fs * 0.01)
    p = np.abs(x) ** 2
    pb = p[: len(p) // blk * blk].reshape(-1, blk).mean(1)
    floor = np.percentile(pb, 5)
    if floor > 1e3:
        floor = 1.0
    on = pb > floor * 100
    d = np.diff(np.concatenate([[0], on.astype(int), [0]]))
    s, e = np.nonzero(d == 1)[0], np.nonzero(d == -1)[0]
    runs = []
    for a, b in zip(s, e):
        if runs and (a - runs[-1][1]) * 0.01 < gap_s:
            runs[-1][1] = b
        else:
            runs.append([a, b])
    return [(a * 0.01, b * 0.01) for a, b in runs if (b - a) * 0.01 >= min_s]


@contextlib.contextmanager
def quiet(path):
    """sdrsuite's report goes to the terminal through sys.__stdout__: into `path` instead (None: leave it)."""
    if path is None:
        yield
        return
    with open(path, "a") as f:
        old, old2 = sys.__stdout__, sys.stdout
        sys.__stdout__ = sys.stdout = f
        try:
            yield
        finally:
            sys.__stdout__, sys.stdout = old, old2


def jsonable(x):
    if isinstance(x, dict):
        return {str(k): jsonable(v) for k, v in x.items()}
    if isinstance(x, (list, tuple)):
        return [jsonable(v) for v in x]
    if isinstance(x, np.ndarray):
        return jsonable(x.tolist())
    if isinstance(x, (np.bool_, bool)):
        return bool(x)
    if isinstance(x, (np.integer,)):
        return int(x)
    if isinstance(x, (np.floating, float)):
        return None if not math.isfinite(float(x)) else float(x)
    return x


def slug(s):
    return re.sub(r"[^a-z0-9.]+", "-", str(s).lower()).strip("-")


def metrics_of(R):
    """sdrsuite's results to flat key numbers (name -> number or string) and curves (for the charts)."""
    m, curves = {}, {}
    if not R.get("fs"):
        m["markers_found"] = 0
        return m, curves
    m["keyed_s"] = R.get("keyed_s")
    m["snr_db"] = R.get("snr")
    m["carrier.offset_hz"] = R.get("offset")
    m["timing_ppm"] = R.get("timing_ppm")
    m["markers_found"] = len(R.get("markers", {}))
    S = R.get("sections", {})
    for n, name in ((1, "data"), (2, "voice")):
        if n in S:
            m[f"sweep.{name}.low_hz"] = S[n]["low"]
            m[f"sweep.{name}.high_hz"] = S[n]["high"]
            m[f"sweep.{name}.ref_dev"] = S[n]["ref"]
            curves.setdefault("sweep", {})[name] = dict(freqs=list(S[n]["freqs"]), db=list(S[n]["db"]))
    if 3 in S:
        for lvl, a in S[3].items():
            m[f"afsk.dac{lvl}.mark_dev"] = a.get(1200)
            m[f"afsk.dac{lvl}.space_dev"] = a.get(2200)
            m[f"afsk.dac{lvl}.twist_db"] = a.get("twist")
            m[f"afsk.dac{lvl}.jitter_rms_us"] = a.get("jitter_rms_us")
    if 4 in S:
        pts = []
        for b in S[4]["bursts"]:
            k = f"reg40.0x{b['reg40']:03X}" + ("" if b["filters"] == "voice" else f"-{b['filters']}")
            m[f"{k}.dev"] = b["dev"]
            m[f"{k}.twist_db"] = b.get("twist")
            m[f"{k}.jitter_rms_us"] = b.get("jitter_rms_us")
            m[f"{k}.tau_ms"] = b["swell"]["tau_ms"]
            m[f"{k}.start_frac"] = b["swell"]["start"]
            if b["filters"] == "voice":
                pts.append([b["reg40"], b["dev"]])
        curves["reg40"] = pts
        fit = S[4].get("fit")
        if fit:
            m["reg40.x3000"] = fit.get("x3000")
            m["reg40.lin3000"] = fit.get("lin3000")
            m["reg40.fit"] = fit.get("kind")
        bf = S[4].get("by_filters", {})
        for k in ("data", "flat"):
            if k in bf and bf.get("voice"):
                m[f"reg40.0x43C.{k}_vs_voice_db"] = 20 * math.log10(max(bf[k], 1e-3) / bf["voice"])
    if 5 in S:
        a = S[5]["attack"]
        m["tone1k.steady_dev"] = S[5]["steady"]
        m["tone1k.start_frac"] = a["start"]
        m["tone1k.tau_ms"] = a["tau_ms"]
        m["tone1k.t90_ms"] = a["t90_ms"]
        for t in S[5]["tones"]:
            k = f"endtone.{slug(t['name'])}.{t['want_hz']}"
            m[f"{k}.found"] = "yes" if t["found"] else "no"
            if t["found"]:
                m[f"{k}.hz"] = t["hz"]
                m[f"{k}.ms"] = t["ms"]
                m[f"{k}.start_dev"] = t["start"]
                m[f"{k}.dev"] = t["dev"]
                m[f"{k}.end_dev"] = t["end"]
    if 6 in S:
        for x in S[6]:
            if x["kind"] == "CTCSS":
                m[f"ctcss.{x['want']:.1f}.hz"] = x["hz"]
                m[f"ctcss.{x['want']:.1f}.dev"] = x["dev"]
            else:
                m["dcs.code"] = (f"{x['code']:03o} {'I' if x['inverted'] else 'N'}" if x["code"] is not None else "none")
                m["dcs.baud"] = x["baud"]
                m["dcs.dev"] = x["dev"]
                good, tot = x["windows"]
                m["dcs.windows_frac"] = good / tot if tot else None
    if S.get(7):
        r = S[7]
        m["fsk4.errors"] = r["errors"]
        m["fsk4.symbols"] = r["symbols"]
        for i, lv in enumerate(np.asarray(r["levels"]).tolist()):
            m[f"fsk4.levels.{i}"] = lv
        m["fsk4.ratio"] = r.get("ratio")
    m["hold"] = "yes" if R.get("hold") else "no"
    return jsonable(m), jsonable(curves)


HEADLINE = [
    ("Sweep data −3 dB", ["sweep.data.low_hz", "sweep.data.high_hz"], "{:.0f}–{:.0f} Hz"),
    ("Sweep voice −3 dB", ["sweep.voice.low_hz", "sweep.voice.high_hz"], "{:.0f}–{:.0f} Hz"),
    ("AFSK DAC1 100 %", ["afsk.dac100.mark_dev", "afsk.dac100.space_dev", "afsk.dac100.twist_db"], "{:.0f} / {:.0f} Hz, {:+.2f} dB"),
    ("AFSK jitter", ["afsk.dac100.jitter_rms_us"], "{:.1f} µs rms"),
    ("REG_40 for 3.0 kHz", ["reg40.x3000"], "{:.0f}"),
    ("1 kHz attack", ["tone1k.start_frac", "tone1k.tau_ms"], "starts {:.0%}, τ {:.0f} ms"),
    ("CTCSS 100.0", ["ctcss.100.0.hz", "ctcss.100.0.dev"], "{:.2f} Hz, {:.0f} Hz dev"),
    ("DCS", ["dcs.code"], "{}"),
    ("4FSK errors", ["fsk4.errors", "fsk4.symbols"], "{} of {}"),
    ("Carrier offset", ["carrier.offset_hz"], "{:+.0f} Hz"),
]


def headline(m):
    out = []
    for name, keys, fmt in HEADLINE:
        v = [m.get(k) for k in keys]
        if any(x is None for x in v):
            continue
        with contextlib.suppress(Exception):
            out.append((name, fmt.format(*v)))
    return out


# ---- Comparison ----


def tolerance(cfg, key):
    for pat, kind, val in cfg["tolerances"]:
        if fnmatch.fnmatchcase(key, pat):
            return kind, val
    return "rel", 0.05


def compare(cfg, now, prev):
    """Rows (key, now, prev, delta, tol text, status), status ok / CHANGED / missing / new; flagged first."""
    rows = []
    for k in sorted(set(now) | set(prev)):
        a, b = now.get(k), prev.get(k)
        kind, val = tolerance(cfg, k)
        tol = {"abs": f"±{val:g}", "rel": f"±{val * 100:g} %", "exact": "same", "info": "info only"}[kind]
        if a is None and b is None:
            continue
        if b is None:
            rows.append((k, a, None, None, tol, "new"))
            continue
        if a is None:
            rows.append((k, None, b, None, tol, "missing"))
            continue
        if kind == "info":
            rows.append((k, a, b, None if isinstance(a, str) or isinstance(b, str) else a - b, tol, "ok" if a == b else "info"))
            continue
        if isinstance(a, str) or isinstance(b, str) or kind == "exact":
            rows.append((k, a, b, None, tol, "ok" if a == b else "CHANGED"))
            continue
        d = a - b
        lim = val if kind == "abs" else val * max(abs(b), 1e-9)
        rows.append((k, a, b, d, tol, "ok" if abs(d) <= lim + 1e-12 else "CHANGED"))
    order = {"CHANGED": 0, "missing": 1, "new": 2, "info": 3, "ok": 4}
    rows.sort(key=lambda r: (order[r[5]], r[0]))
    return rows


def config_key(run):
    return f"{run['name']}@{run['mhz']:.4f}-{run['mode']}"


def previous_run(cfg, run, before_session):
    root = cfg["results"]
    if not os.path.isdir(root):
        return None
    for d in sorted(os.listdir(root), reverse=True):
        if d >= before_session:
            continue
        p = os.path.join(root, d, run["name"] + ".json")
        if os.path.exists(p):
            with contextlib.suppress(Exception):
                j = json.load(open(p))
                if j.get("config_key") == config_key(run) and j.get("metrics", {}).get("markers_found"):
                    j["session_dir"] = os.path.join(root, d)
                    return j
    return None


# ---- The build ----


def image_is_tx_dry(path):
    """Whether an image (.btf or raw .bin) is a bench transmit build that starts dry: the tx-bench build's boot
    line (TX-48.1: Dry TX on at power-on), or an older tx-dry image's."""
    data = open(path, "rb").read()
    if path.lower().endswith(".btf"):
        import btf
        data = btf.unpack(data)[0]
    return b"TX BENCH" in data or b"TX DRY" in data, hashlib.sha256(data).hexdigest()[:10]


def host_build():
    def git(*a):
        r = subprocess.run(["git", "-C", FERRITE, *a], capture_output=True, text=True)
        return r.stdout.strip() if r.returncode == 0 else ""
    h = git("rev-parse", "--short=7", "HEAD") or "unknown"
    dirty = bool(git("status", "--porcelain", "--untracked-files=no"))
    return h + ("+" if dirty else "")


def says_tx_dry(version):
    return bool(re.search(r"tx[ _-]?dry|\bdry\b", version or "", re.I))


# ---- Sweeps (--sweep): many values of one thing, one short key-up each ----

SWEEP_NAMES = ["refusals-wide", "ctcss", "dcs", "endtones", "sublevel", "afsk-level"]
SLOT_RS = os.path.join(FERRITE, "crates", "ferrite-app", "src", "slot.rs")
# 0 8 4's choices (ferrite-app settings.rs END_TONE_NAMES) and their steps in END_TONES
ENDTONE_SETTINGS = [("Beep", "beep"), ("Two-tone up", "two-tone-up"), ("Two-tone down", "two-tone-down"),
                    ("Chirp", "chirp"), ("Quindar", "quindar")]
TONE_19_4 = {800: 1100, 1000: 1450, 1500: 2300, 2475: 2560}  # Hz deviation at REG_40 0x43C, wide FM; NFM halves it


def tone_lists():
    """The CTCSS tones (Hz) and DCS codes Ferrite offers: ferrite-app slot.rs's CTCSS and DCS arrays."""
    src = open(SLOT_RS).read()

    def arr(name):
        m = re.search(rf"pub const {name}: \[u16; (\d+)\] = \[(.*?)\];", src, re.S)
        if not m:
            raise BenchError(f"no {name} array in {SLOT_RS}")
        vals = [int(x, 0) for x in re.findall(r"0o[0-7]+|\d+", m.group(2))]
        if len(vals) != int(m.group(1)):
            raise BenchError(f"{SLOT_RS}: {name} has {len(vals)} values for [u16; {m.group(1)}]")
        return vals

    return [v / 10 for v in arr("CTCSS")], arr("DCS")


def tone_word(t):
    """A terminal tone word (off, 100.0, D023N, D023I) as SLOT gives it: 0.1 Hz, or bit 15 DCS, bit 14 inverted."""
    if t.lower() == "off":
        return 0
    m = re.fullmatch(r"[Dd]([0-7]{3})([NIni])", t)
    if m:
        return 0x8000 | int(m.group(1), 8) | (0x4000 if m.group(2).upper() == "I" else 0)
    return int(round(float(t) * 10))


# The band plan as ferrite-core's txplan.rs (TX-1's table, the 18-64 MHz bands of TX-4, the emergency frequencies)
# and the order a remote KEY meets the checks (remote.rs: emergency first; then transmit.rs: the band plan). 1.25 m
# transmits like any band since Oct 21, 2026 (TX-9, TX-10: as stock).
TX_HF = [(18.068, 18.168), (21.0, 21.45), (24.89, 24.99), (28.0, 29.7)]
TX_BANDS = {"US": [(144, 148), (222, 225), (420, 450)], "CA": [(144, 148), (219, 225), (430, 450)],
            "AU": [(144, 148), (420, 450)], "NZ": [(144, 148), (430, 440)], "ITU": [(144, 146), (430, 440)]}
REGIONS = {0: ("no region set", "ITU"), 1: ("the US", "US"), 2: ("Canada", "CA"), 3: ("the UK", "ITU"),
           4: ("Europe", "ITU"), 5: ("Australia", "AU"), 6: ("New Zealand", "NZ"), 7: ("Japan", "ITU"),
           8: ("this region", "ITU")}


def region_of(byte):
    return REGIONS.get(byte or 0, REGIONS[0])


def tx_expect(mhz, region_byte):
    """Why a remote KEY on `mhz` is refused with Extend TX off: "emergency" or "band plan" (words of the radio's
    reasons), or None where it transmits."""
    f = mhz10(mhz)

    def r(lo, hi):
        return mhz10(lo) <= f < mhz10(hi)

    m = 2500  # ±25 kHz, inclusive (txplan::emergency)
    if mhz10(406.0) - m <= f <= mhz10(406.1) + m or abs(f - mhz10(243.0)) <= m or abs(f - mhz10(121.5)) <= m:
        return "emergency"
    plan = region_of(region_byte)[1]
    six = (50, 54) if plan in ("US", "CA", "AU", "NZ") or region_byte == 7 else (50, 52)  # 6 m: 50-54 in Japan too
    if not any(r(lo, hi) for lo, hi in TX_BANDS[plan] + TX_HF + [six]):
        return "band plan"
    return None


def sweep_items(cfg, name):
    """The sweep's items in order, each a dict with its `label` (what --from, --items and --resume name)."""
    sw = cfg["sweeps"]
    c = sw.get(name, {})
    if name == "ctcss":
        ctcss, _ = tone_lists()
        items = [dict(label=f"{t:.1f}", tone=f"{t:.1f}", hz=t, typed=False) for t in ctcss]
        items += [dict(label=f"{t:.1f}", tone=f"{t:.1f}", hz=float(t), typed=True) for t in c["typed"] if t not in ctcss]
    elif name == "dcs":
        _, dcs = tone_lists()
        items = [dict(label=f"D{d:03o}{p}", tone=f"D{d:03o}{p}", code=d, inverted=p == "I", typed=False)
                 for d in dcs for p in "NI"]
        for t in c["typed"]:
            m = re.fullmatch(r"D([0-7]{3})([NI])", t)
            items.append(dict(label=t, tone=t, code=int(m.group(1), 8), inverted=m.group(2) == "I", typed=True))
    elif name == "refusals-wide":
        items = [dict(label=f"{f:.3f}", mhz=float(f)) for f in c["freqs"]]
    elif name == "endtones":
        items = [dict(label=f"{slug_}-{mode}", setting=nm, slug=slug_, mode=mode, steps=dict(END_TONES)[slug_])
                 for mode in c["modes"] for nm, slug_ in ENDTONE_SETTINGS]
    elif name == "sublevel":
        items = []
        for t in c["tones"]:
            for lvl in ("Low", "Normal", "High"):
                it = dict(label=f"{lvl}-{t}", level=lvl, tone=t)
                m = re.fullmatch(r"D([0-7]{3})([NI])", t)
                if m:
                    it.update(code=int(m.group(1), 8), inverted=m.group(2) == "I")
                else:
                    it.update(hz=float(t))
                items.append(it)
    elif name == "afsk-level":
        items = [dict(label=f"afsk-{lvl}", level=int(lvl)) for lvl in c["levels"]]
    else:
        raise BenchError(f"no sweep {name!r}; the sweeps: {', '.join(SWEEP_NAMES)}")
    return items


def sweep_key_s(cfg, name):
    """(key-up seconds, extra unkey tail) of one item of a sweep, for the duty limit."""
    c = cfg["sweeps"][name]
    if name == "refusals-wide":
        return KEY_ANSWER_S, 0.0
    return c["key_s"], c.get("extra_tail_s", 0.0)


def _keyed_dev(raw):
    """The keyed part's deviation (Hz, offset removed, low-passed at 6 kHz as sdrsuite's) and where it is."""
    k = sdrsuite.keyed(raw)
    if k is None:
        return None
    dev, fs, off, i0, i1, snr, clip = k
    return dict(dev=sdrtx.conv_same(dev, sdrtx.lowpass(6000, fs)), fs=fs, off=off, i0=i0, i1=i1, snr=float(snr),
                clip=float(clip))


def analyse_subtone(raw, item, tol_hz=0.2, min_windows=2):
    """A CTCSS tone's frequency and deviation, or a DCS code's polarity (its own windows, none of the other
    polarity's) and deviation, from the key-up's steady middle (0.4 s in, 0.5 s clear of the carrier's end: the
    hold and any tail elimination)."""
    k = _keyed_dev(raw)
    if not k:
        return dict(ok=False, why="no carrier on the SDR", text="no carrier on the SDR")
    dev, fs = k["dev"], k["fs"]
    base = dict(keyed_s=len(dev) / fs, snr_db=k["snr"], offset_hz=k["off"])
    a, b = int(0.4 * fs), len(dev) - int(0.5 * fs)
    if b - a < int(0.8 * fs):
        return dict(base, ok=False, why=f"keyed only {len(dev) / fs:.2f} s", text=f"keyed only {len(dev) / fs:.2f} s")
    x, fs2 = sdrsuite.sub_band(dev, fs, a, b)
    if "hz" in item:
        hz, d = sdrsuite.ctcss(x, fs2, 55, 265)
        err = hz - item["hz"]
        ok = abs(err) <= tol_hz
        return dict(base, hz=hz, err_hz=err, dev=d, ok=ok, why="" if ok else f"{hz:.2f} Hz",
                    text=f"{hz:.2f} Hz ({err:+.2f}), {d:.0f} Hz dev")
    votes, baud, d, n = sdrsuite.dcs_votes(x, fs2)
    own = votes.get((item["code"], item["inverted"]), 0)
    other = votes.get((item["code"], not item["inverted"]), 0)
    top = max(votes.items(), key=lambda kv: (kv[0][0] in sdrsuite.DCS_CODES, not kv[0][1], kv[1]))[0] if votes else None
    name = f"D{top[0]:03o}{'I' if top[1] else 'N'}" if top else "none"
    ok = own >= min_windows and other == 0
    want = f"D{item['code']:03o}{'I' if item['inverted'] else 'N'}"
    return dict(base, decoded=name, own=own, other=other, windows=n, baud=baud, dev=d, ok=ok,
                why="" if ok else f"{own} windows {want}, {other} of the other polarity (decoded {name})",
                text=f"{own} of {n} windows {want}, {other} other polarity (named {name}); {baud:.1f} bit/s, {d:.0f} Hz dev")


def analyse_endtone(raw, item, sc):
    """An end tone on a real unkey: its steps (frequency, length, deviation, the first one's start against its
    steady level), the quiet before it (the silent lead-in) and the carrier after it (the 400 ms hold)."""
    k = _keyed_dev(raw)
    if not k:
        return dict(ok=False, why="no carrier on the SDR", text="no carrier on the SDR")
    dev, fs = k["dev"], k["fs"]
    runs = carrier_runs(raw)
    c_end = runs[-1][1] if runs else None
    a = max(0, len(dev) - int(1.8 * fs))
    seg = dev[a:] - np.median(dev[a:])
    t_seg = (k["i0"] + 1 + a) / fs  # seg's start, s from the capture's start
    steps = item["steps"]
    m = sdrsuite.match_tones(seg, fs, [(str(hz), hz, ms) for hz, ms in steps])
    out = dict(keyed_s=len(dev) / fs, snr_db=k["snr"], steps=[], mode=item["mode"])
    nfm = item["mode"] == "nfm"
    found = [x for x in m if x["found"]]
    bad = []
    for x in m:
        st = dict(want_hz=x["want_hz"], want_ms=x["want_ms"], found=x["found"])
        if x["found"]:
            fig = TONE_19_4.get(x["want_hz"])
            want_dev = fig * (0.5 if nfm else 1.0) if fig else None
            st.update(hz=x["hz"], ms=x["ms"], dev=x["dev"], start=x["start"], want_dev=want_dev,
                      hz_ok=abs(x["hz"] - x["want_hz"]) <= x["want_hz"] * RULE_TOL["endtone_hz_frac"],
                      ms_ok=abs(x["ms"] - x["want_ms"]) <= RULE_TOL["endtone_ms"],
                      dev_ok=None if want_dev is None else abs(x["dev"] / want_dev - 1) <= sc["level_tol"])
            bad += [f"{x['want_hz']} Hz {w}" for w, okk in (("frequency", st["hz_ok"]), ("length", st["ms_ok"]),
                                                            ("level", st["dev_ok"])) if okk is False]
        else:
            bad.append(f"{x['want_hz']} Hz not found")
        out["steps"].append(st)
    if found and len(found) == len(m):
        first, last = found[0], found[-1]
        out["hold_s"] = (c_end - (t_seg + last["t1"])) if c_end is not None else None
        q0, q1 = int((first["t0"] - 0.17) * fs), int((first["t0"] - 0.03) * fs)
        q = seg[max(q0, 0): max(q1, 0)]
        out["lead_quiet"] = (float(np.sqrt(2 * np.mean((q - q.mean()) ** 2))) / max(first["dev"], 1e-9)
                             if q0 >= 0 and len(q) > int(0.1 * fs) else None)
        out["start_frac"] = first["start"] / max(first["dev"], 1e-9)
        hold_ok = out["hold_s"] is not None and abs(out["hold_s"] - sc["hold_s"]) <= sc["hold_tol_s"]
        lead_ok = out["lead_quiet"] is not None and out["lead_quiet"] < 0.25 and out["start_frac"] >= 0.8
        if not hold_ok:
            bad.append(f"hold {_f(out['hold_s'], '{:.3f} s')}")
        if not lead_ok:
            bad.append(f"lead-in quiet {_f(out['lead_quiet'], '{:.2f}')}, first step starts at {out['start_frac']:.0%}")
    out["ok"] = not bad
    out["why"] = "; ".join(bad)
    out["text"] = (" ".join(f"{_f(st.get('hz'), '{:.0f}')}/{_f(st.get('ms'), '{:.0f}')}ms/{_f(st.get('dev'), '{:.0f}')}"
                            for st in out["steps"])
                   + f"; hold {_f(out.get('hold_s'), '{:.3f}')} s; lead-in quiet {_f(out.get('lead_quiet'), '{:.2f}')}"
                   + f", starts at {_f(out.get('start_frac'), '{:.0%}')}")
    return out


def analyse_afsk(raw):
    """`txtest afsk data LEVEL`'s mark and space deviation (sdrtx's afsk), 0.3 s in, 0.5 s clear of the end."""
    k = _keyed_dev(raw)
    if not k:
        return dict(ok=False, why="no carrier on the SDR", text="no carrier on the SDR")
    dev, fs = k["dev"], k["fs"]
    a, b = int(0.3 * fs), len(dev) - int(0.5 * fs)
    if b - a < int(1.0 * fs):
        return dict(ok=False, why=f"keyed only {len(dev) / fs:.2f} s", text=f"keyed only {len(dev) / fs:.2f} s")
    with quiet(os.devnull):
        r = sdrtx.afsk(dev[a:b], fs)
    mk, sp = r.get(1200) or 0.0, r.get(2200) or 0.0
    ok = mk > 0 and sp > 0
    return dict(keyed_s=len(dev) / fs, snr_db=k["snr"], mark=mk, space=sp, mean=(mk + sp) / 2, twist_db=r.get("twist"),
                ok=ok, why="" if ok else "no AFSK found", text=f"mark {mk:.0f} Hz, space {sp:.0f} Hz")


def load_resume(path):
    """--resume: a session directory's (or its sweeps.json's) measured items, {sweep: {label: item}} (the sweep
    takes over the ones that passed and runs the rest again)."""
    p = path if path.endswith(".json") else os.path.join(path, "sweeps.json")
    if not os.path.exists(p):
        raise BenchError(f"--resume: no sweeps.json in {path}")
    j = json.load(open(p))
    sess = os.path.basename(os.path.dirname(os.path.abspath(p)))
    out = {}
    for name, sw in j.items():
        for label, it in (sw.get("items") or {}).items():
            out.setdefault(name, {})[label] = dict(it, session=it.get("session") or sess, resumed=True)
    return out


def sweep_selection(cfg, names, a):
    """[(sweep, [items to run now])] for --sweep with --from / --items / --count (resume skips are done later):
    sweeps before the one holding --from's label are left out; it starts there; those after run whole."""
    sel = []
    started = not a.sweep_from
    only = set(x for x in (a.sweep_items or "").split(",") if x)
    for name in names:
        items = sweep_items(cfg, name)
        labels = [i["label"] for i in items]
        if not started:
            if a.sweep_from not in labels:
                continue
            items = items[labels.index(a.sweep_from):]
            started = True
        conf_only = set(cfg["sweeps"].get(name, {}).get("only") or [])
        if only or conf_only:
            items = [i for i in items if i["label"] in (only | conf_only)]
        spot = cfg["sweeps"].get(name, {}).get("spot")
        if spot and not getattr(a, "exhaustive", False) and not (only or a.sweep_from or a.resume):
            # The chip's own tone and code hardware passed whole once (Oct 14, 2026); later runs spot-check a few at
            # random (Alex); every item only with --exhaustive.
            items = sorted(random.sample(items, min(spot, len(items))), key=lambda i: labels.index(i["label"]))
        sel.append((name, items))
    if not started:
        raise BenchError(f"--from {a.sweep_from}: no such item in {', '.join(names)}")
    return sel


def parse_sweeps(spec):
    names = SWEEP_NAMES if spec == "all" else [x for x in spec.split(",") if x]
    for n in names:
        if n not in SWEEP_NAMES:
            sys.exit(f"no sweep {n!r}; the sweeps: {', '.join(SWEEP_NAMES)} (or all)")
    return names


def estimate_sweep(cfg, name, n, overhead_s=1.5):
    """(wall s, keyed s, counted s) of n items of a sweep at the duty limits, from a fresh limiter (the bench's own
    simulated on a virtual clock): each item's set-up and analysis `overhead_s`, the SDR's lead, the key-up, the tail."""
    clk = FakeClock()
    d = DutyLimiter(cfg.get("duty"), clk, out=lambda *_: None)
    sc = cfg["sweeps"]
    lead = sc.get(name, {}).get("lead_s", sc["lead_s"])
    tail = sc.get(name, {}).get("tail_s", sc["tail_s"])
    key_s, extra = sweep_key_s(cfg, name)
    probe = name == "refusals-wide"
    t0 = clk.time()
    for _ in range(n):
        clk.sleep(overhead_s)
        planned = d.planned(key_s, extra)
        d.wait(planned, "probe" if probe else "burst", name, lead)
        clk.sleep(lead)
        d.admit(planned, "probe" if probe else "burst", name)
        t = clk.time()
        clk.sleep(key_s)
        d.record(name, "probe" if probe else "burst", dict(accepted=not probe, t_on=t, t_off=clk.time()), t, extra)
        clk.sleep(tail)
    s = d.summary()
    return clk.time() - t0, s["keyed_s"], s["counted_s"]


# ---- The bench ----


class Bench:
    def __init__(self, cfg, args, radio=None, sdr=None, clock=None, log=None):
        self.cfg, self.a = cfg, args
        self.radio, self.sdr = radio, sdr
        self.clock = clock or Clock()
        self.slot_letter = cfg["slot"].upper()
        self.slot_n = "ABC".index(self.slot_letter)
        self.slot_code = fl.SLOTS[self.slot_letter.lower()]
        self.keyed = False
        self.gatelog = []
        self.warnings = []
        self.tx_dry = None  # how it is known, or None
        self.bench = True  # the bench commands (txtest) are in: False on a tx build with Dry TX on (TX-48)
        self.live_dry = False  # a tx build with Dry TX on: the bar's dry=1 is checked before every KEY on (TX-48)
        self.restore_notes = []
        self.saved = None
        self.out = log or say
        self.duty = DutyLimiter(cfg.get("duty"), self.clock, self.out)
        self.info = {}
        self.extra_settings = []  # settings a sweep changes, saved and put back (0 8 8 1)

    # -- gates --
    def gate(self, what, ok, detail=""):
        self.gatelog.append(dict(gate=what, ok=bool(ok), detail=detail))
        if not ok:
            raise SafetyError(f"refused: {what}{': ' + detail if detail else ''}")

    def check_build(self, hello, info, flashed_dry=None):
        ver = info.get("version") or hello.get("version")
        said = hello.get("build", "not given")
        if said == "tx" and getattr(self.a, "dry_on", False):
            # TX-48.2: the PC may turn Dry TX on (never off); the radio must then say tx+dry, checked below as usual.
            self.radio.term("set 0 8 8 5 On")
            hello = self.radio.hello()
            said = hello.get("build", "not given")
            self.out(f"Dry TX turned on from the PC (--dry-on, TX-48.2): the radio now says {said}")
        if said not in ("not given", "tx-dry", "tx+dry"):
            # LINK-35 (decision 151): the radio names its build; a receive or full-power tx build is never keyed,
            # unless Dry TX is on (TX-48: it then says tx+dry; the tx-bench build starts so, TX-48.1).
            self.gate("tx-dry build", False, f"the radio says it runs a {said} build (LINK-35): turn Dry TX on on "
                      "the radio (MENU 0 8 8 5; TX-48)" if said == "tx" else
                      f"the radio says it runs a {said} build (LINK-35): flash a tx-bench image")
        if said == "tx+dry":
            # TX-48: Dry TX on. The tx-bench build (decision 193) has the bench commands too (the bench bit); a
            # plain tx build doesn't (no txtest): its suites skip.
            self.bench, self.live_dry = bool(hello.get("bench")), True
            self.tx_dry = "the radio says so (LINK-35 build byte: tx with Dry TX on, TX-48)"
            self.gate("tx-dry build", True, self.tx_dry)
            return
        if said == "tx-dry":
            self.gate("a bench build (txtest is in it)", hello.get("bench"),
                      "the build byte says tx-dry without the bench bit (0x80): txtest isn't in it")
        if said == "tx-dry" and not flashed_dry:
            self.tx_dry = "the radio says so (LINK-35 build byte: tx-dry, bench)"
        elif flashed_dry:
            self.tx_dry = f"flashed {flashed_dry} (its image has the TX DRY boot line)"
        elif says_tx_dry(ver) or says_tx_dry(hello.get("version")):
            self.tx_dry = f"the version says so ({ver})"
        elif self.a.i_know_its_tx_dry:
            self.tx_dry = "--i-know-its-tx-dry (not verifiable over the link)"
            w = (f"WARNING: the radio's version ({ver!r}) doesn't say tx-dry and nothing was flashed here; going on "
                 "because of --i-know-its-tx-dry. A `tx` build would transmit at full drive. On the cable the KEY "
                 "answer check (TX-43) still stops a `tx` build at its first key-up.")
            self.warnings.append(w)
            self.out(w)
        self.gate("tx-dry build", self.tx_dry is not None,
                  self.tx_dry or f"version {ver!r} doesn't say tx-dry (Ferrite v0.1 can't tell over the link): flash "
                                 "a tx-dry image with --flash, or check the boot screen's TX DRY line and give "
                                 "--i-know-its-tx-dry")
        try:
            t = self.radio.term("txtest")
            ok = "receive-only" not in t
        except TermError as e:
            t, ok = str(e), "receive-only" not in str(e)
        self.gate("a transmit build (txtest answers)", ok, t.strip()[:120])

    def gate_key(self, mhz_allowed, purpose):
        """Every gate on the slot, right before a KEY on."""
        st = self.radio.slot(self.slot_n)
        f = st["freq10"]
        self.gate(f"{purpose}: frequency approved", f in [mhz10(x) for x in mhz_allowed],
                  f"{f / 1e5:.5f} MHz; allowed {', '.join(f'{x:.3f}' for x in mhz_allowed)}")
        self.gate(f"{purpose}: on the VFO", st["channel"] is None and not st["aprs"], f"channel {st['channel']}")
        self.gate(f"{purpose}: no offset, no split", st["offset"] == 0 and not st["split"],
                  f"offset {st['offset'] / 1e5:+.5f} MHz{' split' if st['split'] else ''}: set the shift to simplex "
                  "on the radio, or give --clear-offset")
        self.gate(f"{purpose}: power low", st["power"] == 0, ["low", "mid", "high"][st["power"] % 3])
        self.gate(f"{purpose}: not scanning", not st["scanning"])
        self.gate(f"{purpose}: slot {self.slot_letter} active", st["active"])
        self.gate(f"{purpose}: tx-dry", self.tx_dry is not None, self.tx_dry or "")
        if self.live_dry:
            # TX-48: Dry TX can be turned off on the radio at any time: its bar must still say dry=1.
            bar = fl.parse_ui(self.radio.term("ui")).get("bar") or {}
            self.gate(f"{purpose}: Dry TX still on (TX-48)", bar.get("dry") == 1,
                      f"the radio's bar says dry={bar.get('dry')}: Dry TX was turned off on the radio")
        return st

    # -- the radio's state --
    def setting(self, path):
        t = self.radio.term(f"get {path}").strip()
        m = re.match(rf"{re.escape(path)} [^:]*: (.*?)(?: \(not the default\))?$", t)
        if not m:
            raise BenchError(f"can't read setting {path}: {t!r}")
        return m.group(1)

    def save_state(self):
        line = self.radio.term(f"slot {self.slot_letter}")
        s = parse_slot_line(line)
        sv = dict(slot=s, active=None, end_tone=None, timeout=None)
        for n in range(3):
            if self.radio.slot(n)["active"]:
                sv["active"] = "ABC"[n]
        if s["channel"] is not None:
            # Its VFO's own state is what changes; the channel itself is never touched.
            vfo = parse_slot_line(self.radio.term(f"slot {self.slot_letter} vfo"))
            sv["vfo"] = vfo
            self.restore_notes.append(f"slot {self.slot_letter} was on channel {s['channel']}: its VFO is put back, "
                                      "but press V/M on the radio to return to the channel (the slot command can't)")
        else:
            sv["vfo"] = s
        sv["end_tone"] = self.setting(END_TONE)
        sv["timeout"] = self.setting(TX_TIMEOUT)
        sv["extra"] = {p: self.setting(p) for p in self.extra_settings}
        self.saved = sv
        self.out(f"saved: {s['line']}; active {sv['active']}; end tone {sv['end_tone']}; TX timeout {sv['timeout']}"
                 + "".join(f"; {p} {v}" for p, v in sv["extra"].items()))
        return sv

    def restore(self):
        sv = self.saved
        if not sv:
            return
        steps = [("txtest off", "txtest off")] if self.bench else []
        v = sv["vfo"]
        words = [f"slot {self.slot_letter} vfo {v['freq']}", v.get("mode", "").lower(), v.get("power", "")]
        if v.get("txtone"):
            words += ["txtone", v["txtone"], "rxtone", v.get("rxtone", "off")]
        steps.append(("slot", " ".join(w for w in words if w)))
        steps.append(("end tone", f"set {END_TONE} {sv['end_tone']}"))
        steps.append(("TX timeout", f"set {TX_TIMEOUT} {sv['timeout']}"))
        for p, v in (sv.get("extra") or {}).items():
            steps.append((p, f"set {p} {v}"))
        if sv["active"]:
            steps.append(("active slot", f"slot {sv['active']} active"))
        for what, line in steps:
            try:
                self.radio.term(line)
            except Exception as e:  # keep putting the rest back
                self.restore_notes.append(f"restore {what} ({line}) failed: {e}")
        with contextlib.suppress(Exception):
            now = parse_slot_line(self.radio.term(f"slot {self.slot_letter}"))
            for k in ("freq", "mode", "power", "txtone", "rxtone", "offset"):
                if now.get(k) != v.get(k):
                    self.restore_notes.append(f"slot {self.slot_letter} {k}: was {v.get(k)}, now {now.get(k)}")
        self.out("restored: " + ("; ".join(self.restore_notes) if self.restore_notes else "everything as it was"))

    def setup_slot(self, mhz, mode):
        L = self.slot_letter
        self.radio.term(f"slot {L} vfo {mhz:.5f} {mode} low tone off")
        st = self.radio.slot(self.slot_n)
        if (st["offset"] or st["split"]) and self.a.clear_offset and not st["split"]:
            # The keypad's own rule (entry::tune_vfo): a shift is scaled to the new band's, zero where the band has
            # none; a tune there and back clears it. The original shift is noted as not put back.
            self.radio.term(f"slot {L} vfo 160.00000")
            self.radio.term(f"slot {L} vfo {mhz:.5f}")
            self.restore_notes.append(f"slot {L}'s VFO shift was cleared (--clear-offset): set it again on the radio")
        if not st["active"]:
            self.radio.term(f"slot {L} active")
        self.radio.term(f"set {END_TONE} Off")
        t = self.setting(TX_TIMEOUT)
        secs = int(re.match(r"(\d+)", t).group(1)) if re.match(r"\d+", t) else 10 ** 6  # Off: none
        if secs < self.cfg["timeout_min_s"]:
            self.radio.term(f"set {TX_TIMEOUT} {self.cfg['timeout_raise']}")

    # -- keying --
    def key_session(self, seconds, purpose, alive_s=None, stop_on_unkey=True, kind="burst", expect_refusal=False,
                    extra_tail=0.0):
        """Key (LINK-20), keepalives every 0.5 s (only for the first `alive_s` s if given), KEY off after `seconds`;
        always KEY off in `finally`. The duty limit admits it first and logs it after (kind: burst, suite, timeout,
        probe). expect_refusal: a refusal probe, KEY off at once if the radio keys. Times on the bench's clock."""
        ev = dict(purpose=purpose, kind=kind, t_on=None, accepted=None, refused=None, t_last_alive=None, unkeyed=None,
                  t_unkeyed=None, t_off=None, off_answered=None)
        c = self.clock
        self.duty.admit(self.duty.planned(KEY_ANSWER_S if expect_refusal else seconds, extra_tail), kind, purpose)
        t_sent = c.time()
        try:
            self.keyed = True
            r = self.radio.key_on(self.slot_code)
            ev["t_on"] = c.time()
            if r is None:
                if self.radio.cable:
                    raise SafetyError("KEY on wasn't answered within 0.6 s: a `tx` build holds its answers while "
                                      "keyed (TX-43), a tx-dry build doesn't. Unkeyed at once; stopping.")
                ev["accepted"] = True  # not on the cable: answers may be late
            elif r[0] != 0:
                ev["accepted"], ev["refused"] = False, r[1]
                return ev
            else:
                ev["accepted"] = True
            if expect_refusal:
                self.out("  the radio KEYED a key-up it must refuse: KEY off at once")
                return ev
            stop = ev["t_on"] + seconds
            while c.time() < stop:
                if alive_s is None or c.time() < ev["t_on"] + alive_s:
                    self.radio.key_alive()
                    ev["t_last_alive"] = c.time()
                u = self.radio.poll_unkey()
                if u and ev["unkeyed"] is None:
                    ev["unkeyed"], ev["t_unkeyed"] = u, c.time()
                    self.out(f"  the radio unkeyed: {u}")
                    if stop_on_unkey:
                        break
                c.sleep(min(0.5, max(stop - c.time(), 0)))
        finally:
            try:
                ev["off_answered"] = self.radio.key_off()
            finally:
                ev["t_off"] = c.time()
                self.keyed = False
                ev["duty"] = self.duty.record(purpose, kind, ev, t_sent, extra_tail)
        return ev

    def emergency_unkey(self):
        if self.keyed and self.radio:
            with contextlib.suppress(Exception):
                self.radio.key_off()
            self.keyed = False

    # -- the steps --
    def capture_keyed(self, mhz, key_s, path, purpose, alive_s=None, stop_on_unkey=True, lead=None, tail=None,
                      kind="burst", expect_refusal=False, extra_tail=0.0):
        lead = self.cfg["sdr_lead_s"] if lead is None else lead
        tail = self.cfg["sdr_tail_s"] if tail is None else tail
        total = lead + key_s + tail
        # The duty limit's wait comes before the SDR starts (its lead overlaps the rest); key_session admits again.
        self.duty.wait(self.duty.planned(KEY_ANSWER_S if expect_refusal else key_s, extra_tail), kind, purpose, lead)
        self.sdr.start(mhz, total, path)
        t0 = self.clock.time()
        try:
            self.clock.sleep(lead)
            ev = self.key_session(key_s, purpose, alive_s=alive_s, stop_on_unkey=stop_on_unkey, kind=kind,
                                  expect_refusal=expect_refusal, extra_tail=extra_tail)
            self.clock.sleep(max(t0 + total - self.clock.time(), 0))
            raw = self.sdr.finish()
        except BaseException:
            self.sdr.stop()
            raise
        ev["t_capture"] = t0
        return ev, raw

    def run_suite(self, run, sess_dir):
        name, mhz, mode = run["name"], run["mhz"], run["mode"]
        self.out(f"\n== {name}: {mhz:.3f} MHz {mode.upper()}, slot {self.slot_letter}")
        self.setup_slot(mhz, mode)
        armed = self.radio.term("txtest suite")
        if "armed" not in armed:
            raise BenchError(f"txtest suite not armed: {armed.strip()}")
        self.gate_key(self.cfg["test_freqs"], name)
        path = os.path.join(sess_dir, name + ".u8")
        ev, raw = self.capture_keyed(mhz, self.cfg["suite_key_s"], path, name, kind="suite")
        if ev["refused"]:
            raise BenchError(f"{name}: KEY refused: {ev['refused']}")
        self.radio.term("txtest off")
        return dict(run=run, capture=path, events=ev, raw_bytes=int(len(raw)))

    def run_refusals(self, sess_dir):
        out = []
        L = self.slot_letter
        for r in self.cfg["refusals"]:
            f = r["mhz"]
            self.out(f"\n== refusal at {f:.3f} MHz (expect: {r['reason']})")
            res = dict(mhz=f, expect=r["reason"])
            try:
                self.radio.term(f"slot {L} vfo {f:.5f} fm low tone off")
            except TermError as e:
                res.update(ok=False, why=f"couldn't tune: {e}")
                out.append(res)
                continue
            self.gate_key([f], f"refusal {f:.3f}")
            path = os.path.join(sess_dir, f"refusal-{f:.3f}.u8")
            ev, raw = self.capture_keyed(f, 1.0, path, f"refusal {f:.3f}", lead=1.5, tail=1.5, kind="probe",
                                         expect_refusal=True)
            runs = carrier_runs(raw)
            reason_ok = bool(ev["refused"]) and r["reason"].lower() in ev["refused"].lower()
            res.update(refused=ev["refused"], accepted=ev["accepted"], carrier=[list(x) for x in runs],
                       ok=reason_ok and not runs and not ev["accepted"])
            if ev["accepted"]:
                res["why"] = "the radio KEYED (unkeyed at once)"
            elif not reason_ok:
                res["why"] = f"refused, but the reason differs: {ev['refused']!r}"
            elif runs:
                res["why"] = f"a carrier on the SDR: {runs}"
            self.out(f"  {'pass' if res['ok'] else 'FAIL'}: {ev['refused'] or 'not refused'}; "
                     f"{'no carrier' if not runs else 'carrier ' + str(runs)}")
            if not self.a.keep_captures:
                with contextlib.suppress(OSError):
                    os.remove(path)
            out.append(res)
        return out

    def run_keepalive(self, sess_dir):
        ka = self.cfg["keepalive"]
        run = self.cfg["runs"][0]
        self.out(f"\n== keepalive: keepalives for {ka['alive_s']} s, then none (drop expected within {ka['max_drop_s']} s)")
        self.setup_slot(run["mhz"], "fm")
        self.gate_key(self.cfg["test_freqs"], "keepalive")
        path = os.path.join(sess_dir, "keepalive.u8")
        ev, raw = self.capture_keyed(run["mhz"], ka["alive_s"] + ka["wait_s"], path, "keepalive",
                                     alive_s=ka["alive_s"], stop_on_unkey=False)
        res = analyse_keepalive(raw, ev, ka)
        json.dump(jsonable(dict(events=ev, cfg=ka, kind="keepalive")), open(path[:-3] + ".json", "w"), indent=1)
        self.out(f"  {'pass' if res['ok'] else 'FAIL'}: {res['text']}")
        return res

    def run_timeout(self, sess_dir):
        tt = self.cfg["timeout_test"]
        run = self.cfg["runs"][0]
        self.out(f"\n== TX timeout: set {tt['set']}, keyed {tt['key_s']} s (carrier ~{tt['expect_s']} s expected)")
        self.setup_slot(run["mhz"], "fm")
        self.radio.term(f"set {TX_TIMEOUT} {tt['set']}")
        try:
            self.gate_key(self.cfg["test_freqs"], "timeout")
            path = os.path.join(sess_dir, "timeout.u8")
            ev, raw = self.capture_keyed(run["mhz"], tt["key_s"], path, "timeout", stop_on_unkey=False, kind="timeout")
        finally:
            with contextlib.suppress(Exception):
                self.radio.term(f"set {TX_TIMEOUT} {self.cfg['timeout_raise']}")
        res = analyse_timeout(raw, ev, tt)
        json.dump(jsonable(dict(events=ev, cfg=tt, kind="timeout")), open(path[:-3] + ".json", "w"), indent=1)
        self.out(f"  {'pass' if res['ok'] else 'FAIL'}: {res['text']}")
        return res

    # -- the sweeps (--sweep) --
    def run_sweeps(self, sess_dir, selection, resume=None, count=None):
        """Each selected sweep's items, one short key-up each through the duty limit; each item's result saved to
        sweeps.json as it comes (what --resume reads); the extras' "sweep:NAME" each."""
        out = {}
        prog_path = os.path.join(sess_dir, "sweeps.json")
        prog = {}
        prior = load_resume(resume) if resume else {}
        for name, items in selection:
            sc = dict(self.cfg["sweeps"], **self.cfg["sweeps"].get(name, {}))
            all_items = sweep_items(self.cfg, name)
            labels = [i["label"] for i in all_items]
            done = {k: v for k, v in prior.get(name, {}).items() if k in labels and v.get("ok")}  # failures run again
            todo = [i for i in items if i["label"] not in done]
            if count is not None:
                todo = todo[:count]
            res = dict(name=name, total=len(labels), resumed_from=os.path.basename(os.path.normpath(resume)) if resume else None)
            self.out(f"\n== sweep {name}: {len(todo)} of its {len(labels)} items now"
                     + (f" ({len(done)} from {res['resumed_from']})" if done else ""))
            prog[name] = dict(items={})
            now = {}
            why_not = self.sweep_prepare(name, sc)
            if why_not:
                self.out(f"  not run: {why_not}")
                res.update(ok=False, why=why_not, items=list(done.values()), complete=False, run=0)
                out[f"sweep:{name}"] = res
                continue
            d = os.path.join(sess_dir, "sweeps", name)
            os.makedirs(d, exist_ok=True)
            try:
                for it in todo:
                    r = self.sweep_item(name, sc, it, d)
                    r.update(label=it["label"], session=os.path.basename(os.path.normpath(sess_dir)))
                    now[it["label"]] = r
                    prog[name]["items"][it["label"]] = jsonable(r)
                    json.dump(prog, open(prog_path, "w"), indent=1)
                    if VERBOSE or not r.get("ok"):  # quiet: an item that passed has its line in the results files
                        self.out(f"  {it['label']:>14s}  {'ok  ' if r.get('ok') else 'FAIL'}  {r.get('text', '')}"
                                 + (f"  ({r['why']})" if r.get("why") and not r.get("ok") else ""))
            finally:
                self.sweep_finish(name, sc)
                merged = dict(done, **now)
                res.update(items=[merged[x] for x in labels if x in merged], run=len(now),
                           complete=all(x in merged for x in labels))
                res["ok"] = bool(res["items"]) and all(x.get("ok") for x in res["items"])
                out[f"sweep:{name}"] = res
                self.out(f"  sweep {name}: {sum(1 for x in res['items'] if x.get('ok'))} of {len(res['items'])} ok"
                         + ("" if res["complete"] else f"; {len(labels) - len(res['items'])} items not yet run "
                            f"(partial: --resume {sess_dir} to go on)"))
        return out

    def sweep_prepare(self, name, sc):
        """Set-up before a sweep; a reason it can't run, or None."""
        if name == "refusals-wide":
            ext = self.setting(EXTEND_TX)
            if ext.strip().lower() != "off":
                return f"Extend TX (0 8 8) is {ext}: the band plan's refusals don't hold with it on"
        self.setup_slot(sc["mhz"], "fm")
        return None

    def sweep_finish(self, name, sc):
        with contextlib.suppress(Exception):
            if name == "endtones":
                self.radio.term(f"set {END_TONE} Off")
            elif name == "sublevel" and self.saved and SUBTONE_LEVEL in (self.saved.get("extra") or {}):
                self.radio.term(f"set {SUBTONE_LEVEL} {self.saved['extra'][SUBTONE_LEVEL]}")
            elif name == "afsk-level":
                self.radio.term("txtest off")

    def sweep_item(self, name, sc, it, d):
        L = self.slot_letter
        path = os.path.join(d, slug(it["label"]) + ".u8")
        lead, tail = sc["lead_s"], sc["tail_s"]
        purpose = f"{name} {it['label']}"
        if name == "refusals-wide":
            f = it["mhz"]
            want = tx_expect(f, self.info.get("region"))
            if want is None:
                return dict(ok=True, skipped=True, expect=None, text=f"allowed in {region_of(self.info.get('region'))[0]}: not probed")
            try:
                self.radio.term(f"slot {L} vfo {f:.5f} fm low tone off")
            except TermError as e:
                return dict(ok=False, expect=want, why=f"couldn't tune: {e}", text="couldn't tune")
            st = self.radio.slot(self.slot_n)
            if st["freq10"] != mhz10(f):
                return dict(ok=False, expect=want, why=f"the slot reads {st['freq10'] / 1e5:.5f} MHz: not probed",
                            text="not probed")
            self.gate_key([f], purpose)
            ev, raw = self.capture_keyed(f, KEY_ANSWER_S, path, purpose, lead=lead, tail=tail, kind="probe",
                                         expect_refusal=True)
            runs = carrier_runs(raw)
            said = ev.get("refused") or ""
            reason_ok = bool(said) and want in said.lower()
            ok = reason_ok and not runs and not ev["accepted"]
            why = ("the radio KEYED (unkeyed at once)" if ev["accepted"] else
                   f"refused, but not for {want}: {said!r}" if not reason_ok else
                   f"a carrier on the SDR: {runs}" if runs else "")
            self.sweep_capture_done(path, sc, ok)
            return dict(ok=ok, expect=want, refused=said, accepted=bool(ev["accepted"]), carrier=[list(x) for x in runs],
                        why=why, text=f"{said.replace('invalid: ', '') or 'not refused'}; "
                                      f"{'no carrier' if not runs else 'CARRIER'}")
        mhz = sc["mhz"]
        mode = it.get("mode", "fm")
        tone = it.get("tone", "off")
        try:
            if name == "sublevel":
                self.radio.term(f"set {SUBTONE_LEVEL} {it['level']}")
            if name == "endtones":
                self.radio.term(f"set {END_TONE} {it['setting']}")
            self.radio.term(f"slot {L} vfo {mhz:.5f} {mode} low txtone {tone} rxtone off")
            if name == "afsk-level" and not self.bench:
                return dict(ok=True, skipped=True, expect=None, text="skipped: needs txtest, a bench build's (Dry TX on a tx build)")
            if name == "afsk-level":
                armed = self.radio.term(f"txtest afsk data {it['level']}")
                if "armed" not in armed:
                    return dict(ok=False, why=f"txtest not armed: {armed.strip()}", text="not armed")
        except TermError as e:
            return dict(ok=False, why=str(e), text="refused by the terminal")
        st = self.gate_key(self.cfg["test_freqs"], purpose)
        if st["tx_tone"] != tone_word(tone):
            return dict(ok=False, why=f"the slot's TX tone reads 0x{st['tx_tone']:04X}, not {tone}: not keyed",
                        text="not keyed")
        key_s, extra = sweep_key_s(self.cfg, name)
        ev, raw = self.capture_keyed(mhz, key_s, path, purpose, lead=lead, tail=tail, kind="burst", extra_tail=extra)
        if not ev.get("accepted"):
            return dict(ok=False, why=f"KEY refused: {ev.get('refused')}", text="not keyed")
        if name in ("ctcss", "dcs", "sublevel"):
            r = analyse_subtone(raw, it, sc.get("tol_hz", 0.2), sc.get("min_windows", 2))
        elif name == "endtones":
            r = analyse_endtone(raw, it, sc)
        else:
            r = analyse_afsk(raw)
        r.update({k: it[k] for k in ("hz", "code", "inverted", "typed", "level", "mode", "slug", "tone") if k in it})
        self.sweep_capture_done(path, sc, r.get("ok"))
        return r

    def sweep_capture_done(self, path, sc, ok):
        if ok and not sc.get("keep_captures"):
            with contextlib.suppress(OSError):
                os.remove(path)

    def on_air(self, sess_dir, runs, hello, info, flashed_dry=None):
        """Everything keyed, inside one save/restore; returns the captures and the extra checks' results."""
        caps, extras = [], {}
        self.info = info or {}
        selection = sweep_selection(self.cfg, parse_sweeps(self.a.sweep), self.a) if getattr(self.a, "sweep", None) else []
        if any(n == "sublevel" for n, _ in selection):
            self.extra_settings.append(SUBTONE_LEVEL)
        self.check_build(hello, info, flashed_dry)
        try:
            self.save_state()
            for run in runs:
                if not self.bench:  # TX-48: a tx build with Dry TX on has no txtest
                    w = f"{run['name']}: skipped: the suite needs txtest, a bench build's (this is a tx build with Dry TX on)"
                    self.warnings.append(w)
                    self.out(w)
                    continue
                caps.append(self.run_suite(run, sess_dir))
            if self.a.refusals:
                extras["refusals"] = self.run_refusals(sess_dir)
            if self.a.keepalive:
                extras["keepalive"] = self.run_keepalive(sess_dir)
            if self.a.timeout:
                extras["timeout"] = self.run_timeout(sess_dir)
            if selection:
                extras.update(self.run_sweeps(sess_dir, selection, getattr(self.a, "resume", None),
                                              getattr(self.a, "count", None)))
        finally:
            self.emergency_unkey()
            self.restore()
        return caps, extras


def analyse_keepalive(raw, ev, ka):
    runs = carrier_runs(raw)
    if not ev.get("accepted"):
        return dict(ok=False, text=f"not keyed: {ev.get('refused')}", carrier=runs)
    if not runs:
        return dict(ok=False, text="no carrier on the SDR", carrier=runs)
    on = max(runs, key=lambda r: r[1] - r[0])
    carrier_s = on[1] - on[0]
    alive_span = (ev["t_last_alive"] or ev["t_on"]) - ev["t_on"]
    drop = carrier_s - alive_span
    ok = 0 < drop <= ka["max_drop_s"]
    return dict(ok=ok, carrier_s=carrier_s, alive_span_s=alive_span, drop_s=drop, unkeyed=ev.get("unkeyed"),
                text=f"carrier {carrier_s:.2f} s; last keepalive {alive_span:.2f} s after the key-up; dropped "
                     f"{drop:.2f} s after it (limit {ka['max_drop_s']} s)" + (f"; the radio said: {ev['unkeyed']}" if ev.get("unkeyed") else ""))


def analyse_timeout(raw, ev, tt):
    runs = carrier_runs(raw)
    if not ev.get("accepted"):
        return dict(ok=False, text=f"not keyed: {ev.get('refused')}", carrier=runs)
    if not runs:
        return dict(ok=False, text="no carrier on the SDR", carrier=runs)
    on = max(runs, key=lambda r: r[1] - r[0])
    carrier_s = on[1] - on[0]
    ok = abs(carrier_s - tt["expect_s"]) <= tt["tol_s"]
    return dict(ok=ok, carrier_s=carrier_s, unkeyed=ev.get("unkeyed"),
                text=f"carrier {carrier_s:.2f} s (expected {tt['expect_s']:.0f} ± {tt['tol_s']} s)"
                     + (f"; the radio said: {ev['unkeyed']}" if ev.get("unkeyed") else ""))


# ---- The manual's rules: what each measurement checks (bench/results.json, tools/record_results.py) ----
#
# Which rules each check measures, and how much of each: tools/bench_rules.py's RULE_TAGS (shared with
# tools/record_results.py; sub-rules since decision 160). The rest of the suite's numbers (REG_40 steps, 4FSK,
# jitter, carrier offset) are regression numbers with no rule figure: they stay in the run comparison and are not
# tagged.

END_TONES = [  # TONE-10's list: (slug, [(Hz, ms), ...])
    ("beep", [(1000, 150)]),
    ("two-tone-up", [(800, 100), (1200, 100)]),
    ("two-tone-down", [(1200, 100), (800, 100)]),
    ("chirp", [(1000, 60), (1250, 60), (1500, 60)]),
    ("quindar", [(2475, 250)]),
]
SUITE_SUBTONES = [67.0, 100.0, 151.4, 250.3]  # TERM-34 section 6, then DCS 023 normal
RULE_TOL = dict(
    endtone_hz_frac=0.005,  # the generator's tone, ±0.5 %
    endtone_ms=15,          # the analysis' edges are 5 ms steps; a tone's swell blurs its start
    swell_tau_ms=(80, 20),  # TONE-10: time constant about 80 ms
    swell_t90_ms=(190, 40),  # TONE-10: 90 % at about 190 ms
    afsk_dev=(3000, 300),   # APRS-14: about 3 kHz (wide)
    afsk_twist_db=1.0,      # APRS-14: twist -0.2 dB measured; within ±1 dB
    afsk_half=0.1,          # TERM-34: the 50 % flags at half the 100 % deviation, ±10 %
    data_flat_db=1.0,       # TERM-33: `data` flat within 1 dB from 250 Hz to 2.5 kHz (against 1 kHz)
    stock_tone=0.05,        # TONE-19: stock's 0x50A, 1000 Hz 2.43 kHz and 2475 Hz 4.30 kHz, ±5 %
    ctcss_hz=0.2,
    ctcss_spread=0.05,      # one calibration byte: every CTCSS at the same deviation, ±5 %
    keepalive_cut_s=2.4,    # LINK-20: the carrier ends 2 s after the last keepalive, "not 2.4 s and more"
)


def _chk(what, ok, got=""):
    return dict(what=what, ok=bool(ok), got=got)


def _f(v, spec="{:.0f}"):
    return "-" if v is None else (spec.format(v) if isinstance(v, (int, float)) else str(v))


def sweep_tests(ex, add, cfg=None):
    """The sweeps' rule-tagged checks (session extras "sweep:NAME"); a sweep that didn't cover all its items names
    its checks NAME~part (bench_rules: every rule part)."""
    sc_all = (cfg or DEFAULTS)["sweeps"]
    for key, sw in sorted(ex.items()):
        if not key.startswith("sweep:") or not sw.get("items"):
            continue
        name = key[6:]
        sc = dict(sc_all, **sc_all.get(name, {}))
        sfx = "" if sw.get("complete") else "~part"
        items = sw["items"]

        def chk(it, what, ok, got):
            return _chk(f"{it['label']}: {what}", ok, got)

        if name == "refusals-wide":
            for word, test in (("band plan", "refusal_wide_band_plan"), ("emergency", "refusal_wide_emergency")):
                its = [x for x in items if x.get("expect") == word]
                if not its:
                    continue
                cs = []
                for x in its:
                    said = x.get("refused") or ""
                    cs += [chk(x, "remote KEY refused", "refused" in x and not x.get("accepted"), said or x.get("why")),
                           chk(x, f"the reason names {word}", word in said.lower(), said),
                           chk(x, "no carrier on the SDR", "carrier" in x and not x.get("carrier"), x.get("carrier"))]
                add(test + sfx, cs, f"{len(its)} frequencies ({its[0]['label']}-{its[-1]['label']} MHz) refused "
                                    f"\"{word}\", no carrier")
        elif name == "ctcss":
            std = [x for x in items if not x.get("typed")]
            typed = [x for x in items if x.get("typed")]
            tol = sc["tol_hz"]
            if std:
                errs = [abs(x["err_hz"]) for x in std if x.get("err_hz") is not None]
                add("ctcss_freq" + sfx, [chk(x, f"CTCSS within {tol} Hz", x.get("err_hz") is not None
                                             and abs(x["err_hz"]) <= tol, _f(x.get("hz"), "{:.2f} Hz")) for x in std],
                    f"{len(std)} CTCSS tones {std[0]['label']}-{std[-1]['label']} Hz, worst {_f(max(errs) if errs else None, '{:.3f}')} Hz off")
                devs = [x["dev"] for x in std if x.get("dev")]
                spread = _median_spread(devs) if devs and len(devs) == len(std) else None
                add("ctcss_level" + sfx, [_chk(f"every CTCSS at one deviation (±{sc['spread']:.0%})",
                                               spread is not None and spread <= sc["spread"], _f(spread, "{:.1%}"))],
                    f"{len(devs)} CTCSS tones at {_f(min(devs) if devs else None)}-{_f(max(devs) if devs else None)} Hz deviation")
            if typed:
                add("ctcss_typed" + sfx, [chk(x, f"typed CTCSS within {tol} Hz", x.get("err_hz") is not None
                                              and abs(x["err_hz"]) <= tol, _f(x.get("hz"), "{:.2f} Hz")) for x in typed],
                    "typed " + ", ".join(f"{x['label']} Hz -> {_f(x.get('hz'), '{:.2f}')}" for x in typed))
        elif name == "dcs":
            for pol, test in ((False, "dcs_normal"), (True, "dcs_inverted")):
                its = [x for x in items if not x.get("typed") and bool(x.get("inverted")) == pol]
                if its:
                    add(test + sfx, [chk(x, f"decodes {'inverted' if pol else 'normal'}, never the other polarity",
                                         x.get("ok"), x.get("why") or x.get("decoded")) for x in its],
                        f"{len(its)} DCS codes {'I' if pol else 'N'} ({its[0]['label']}-{its[-1]['label']}) "
                        f"{sum(1 for x in its if x.get('ok'))} decoded {'inverted' if pol else 'normal'}")
            std = [x for x in items if not x.get("typed")]
            devs = [x["dev"] for x in std if x.get("dev")]
            if std:
                spread = _median_spread(devs) if devs and len(devs) == len(std) else None
                add("dcs_level" + sfx, [_chk(f"every DCS code at one deviation (±{sc['spread']:.0%})",
                                             spread is not None and spread <= sc["spread"], _f(spread, "{:.1%}"))],
                    f"{len(devs)} DCS codes at {_f(min(devs) if devs else None)}-{_f(max(devs) if devs else None)} Hz deviation")
            typed = [x for x in items if x.get("typed")]
            if typed:
                add("dcs_typed" + sfx, [chk(x, "typed code decodes, its polarity only", x.get("ok"),
                                            x.get("why") or x.get("decoded")) for x in typed],
                    "typed " + ", ".join(f"{x['label']} {'ok' if x.get('ok') else 'FAIL'}" for x in typed))
        elif name == "endtones":
            steps, level, lead, hold = [], [], [], []
            for x in items:
                for st in x.get("steps") or [{"found": False, "want_hz": "?", "want_ms": "?"}]:
                    w = f"{st['want_hz']} Hz"
                    steps += [chk(x, f"{w} found, within 0.5 %", st.get("hz_ok"), _f(st.get("hz"), "{:.1f} Hz")),
                              chk(x, f"{w} lasts {st['want_ms']} ms (±{RULE_TOL['endtone_ms']})", st.get("ms_ok"),
                                  _f(st.get("ms"), "{:.0f} ms"))]
                    if st.get("want_dev") or not st.get("found"):
                        level.append(chk(x, f"{w} at about {_f(st.get('want_dev'))} Hz (±{sc['level_tol']:.0%})",
                                         st.get("dev_ok"), _f(st.get("dev"), "{:.0f} Hz")))
                lq, sf = x.get("lead_quiet"), x.get("start_frac")
                lead.append(chk(x, "a silent lead-in, then the first step at full level from its start",
                                lq is not None and lq < 0.25 and sf is not None and sf >= 0.8,
                                f"quiet {_f(lq, '{:.2f}')}, starts at {_f(sf, '{:.0%}')}"))
                hs = x.get("hold_s")
                hold.append(chk(x, f"carrier held {sc['hold_s'] * 1000:.0f} ms (±{sc['hold_tol_s'] * 1000:.0f}) after it",
                                hs is not None and abs(hs - sc["hold_s"]) <= sc["hold_tol_s"], _f(hs, "{:.3f} s")))
            modes = sorted({x.get("mode") for x in items})
            add("endtone_steps" + sfx, steps, f"{len(items)} end tones on a real unkey ({', '.join(modes)}): "
                + " ".join(f"{x['label']} {'ok' if all(s.get('hz_ok') and s.get('ms_ok') for s in x.get('steps') or [{}]) else 'FAIL'}" for x in items))
            add("endtone_level" + sfx, level, "levels: " + "; ".join(
                f"{x['label']} " + "/".join(_f(s.get("dev")) for s in x.get("steps") or []) for x in items))
            add("endtone_lead_in" + sfx, lead, "first step's start: " + " ".join(
                f"{x['label']} {_f(x.get('start_frac'), '{:.0%}')}" for x in items))
            add("endtone_hold" + sfx, hold, "hold after the tone: " + " ".join(
                f"{x['label']} {_f(x.get('hold_s'), '{:.3f}')}" for x in items))
        elif name == "sublevel":
            cs = []
            for t in sorted({x["tone"] for x in items}):
                by = {x["level"]: x.get("dev") for x in items if x["tone"] == t}
                lo, nm, hi = by.get("Low"), by.get("Normal"), by.get("High")
                good = None not in (lo, nm, hi) and lo < nm * (1 - sc["min_step"]) and hi > nm * (1 + sc["min_step"])
                cs.append(_chk(f"{t}: Low < Normal < High (steps over {sc['min_step']:.0%})", good,
                               f"{_f(lo)} / {_f(nm)} / {_f(hi)} Hz"))
            add("subtone_level" + sfx, cs, "Low/Normal/High: " + "; ".join(c["got"] for c in cs))
        elif name == "afsk-level":
            ref = next((x.get("mean") for x in items if x.get("level") == 100), None)
            cs, prev = [], None
            for x in sorted(items, key=lambda x: x["level"]):
                lvl, m = x["level"], x.get("mean")
                if lvl <= 100:
                    want = ref * lvl / 100 if ref else None
                    ok = m is not None and want is not None and abs(m - want) <= max(sc["tol"] * want, sc["tol_hz"])
                    cs.append(chk(x, f"deviation {lvl} % of the 100 % one (±{sc['tol']:.0%} or {sc['tol_hz']} Hz)", ok,
                                  f"{_f(m)} Hz, want {_f(want)}"))
                else:
                    cs.append(chk(x, "deviation above the level below it", m is not None and prev is not None and m > prev,
                                  f"{_f(m)} Hz after {_f(prev)}"))
                prev = m
            add("afsk_level_steps" + sfx, cs, "AFSK " + ", ".join(f"{x['level']} % {_f(x.get('mean'))} Hz"
                                                               for x in sorted(items, key=lambda x: x["level"])))


def _median_spread(devs):
    """The largest distance of any deviation from the median, as a fraction of it: "every one at one deviation
    (±N %)" read as ±N % around the typical value (max/min - 1 is about twice as strict)."""
    m = statistics.median(devs)
    return max(abs(d - m) for d in devs) / m if m else None


def rule_tests(session, runs):
    """The rule-tagged checks of one session: session.json's extras (refusals, keepalive, timeout, reboot) and each
    suite run's metrics. Each: dict(name, rules, scope {rule: part|all}, result, checks, summary, why)."""
    tests = []

    def add(name, checks, summary):
        ok = all(c["ok"] for c in checks)
        bad = [c for c in checks if not c["ok"]]
        scope = bench_rules.tags(name)
        tests.append(dict(name=name, module="bench", rules=list(scope), scope=scope,
                          result="pass" if ok and checks else "fail", checks=checks, summary=summary,
                          why="; ".join(f"{c['what']}: got {c['got']}" for c in bad[:3])))

    ex = session.get("extras") or {}
    refusal_rules = [  # (expected reason word, test name)
        ("band plan", "refusal_band_plan"),
        ("emergency", "refusal_emergency"),
    ]
    for r in ex.get("refusals") or []:
        for word, name in refusal_rules:
            if word in (r.get("expect") or ""):
                said = r.get("refused") or ""
                add(name, [_chk(f"remote KEY on {r['mhz']:.3f} MHz refused", not r.get("accepted"), said or "keyed"),
                           _chk(f"the reason names {word}", word in said, said),
                           _chk("no carrier on the SDR", not r.get("carrier"), r.get("carrier"))],
                    f"{r['mhz']:.3f} MHz refused \"{said.replace('invalid: ', '')}\", no carrier")
    ka = ex.get("keepalive")
    if ka:
        drop = ka.get("drop_s")
        lim = RULE_TOL["keepalive_cut_s"]
        add("keepalive_cut",
            [_chk("keyed and on the SDR", drop is not None, ka.get("text")),
             _chk(f"carrier ends under {lim} s after the last keepalive (a cut: no 400 ms hold)",
                  drop is not None and 0 < drop < lim, _f(drop, "{:.2f} s"))],
            f"carrier ended {_f(drop, '{:.2f}')} s after the last keepalive")
    tt = ex.get("timeout")
    if tt:
        cs = tt.get("carrier_s")
        add("tx_timeout",
            [_chk("carrier about 30 s at the 30 s setting (±1.5 s)", cs is not None and abs(cs - 30.0) <= 1.5,
                  _f(cs, "{:.2f} s")),
             _chk("the radio ended it as a timeout", "timeout" in (tt.get("unkeyed") or ""), tt.get("unkeyed"))],
            f"30 s timeout: carrier {_f(cs, '{:.1f}')} s, \"{(tt.get('unkeyed') or '').replace('invalid: ', '')}\"")
    rb = session.get("reboot")
    if rb:
        add("reboot", [_chk("HELLO again, home, `dump` unchanged", rb.get("ok"), rb.get("text"))],
            f"REBOOT: {rb.get('text')}")
    sweep_tests(ex, add, session.get("cfg"))
    for mod in benchtests.all_modules():  # the --suite runs' checks (extras "suite:NAME")
        mod.tests_of(ex, add)

    for j in runs:
        m, name, wide = j.get("metrics") or {}, j["run"], j.get("mode") == "fm"
        if not m.get("markers_found"):
            add(f"suite@{name}", [_chk("the suite's markers on the SDR", False, m.get("markers_found"))], "no markers")
            continue
        d100 = [m.get("afsk.dac100.mark_dev"), m.get("afsk.dac100.space_dev")]
        d50 = [m.get("afsk.dac50.mark_dev"), m.get("afsk.dac50.space_dev")]
        sections = {1: "sweep.data.low_hz", 2: "sweep.voice.low_hz", 3: "afsk.dac100.mark_dev",
                    4: "reg40.0x43C.dev", 5: "tone1k.steady_dev", 6: "ctcss.100.0.hz", 7: "fsk4.symbols"}
        half = (sum(d50) / sum(d100)) if None not in d100 + d50 and sum(d100) else None
        add(f"suite@{name}",
            [_chk("all 7 section markers found", m.get("markers_found") == 7, m.get("markers_found")),
             _chk("the 1 kHz hold after the last step", m.get("hold") == "yes", m.get("hold")),
             _chk("every section measured", all(m.get(k) is not None for k in sections.values()),
                  ", ".join(str(n) for n, k in sections.items() if m.get(k) is None) or "all"),
             _chk("section 3's 50 % flags at half the 100 % deviation (±10 %)",
                  half is not None and abs(half - 0.5) <= 0.5 * RULE_TOL["afsk_half"], _f(half, "{:.3f}"))],
            f"{name}: 7 markers, hold, every section measured; AFSK 50 % at {_f(half, '{:.2f}')} of 100 %")
        # TERM-33.10: `data` flat within 1 dB from 250 Hz to 2.5 kHz (the sweep against its 1 kHz point)
        c = (j.get("curves") or {}).get("sweep", {}).get("data")
        if c:
            pts = [(f, db) for f, db in zip(c["freqs"], c["db"]) if 250 <= f <= 2500 and db is not None]
            ref = next((db for f, db in zip(c["freqs"], c["db"]) if abs(f - 1000) < 1), None)
            if pts and ref is not None:
                worst = max(pts, key=lambda p: abs(p[1] - ref))
                add(f"data_flat@{name}",
                    [_chk("`data` sweep within ±1 dB of 1 kHz from 250 Hz to 2.5 kHz",
                          abs(worst[1] - ref) <= RULE_TOL["data_flat_db"], f"{worst[1] - ref:+.2f} dB at {worst[0]:.0f} Hz")],
                    f"{name}: `data` {min(p[1] for p in pts) - ref:+.1f} to {max(p[1] for p in pts) - ref:+.1f} dB, 250 Hz-2.5 kHz")
        # TONE-10.2: the end tones' frequencies and lengths; TONE-10.4: the chip's swell on the first tone (part: the
        # lead-in that keeps it off the air is seen on an end tone, not in the suite)
        checks, found = [], []
        for slug_, steps in END_TONES:
            for hz, ms in steps:
                k = f"endtone.{slug_}.{hz}"
                got_hz, got_ms = m.get(k + ".hz"), m.get(k + ".ms")
                checks.append(_chk(f"{slug_} {hz} Hz", m.get(k + ".found") == "yes" and got_hz is not None
                                   and abs(got_hz - hz) <= hz * RULE_TOL["endtone_hz_frac"], _f(got_hz, "{:.1f} Hz")))
                checks.append(_chk(f"{slug_} {hz} Hz lasts {ms} ms (±{RULE_TOL['endtone_ms']})", got_ms is not None
                                   and abs(got_ms - ms) <= RULE_TOL["endtone_ms"], _f(got_ms, "{:.0f} ms")))
                found.append(f"{hz}/{_f(got_ms)}")
        add(f"end_tones@{name}", checks, f"{name}: end tones Hz/ms {' '.join(found)}")
        tau, t90 = m.get("tone1k.tau_ms"), m.get("tone1k.t90_ms")
        add(f"tone_swell@{name}",
            [_chk("swell time constant about 80 ms", tau is not None
                  and abs(tau - RULE_TOL["swell_tau_ms"][0]) <= RULE_TOL["swell_tau_ms"][1], _f(tau, "{:.0f} ms")),
             _chk("90 % at about 190 ms", t90 is not None
                  and abs(t90 - RULE_TOL["swell_t90_ms"][0]) <= RULE_TOL["swell_t90_ms"][1], _f(t90, "{:.0f} ms"))],
            f"{name}: the first tone's swell τ {_f(tau)} ms, 90 % {_f(t90)} ms")
        if wide:
            # APRS-14.2: the beacon's AFSK level (section 3 runs at it), wide FM
            mean = sum(d100) / 2 if None not in d100 else None
            tw = m.get("afsk.dac100.twist_db")
            add(f"afsk_level@{name}",
                [_chk("AFSK deviation about 3.0 kHz (±0.3)", mean is not None
                      and abs(mean - RULE_TOL["afsk_dev"][0]) <= RULE_TOL["afsk_dev"][1], _f(mean, "{:.0f} Hz")),
                 _chk("twist within ±1 dB", tw is not None and abs(tw) <= RULE_TOL["afsk_twist_db"], _f(tw, "{:+.2f} dB"))],
                f"{name}: AFSK {_f(d100[0])}/{_f(d100[1])} Hz, twist {_f(tw, '{:+.2f}')} dB")
            # TONE-19.5: stock's figures (the suite's section 5 plays at stock's REG_40 0x50A)
            a, q = m.get("endtone.beep.1000.dev"), m.get("endtone.quindar.2475.dev")
            add(f"stock_tone_level@{name}",
                [_chk("1000 Hz at 0x50A about 2.43 kHz (±5 %)", a is not None and abs(a / 2430 - 1) <= RULE_TOL["stock_tone"], _f(a, "{:.0f} Hz")),
                 _chk("2475 Hz at 0x50A about 4.30 kHz (±5 %)", q is not None and abs(q / 4300 - 1) <= RULE_TOL["stock_tone"], _f(q, "{:.0f} Hz"))],
                f"{name}: stock's 0x50A: 1000 Hz {_f(a)} Hz, 2475 Hz {_f(q)} Hz (Ferrite's 0x43C levels not in the suite)")
        # TONE-1.1 (the generator's sub-tones), TONE-1.2 in part (one deviation; the byte isn't read back) and
        # TONE-20.1 (D023N goes out normal)
        hz = {t: m.get(f"ctcss.{t:.1f}.hz") for t in SUITE_SUBTONES}
        dev = [m.get(f"ctcss.{t:.1f}.dev") for t in SUITE_SUBTONES]
        spread = (max(dev) / min(dev) - 1) if None not in dev and min(dev) > 0 else None
        add(f"subtones@{name}",
            [_chk(f"CTCSS {t} Hz (±{RULE_TOL['ctcss_hz']})", v is not None and abs(v - t) <= RULE_TOL["ctcss_hz"],
                  _f(v, "{:.2f} Hz")) for t, v in hz.items()]
            + [_chk("every CTCSS at one deviation (±5 %)", spread is not None and spread <= RULE_TOL["ctcss_spread"],
                    _f(spread, "{:.1%}")),
               _chk("DCS on the air", m.get("dcs.code") not in (None, "none"), m.get("dcs.code"))],
            f"{name}: CTCSS 67.0-250.3 Hz at {_f(dev[1])} Hz, DCS {m.get('dcs.code')} at {_f(m.get('dcs.dev'))} Hz "
            "(the calibration byte isn't read back)")
        add(f"dcs_polarity@{name}",
            [_chk("D023N decodes as 023 normal", m.get("dcs.code") == "023 N", m.get("dcs.code"))],
            f"{name}: D023N decoded {m.get('dcs.code')}")
    return tests


def rule_summary(sess_dir):
    """bench/results.json for a session, in uitest's shape (tool, started, port, sim, build, version, commit, label,
    counts, rules {rule: result, test, why, scope, note}, tests), from session.json and the <run>.json files."""
    session = json.load(open(os.path.join(sess_dir, "session.json")))
    # A session stopped by a safety check (or one that crashed on the way out) has no extras, but its sweeps.json
    # holds every item measured: rebuild the sweep extras from it, whole only if every item is there.
    sp = os.path.join(sess_dir, "sweeps.json")
    if os.path.exists(sp):
        ex = dict(session.get("extras") or {})
        for name, sw in json.load(open(sp)).items():
            got = list((sw.get("items") or {}).values())
            if f"sweep:{name}" in ex or not got or name not in SWEEP_NAMES:
                continue
            want = {i["label"] for i in sweep_items(session.get("cfg") or DEFAULTS, name)}
            ex[f"sweep:{name}"] = dict(name=name, total=len(want), items=got, complete=want <= {i["label"] for i in got},
                                       ok=all(i.get("ok") for i in got))
        session["extras"] = ex
    runs = []
    for r in session.get("runs") or []:
        p = os.path.join(sess_dir, r + ".json")
        if os.path.exists(p):
            runs.append(json.load(open(p)))
    tests = rule_tests(session, runs)
    rules = bench_rules.per_rule(tests)
    b = session.get("build") or {}
    stamp = os.path.basename(os.path.normpath(sess_dir))[:15]
    port = session.get("port")
    sim = bool(session.get("sim")) or str(port or "").startswith("tcp:") or port == "fake"
    commit = session.get("commit") or b.get("host")
    return dict(tool="bench.py", started=datetime.datetime.strptime(stamp, "%Y%m%d-%H%M%S").isoformat()
                if re.match(r"\d{8}-\d{6}$", stamp) else None,
                port=port, sim=sim, source=session.get("source"), build="tx-dry" if session.get("tx_dry") else None,
                version=b.get("version"), commit=commit, label=b.get("label"),
                counts={k: sum(1 for t in tests if t["result"] == k) for k in ("pass", "fail")},
                rules=rules, tests=tests)


def write_rule_results(sess_dir):
    s = rule_summary(sess_dir)
    d = os.path.join(sess_dir, "bench")
    os.makedirs(d, exist_ok=True)
    p = os.path.join(d, "results.json")
    json.dump(jsonable(s), open(p, "w"), indent=1, ensure_ascii=False)
    return p, s


# ---- Analysis, storage, the page ----


def analyse_run(cfg, a, run, capture, sess_dir, session, build, raw=None, events=None):
    rd = os.path.join(sess_dir, run["name"])
    os.makedirs(rd, exist_ok=True)
    say(f"analysing {run['name']} ({capture}) ...")
    if raw is None:
        raw = np.fromfile(capture, np.uint8)
    with quiet(None if a.verbose else os.path.join(rd, "analysis.log")):
        R = sdrsuite.analyse(raw, rd, run["name"], run["mhz"])
    m, curves = metrics_of(R)
    j = dict(run=run["name"], config_key=config_key(run), mhz=run["mhz"], mode=run["mode"], slot=cfg["slot"],
             session=session, build=build, date=datetime.datetime.now().isoformat(timespec="seconds"),
             ok=bool(R.get("ok")), capture=os.path.abspath(capture), metrics=m, curves=curves,
             files=dict(suite_html=os.path.relpath(R["html"], sess_dir) if R.get("html") else None,
                        suite_txt=os.path.relpath(R["txt"], sess_dir) if R.get("txt") else None),
             events=jsonable(events or {}))
    prev = previous_run(cfg, run, session)
    if prev:
        rows = compare(cfg, m, prev["metrics"])
        j["previous"] = dict(session=prev["session"], build=prev.get("build"), curves=prev.get("curves"))
        j["comparison"] = [list(r) for r in rows]
        nflag = sum(r[5] in ("CHANGED", "missing") for r in rows)
        say(f"  {run['name']}: {len(m)} numbers; against {prev['session']}: {nflag} beyond tolerance")
    else:
        say(f"  {run['name']}: {len(m)} numbers; no earlier run of {config_key(run)} to compare with")
    json.dump(jsonable(j), open(os.path.join(sess_dir, run["name"] + ".json"), "w"), indent=1)
    return j


CSS = """
:root{--bg:#fbfbfa;--card:#ffffff;--ink:#141413;--muted:#5e5d59;--line:#e3e2dd;--accent:#2a78d6;--accent2:#d0632a;
--ok:#1a7f4b;--bad:#c4321f;--warn:#a86b00;--okbg:#e6f4ec;--badbg:#fbe9e6;--warnbg:#fbf1dc;color-scheme:light}
@media (prefers-color-scheme:dark){:root:not([data-theme=light]){--bg:#1a1a19;--card:#232322;--ink:#f2f1ec;
--muted:#b5b4ab;--line:#3a3a37;--accent:#5b9cf0;--accent2:#f0894f;--ok:#5ccb8c;--bad:#ff7a66;--warn:#e8b14a;
--okbg:#1d3328;--badbg:#3d211c;--warnbg:#3a2f17;color-scheme:dark}}
:root[data-theme=dark]{--bg:#1a1a19;--card:#232322;--ink:#f2f1ec;--muted:#b5b4ab;--line:#3a3a37;--accent:#5b9cf0;
--accent2:#f0894f;--ok:#5ccb8c;--bad:#ff7a66;--warn:#e8b14a;--okbg:#1d3328;--badbg:#3d211c;--warnbg:#3a2f17;color-scheme:dark}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--ink);font:15px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif}
main{max-width:1040px;margin:0 auto;padding:24px 16px 48px}h1{font-size:24px;margin:0 0 2px}h2{font-size:19px;margin:32px 0 8px}
h3{font-size:16px;margin:18px 0 6px}.sub{color:var(--muted);margin:0 0 16px}a{color:var(--accent)}
.card{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:14px 16px;margin:12px 0}
.tiles{display:grid;grid-template-columns:repeat(auto-fit,minmax(190px,1fr));gap:8px;margin:10px 0}
.tile{border:1px solid var(--line);border-radius:8px;padding:8px 10px}.k{color:var(--muted);font-size:12.5px}.v{font-weight:600;font-variant-numeric:tabular-nums}
.wrap{overflow-x:auto}table{border-collapse:collapse;width:100%;font-size:13.5px;font-variant-numeric:tabular-nums}
td,th{border-bottom:1px solid var(--line);padding:5px 8px;text-align:left;white-space:nowrap}th{color:var(--muted);font-weight:600}
td.n{text-align:right}.pill{display:inline-block;border-radius:99px;padding:0 8px;font-size:12.5px;font-weight:600}
.ok{color:var(--ok);background:var(--okbg)}.bad{color:var(--bad);background:var(--badbg)}.warn{color:var(--warn);background:var(--warnbg)}
.muted{color:var(--muted)}details summary{cursor:pointer;color:var(--accent);margin:6px 0}
svg text{fill:var(--muted);font-size:11px;font-family:inherit}svg .grid{stroke:var(--line)}svg .axis{stroke:var(--muted)}
.legend span{display:inline-flex;align-items:center;gap:6px;margin-right:14px;font-size:13px;color:var(--muted)}
.sw{width:18px;height:0;border-top:2px solid}.sw.d{border-top-style:dashed}
ul.notes{margin:6px 0;padding-left:20px}
"""


def fmt(v):
    if v is None:
        return "–"
    if isinstance(v, str):
        return v
    if isinstance(v, int) or (isinstance(v, float) and v.is_integer() and abs(v) < 1e6):
        return f"{v:.0f}"
    a = abs(v)
    return f"{v:.3f}" if a < 10 else f"{v:.2f}" if a < 100 else f"{v:.1f}" if a < 1000 else f"{v:.0f}"


def sweep_svg(cur, prev):
    """The two sweeps (data, voice) now (solid) and before (dashed): log frequency, plain Hz tick labels."""
    if not cur:
        return ""
    W, H, l, r, t, b = 640, 260, 46, 12, 10, 34
    f0, f1, y0, y1 = 45, 9000, -30, 6

    def X(f):
        return l + (math.log10(f) - math.log10(f0)) / (math.log10(f1) - math.log10(f0)) * (W - l - r)

    def Y(d):
        return t + (y1 - max(min(d, y1), y0)) / (y1 - y0) * (H - t - b)

    g = []
    for f in (50, 100, 200, 300, 500, 1000, 2000, 3000, 5000, 8000):
        g.append(f'<line class="grid" x1="{X(f):.1f}" x2="{X(f):.1f}" y1="{t}" y2="{H - b}"/>'
                 f'<text x="{X(f):.1f}" y="{H - b + 15}" text-anchor="middle">{f}</text>')
    for d in (-30, -24, -18, -12, -6, -3, 0, 6):
        dash = ' stroke-dasharray="3 3"' if d == -3 else ""
        g.append(f'<line class="grid" x1="{l}" x2="{W - r}" y1="{Y(d):.1f}" y2="{Y(d):.1f}"{dash}/>'
                 f'<text x="{l - 6}" y="{Y(d) + 4:.1f}" text-anchor="end">{d:+d}</text>'.replace(">+0<", ">0<"))
    g.append(f'<text x="{(l + W - r) / 2}" y="{H - 2}" text-anchor="middle">Hz</text>')
    g.append(f'<text x="12" y="{(t + H - b) / 2}" text-anchor="middle" transform="rotate(-90 12 {(t + H - b) / 2})">dB vs 1 kHz</text>')
    lines = []
    for src, dash in ((prev or {}, "5 4"), (cur, "")):
        for name, col in (("data", "var(--accent)"), ("voice", "var(--accent2)")):
            c = src.get(name)
            if not c:
                continue
            pts = " ".join(f"{X(f):.1f},{Y(d):.1f}" for f, d in zip(c["freqs"], c["db"]) if f0 <= f <= f1)
            st = f"stroke:{col};stroke-width:{1.4 if dash else 2}" + (f";stroke-dasharray:{dash};opacity:.75" if dash else "")
            lines.append(f'<polyline fill="none" style="{st}" points="{pts}"/>')
    leg = ('<div class=legend><span><i class=sw style="border-color:var(--accent)"></i>data now</span>'
           '<span><i class=sw style="border-color:var(--accent2)"></i>voice now</span>'
           + ('<span><i class="sw d" style="border-color:var(--muted)"></i>previous run</span>' if prev else "") + "</div>")
    return (f'<div class="wrap"><svg viewBox="0 0 {W} {H}" width="100%" style="max-width:{W}px" role="img" '
            f'aria-label="TX audio response sweep">{"".join(g)}{"".join(lines)}</svg></div>{leg}')


def pill(status):
    cls = {"ok": "ok", "pass": "ok", "info": "ok", "CHANGED": "bad", "FAIL": "bad", "missing": "bad", "new": "warn"}.get(status, "warn")
    return f"<span class='pill {cls}'>{html.escape(status)}</span>"


def summary_page(session, sess_dir, runs, extras, meta):
    e = html.escape
    parts = []
    nflag = sum(sum(r[5] in ("CHANGED", "missing") for r in j.get("comparison", [])) for j in runs)
    notes = meta.get("warnings", []) + meta.get("restore_notes", [])
    head = (f"<h1>Ferrite {'TX ' if runs or extras else ''}bench</h1><p class=sub>{e(session)} · build "
            f"{e(meta['build']['label'])} · {e(meta['source'])}"
            + (f" · commit {e(meta['commit'])}" if meta.get("commit") else "") + "</p>")
    if meta.get("decision"):
        d = meta["decision"]
        head += (f"<div class=card><b>Why this ran</b><p>{e(d['what'])}</p><p class=muted>{e(d['why'])}</p>"
                 + (f"<details><summary>{len(d['files'])} files changed</summary><ul class=notes>"
                    + "".join(f"<li>{e(f)} <span class=muted>({e(c)})</span></li>" for f, c in d["files"])
                    + "</ul></details>" if d.get("files") else "") + "</div>")
    if not runs and not extras:
        parts.append(head)
        head = None
    over = [("Runs", str(len(runs))), ("Beyond tolerance", str(nflag)),
            ("Extra checks", ", ".join(f"{k} {'pass' if all_ok(v) else 'FAIL'}" for k, v in extras.items()) or "none"),
            ("TX-dry known by", meta.get("tx_dry") or "–")]
    if head is not None:
        parts.append(head + "<div class=tiles>" + "".join(
            f"<div class=tile><div class=k>{e(k)}</div><div class=v>{e(v)}</div></div>" for k, v in over) + "</div>")
    if notes:
        parts.append("<div class=card><b>Notes</b><ul class=notes>" + "".join(f"<li>{e(n)}</li>" for n in notes) + "</ul></div>")
    for j in runs:
        m = j["metrics"]
        prev = j.get("previous")
        rows = j.get("comparison", [])
        flagged = [r for r in rows if r[5] not in ("ok", "info")]
        tiles = "".join(f"<div class=tile><div class=k>{e(k)}</div><div class=v>{e(v)}</div></div>" for k, v in headline(m))
        link = f"<a href='{e(j['files']['suite_html'])}'>sdrsuite page</a>" if j["files"].get("suite_html") else ""
        cmp_txt = (f"against {e(prev['session'])}: " + (f"{pill('CHANGED')} {len([r for r in rows if r[5] in ('CHANGED', 'missing')])} beyond tolerance"
                                                      if any(r[5] in ("CHANGED", "missing") for r in rows) else pill("ok") + " all within tolerance")
                   if prev else "<span class=muted>no earlier run of this configuration</span>")

        def table(rs):
            return ("<div class=wrap><table><tr><th>number</th><th>now</th><th>before</th><th>change</th><th>tolerance</th><th></th></tr>"
                    + "".join(f"<tr><td>{e(k)}</td><td class=n>{e(fmt(a))}</td><td class=n>{e(fmt(b))}</td>"
                              f"<td class=n>{'' if d is None else e(('+' if d > 0 else '') + fmt(d))}</td><td>{e(tol)}</td><td>{pill(s)}</td></tr>"
                              for k, a, b, d, tol, s in rs) + "</table></div>")
        body = f"<p>{cmp_txt} · {link}</p><div class=tiles>{tiles}</div>"
        body += sweep_svg(j.get("curves", {}).get("sweep"), (prev or {}).get("curves", {}) and prev["curves"].get("sweep"))
        if rows:
            if flagged:
                body += "<h3>Changed, missing or new</h3>" + table(flagged)
            body += f"<details><summary>All {len(rows)} numbers</summary>{table(rows)}</details>"
        else:
            body += ("<details><summary>All numbers</summary><div class=wrap><table>" + "".join(
                f"<tr><td>{e(k)}</td><td class=n>{e(fmt(v))}</td></tr>" for k, v in sorted(m.items())) + "</table></div></details>")
        parts.append(f"<h2>{e(j['run'])} <span class=muted>· {j['mhz']:.3f} MHz {e(j['mode'].upper())}</span> "
                     f"{pill('pass' if j['ok'] else 'FAIL')}</h2><div class=card>{body}</div>")
    if extras:
        parts.append("<h2>Extra checks</h2>")
        if "refusals" in extras:
            parts.append("<div class=card><h3>Remote PTT refused out of band (LINK-20)</h3><div class=wrap><table>"
                         "<tr><th>MHz</th><th>expected</th><th>the radio said</th><th>carrier</th><th></th></tr>" + "".join(
                             f"<tr><td>{r['mhz']:.3f}</td><td>{e(r['expect'])}</td><td>{e(r.get('refused') or r.get('why') or '–')}</td>"
                             f"<td>{'none' if not r.get('carrier') else e(str(r['carrier']))}</td><td>{pill('pass' if r['ok'] else 'FAIL')}</td></tr>"
                             for r in extras["refusals"]) + "</table></div></div>")
        for k, title in (("keepalive", "Keepalive lost: the carrier drops (LINK-20)"), ("timeout", "TX timeout")):
            if k in extras:
                r = extras[k]
                parts.append(f"<div class=card><h3>{e(title)} {pill('pass' if r['ok'] else 'FAIL')}</h3><p>{e(r['text'])}</p></div>")
        for k, sw in extras.items():
            if not k.startswith(("sweep:", "suite:")):
                continue
            its = sw.get("items") or []
            nok = sum(1 for x in its if x.get("ok"))
            sub = (f"{nok} of {len(its)} items ok, {len(its)} of {sw.get('total')} items"
                   + ("" if sw.get("complete") else " (partial)")
                   + (f"; resumed from {sw['resumed_from']}" if sw.get("resumed_from") else "")
                   + (f"; not run: {sw['why']}" if sw.get("why") else ""))
            rows = "".join(f"<tr><td>{e(x.get('label', ''))}</td><td>{e(x.get('text', ''))}"
                           f"{' <span class=muted>(' + e(x['why']) + ')</span>' if x.get('why') and not x.get('ok') else ''}"
                           f"{' <span class=muted>' + e(x['session']) + '</span>' if x.get('resumed') else ''}</td>"
                           f"<td>{pill('pass' if x.get('ok') else 'FAIL')}</td></tr>" for x in its)
            parts.append(f"<div class=card><h3>{'Sweep' if k.startswith('sweep:') else 'Suite'} {e(k[6:])} "
                         f"{pill('pass' if sw.get('ok') else 'FAIL')}</h3>"
                         f"<p class=muted>{e(sub)}</p><details{' open' if nok < len(its) else ''}><summary>"
                         f"{len(its)} items</summary><div class=wrap><table><tr><th>item</th><th>measured</th><th></th></tr>"
                         f"{rows}</table></div></details></div>")
    if meta.get("ui"):
        import uitest
        parts.append(uitest.ui_section(meta["ui"]))
    if meta.get("reboot"):
        r = meta["reboot"]
        parts.append(f"<h2>Reboot check {pill('pass' if r['ok'] else 'FAIL')}</h2><div class=card><p>{e(r['text'])}</p></div>")
    du = meta.get("duty")
    if du and du.get("log"):
        lim = du["limits"]
        parts.append(
            "<h2>Duty</h2><div class=card><div class=tiles>" + "".join(
                f"<div class=tile><div class=k>{e(k)}</div><div class=v>{e(v)}</div></div>" for k, v in (
                    ("Key-ups", str(du["keyups"])), ("Keyed", f"{du['keyed_s']:.0f} s ({du['counted_s']:.0f} s with tails)"),
                    ("Over", f"{du['wall_s'] / 60:.1f} min, {du['duty']:.0%}"),
                    (f"Fullest {lim['window_s'] / 60:g} min", f"{du['peak_window_s']:.0f} s ({du['peak_window_frac']:.0%}, cap {lim['window_frac']:.0%})"),
                    ("Longest key-up", f"{du['longest_s']:.1f} s (limit {lim['max_burst_s']:g} s; {', '.join(lim['exceptions'])} excepted)"),
                    ("Waited", f"{du['waited_s']:.0f} s"))) + "</div>"
            + f"<details><summary>{len(du['log'])} key-ups and key-downs</summary><div class=wrap><table><tr><th>key-up</th>"
              "<th>key-down</th><th>keyed</th><th>counted</th><th>what</th></tr>" + "".join(
                f"<tr><td>{e(x['key_up'][11:])}</td><td>{e(x['key_down'][11:])}</td><td class=n>{x['keyed_s']:.1f}</td>"
                f"<td class=n>{x['counted_s']:.1f}</td><td>{e(x['purpose'])}{' (refused)' if x.get('refused') else ''}</td></tr>"
                for x in du["log"]) + "</table></div></details></div>")
    gl = meta.get("gates", [])
    if gl:
        seen, rows = set(), []
        for g in gl:
            key = (g["gate"].split(": ")[-1], g["ok"])
            if key in seen:
                continue
            seen.add(key)
            rows.append(f"<tr><td>{e(g['gate'])}</td><td>{pill('pass' if g['ok'] else 'FAIL')}</td><td>{e(g['detail'])}</td></tr>")
        parts.append("<h2>Safety gates</h2><div class=card><div class=wrap><table><tr><th>gate</th><th></th><th>detail</th></tr>"
                     + "".join(rows) + "</table></div></div>")
    b = meta["build"]
    parts.append(f"<p class=muted>Radio {e(b.get('version') or '–')} · host checkout {e(b.get('host') or '–')}"
                 f"{' · image ' + e(b['image']) if b.get('image') else ''} · port {e(meta.get('port') or '–')} · "
                 f"slot {e(meta.get('slot', 'A'))} · results {e(sess_dir)}</p>")
    return (f"<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'>"
            f"<title>Ferrite bench</title><style>{CSS}</style></head><body><main>{''.join(parts)}</main></body></html>")


def all_ok(v):
    if isinstance(v, list):
        return all(x.get("ok") for x in v)
    return bool(v.get("ok"))


def write_session(cfg, sess_dir, session, runs, extras, meta):
    meta = dict(meta, session=session, runs=[j["run"] for j in runs], extras=extras,
                flagged={j["run"]: [r[0] for r in j.get("comparison", []) if r[5] in ("CHANGED", "missing")] for j in runs})
    json.dump(jsonable(meta), open(os.path.join(sess_dir, "session.json"), "w"), indent=1)
    write_rule_results(sess_dir)
    page = os.path.join(sess_dir, "summary.html")
    open(page, "w").write(summary_page(session, sess_dir, runs, jsonable(extras), meta))
    return page


def record_session(sess_dir, docs):
    """--record: the session's bench and UI results into the test plan (tools/record_results.py)."""
    import record_results
    for line in record_results.record_paths([sess_dir], docs):
        say(line)


def new_session(cfg, label):
    os.makedirs(cfg["results"], exist_ok=True)
    while True:  # one session a second: the names sort by time
        stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
        if not any(x.startswith(stamp) for x in os.listdir(cfg["results"])):
            break
        time.sleep(0.3)
    session = f"{stamp}_{slug(label)[:40]}"
    d = os.path.join(cfg["results"], session)
    os.makedirs(d)
    return session, d


# ---- Flash ----


def do_flash(cfg, a, image):
    dry, sha = image_is_tx_dry(image)
    if not dry:
        raise SafetyError(f"{image} is not a tx-bench image (no TX BENCH boot line in it): build it with tools/mkbtf.sh tx-bench")
    if not os.path.exists(cfg["backup"]):
        raise BenchError(f"no flash backup at {cfg['backup']}")
    say(f"flashing {image} (tx-bench, sha256 {sha}) with flash.py --backup {cfg['backup']}")
    r = subprocess.run([sys.executable, os.path.join(HERE, "flash.py"), cfg["port"], image, "--backup", cfg["backup"]])
    if r.returncode != 0:
        raise BenchError(f"flash.py failed ({r.returncode})")
    say("waiting for Ferrite to boot ...")
    end = time.time() + 90
    while time.time() < end:
        time.sleep(3)
        try:
            rad = Radio(cfg["port"])
            return rad, os.path.basename(image), sha
        except Exception:
            continue
    raise BenchError("the radio didn't answer HELLO within 90 s of the flash")


# ---- Plan ----


def plan(cfg, a, runs):
    p = []
    L = cfg["slot"].upper()
    p.append(f"port {cfg['port']}; slot {L}; approved test frequencies {', '.join(f'{f:.3f}' for f in cfg['test_freqs'])} MHz")
    if a.flash:
        p.append(f"flash: check {a.flash} has the TX DRY boot line; flash.py {cfg['port']} {a.flash} --backup {cfg['backup']}; "
                 "wait for HELLO (up to 90 s)")
    p.append("connect: HELLO (the LINK-35 build byte: tx-dry with the bench bit, or tx+dry: Dry TX on, TX-48; else "
             "refused), INFO (version), "
             "`txtest` (a transmit build?)")
    p.append("gate: tx-dry known: version says dry / flashed a TX DRY image here / --i-know-its-tx-dry"
             + (" [given: a warning will be printed]" if a.i_know_its_tx_dry else " [not given]"))
    p.append(f"save: `slot {L}` (and `slot {L} vfo` if on a channel), the active slot (SLOT), `get {END_TONE}`, `get {TX_TIMEOUT}`")
    total_key = 0
    for r in runs:
        p.append(f"run {r['name']}: `slot {L} vfo {r['mhz']:.5f} {r['mode']} low tone off`, `slot {L} active` if not, "
                 f"`set {END_TONE} Off`, TX timeout to {cfg['timeout_raise']} if under {cfg['timeout_min_s']} s, `txtest suite`; "
                 f"gates (SLOT: frequency approved, VFO, offset 0, no split, power low, not scanning, active); rtl_sdr at "
                 f"{r['mhz']:.3f} MHz for {cfg['sdr_lead_s'] + cfg['suite_key_s'] + cfg['sdr_tail_s']:.1f} s; key {cfg['suite_key_s']} s "
                 f"(keepalive 0.5 s; KEY on must be answered within {KEY_ANSWER_S} s on the cable); KEY off; `txtest off`")
        total_key += cfg["suite_key_s"]
    if a.refusals:
        for r in cfg["refusals"]:
            p.append(f"refusal {r['mhz']:.3f}: `slot {L} vfo {r['mhz']:.5f} fm low tone off`; rtl_sdr 4 s; KEY on, expect refused "
                     f"({r['reason']!r}); KEY off at once whatever the answer; no carrier expected")
    if a.keepalive:
        k = cfg["keepalive"]
        p.append(f"keepalive: at {cfg['runs'][0]['mhz']:.3f} FM; key, keepalives {k['alive_s']} s then none, KEY off after "
                 f"{k['alive_s'] + k['wait_s']} s; carrier must end within {k['max_drop_s']} s of the last keepalive")
        total_key += k["alive_s"] + k["wait_s"]
    if a.timeout:
        t = cfg["timeout_test"]
        p.append(f"timeout: `set {TX_TIMEOUT} {t['set']}`; key {t['key_s']} s with keepalives; carrier {t['expect_s']} ± {t['tol_s']} s; "
                 f"timeout back to {cfg['timeout_raise']} (then to the saved value)")
        total_key += t["key_s"]
    sel = []
    if getattr(a, "sweep", None):
        try:
            sel = sweep_selection(cfg, parse_sweeps(a.sweep), a)
        except BenchError as e:
            sys.exit(str(e))
        sc = cfg["sweeps"]
        for name, items in sel:
            key_s, extra = sweep_key_s(cfg, name)
            n = len(items) if a.count is None else min(len(items), a.count)
            wall, keyed, counted = estimate_sweep(cfg, name, n)
            what = {"refusals-wide": "`slot A vfo F fm low tone off`, remote KEY on: refused with the band plan's "
                                     "reason for the radio's region (ones it allows aren't probed), no carrier; "
                                     "KEY off at once if it keys",
                    "ctcss": f"`slot A vfo {sc['mhz']:.3f} fm low txtone T rxtone off`: the tone within "
                             f"{sc['ctcss']['tol_hz']} Hz, every tone at one deviation",
                    "dcs": f"`... txtone DnnnN|I`: the code decoded with its own polarity only",
                    "endtones": "`set 0 8 4 X`, FM then NFM: on the unkey each step's Hz, ms and level (TONE-19.4), "
                                "the silent lead-in, the 400 ms hold",
                    "sublevel": "`set 0 8 8 1 Low|Normal|High` on 100.0 and D023N: the deviation steps",
                    "afsk-level": "`txtest afsk data L`: deviation in proportion to L"}[name]
            p.append(f"sweep {name}: {n} items ({items[0]['label']} ... {items[-1]['label']}"
                     + (f", --count {a.count}" if a.count is not None else "") + f"), key-ups of {key_s:g} s; {what}; "
                     f"about {wall / 60:.0f} min, {keyed:.0f} s keyed ({counted:.0f} s with tails) at the duty limits")
            total_key += keyed
        if getattr(a, "resume", None):
            p.append(f"--resume {a.resume}: items measured there are taken over, not run again")
    d = cfg["duty"]
    p.append(f"duty limit (DutyLimiter, every key-up): at most {d['max_burst_s']:g} s a key-up with its "
             f"{d['unkey_tail_s']:g} s unkey tail ({', '.join(d['exceptions'])} excepted), then a rest of "
             f"{d['rest_factor']:g}x; at most {d['window_frac']:.0%} of any {d['window_s'] / 60:g} min keyed (waits "
             "printed); refusal probes " + ("rest too" if d["probes_rest"] else "don't wait") + "; every key-up "
             "logged in session.json (duty)")
    p.append("restore (finally, whatever happened): KEY off, `txtest off`, the slot's VFO (freq, mode, power, tones), "
             "end tone, TX timeout" + (", 0 8 8 1" if any(n == "sublevel" for n, _ in sel) else "")
             + ", the active slot; check the slot reads back as saved")
    p.append(f"analyse each capture (sdrsuite), store under {cfg['results']}/<date-time>_<build>/, compare with the newest "
             "earlier run of the same configuration, write summary.html")
    say("dry plan (nothing is touched):")
    for i, s in enumerate(p, 1):
        say(f" {i:2d}. {s}")
    say(f"keyed in all: about {total_key:.0f} s; rtl_sdr {'found' if shutil.which('rtl_sdr') else 'NOT found'}")


# ---- Fakes for the selftest ----


class FakeRadio:
    """The radio as the bench sees it, on a virtual clock: slots, settings, txtest, KEY with the band plan's
    refusals, keepalive loss (2 s) and the TX timeout; every carrier logged for FakeSdr."""

    BANDS = [(144.0, 148.0), (420.0, 450.0)]

    def __init__(self, clock, version="v0.1.0", rx_only=False, hold_answers=False, offset=0, cable=True,
                 ignore_freq=False, stuck_power=None, fail_alive_after=None, build="not given", region=2, leaky=(),
                 dcs_bug=False, dry_off_after=None):
        self.build = build
        # TX-48: a tx+dry fake's Dry TX goes off (on the radio) after this many `ui` reads
        self.dry_off_after, self.ui_reads = dry_off_after, 0
        self.region, self.leaky, self.dcs_bug = region, set(leaky), dcs_bug
        self.clock, self.version, self.rx_only, self.hold, self.cable = clock, version, rx_only, hold_answers, cable
        self.ignore_freq, self.stuck_power, self.fail_alive_after = ignore_freq, stuck_power, fail_alive_after
        self.slots = [dict(ch=None, freq=146.52, mode="FM", power="low", tx="100.0", rx="off", offset=offset),
                      dict(ch=11, freq=443.475, mode="NFM", power="low", tx="100.0", rx="off", offset=500000),
                      dict(ch=None, freq=118.95, mode="AM", power="low", tx="off", rx="off", offset=0)]
        self.vfo_b = dict(ch=None, freq=446.0, mode="FM", power="low", tx="off", rx="off", offset=0)
        self.active = 0
        self.set = {END_TONE: "Beep", TX_TIMEOUT: "60 s", SUBTONE_LEVEL: "Normal", EXTEND_TX: "Off"}
        self.set0 = dict(self.set)
        self.txtest = None
        self.key = None  # dict(on, last, slot, kind)
        self.carriers = []  # (t_on, t_off, mhz, kind)
        self.calls = []
        self.unkey_msgs = []
        self.alive_n = 0

    def close(self):
        pass

    def hello(self):
        return dict(protocol=2, version=self.version, build=self.build, bench=self.build == "tx-dry")

    def dry(self):
        """The PA held at 0 now: the tx-dry build, or a tx build with Dry TX on until it is turned off (TX-48)."""
        if self.build == "tx+dry":
            return self.dry_off_after is None or self.ui_reads <= self.dry_off_after
        return self.build == "tx-dry"

    def info(self):
        return dict(version=self.version, channels=21, zones=6, presets=4, region=self.region, callsign="TEST")

    def _line(self, n):
        s = self.slots[n]
        where = f'ch {s["ch"] + 1} "Rpt"' if s["ch"] is not None else "vfo"
        tone = f"tone={s['tx']}" if s["tx"] == s["rx"] else f"txtone={s['tx']} rxtone={s['rx']}"
        off = f" offset={s['offset'] / 1e5:+.5f}" if s["offset"] else ""
        return f"{'ABC'[n]}{'*' if self.active == n else ' '} {where} {s['freq']:.5f} mode={s['mode']} power={s['power']} {tone}{off}\n"

    def term(self, line):
        self.calls.append(line)
        if self.key:
            raise TermError(f"{line}: transmitting: nothing changes while keyed (TX-22)")
        w = line.split()
        if line == "set 0 8 8 5 On" and self.build in ("tx", "tx+dry"):
            self.build = "tx+dry"  # TX-48.2: on from the PC
            return "0 8 8 5 Dry TX: On\n"
        if w[0] == "ui":
            self.ui_reads += 1
            return f"screen Home \"Home\"\nbar lock=0 scan=0 bt=0 cutoff=0 dry={int(self.dry())}\n"
        if w[0] == "txtest" and self.build == "tx+dry":
            raise TermError("txtest: no such command (a bench build's: TERM-38)")
        if w[0] == "txtest":
            if self.rx_only:
                raise TermError("txtest: the receive-only build has no transmit: txtest needs a tx or tx-dry build")
            if len(w) > 1 and w[1] == "suite":
                if int(re.match(r"\d+", self.set[TX_TIMEOUT]).group()) < 90:
                    raise TermError("txtest suite: the suite runs 60 s: set the TX timeout (0 8 1) to 90 s or more first")
                self.txtest = "suite"
            elif len(w) > 1 and w[1] == "afsk":
                self.txtest = ("afsk", int(w[-1]) if w[-1].isdigit() else 100)
            elif len(w) > 1 and w[1] == "off":
                self.txtest = None
            return f"  txtest: {'suite at 100 % armed for the next PTT key-up' if self.txtest else 'off (nothing armed)'}\n"
        if w[0] == "get":
            p = " ".join(w[1:])
            names = {END_TONE: "End tone", TX_TIMEOUT: "TX timeout", SUBTONE_LEVEL: "Subtone level", EXTEND_TX: "Extend TX"}
            return f"{p} {names.get(p, 'x')}: {self.set[p]}\n"
        if w[0] == "set":
            p = next(k for k in self.set if line.startswith(f"set {k} "))
            self.set[p] = line[len(f"set {p} "):]
            return f"{p} x: {self.set[p]}\n"
        if w[0] == "slot":
            if len(w) == 1:
                return "".join(self._line(n) for n in range(3))
            n = "ABC".index(w[1].upper())
            s = self.slots[n]
            i = 2
            while i < len(w):
                x = w[i].lower()
                if x == "vfo":
                    if s["ch"] is not None:
                        self.slots[n] = s = dict(self.vfo_b)
                elif x == "active":
                    self.active = n
                elif x in ("fm", "nfm", "am"):
                    s["mode"] = x.upper()
                elif x in ("low", "mid", "high"):
                    s["power"] = self.stuck_power or x
                elif x in ("tone", "txtone", "rxtone"):
                    i += 1
                    if x in ("tone", "txtone"):
                        s["tx"] = w[i]
                    if x in ("tone", "rxtone"):
                        s["rx"] = w[i]
                elif x[0].isdigit():
                    if s["ch"] is not None:
                        raise TermError(f"slot {w[1]}: the slot is on a channel: add vfo to tune its VFO")
                    if not self.ignore_freq:
                        f = float(x)
                        s["offset"] = 0 if not (144 <= f <= 148 or 420 <= f <= 450) else s["offset"]
                        s["freq"] = f
                i += 1
            return self._line(n)
        raise TermError(f"{w[0]}: no such command")

    def slot(self, n):
        s = self.slots[n]
        return dict(active=self.active == n, channel=s["ch"], freq10=mhz10(s["freq"]), mode=["FM", "NFM", "AM"].index(s["mode"]),
                    power=["low", "mid", "high"].index(s["power"]), rx_tone=tone_word(s["rx"]),
                    tx_tone=tone_word(s["tx"]), offset=s["offset"], split=False, aprs=False, scanning=False)

    def _refuse(self, f):
        """The radio's reasons (remote.rs, transmit.rs) by the band plan; `leaky` frequencies a broken radio keys."""
        if any(abs(f - x) < 1e-6 for x in self.leaky):
            return None
        why = tx_expect(f, self.region)
        return {"emergency": "invalid: emergency frequency: only PTT on the radio, twice",
                "band plan": "invalid: outside the band plan", None: None}[why]

    def key_on(self, slot_code):
        self.calls.append("KEY on")
        s = self.slots[self.active]
        why = self._refuse(s["freq"])
        if why:
            return 9, why
        t = self.clock.time()
        kind = "suite" if self.txtest == "suite" else "afsk" if self.txtest else "carrier"
        sig = dict(kind=kind, tone=s["tx"], mode=s["mode"], level=self.set[SUBTONE_LEVEL], endtone=self.set[END_TONE],
                   afsk=self.txtest[1] if kind == "afsk" else None, dcs_bug=self.dcs_bug)
        self.key = dict(on=t, last=t, mhz=s["freq"], kind=kind, sig=sig)
        self.txtest = None
        return None if self.hold else (0, "")

    def _check(self):
        k = self.key
        if not k:
            return None
        t = self.clock.time()
        tmo = int(re.match(r"\d+", self.set[TX_TIMEOUT]).group())
        end = min(k["last"] + 2.0, k["on"] + tmo)
        if t >= end:
            self.carriers.append((k["on"], end, k["mhz"], k["kind"], dict(k["sig"], t_off=end)))
            self.key = None
            msg = "invalid: TX timeout" if end == k["on"] + tmo else "invalid: keepalive lost: unkeyed"
            self.unkey_msgs.append(msg)
            return msg
        return None

    def key_alive(self):
        self.alive_n += 1
        if self.fail_alive_after and self.alive_n > self.fail_alive_after:
            raise BenchError("the cable fell out (fake)")
        self._check()
        if self.key:
            self.key["last"] = self.clock.time()

    def poll_unkey(self):
        self._check()
        return self.unkey_msgs.pop(0) if self.unkey_msgs else None

    def key_off(self):
        self.calls.append("KEY off")
        self._check()
        if self.key:
            k, t = self.key, self.clock.time()
            # TX-47: a let-go unkey plays the end tone (a voice key-up, after a 200 ms lead-in), then holds 400 ms
            seq = 0.0
            if k["kind"] == "carrier" and k["sig"]["endtone"] != "Off":
                seq = 0.2 + sum(ms for _, ms in dict(END_TONES)[dict(ENDTONE_SETTINGS)[k["sig"]["endtone"]]]) / 1000
            hold = 0.0 if k["kind"] == "suite" else seq + 0.5  # TX-47.2 on tx-dry: 400 ms hold + 100 ms unkey wait
            self.carriers.append((k["on"], t + hold, k["mhz"], k["kind"], dict(k["sig"], t_off=t)))
            self.key = None
        return True


FAKE_SUB = dict(ctcss=800.0, dcs=700.0, ctcss_byte=0x40, dcs_byte=0x38)  # the fake's calibration


def fake_dev(sig, tt, rel_off, rng):
    """A fake key-up's deviation (Hz) at tt s after its key-up, KEY off at rel_off: the sub-tone at the fake's
    calibration scaled by 0 8 8 1 (the byte +-0x0F CTCSS, +-0x14 DCS; `dcs_bug` sends a normal code inverted);
    `txtest afsk data L` flags at L % of 3 kHz until KEY off, else a little room noise on the mic; after KEY off the
    end tone's steps after a 200 ms lead-in at TONE-19.4's levels; NFM halves it all."""
    out = np.zeros(len(tt))
    before = tt < rel_off
    if sig["kind"] == "afsk":
        nb = int(max(tt.max(), 0) * 1200) + 2
        flag = (0x7E >> (np.arange(nb) % 8)) & 1
        hz = np.where((np.cumsum(flag == 0) % 2) == 0, 1200.0, 2200.0)
        ph0 = np.concatenate([[0.0], np.cumsum(2 * np.pi * hz / 1200)])
        b = np.clip(np.floor(tt * 1200).astype(int), 0, nb - 1)
        out += np.where(before, sig["afsk"] / 100 * 3000 * np.sin(ph0[b] + 2 * np.pi * hz[b] * (tt - b / 1200)), 0)
    else:
        out += np.where(before, rng.normal(0, 15, len(tt)), 0)
    tone = sig.get("tone") or "off"
    if tone != "off":
        m = re.fullmatch(r"D([0-7]{3})([NI])", tone)
        if m:
            k = {"Low": -1, "Normal": 0, "High": 1}[sig.get("level", "Normal")]
            amp = FAKE_SUB["dcs"] * (FAKE_SUB["dcs_byte"] + 0x14 * k) / FAKE_SUB["dcs_byte"]
            inv = m.group(2) == "I" or sig.get("dcs_bug", False)
            w = sdrsuite.dcs_codeword(int(m.group(1), 8), inv)
            bit = np.floor(tt * 134.4).astype(int) % 23
            out += np.where((w >> bit) & 1, amp, -amp)
        else:
            k = {"Low": -1, "Normal": 0, "High": 1}[sig.get("level", "Normal")]
            amp = FAKE_SUB["ctcss"] * (FAKE_SUB["ctcss_byte"] + 0x0F * k) / FAKE_SUB["ctcss_byte"]
            out += amp * np.sin(2 * np.pi * float(tone) * tt)
    if sig["kind"] == "carrier" and sig.get("endtone", "Off") != "Off":
        t = rel_off + 0.2
        for hz, ms in dict(END_TONES)[dict(ENDTONE_SETTINGS)[sig["endtone"]]]:
            m = (tt >= t) & (tt < t + ms / 1000)
            lvl = np.interp(hz, sorted(TONE_19_4), [TONE_19_4[f] for f in sorted(TONE_19_4)])
            out[m] += lvl * np.sin(2 * np.pi * hz * (tt[m] - t))
            t += ms / 1000
    return out * (0.5 if sig.get("mode") == "NFM" else 1.0)


class FakeSdr:
    """Synthetic captures from the fake radio's carrier log: the suite (sdrsuite's model radio) or a bare carrier."""

    def __init__(self, radio, clock, seed=7):
        self.r, self.clock, self.rng = radio, clock, np.random.default_rng(seed)

    def start(self, mhz, seconds, path):
        self.mhz, self.seconds, self.path, self.t0 = mhz, seconds, path, self.clock.time()

    def finish(self):
        fs = sdrtx.FS
        cs = [c for c in self.r.carriers if c[1] > self.t0 and c[0] < self.t0 + self.seconds and abs(c[2] - self.mhz) < 0.01]
        if cs and cs[0][3] == "suite":
            on, off = cs[0][0] - self.t0, cs[0][1] - self.t0
            raw, _ = sdrsuite.synth_iq(self.rng, pre=on, hold=max(off - on - sdrsuite.SUITE_S, 0.2),
                                       post=max(self.seconds - off, 0.2))
        else:
            # Each carrier's modulation (fake_dev) at 64 kS/s, held over each 16 samples at FS, FM onto the carrier
            # 700 Hz off, phase-continuous; noise everywhere.
            n = int(self.seconds * fs)
            fd = fs // sdrtx.DEC
            td = self.t0 + np.arange(n // sdrtx.DEC + 1) / fd
            dev = np.zeros(len(td))
            on = np.zeros(len(td), bool)
            for c in cs:
                m = (td >= c[0]) & (td < c[1])
                on |= m
                if len(c) > 4 and m.any():
                    dev[m] += fake_dev(c[4], td[m] - c[0], c[4].get("t_off", c[1]) - c[0], self.rng)
            raw = np.empty(2 * n, np.uint8)
            chunk, ph = 1 << 20, 0.0
            for a in range(0, n, chunk):
                idx = np.arange(a, min(a + chunk, n))
                j = idx // sdrtx.DEC
                d = np.where(on[j], dev[j] + 700.0, 0.0) - sdrtx.OFF
                p = ph + 2 * np.pi * np.cumsum(d) / fs
                ph = p[-1] % (2 * np.pi)
                x = np.where(on[j], 60.0, 0.0) * np.exp(1j * p) \
                    + self.rng.normal(0, 1.5, len(idx)) + 1j * self.rng.normal(0, 1.5, len(idx))
                raw[2 * a: 2 * idx[-1] + 2: 2] = np.clip(np.round(x.real + 127.5), 0, 255)
                raw[2 * a + 1: 2 * idx[-1] + 2: 2] = np.clip(np.round(x.imag + 127.5), 0, 255)
        raw.tofile(self.path)
        return raw

    def stop(self):
        pass


class SimSdr(FakeSdr):
    """For --selftest --sim: the simulator keys nothing on air, so its carriers are taken from the bench's own key
    events (on to the last keepalive + 2 s, or the KEY off), the suite from sdrsuite's model."""

    def __init__(self, clock, bench_ref):
        super().__init__(None, clock)
        self.bench_ref = bench_ref
        self.r = self

    @property
    def carriers(self):
        ev = self.bench_ref.get("last_ev")
        if not ev or not ev.get("accepted"):
            return []
        end = min(ev["t_off"], (ev["t_last_alive"] or ev["t_on"]) + 2.0)
        return [(ev["t_on"], end, self.mhz, "suite" if ev["purpose"] in [r["name"] for r in DEFAULTS["runs"]] else "carrier")]


# ---- Main flows ----


def merge(base, over):
    """`over` into `base`: dicts merged key by key (so a config can change one sweep's one number), the rest replaced."""
    for k, v in over.items():
        if isinstance(v, dict) and isinstance(base.get(k), dict):
            merge(base[k], v)
        else:
            base[k] = v
    return base


def load_cfg(path):
    cfg = json.loads(json.dumps(DEFAULTS))
    if path:
        merge(cfg, json.load(open(path)))
    return cfg


def pick_runs(cfg, names):
    if not names:
        return cfg["runs"]
    if names == "none":  # only the extra checks (--refusals/--keepalive/--timeout): no 60 s suite keying
        return []
    by = {r["name"]: r for r in cfg["runs"]}
    out = []
    for n in names.split(","):
        if n not in by:
            sys.exit(f"no run {n!r}; the runs: {', '.join(by)}")
        out.append(by[n])
    return out


def find_captures(cfg, specs):
    """--load: DIRs searched (recursively) for <run>.u8 or an alias's; or RUN=FILE."""
    found = {}
    names = {r["name"]: r for r in cfg["runs"]}
    for spec in specs:
        if "=" in spec and not os.path.isdir(spec):
            n, p = spec.split("=", 1)
            found[n] = p
            continue
        for dp, _, fs in os.walk(spec):
            for f in sorted(fs):
                if not f.endswith(".u8"):
                    continue
                stem = f[:-3]
                n = stem if stem in names else cfg["aliases"].get(stem)
                if n in names and n not in found:
                    found[n] = os.path.join(dp, f)
            for f in fs:
                if f in ("keepalive.u8", "timeout.u8") and os.path.exists(os.path.join(dp, f[:-3] + ".json")):
                    found.setdefault("_" + f[:-3], os.path.join(dp, f))
    return found


def do_load(cfg, a):
    caps = find_captures(cfg, a.load)
    runs = [r for r in cfg["runs"] if r["name"] in caps and (not a.runs or r["name"] in a.runs.split(","))]
    if not runs:
        sys.exit(f"no captures found in {a.load} (looked for {', '.join(r['name'] + '.u8' for r in cfg['runs'])} and "
                 f"{', '.join(k + '.u8' for k in cfg['aliases'])})")
    label = a.build or ("load-" + slug(os.path.basename(os.path.normpath(a.load[0]))))
    session, sess_dir = new_session(cfg, label)
    build = dict(label=label, version=None, host=host_build(), source="load")
    say(f"re-analysing {len(runs)} capture(s) into {sess_dir}")
    out = [analyse_run(cfg, a, r, caps[r["name"]], sess_dir, session, build) for r in runs]
    extras = {}
    for k, fn in (("keepalive", analyse_keepalive), ("timeout", analyse_timeout)):
        p = caps.get("_" + k)
        if p:
            side = json.load(open(p[:-3] + ".json"))
            extras[k] = fn(np.fromfile(p, np.uint8), side["events"], side["cfg"])
    page = write_session(cfg, sess_dir, session, out, extras,
                         dict(build=build, source="re-analysed captures from " + ", ".join(
                             os.path.basename(os.path.normpath(x)) for x in a.load), port=None, slot=cfg["slot"]))
    finish_report(out, extras, page)
    return out, page


def finish_report(runs, extras, page):
    say("\nsummary:")
    for j in runs:
        rows = j.get("comparison", [])
        fl_ = [r for r in rows if r[5] in ("CHANGED", "missing")]
        say(f"  {j['run']:9s} {'ok  ' if j['ok'] else 'FAIL'}  " + "  ".join(f"{k}: {v}" for k, v in headline(j["metrics"])[:4]))
        if j.get("previous"):
            say(f"            against {j['previous']['session']}: " + (", ".join(
                f"{r[0]} {fmt(r[2])} -> {fmt(r[1])}" for r in fl_[:8]) + (" ..." if len(fl_) > 8 else "") if fl_ else "all within tolerance"))
    for k, v in extras.items():
        say(f"  {k}: {'pass' if all_ok(v) else 'FAIL'}")
    say(f"\npage: {page}")


def do_bench(cfg, a, radio=None, sdr=None, clock=None):
    # --sweep alone runs no 60 s suite (give --runs too for both)
    runs = pick_runs(cfg, "none" if getattr(a, "sweep", None) and not a.runs else a.runs)
    clock = clock or Clock()
    image = sha = None
    if a.flash:
        radio, image, sha = do_flash(cfg, a, a.flash)
    radio = radio or Radio(cfg["port"])
    bench = Bench(cfg, a, radio=radio, sdr=sdr or Sdr(cfg, clock), clock=clock)
    _CURRENT.append(bench)
    try:
        hello = radio.hello() if hasattr(radio, "hello") else {}
        info = radio.info()
        say(f"radio: firmware {info['version']}, callsign {info.get('callsign') or '-'}")
        label = a.build or (f"{info['version']}-img{sha}" if sha else f"{info['version']}-host{host_build()}")
        build = dict(label=label, version=info["version"], host=host_build(), image=f"{image} {sha}" if image else None)
        session, sess_dir = new_session(cfg, label)
        say(f"session {sess_dir}")
        meta = dict(build=build, source="on air", port=getattr(radio, "spec", "fake"), slot=cfg["slot"],
                    commit=host_build(), kinds=["rf"], cfg=dict(sweeps=cfg["sweeps"]),
                    sim=not isinstance(radio, Radio) or not radio.cable)  # selftest or simulator: never recorded
        try:
            caps, extras = bench.on_air(sess_dir, runs, hello, info, flashed_dry=image)
        except SafetyError as e:
            meta.update(gates=bench.gatelog, warnings=bench.warnings + [str(e)], restore_notes=bench.restore_notes,
                        tx_dry=bench.tx_dry, refused=str(e), duty=bench.duty.summary())
            json.dump(jsonable(meta), open(os.path.join(sess_dir, "session.json"), "w"), indent=1)
            say(f"\n{e}")
            raise
        meta.update(gates=bench.gatelog, warnings=bench.warnings, restore_notes=bench.restore_notes, tx_dry=bench.tx_dry,
                    duty=bench.duty.summary())
    finally:
        bench.emergency_unkey()
        _CURRENT.remove(bench)
        if bench.duty.log:
            say(bench.duty.report())
    out = []
    for c in caps:
        out.append(analyse_run(cfg, a, c["run"], c["capture"], sess_dir, session, build, events=c["events"]))
        if not a.keep_captures:
            with contextlib.suppress(OSError):
                os.remove(c["capture"])
    page = write_session(cfg, sess_dir, session, out, extras, meta)
    finish_report(out, extras, page)
    return out, extras, page, bench


_CURRENT = []


def _last_ditch():
    for b in list(_CURRENT):
        b.emergency_unkey()


atexit.register(_last_ditch)


# ---- Change-aware selection (--auto) ----

# (glob, class, why): first match wins. Paths are in the ferrite checkout. Classes: link (link, store, boot,
# ports: both sides and a reboot check), rf (radio, drivers, DSP, TX, calibration, firmware tasks: the SDR suite on
# tx-dry and the UI smoke set), ui (screens, drawing, app logic, docs: the UI tests on the normal build), host
# (tests, the simulators, CI, PC tools that don't touch the radio: nothing on the radio).
CLASSES = [
    # Link, store, boot, ports, the image itself.
    ("crates/ferrite-app/src/link.rs", "link", "the link"),
    ("crates/ferrite-app/src/link/*", "link", "the link"),
    ("crates/ferrite-core/src/link.rs", "link", "the link protocol"),
    ("crates/ferrite-core/src/memmap.rs", "link", "the flash map"),
    ("crates/ferrite-store/*", "link", "the store"),
    ("crates/ferrite-app/src/persist.rs", "link", "what is saved"),
    ("crates/ferrite-app/src/ports.rs", "link", "the ports"),
    ("crates/ferrite-radio/src/cable.rs", "link", "the cable"),
    ("crates/ferrite-radio/src/flash.rs", "link", "the SPI flash"),
    ("firmware/src/tasks/link.rs", "link", "the link task"),
    ("firmware/src/tasks/storage.rs", "link", "the storage task"),
    ("firmware/src/store.rs", "link", "the store"),
    ("firmware/src/main.rs", "link", "boot"),
    ("firmware/src/board.rs", "link", "boot"),
    ("firmware/src/bringup.rs", "link", "boot"),
    ("firmware/src/report.rs", "link", "the boot report"),
    ("firmware/src/hal/uart.rs", "link", "the UART"),
    ("firmware/src/hal/clock.rs", "link", "clocks"),
    ("firmware/src/hal/irq.rs", "link", "interrupts"),
    ("firmware/src/hal/sys.rs", "link", "the system"),
    ("firmware/memory.x", "link", "the memory map"),
    ("firmware/build.rs", "link", "the build"),
    ("firmware/Cargo.toml", "link", "the firmware's features"),
    ("drivers/w25q/*", "link", "the SPI flash driver"),
    ("Cargo.toml", "link", "dependencies"),
    ("Cargo.lock", "link", "dependencies"),
    ("rust-toolchain.toml", "link", "the toolchain"),
    (".cargo/*", "link", "the build"),
    ("tools/ferrite_link.py", "link", "the PC side of the link"),
    ("tools/flash.py", "link", "flashing"),
    ("tools/btf.py", "link", "the image format"),
    ("tools/mkbtf.sh", "link", "the image build"),
    # Radio, drivers, DSP, transmit, calibration.
    ("firmware/src/tasks/radio/*", "rf", "the Radio task"),
    ("firmware/src/tasks/dsp.rs", "rf", "DSP"),
    ("firmware/src/tasks/decode.rs", "rf", "decoders"),
    ("firmware/src/tasks/aprs.rs", "rf", "APRS"),
    ("firmware/src/tasks/gps.rs", "rf", "GPS"),
    ("firmware/src/tasks/input.rs", "ui", "keys"),
    ("firmware/src/tasks/ui.rs", "ui", "the UI task"),
    ("firmware/src/display.rs", "ui", "the display"),
    ("firmware/src/hal/backlight.rs", "ui", "the backlight"),
    ("firmware/*", "rf", "firmware"),
    ("drivers/st7789/*", "ui", "the display driver"),
    ("drivers/keypad/*", "ui", "the keypad driver"),
    ("drivers/*", "rf", "a chip driver"),
    ("crates/ferrite-radio/*", "rf", "chip recipes"),
    ("crates/ferrite-core/src/tx*.rs", "rf", "transmit"),
    ("crates/ferrite-core/src/*cal*.rs", "rf", "calibration"),
    ("crates/ferrite-core/src/tonegen.rs", "rf", "tones"),
    ("crates/ferrite-core/src/audio.rs", "rf", "audio"),
    ("crates/ferrite-core/src/settle.rs", "rf", "chip timing"),
    ("crates/ferrite-core/src/rxsleep.rs", "rf", "receive sleep"),
    ("crates/ferrite-core/src/vox.rs", "rf", "VOX"),
    ("crates/ferrite-core/src/ssb.rs", "rf", "SSB"),
    ("crates/ferrite-core/src/xb.rs", "rf", "crossband"),
    ("crates/ferrite-core/src/same*.rs", "rf", "SAME"),
    ("crates/ferrite-core/src/decoders.rs", "rf", "decoders"),
    ("crates/ferrite-core/src/stuck.rs", "rf", "the PTT check"),
    ("crates/ferrite-core/src/gpspower.rs", "rf", "GPS power"),
    ("crates/ferrite-core/src/chipid.rs", "rf", "chip IDs"),
    ("crates/ferrite-core/src/acc.rs", "rf", "the accumulator"),
    ("tools/bench.py", "rf", "the bench itself"),
    ("tools/sdr*.py", "rf", "the SDR analysis"),
    # The UI and the app's logic.
    ("crates/ferrite-app/src/term/*", "ui", "the terminal"),
    ("crates/ferrite-app/*", "ui", "the app"),
    ("crates/ferrite-ui/*", "ui", "drawing"),
    ("crates/ferrite-core/*", "ui", "core logic"),
    ("fonts/*", "ui", "fonts"),
    ("assets/*", "ui", "assets"),
    ("tools/uitest.py", "ui", "the UI tests"),
    ("tools/uitests/*", "ui", "the UI tests"),
    ("README.md", "ui", "docs"),
    ("*.md", "ui", "docs"),
    # Nothing on the radio.
    ("*/tests/*", "host", "host tests"),
    ("*/tests.rs", "host", "host tests"),
    ("*/tests_*.rs", "host", "host tests"),
    ("sim/*", "host", "the simulator"),
    ("sim-web/*", "host", "the web simulator"),
    (".github/*", "host", "CI"),
    ("crates/ferrite-checks/*", "host", "the rule checks"),
    ("tools/*", "host", "a PC tool"),
]
CLASS_ORDER = {"host": 0, "ui": 1, "rf": 2, "link": 3}


# Screen files several screens share: a change there runs every UI test, not a page's.
SHARED_SCREEN_FILES = {"mod", "shared", "lists", "forms", "parts", "table", "nav", "layouts"}


def screen_pages(paths):
    """The pages (screen file stems) when every changed UI file is one screen's own file
    (crates/ferrite-app/src/screens/<page>.rs, not a shared one), else None: --auto then runs only the UI tests
    that visit those screens (uitest.py --page)."""
    pages = set()
    for f in paths:
        m = re.fullmatch(r"crates/ferrite-app/src/screens/(\w+)\.rs", f)
        if not m or m.group(1) in SHARED_SCREEN_FILES or m.group(1).startswith("tests"):
            return None
        pages.add(m.group(1))
    return sorted(pages) or None


def classify(path):
    """(class, why) of one changed file; unknown files count as link (both sides and a reboot: the safe side)."""
    for glob, cls, why in CLASSES:
        if fnmatch.fnmatch(path, glob):
            # Host tests inside src/ (tests.rs, tests/...) don't change the image.
            if cls != "host" and re.search(r"(^|/)tests?(_\w+)?(\.rs|/)", path) and path.endswith(".rs"):
                return "host", "host tests"
            return cls, why
    return "link", "not classified: both sides to be safe"


def git_ferrite(*a):
    r = subprocess.run(["git", "-C", FERRITE, *a], capture_output=True, text=True)
    return r.stdout if r.returncode == 0 else None


def changed_files(base):
    """Files changed from base to HEAD (and uncommitted ones); None if base isn't in this checkout."""
    if git_ferrite("cat-file", "-e", f"{base}^{{commit}}") is None:
        return None
    out = git_ferrite("diff", "--name-only", f"{base}..HEAD") or ""
    out += git_ferrite("diff", "--name-only", "HEAD") or ""
    return sorted({x for x in out.splitlines() if x})


def last_runs(results):
    """The newest on-radio session per side: {"ui": (session, commit), "rf": (...)}. Simulator, selftest and
    --load sessions don't count."""
    found = {}
    if not os.path.isdir(results):
        return found
    for name in sorted(os.listdir(results)):
        p = os.path.join(results, name, "session.json")
        try:
            m = json.load(open(p))
        except (OSError, ValueError):
            continue
        if m.get("source") not in ("on air", "radio") or str(m.get("port", "")).startswith("tcp:"):
            continue
        commit = (m.get("commit") or (m.get("build") or {}).get("host") or "").rstrip("+")
        if not commit or commit == "unknown":
            continue
        kinds = m.get("kinds") or (["rf"] if m.get("runs") else [])
        for k in kinds:
            found[k] = (name, commit)
    return found


def decide(cfg, a):
    """What --auto runs and why: dict(ui=None|"full"|"smoke", rf=bool, reboot=bool, what, why, files)."""
    head = (git_ferrite("rev-parse", "--short=7", "HEAD") or "?").strip()
    if a.all:
        return dict(ui="full", rf=True, reboot=True, what="everything (--all)", why="forced", files=[], head=head)
    if a.ui or a.rf:
        return dict(ui="full" if a.ui else ("smoke" if a.rf else None), rf=bool(a.rf), reboot=False,
                    what=("UI tests" if a.ui else "") + (" and " if a.ui and a.rf else "") + ("the SDR suite" if a.rf else ""),
                    why="forced (--ui/--rf)", files=[], head=head)
    last = last_runs(cfg["results"])
    if a.diff_files is not None:
        files = {k: [f for f in a.diff_files.split(",") if f] for k in ("ui", "rf")}
        bases = {k: "(--diff-files)" for k in ("ui", "rf")}
    else:
        files, bases = {}, {}
        for k in ("ui", "rf"):
            if a.since:
                bases[k] = a.since
            elif k in last:
                bases[k] = last[k][1]
            else:
                bases[k] = None
            files[k] = changed_files(bases[k]) if bases[k] else None
    if files["ui"] is None and files["rf"] is None:
        why = ("no earlier on-radio run with a commit in " + cfg["results"]) if not last else             f"the last runs' commits ({', '.join(c for _, c in last.values())}) aren't in this checkout"
        return dict(ui="full", rf=True, reboot=True, what="everything: the first run", why=why, files=[], head=head)
    cls = {k: [(f, *classify(f)) for f in (v or [])] for k, v in files.items()}
    # A side with no earlier on-radio run runs in full (the first time: everything on that side).
    rf_cls = {c for _, c, _ in cls["rf"]} if files["rf"] is not None else {"rf"}
    ui_cls = {c for _, c, _ in cls["ui"]} if files["ui"] is not None else {"ui"}
    first = [k.upper() for k in ("ui", "rf") if files[k] is None]
    allf = sorted({(f, f"{c}: {w}") for k in cls for f, c, w in cls[k]})
    link = "link" in rf_cls | ui_cls
    rf = link or "rf" in rf_cls
    ui_full = link or bool(ui_cls & {"ui"})
    since = "; ".join(f"{k.upper()} since {bases[k]}" + (f" ({last[k][0]})" if k in last and bases[k] == last[k][1] else "")
                      for k in ("ui", "rf") if bases.get(k))
    names = lambda c: ", ".join(f for f, cc, _ in sorted({x for k in cls for x in cls[k]}) if cc == c)  # noqa: E731
    if link:
        d = dict(ui="full", rf=True, reboot=True, what="both: the UI tests and the SDR suite on tx-dry, then a reboot check",
                 why=f"link/store/boot/port changes ({names('link')})")
    elif rf:
        d = dict(ui="full" if ui_full else "smoke", rf=True, reboot=False,
                 what="the SDR suite on tx-dry + " + ("the UI tests" if ui_full else "the UI smoke set"),
                 why=f"radio/driver/DSP/TX/calibration changes ({names('rf')})"
                 + (f"; UI changes too ({names('ui')})" if ui_full else ""))
    elif ui_full and screen_pages([f for k in cls for f, c, _ in cls[k] if c == "ui"]):
        pages = screen_pages([f for k in cls for f, c, _ in cls[k] if c == "ui"])
        d = dict(ui="full", pages=pages, rf=False, reboot=False,
                 what=f"the UI tests that visit {', '.join(pages)} (--page) on the normal build only",
                 why=f"only screen files changed ({names('ui')})")
    elif ui_full:
        d = dict(ui="full", rf=False, reboot=False, what="the UI tests on the normal build only (no TX, no SDR)",
                 why=f"UI/app-only changes ({names('ui')})")
    elif allf:
        d = dict(ui=None, rf=False, reboot=False, what="nothing on the radio",
                 why=f"host-only changes (tests, simulators, CI, PC tools: {names('host')}); --ui or --rf to run anyway")
    else:
        d = dict(ui=None, rf=False, reboot=False, what="nothing on the radio", why="nothing changed since the last runs")
    if first:
        d["why"] += f"; no earlier on-radio {' or '.join(first)} run: that side runs in full"
    d.update(files=allf, head=head, since=since)
    return d


def print_decision(d):
    say(f"decision: {d['what']}")
    say(f"  why: {d['why']}" + (f" [{d['since']}]" if d.get("since") else "") + f"; HEAD {d['head']}")
    for f, c in d.get("files", [])[:40]:
        say(f"    {c:28s} {f}")
    if len(d.get("files", [])) > 40:
        say(f"    ... {len(d['files']) - 40} more")


def reboot_check(port, log=say):
    """LINK-24 restart (into the bootloader, which starts Ferrite again after ~200 ms), HELLO again within 60 s,
    home on the screen, and every setting, channel and zone as before (`dump`)."""
    r = fl.Radio(port)
    before = r.cmd("dump")
    r.link.ok(fl.REBOOT)
    r.close()
    t0 = time.time()
    time.sleep(3)
    while True:
        try:
            r = fl.Radio(port)
            break
        except Exception:
            if time.time() - t0 > 60:
                return dict(ok=False, text="no HELLO within 60 s of the restart")
            time.sleep(2)
    up = time.time() - t0
    time.sleep(2)
    screen = r.ui()["screen"]["id"]
    after = r.cmd("dump")
    r.close()
    la, lb = [x for x in before.splitlines() if not x.startswith("#")], [x for x in after.splitlines() if not x.startswith("#")]
    diff = [x for x in la if x not in lb] + [x for x in lb if x not in la]
    ok = screen == "Home" and not diff
    text = f"back after {up:.0f} s; screen {screen}; dump {'unchanged' if not diff else 'changed: ' + '; '.join(diff[:6])}"
    log(f"reboot check: {'pass' if ok else 'FAIL'}: {text}")
    return dict(ok=ok, text=text)


def do_auto(cfg, a):
    """--auto (and --all, --ui, --rf): decide, then run what was decided, one session and one page for both."""
    import uitest
    d = decide(cfg, a)
    print_decision(d)
    if a.dry_plan:
        if d["rf"]:
            say("")
            plan(cfg, a, pick_runs(cfg, a.runs))
        if d["ui"]:
            say(f"UI: tools/uitest.py --port {cfg['port']}{' --smoke' if d['ui'] == 'smoke' else ''}"
                f"{' --page ' + ','.join(d['pages']) if d.get('pages') else ''} (remote keys and readback; "
                "no PTT; settings and slots put back)")
        if d["reboot"]:
            say("reboot: LINK-24 restart, HELLO within 60 s, home on the screen, `dump` unchanged")
        return 0
    if not (d["ui"] or d["rf"] or d["reboot"]):
        return 0
    bad = False
    sess_dir = page = None
    meta = None
    if d["rf"]:
        try:
            out, extras, page, _ = do_bench(cfg, a)
        except SafetyError as e:
            say(f"SDR suite refused: {e}")
            return 2
        sess_dir = os.path.dirname(page)
        meta = json.load(open(os.path.join(sess_dir, "session.json")))
        runs = out
        bad |= any(not j["ok"] for j in out) or any(not all_ok(v) for v in extras.values())
    else:
        runs, extras = [], {}
    ui_summary = None
    if d["ui"]:
        ua = uitest.main_args(["--port", cfg["port"]] + (["--smoke"] if d["ui"] == "smoke" else [])
                              + (["--page", ",".join(d["pages"])] if d.get("pages") else []))
        sel = uitest.select(uitest.load_tests(), ua, cfg["port"].startswith("tcp:"))
        if d.get("pages") and not sel:
            say(f"no UI test visits {', '.join(d['pages'])}: the full set instead")
            ua = uitest.main_args(["--port", cfg["port"]])
            sel = uitest.select(uitest.load_tests(), ua, cfg["port"].startswith("tcp:"))
        sim = cfg["port"].startswith("tcp:")
        ua.verbose = VERBOSE
        ui_summary = uitest.run(ua, cfg["port"], sim, sel)
        if sess_dir is None:
            label = f"{ui_summary['version']}-{ui_summary['commit']}-{ui_summary['build']}"
            session, sess_dir = new_session(cfg, label)
            meta = dict(build=dict(label=label, version=ui_summary["version"], host=host_build()),
                        source="simulator" if sim else "radio",
                        port=cfg["port"], commit=host_build(), kinds=[])
        uitest.write_results(ui_summary, sess_dir)
        meta["kinds"] = sorted(set(meta.get("kinds", [])) | {"ui"})
        meta["ui"] = dict(counts=ui_summary["counts"], set="smoke" if d["ui"] == "smoke" else "full")
        c = ui_summary["counts"]
        bad |= bool(c["fail"] or c["error"] or ui_summary["left_changed"])
    if d["reboot"]:
        rb = reboot_check(cfg["port"])
        meta["reboot"] = rb
        bad |= not rb["ok"]
    meta["decision"] = {k: d[k] for k in ("what", "why", "files", "head")}
    json.dump(jsonable(meta), open(os.path.join(sess_dir, "session.json"), "w"), indent=1)
    if d["rf"] or d["reboot"]:
        write_rule_results(sess_dir)
    page_meta = dict(meta, ui=ui_summary) if ui_summary else meta
    page = os.path.join(sess_dir, "summary.html")
    open(page, "w").write(summary_page(os.path.basename(sess_dir), sess_dir, runs, jsonable(extras), page_meta))
    say(f"\npage: {page}")
    if getattr(a, "record", False):
        record_session(sess_dir, a.docs)
    return 1 if bad else 0


def selftest_auto():
    """The selection rules on example diffs (no radio): each example's decision."""
    examples = [
        (["crates/ferrite-app/src/screens/home.rs", "crates/ferrite-ui/src/widgets.rs"], "ui", False, False),
        (["crates/ferrite-app/src/screens/zones.rs", "crates/ferrite-app/src/screens/browse.rs"], ("ui", ["browse", "zones"]), False, False),
        (["crates/ferrite-app/src/screens/zones.rs", "crates/ferrite-app/src/screens/lists.rs"], "ui", False, False),
        (["firmware/src/tasks/radio/tx.rs", "crates/ferrite-app/src/screens/home.rs"], "full", True, False),
        (["drivers/bk4829/src/lib.rs"], "smoke", True, False),
        (["crates/ferrite-core/src/txcal.rs"], "smoke", True, False),
        (["crates/ferrite-store/src/lib.rs"], "full", True, True),
        (["crates/ferrite-app/src/link.rs"], "full", True, True),
        (["sim/src/main.rs", "crates/ferrite-app/src/tests/qmenu.rs", ".github/workflows/ci.yml"], None, False, False),
        (["some/new/thing.rs"], "full", True, True),
    ]
    ok = True
    for files, ui, rf, rb in examples:
        a = argparse.Namespace(all=False, ui=False, rf=False, since=None, diff_files=",".join(files))
        d = decide(dict(results="/nonexistent"), a)
        ui, pages = ui if isinstance(ui, tuple) else (ui, None)
        want_ui = "full" if ui == "ui" else ui
        good = (d["ui"], d["rf"], d["reboot"], d.get("pages")) == (want_ui, rf, rb, pages)
        ok &= good
        say(f"  [{'pass' if good else 'FAIL'}] {', '.join(files)}: {d['what']}")
    return ok


# ---- Selftest ----


def duty_ok(log, dcfg):
    """(ok, worst window s, rests ok, bursts ok) of a duty log against its limits."""
    lim = dict(DEFAULTS["duty"], **dcfg)
    spans = [(e["t_on"], e["t_on"] + e["counted_s"], e) for e in log if e["counted_s"] > 0]
    W = lim["window_s"]
    worst = max((sum(max(0, min(b, y) - max(b - W, x)) for x, y, _ in spans) for _, b, _ in spans), default=0)
    rests = all(n[0] >= p[0] + p[2]["counted_s"] * (1 + lim["rest_factor"]) - 1e-6 for p, n in zip(spans, spans[1:]))
    bursts = all(e["counted_s"] <= lim["max_burst_s"] + 1e-6 or e["kind"] in lim["exceptions"] for _, _, e in spans)
    return worst <= lim["window_frac"] * W + 1e-6 and rests and bursts, worst, rests, bursts


def selftest_duty(check):
    clk = FakeClock()
    msgs = []
    d = DutyLimiter({}, clk, out=msgs.append)
    try:
        d.check_burst(d.planned(5.0), "burst", "a 5 s key-up")
        check("duty: a 5 s key-up (+0.5 s tail) is refused", False)
    except BenchError as e:
        check("duty: a 5 s key-up (+0.5 s tail) is refused", "burst limit" in str(e), str(e)[:70])
    try:
        d.check_burst(d.planned(63.5), "suite", "suite")
        check("duty: the 60 s suite passes as an exception", True)
    except BenchError as e:
        check("duty: the 60 s suite passes as an exception", False, e)
    for _ in range(40):  # 4.0 s key-ups back to back: the rest, then the 5-min cap
        planned = d.planned(4.0)
        d.wait(planned, "burst", "b", lead=1.0)
        clk.sleep(1.0)
        d.admit(planned, "burst", "b")
        t = clk.time()
        clk.sleep(4.0)
        d.record("b", "burst", dict(accepted=True, t_on=t, t_off=clk.time()), t)
        clk.sleep(1.5)
    ok, worst, rests, bursts = duty_ok(d.log, {})
    check("duty: 40 key-ups of 4 s: rest 2x after each, at most 75 s in any 5 min", ok,
          f"fullest 5 min {worst:.1f} s; rests {rests}; bursts {bursts}")
    check("duty: the rolling cap paused them (and said so)", any(w["why"] == "window" for w in d.waits)
          and any("would pass the 75 s cap" in m for m in msgs), f"{sum(w['why'] == 'window' for w in d.waits)} pauses")
    s = d.summary()
    check("duty: overall at most 25 % once the cap holds", s["counted_s"] / s["wall_s"] <= 0.26,
          f"{s['counted_s']:.0f} s over {s['wall_s']:.0f} s")
    w, _ = d.wait_needed(d.planned(KEY_ANSWER_S), clk.time(), "probe")
    check("duty: a refusal probe doesn't wait", w == 0, w)
    t = clk.time() + 1000
    d.record("suite", "suite", dict(accepted=True, t_on=t, t_off=t + 63.5), t)
    clk.t = t + 63.5
    w, why = d.wait_needed(d.planned(4.0), clk.time(), "burst")
    check("duty: after the 64 s suite a rest of 2x it", w >= 2 * 64 - 1e-6, f"{w:.0f} s ({why})")
    e = d.record("refused", "probe", dict(accepted=False, refused="x", t_on=t, t_off=t + 0.1), t)
    check("duty: a refused key-up counts nothing", e["counted_s"] == 0)
    ctcss, dcs = tone_lists()
    check("tone lists from slot.rs: 50 CTCSS, 104 DCS (as sdrsuite's)", len(ctcss) == 50 and len(dcs) == 104
          and dcs == sdrsuite.DCS_CODES and ctcss[0] == 67.0 and ctcss[-1] == 254.1, f"{len(ctcss)}, {len(dcs)}")
    cases = [(146.52, 1, None), (222.0, 1, None), (219.5, 1, "band plan"), (219.5, 2, None), (222.0, 3, "band plan"),
             (51.0, 3, None), (53.0, 3, "band plan"), (53.0, 1, None), (53.0, 7, None), (27.185, 1, "band plan"),
             (21.2, 4, None), (18.1, 1, None),
             (243.025, 1, "emergency"), (243.03, 1, "band plan"), (406.125, 1, "emergency"), (146.0, 3, "band plan"),
             (425.0, 2, "band plan"), (425.0, 1, None), (121.5, 0, "emergency"), (28.4, 1, None)]
    bad = [(f, r, tx_expect(f, r), w) for f, r, w in cases if tx_expect(f, r) != w]
    check("band plan mirror (txplan.rs): the expected refusals", not bad, bad)


def selftest_sweeps(cfg, args, check, tmp):
    c2 = json.loads(json.dumps(cfg))
    c2["duty"]["window_s"] = 60.0  # cap 15 s a minute: the sweeps meet it
    sw = c2["sweeps"]
    sw["ctcss"]["only"] = ["67.0", "100.0", "254.1", "60.0"]
    sw["dcs"]["only"] = ["D023N", "D023I", "D754I", "D000N"]
    sw["endtones"]["only"] = [f"{x}-fm" for _, x in ENDTONE_SETTINGS] + ["quindar-nfm"]
    sw["afsk-level"]["only"] = ["afsk-50", "afsk-100", "afsk-150"]
    clk = FakeClock()
    r = FakeRadio(clk, version="v0.1.0 TX DRY")
    before = json.dumps(r.slots), dict(r.set), r.active
    out, extras, page, bench = do_bench(c2, args(sweep="all", build="selftest-sweeps"), radio=r, sdr=FakeSdr(r, clk),
                                        clock=clk)
    sess = os.path.dirname(page)
    for name in SWEEP_NAMES:
        x = extras.get(f"sweep:{name}") or {}
        bad = [(i["label"], i.get("why")) for i in x.get("items", []) if not i.get("ok")]
        check(f"sweep {name}: every item measured and within its limits", x.get("ok"),
              f"{len(x.get('items', []))} items" + (f"; {bad[:3]}" if bad else ""))
    rw = extras["sweep:refusals-wide"]
    probed = [i for i in rw["items"] if i.get("expect")]
    check("refusals-wide: whole, Canada's allowed ones not probed, the rest refused with no carrier",
          rw["complete"] and len(probed) < len(rw["items"]) and all("refused" in i for i in probed),
          f"{len(probed)} probed of {len(rw['items'])}")
    res = json.load(open(os.path.join(sess, "bench", "results.json")))
    rules = res["rules"]
    want = {"TX-1.1": ("pass", "all"), "TX-35.1": ("pass", "all"),
            "TONE-1.1": ("pass", "part"), "TONE-20.1": ("pass", "part"), "TONE-20.2": ("pass", "part"),
            "TONE-19.4": ("pass", "part"), "TONE-10.4": ("pass", "part"), "TONE-16": ("pass", "part"),
            "TERM-33.6": ("pass", "part"), "TONE-15": ("pass", "part")}
    got = {k: (rules.get(k, {}).get("result"), rules.get(k, {}).get("scope")) for k in want}
    check("results.json: whole sweeps ✓ their rules, partial ones ◐", got == want,
          {k: v for k, v in got.items() if v != want[k]})
    du = json.load(open(os.path.join(sess, "session.json")))["duty"]
    ok, worst, rests, bursts = duty_ok(du["log"], c2["duty"])
    check("sweeps: every key-up logged, each within 5 s, rested 2x, the 60 s window's cap held", ok and du["log"]
          and all(e["key_up"] and e["key_down"] for e in du["log"]), f"fullest {worst:.1f} s; rests {rests}; bursts {bursts}")
    check("sweeps: the rolling cap paused them", any(w["why"] == "window" for w in du["waits"]),
          f"{sum(w['why'] == 'window' for w in du['waits'])} pauses, {du['waited_s']:.0f} s")
    check("sweeps: every KEY on has its KEY off; restored (slots, settings incl. 0 8 8 1, active slot)",
          r.calls.count("KEY on") == r.calls.count("KEY off") and r.key is None
          and (json.dumps(r.slots), dict(r.set), r.active) == before, dict(r.set))
    check("sweeps.json written as it went", os.path.exists(os.path.join(sess, "sweeps.json")))
    pg = open(page).read()
    check("summary page: the sweeps and the duty", "Sweep ctcss" in pg and "<h2>Duty</h2>" in pg)
    # --resume with --from and --count: ctcss left out (before --from's sweep), one more DCS item, the rest taken over
    c3 = json.loads(json.dumps(c2))
    c3["sweeps"]["dcs"]["only"] = ["D023N", "D023I", "D025N", "D754I", "D000N"]
    clk = FakeClock()
    r = FakeRadio(clk, version="v0.1.0 TX DRY")
    _, ex2, page2, _ = do_bench(c3, args(sweep="ctcss,dcs", sweep_from="D025N", count=1, resume=sess,
                                         build="selftest-resume"), radio=r, sdr=FakeSdr(r, clk), clock=clk)
    d2 = ex2.get("sweep:dcs") or {}
    labels = [i["label"] for i in d2.get("items", [])]
    check("--resume --from D025N --count 1: ctcss left out, D025N run, four taken over in order",
          "sweep:ctcss" not in ex2 and d2.get("run") == 1 and labels == ["D023N", "D023I", "D025N", "D754I", "D000N"]
          and sum(1 for i in d2["items"] if i.get("resumed")) == 4, f"{labels}; run {d2.get('run')}")
    # Faults: a radio that keys at 148.000 (unkeyed at once, the check fails) and one sending D023N inverted
    c4 = json.loads(json.dumps(c2))
    c4["sweeps"]["refusals-wide"]["freqs"] = [148.0, 160.0]
    c4["sweeps"]["dcs"]["only"] = ["D023N", "D023I"]
    clk = FakeClock()
    r = FakeRadio(clk, version="v0.1.0 TX DRY", leaky=[148.0], dcs_bug=True)
    _, ex3, page3, _ = do_bench(c4, args(sweep="refusals-wide,dcs", build="selftest-faults"), radio=r,
                                sdr=FakeSdr(r, clk), clock=clk)
    it = {i["label"]: i for i in ex3["sweep:refusals-wide"]["items"]}
    k = r.calls.index("KEY on")
    check("fault: a radio keying 148.000 fails the check, unkeyed at once", not it["148.000"]["ok"]
          and "KEYED" in it["148.000"]["why"] and r.calls[k + 1] == "KEY off" and it["160.000"]["ok"], it["148.000"]["why"])
    dd = {i["label"]: i for i in ex3["sweep:dcs"]["items"]}
    rr = json.load(open(os.path.join(os.path.dirname(page3), "bench", "results.json")))["rules"]
    check("fault: D023N sent inverted fails TONE-20.1, D023I still passes TONE-20.2", not dd["D023N"]["ok"]
          and dd["D023I"]["ok"] and rr["TONE-20.1"]["result"] == "fail" and rr["TONE-20.2"]["result"] == "pass",
          dd["D023N"].get("why"))
    # --dry-plan with the estimates
    with quiet(os.path.join(tmp, "plan.txt")):
        plan(cfg, args(sweep="all"), [])
    txt = open(os.path.join(tmp, "plan.txt")).read()
    check("--dry-plan: each sweep with its time and keyed time at the limits",
          all(f"sweep {n}:" in txt for n in SWEEP_NAMES) and "at the duty limits" in txt, os.path.join(tmp, "plan.txt"))


def selftest(a):
    tmp = a.out or tempfile.mkdtemp(prefix="bench-selftest-")
    ok = True
    lines = []

    def check(what, cond, got=""):
        nonlocal ok
        ok &= bool(cond)
        lines.append(f"  [{'pass' if cond else 'FAIL'}] {what}{': ' + str(got) if got != '' else ''}")
        say(lines[-1])

    def args(**kw):
        d = dict(runs=None, refusals=False, keepalive=False, timeout=False, i_know_its_tx_dry=False, dry_on=False, clear_offset=False,
                 flash=None, build=None, keep_captures=True, verbose=False, load=None, sweep=None, sweep_from=None,
                 sweep_items=None, count=None, resume=None, tests=None)
        d.update(kw)
        return argparse.Namespace(**d)

    cfg = load_cfg(a.config)
    cfg["results"] = os.path.join(tmp, "results")
    say(f"== selftest in {tmp}")

    def gated(what, radio_kw, a_kw, expect_in):
        clk = FakeClock()
        r = FakeRadio(clk, **radio_kw)
        try:
            do_bench(cfg, args(runs="vhf-wide", **a_kw), radio=r, sdr=FakeSdr(r, clk), clock=clk)
            check(what, False, "it keyed")
        except SafetyError as e:
            restored = r.slots[0]["freq"] == 146.52 and r.set == r.set0 and r.txtest is None
            unkeyed = r.key is None and r.calls.count("KEY on") == r.calls.count("KEY off")
            check(what, expect_in in str(e) and restored and r.key is None and unkeyed,
                  f"{str(e)[:90]}; keyed {'KEY on' in r.calls}; restored {restored}")
            return r

    # The gates.
    r = gated("gate: no tx-dry evidence, no flag -> refused before anything", {}, {}, "tx-dry build")
    check("  ... and nothing was changed or keyed", r.calls == [], r.calls)
    gated("gate: a receive-only build", {"version": "v0.1.0 tx-dry", "rx_only": True}, {}, "a transmit build")
    gated("gate: the radio says tx (LINK-35), even with --i-know-its-tx-dry", {"build": "tx"},
          {"i_know_its_tx_dry": True}, "says it runs a tx build")
    gated("gate: the radio says receive (LINK-35)", {"build": "receive"}, {"i_know_its_tx_dry": True},
          "says it runs a receive build")
    # TX-48: a tx build with Dry TX on (LINK-35's tx+dry) keys without the bench bit; the suite skips (no txtest);
    # one with Dry TX turned off on the way is refused before its next key-up.
    clk = FakeClock()
    r = FakeRadio(clk, build="tx+dry")
    try:
        caps, extras = Bench(cfg, args(runs="vhf-wide", keepalive=True), radio=r, sdr=FakeSdr(r, clk), clock=clk).on_air(
            tmp, pick_runs(cfg, "vhf-wide"), r.hello(), r.info())
        check("gate: a tx build reporting Dry TX (tx+dry) is accepted and keyed; the suite skips without txtest",
              "KEY on" in r.calls and not caps and "keepalive" in extras and r.key is None and "txtest suite" not in r.calls,
              f"caps {len(caps)}; keyed {'KEY on' in r.calls}")
    except Exception as e:  # noqa: BLE001 - the selftest reports it
        check("gate: a tx build reporting Dry TX (tx+dry) is accepted", False, str(e)[:120])
    gated("gate: a tx build not reporting Dry TX is refused (TX-48)", {"build": "tx"}, {}, "turn Dry TX on")
    # TX-48.2: --dry-on turns Dry TX on from the PC, then the tx+dry gate applies as usual.
    clk = FakeClock()
    r = FakeRadio(clk, build="tx")
    try:
        b = Bench(cfg, args(runs="vhf-wide", dry_on=True), radio=r, sdr=FakeSdr(r, clk), clock=clk)
        b.check_build(r.hello(), r.info())
        check("gate: --dry-on turns Dry TX on (TX-48.2) and the tx+dry gate passes",
              "set 0 8 8 5 On" in r.calls and r.build == "tx+dry" and b.live_dry, r.calls)
    except Exception as e:  # noqa: BLE001 - the selftest reports it
        check("gate: --dry-on turns Dry TX on (TX-48.2) and the tx+dry gate passes", False, str(e)[:120])
    r = gated("gate: Dry TX turned off on the radio before the key-up -> refused (TX-48)",
              {"build": "tx+dry", "dry_off_after": 0}, {"keepalive": True}, "Dry TX still on")
    check("  ... and never keyed", r is not None and "KEY on" not in r.calls, r and r.calls[-4:])
    gated("gate: frequency not approved (the radio ignores the tune)", {"version": "v0.1.0-txdry", "ignore_freq": True}, {},
          "frequency approved")
    gated("gate: power not low", {"stuck_power": "high"}, {"i_know_its_tx_dry": True}, "power low")
    gated("gate: an offset on the VFO", {"offset": -60000}, {"i_know_its_tx_dry": True}, "no offset")
    r = gated("gate: a tx build holds KEY's answer on the cable (TX-43) -> unkeyed at once", {"hold_answers": True},
              {"i_know_its_tx_dry": True}, "wasn't answered")
    check("  ... KEY off right after KEY on", r.calls[r.calls.index("KEY on") + 1] == "KEY off", r.calls[-6:])
    # A failure while keyed: KEY off and the restore still happen.
    clk = FakeClock()
    r = FakeRadio(clk, fail_alive_after=10)
    try:
        do_bench(cfg, args(runs="vhf-wide", i_know_its_tx_dry=True), radio=r, sdr=FakeSdr(r, clk), clock=clk)
        check("a failure while keyed", False, "no error")
    except BenchError as e:
        check("a failure while keyed: KEY off sent, everything restored", "KEY off" in r.calls and r.key is None
              and r.slots[0]["freq"] == 146.52 and r.set[TX_TIMEOUT] == "60 s" and r.set[END_TONE] == "Beep", str(e))
    # --clear-offset makes the offset gate pass, and says it wasn't put back.
    clk = FakeClock()
    r = FakeRadio(clk, offset=-60000, version="v0.1.0 TX DRY")
    b = Bench(cfg, args(clear_offset=True), radio=r, sdr=FakeSdr(r, clk), clock=clk)
    b.check_build(r.hello(), r.info())
    b.save_state()
    b.setup_slot(145.495, "fm")
    st = b.gate_key(cfg["test_freqs"], "clear-offset")
    b.restore()
    check("--clear-offset: the gate passes and the lost shift is noted", st["offset"] == 0 and any("shift" in n for n in b.restore_notes))
    # Images: tx-dry or not (flash refuses the latter before flash.py runs).
    import btf
    for name, body, want in (("dry", b"x" * 4000 + b"TX DRY: PA drive held at 0", True), ("tx", b"x" * 4100, False)):
        p = os.path.join(tmp, f"img-{name}.btf")
        open(p, "wb").write(btf.pack(body.ljust(16384, b"\xff")))
        check(f"image check: {name} image {'is' if want else 'is not'} tx-dry", image_is_tx_dry(p)[0] == want)
    try:
        do_flash(dict(cfg, port="-"), args(), os.path.join(tmp, "img-tx.btf"))
        check("--flash refuses a non-tx-dry image", False)
    except SafetyError as e:
        check("--flash refuses a non-tx-dry image", True, str(e)[:60])

    # The whole flow: one suite run and the three extra checks, on the fake radio and synthetic IQ.
    say("\n== the whole flow: vhf-wide + refusals + keepalive + timeout (fake radio, synthetic IQ)")
    clk = FakeClock()
    r = FakeRadio(clk, version="v0.1.0 TX DRY")
    before = json.dumps(r.slots), dict(r.set), r.active
    out, extras, page, bench = do_bench(cfg, args(runs="vhf-wide", refusals=True, keepalive=True, timeout=True, build="selftest"),
                                        radio=r, sdr=FakeSdr(r, clk), clock=clk)
    j = out[0]
    check("suite run: all seven markers measured", j["metrics"].get("markers_found") == 7, j["metrics"].get("markers_found"))
    check("suite run: DCS 023 N decoded", j["metrics"].get("dcs.code") == "023 N", j["metrics"].get("dcs.code"))
    check("suite run: 4FSK no errors", j["metrics"].get("fsk4.errors") == 0, j["metrics"].get("fsk4.errors"))
    check("refusals: both refused with their reasons, no carrier", all(x["ok"] for x in extras["refusals"]),
          [x.get("refused") for x in extras["refusals"]])
    check("keepalive: dropped within 2.5 s", extras["keepalive"]["ok"], extras["keepalive"]["text"])
    check("timeout: carrier ~30 s", extras["timeout"]["ok"], extras["timeout"]["text"])
    check("restored exactly (slots, settings, active slot)", (json.dumps(r.slots), dict(r.set), r.active) == before)
    check("every KEY on has its KEY off", r.calls.count("KEY on") == r.calls.count("KEY off") and r.key is None,
          f"{r.calls.count('KEY on')} on, {r.calls.count('KEY off')} off")
    check("the gates logged and all passed", bench.gatelog and all(g["ok"] for g in bench.gatelog), len(bench.gatelog))
    pg = open(page).read()
    check("summary page: self-contained, both themes, plain Hz ticks", "prefers-color-scheme:dark" in pg and "data-theme=dark" in pg
          and ">1000</text>" in pg and not any(x in pg for x in ("<script", "src=", "@import", "<link")), page)

    # The comparison: the same capture again (all within tolerance), then a perturbed copy (flags).
    say("\n== --load the saved capture: compared with the run above")
    cap = j["capture"]
    out2, page2 = do_load(cfg, args(load=[os.path.dirname(cap)], build="selftest-load"))
    rows = out2[0].get("comparison", [])
    check("--load: compared with the previous session", out2[0].get("previous", {}).get("session") == j["session"])
    check("--load: same capture, nothing beyond tolerance", rows and not any(x[5] in ("CHANGED", "missing") for x in rows),
          [x[0] for x in rows if x[5] != "ok"][:5])
    m2 = dict(j["metrics"])
    m2["sweep.data.high_hz"] += 100
    m2["dcs.code"] = "023 I"
    m2["fsk4.errors"] = 3
    m2["afsk.dac100.mark_dev"] *= 1.02
    del m2["tone1k.tau_ms"]
    flagged = {x[0]: x[5] for x in compare(cfg, m2, j["metrics"]) if x[5] != "ok"}
    check("compare: changes beyond tolerance flagged, small ones not", flagged == {"sweep.data.high_hz": "CHANGED", "dcs.code": "CHANGED",
                                                                                  "fsk4.errors": "CHANGED", "tone1k.tau_ms": "missing"}, flagged)
    say("\n== --auto: the selection rules on example diffs")
    check("--auto: each example diff picks the right side", selftest_auto())
    say("\n== the duty limit (virtual clock)")
    selftest_duty(check)
    say("\n== sweeps: every sweep on the fake radio and SDR (a 60 s duty window, so its cap pauses them)")
    selftest_sweeps(cfg, args, check, tmp)
    for mod in benchtests.all_modules():
        say(f"\n== suite {mod.SUITE} (bench.py --suite {mod.SUITE}): every test on a fake radio and SDR")
        mod.selftest(cfg, args, check, tmp)

    say("\n".join(["", "selftest:"] + lines))
    say("selftest: " + ("all pass" if ok else "FAILED") + f"\npage: {page}\npage: {page2}")
    return ok


def selftest_sim(a):
    """The bench against the simulator's link (real frames, the terminal's own commands), the SDR faked."""
    cfg = load_cfg(a.config)
    tmp = a.out or tempfile.mkdtemp(prefix="bench-sim-")
    cfg["results"] = os.path.join(tmp, "results")
    radio = Radio(a.sim)
    h = radio.hello_ans
    if h.get("build") not in ("tx-dry", "tx+dry") or not h.get("bench"):
        # Decision 151: the simulators run without the bench feature (no txtest), and say so in LINK-35's byte.
        say(f"selftest --sim: skipped: the simulator says it is a {h.get('build')} build"
            f"{' with' if h.get('bench') else ' without'} the bench feature; the bench needs a bench build (tx-bench) "
            "(txtest), with Dry TX on. The plain --selftest covers the flow and the gates.")
        radio.close()
        return True
    ok = True
    # The simulator's VFO A may carry a repeater shift; --clear-offset is what a radio with one would need.
    clk = Clock()
    ref = {}

    class Rec(Bench):
        def key_session(self, *k, **kw):
            ev = super().key_session(*k, **kw)
            ref["last_ev"] = ev
            return ev

    sdr = SimSdr(clk, ref)
    before = radio.term("slot A")
    timeout0 = None
    a2 = argparse.Namespace(runs="vhf-wide", refusals=True, keepalive=False, timeout=False, i_know_its_tx_dry=True,
                            clear_offset=True, flash=None, build="sim", keep_captures=False, verbose=False, load=None)
    cfg["suite_key_s"] = 63.5
    say(f"== the simulator at {a.sim}: vhf-wide (keyed 63.5 s in real time) + refusals; SDR faked")
    b = Rec(cfg, a2, radio=radio, sdr=sdr, clock=clk)
    hello, info = radio.hello(), radio.info()
    timeout0 = b.setting(TX_TIMEOUT)
    sess, sd = new_session(cfg, "sim")
    try:
        caps, extras = b.on_air(sd, pick_runs(cfg, "vhf-wide"), hello, info)
    finally:
        b.emergency_unkey()
    after = radio.term("slot A")
    j = analyse_run(cfg, a2, caps[0]["run"], caps[0]["capture"], sd, sess, dict(label="sim"))
    page = write_session(cfg, sd, sess, [j], extras, dict(build=dict(label="sim"), source="simulator", gates=b.gatelog,
                                                          warnings=b.warnings, restore_notes=b.restore_notes, tx_dry=b.tx_dry))
    for what, cond, got in (
            ("sim: the gates passed (the warning for --i-know-its-tx-dry printed)", all(g["ok"] for g in b.gatelog) and b.warnings, len(b.gatelog)),
            ("sim: the run keyed and was measured", j["metrics"].get("markers_found") == 7, j["metrics"].get("markers_found")),
            ("sim: the simulator refused each out-of-band key-up with its reason", all(x["ok"] for x in extras["refusals"]),
             [x.get("refused") for x in extras["refusals"]]),
            ("sim: slot A put back (the shift note aside)", before.strip().split(" offset")[0] == after.strip().split(" offset")[0], f"{before.strip()} / {after.strip()}"),
            ("sim: the TX timeout put back", b.setting(TX_TIMEOUT) == timeout0, timeout0)):
        ok &= bool(cond)
        say(f"  [{'pass' if cond else 'FAIL'}] {what}: {got}")
    radio.close()
    say(f"page: {page}\nselftest --sim: {'all pass' if ok else 'FAILED'}")
    return ok


if __name__ == "__main__":
    # The suites (tools/benchtests/) `import bench`: this same module, not a second copy (one SafetyError, one Radio).
    sys.modules.setdefault("bench", sys.modules[__name__])


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--config", help="JSON overriding any of DEFAULTS")
    ap.add_argument("--port", help="the cable or a radio's name (default: the config, FERRITE_PORT, radios.toml's default, the only cable), tcp:HOST:PORT for the simulator, or ble:NAME_OR_ADDRESS for Bluetooth")
    ap.add_argument("--slot", help="the slot to use (A)")
    ap.add_argument("--runs", help="comma list of run names (default: all in the config)")
    ap.add_argument("--results", help="results root")
    ap.add_argument("--refusals", action="store_true", help="remote key refused at 160, 243 MHz; no RF")
    ap.add_argument("--keepalive", action="store_true", help="keepalives stopped while keyed: carrier drop <= 2.5 s")
    ap.add_argument("--timeout", action="store_true", help="TX timeout 30 s, keyed up to 36 s: carrier ~30 s")
    ap.add_argument("--exhaustive", action="store_true", help="with --sweep: every item of the ctcss and dcs sweeps "
                    "(a level beyond the full test; without it they spot-check SPOT random items)")
    ap.add_argument("--sweep", metavar="A,B|all", help="sweeps, in this order: " + ", ".join(SWEEP_NAMES)
                    + " (alone: no suite runs; add --runs for them)")
    ap.add_argument("--from", dest="sweep_from", metavar="ITEM", help="with --sweep: start at this item (e.g. 151.4, "
                    "D365N, 243.000, quindar-nfm); sweeps listed before its own are left out")
    ap.add_argument("--items", dest="sweep_items", metavar="A,B", help="with --sweep: only these items")
    ap.add_argument("--count", type=int, help="with --sweep: at most N items of each sweep now")
    ap.add_argument("--resume", metavar="SESSION", help="with --sweep: take over the items an earlier session "
                    "passed (its sweeps.json) and run the rest")
    ap.add_argument("--sweep-mhz", type=float, help="with --sweep: the test frequency the sweeps key on (an approved "
                    "one; default 145.495)")
    ap.add_argument("--keep-sweep-captures", action="store_true", help="keep every sweep item's .u8 (default: the "
                    "failures only)")
    ap.add_argument("--suite", metavar="NAME", help="a suite of on-air checks by name (tools/benchtests/: "
                    + ", ".join(benchtests.names()) + "); with --dry-plan its plan; alone, no suite runs or sweeps")
    ap.add_argument("--tests", metavar="A,B", help="with --suite: only these tests (names as --dry-plan lists them)")
    ap.add_argument("--flash", metavar="IMAGE", help="flash this tx-dry image first (flash.py --backup)")
    ap.add_argument("--dry-on", action="store_true",
                    help="a tx build with Dry TX off: turn it on from the PC first (TX-48.2; only the radio turns it off)")
    ap.add_argument("--i-know-its-tx-dry", action="store_true", help="the radio runs a tx-dry build (can't be read over the link)")
    ap.add_argument("--clear-offset", action="store_true", help="clear the VFO's repeater shift by a tune to 160 MHz and back")
    ap.add_argument("--build", help="the build's label for the results (default: version + image or host git hash)")
    ap.add_argument("--no-keep-captures", dest="keep_captures", action="store_false", help="delete the .u8 after analysis")
    ap.add_argument("--verbose", action="store_true", help="everything on the terminal: per-step lines (duty, headers, calibration, restore), sdrsuite's "
                    "full report (default: one line per test and the summary; safety messages always print)")
    ap.add_argument("--dry-plan", action="store_true", help="print the plan; touch nothing")
    ap.add_argument("--load", nargs="+", metavar="DIR|RUN=FILE", help="re-analyse saved captures")
    ap.add_argument("--selftest", action="store_true", help="fake radio + synthetic IQ: the whole flow and the gates")
    ap.add_argument("--sim", metavar="tcp:HOST:PORT", help="with --selftest: against the simulator's link")
    ap.add_argument("--out", help="selftest: where its results go")
    ap.add_argument("--auto", action="store_true", help="pick the UI tests and/or the SDR suite from what changed since "
                    "the last on-radio runs (git diff); prints the decision and why")
    ap.add_argument("--all", action="store_true", help="with --auto: everything (UI tests, SDR suite, reboot check)")
    ap.add_argument("--ui", action="store_true", help="with --auto: the UI tests (full set)")
    ap.add_argument("--rf", action="store_true", help="with --auto: the SDR suite on tx-dry (+ the UI smoke set)")
    ap.add_argument("--since", metavar="COMMIT", help="with --auto: diff from this commit, not the last runs'")
    ap.add_argument("--diff-files", metavar="A,B,...", help="with --auto: decide as if these files had changed")
    ap.add_argument("--record", action="store_true", help="after the run: each rule its checks name gets a result "
                    "row in docs/test-plan/*.md (tools/record_results.py; on-air runs only)")
    ap.add_argument("--docs", default=os.path.join(ROOT, "docs"), help="the docs checkout for --record")
    ap.add_argument("--kiss-ble", metavar="ble:NAME", help="with --suite modes-on-air: KISS frames over Bluetooth "
                    "(0 5 8 Bluetooth while the test runs; the cable stays the link)")
    ap.add_argument("--slow", action="store_true", help="with --suite modes-on-air: also GPS-33's 4-minute test")
    ap.add_argument("--no-audio", action="store_true", help="with --suite modes-on-air: no tone from the Mac's speaker")
    ap.add_argument("--rule-results", nargs="+", metavar="SESSION", help="(re)write bench/results.json for saved "
                    "sessions from their session.json and <run>.json (nothing keyed)")
    a = ap.parse_args()
    global VERBOSE
    VERBOSE = a.verbose
    cfg = load_cfg(a.config)
    for k in ("port", "slot", "results"):
        if getattr(a, k):
            cfg[k] = getattr(a, k)
    try:
        cfg["port"] = fl.resolve_port(cfg["port"])  # a radio's name, or nothing: FERRITE_PORT, the default, the cable
    except fl.PortError:
        cfg["port"] = ""  # only what opens the radio needs one: Port("") says why there
    if a.sweep_mhz:
        if a.sweep_mhz not in cfg["test_freqs"]:
            sys.exit(f"--sweep-mhz {a.sweep_mhz}: not an approved test frequency ({', '.join(map(str, cfg['test_freqs']))})")
        cfg["sweeps"]["mhz"] = a.sweep_mhz
    if a.keep_sweep_captures:
        cfg["sweeps"]["keep_captures"] = True
    if (a.sweep_from or a.sweep_items or a.count is not None or a.resume) and not a.sweep:
        sys.exit("--from, --items, --count and --resume go with --sweep")
    if a.selftest:
        sys.exit(0 if (selftest_sim(a) if a.sim else selftest(a)) else 1)
    if a.rule_results:
        for d in a.rule_results:
            p, s = write_rule_results(d)
            say(f"{p}: " + ", ".join(f"{k} {'pass' if v['result'] == 'pass' else 'FAIL'}"
                                     + (" (part)" if v["scope"] == "part" else "") for k, v in sorted(s["rules"].items())))
        return
    if a.tests and not a.suite:
        sys.exit("--tests goes with --suite")
    if a.suite:
        mod = benchtests.load(a.suite)
        if a.dry_plan:
            mod.plan(cfg, a)
            return
        def on_term_suite(*_):
            raise KeyboardInterrupt

        signal.signal(signal.SIGTERM, on_term_suite)
        try:
            extras, page, _ = mod.run(cfg, a)
        except SafetyError:
            sys.exit(2)
        except KeyboardInterrupt:
            sys.exit("stopped (unkeyed and restored)")
        if a.record:
            record_session(os.path.dirname(page), a.docs)
        sys.exit(0 if all(all_ok(v) for v in extras.values()) else 1)
    if a.auto or a.all or a.ui or a.rf:
        sys.exit(do_auto(cfg, a))
    if a.dry_plan:
        plan(cfg, a, pick_runs(cfg, "none" if a.sweep and not a.runs else a.runs))
        return
    if a.load:
        do_load(cfg, a)
        return

    def on_term(*_):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, on_term)
    try:
        out, extras, page, _ = do_bench(cfg, a)
    except SafetyError:
        sys.exit(2)
    except KeyboardInterrupt:
        sys.exit("stopped (unkeyed and restored)")
    if a.record:
        record_session(os.path.dirname(page), a.docs)
    bad = any(not j["ok"] for j in out) or any(not all_ok(v) for v in extras.values())
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
