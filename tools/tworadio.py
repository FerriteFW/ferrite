#!/usr/bin/env python3
"""Two Ferrite radios, both on the tx-dry build: one keys (LINK-20 remote PTT, PA drive held at 0, a weak carrier the
other hears a metre away), the other receives and is read back (LINK-19's UI readback, `slot`, terminal lines over
the link). The checks that needed the clear radio keying into Ferrite, run by the PC. Tests: tools/tworadio_tests/.

    tools/tworadio.py --dry-plan                               # the plan, key-ups and keyed time; touches nothing
    tools/tworadio.py --tx /dev/cu.usbserial-XXXX --rx ble:UUID        # run: TX keys, RX is read back
    tools/tworadio.py --tx ble:UUID --rx /dev/cu.usbserial-XXXX        # the same radios the other way round, or:
    tools/tworadio.py --tx PORT1 --rx PORT2 --swap                     # roles reversed (the other receive path)
    tools/tworadio.py ... -k scan --rules 'SCAN-*' --module tones      # a selection
    tools/tworadio.py --list                                   # the tests, their rules and key-ups
    tools/tworadio.py ... --record                             # a result row per rule into docs/test-plan/*.md
    tools/tworadio.py --selftest                               # two fake radios on a fake air, a virtual clock
    tools/tworadio.py --selftest --sim                         # two simulators (tcp): connect, the gates, restore
    tools/tworadio.py --tx PORT --rx ble:UUID --module crossband --sdr [--long]
                                                               # the crossband repeater (chapter 19): RX repeats,
                                                               # TX keys its input, the RTL-SDR watches its output

Safety, as tools/bench.py's (nothing keys unless all hold, checked right before every KEY on):
  - both radios say they run tx-dry with the bench bit in HELLO's LINK-35 build byte (0x82; decision 151), or a tx
    build with Dry TX on (tx+dry, 0x03; TX-48: its bar must still say dry=1 before every KEY on), or the run stops
    before anything is changed (no --i-know flag: a tx build without Dry TX would transmit at full drive);
  - the keyed slot (the active one, read with SLOT, LINK-33) transmits on 145.495 or 446.475 MHz exactly (its
    frequency plus its shift, or its split), at power low, not the APRS slot, not scanning (SCAN-19's test alone
    keys a scanning slot, and only when every channel of its test zone transmits on an approved frequency or not at
    all);
  - every key-up passes bench.py's DutyLimiter (one per radio, bench.py's limits: at most 5 s with its 0.5 s unkey
    tail, a rest of 2x after it, at most 25 % of any 5 min keyed; a key-up the radio must refuse is a probe);
  - keepalives every 0.5 s on their own thread (the readback of the other radio can't hold them up), and the
    key-up's own end (KEY off) sent by that thread at its deadline whatever the test is doing;
  - on the cable KEY on must be answered within 0.6 s (a tx build holds its answers while keyed, TX-43), over
    Bluetooth within 2 s, else KEY off and the run stops;
  - KEY off in `finally` for every key-up, and to both radios at the end (and on Ctrl-C, SIGTERM, exit).
The receiving radio keys only in TX-20.2/TX-21's refusal probe (busy lockout: must be refused; KEY off at once if
not) and SCAN-19's PTT on a scan hit.

The crossband tests (tools/tworadio_tests/crossband.py) make the receiving radio a crossband repeater (XB-*,
started with `xb start` or remote keys, XB-20): its slot A on 145.495, B on 446.475, power Low (read back before it
starts), so it transmits only on an approved frequency, on the tx-dry build, and only while the transmitting radio
keys its input. Each of those key-ups is admitted on the receiving radio's DutyLimiter too (its relay: the key-up
plus the hang and the carrier hold, kind "relay"), and recorded there; the repeater is stopped (`xb stop`, hold
BACK) after every test and at the end. --sdr: an RTL-SDR (bench.py's Sdr, fixed gain 0.9 dB) on the repeater's
output frequency for the tests that measure the air (carrier, sub-tone, hang, the cut, the CW ID); without it they
are skipped. Times on the air are compared inside one capture (a carrier's length), never against the PC's clock
(rtl_sdr starts up to a second late). --long: the tests that need a key-up over the 5 s burst limit (XB-9's 30 s
pass timeout, XB-12's ID with its 10 minutes): one continuous key-up each, bench.py's "timeout" exception (rested 2x
after, counted in the 5 min window); without --long they are skipped.

Each test starts from home on both radios; the settings it changes (`side.set`) and the slots (`slot` lines, the
scans that ran) are put back after it, channels and a zone it made are deleted; at the end both radios are compared
with their snapshots and anything left changed is reported (and fails the run). The receiving radio's quick mute and
the transmitting radio's end tone (0 8 4 Off for the run) are put back too.

A link that times out (or a port that fails) on either radio stops the whole run in that test: KEY off to both,
what can be put back is (a radio whose link is gone is reported, not retried), and the run's results are kept but
never recorded (a stopped run). Bluetooth requests take ferrite_link's slower time-out (6 s); polls over Bluetooth
leave 0.25 s between reads, and the crossband polls go through Side.state (the compact readback when the firmware
has it). Set-ups read back what they set (each `slot` line, the repeater's `xb` sides) and fail the test before
anything keys when it didn't take.

Results: <results>/<date-time>_tworadio-<label>/tworadio/results.json in tools/uitest.py's shape (each test: rules,
pass/fail/skip, checks, readback lines; each rule's result, "part" for a test that checks part of it) and
summary.html; `--record` (or tools/record_results.py) writes a row per rule into its test-plan entry. Simulator and
selftest runs are never recorded. A run takes the receiving radio's readback: on the cable about 0.05 s, over
Bluetooth about 1 s, so the tests that time things to tenths (fast) are skipped when the receiving radio is on
Bluetooth: `--swap` runs them with the cable radio receiving.

Needs the project's .venv (numpy, for bench.py's DutyLimiter; pyserial; bleak for Bluetooth): run with python3 and it
re-runs itself under the repo's .venv (just setup), or the older ../.venv, when numpy is missing.
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
FERRITE = os.path.dirname(HERE)
ROOT = os.path.dirname(FERRITE)
VENV_PY = os.path.join(ROOT, ".venv", "bin", "python")

def _venv():
    """The project's .venv python: in the checkout (just setup), beside it, or beside the main checkout of a worktree."""
    yield os.path.join(FERRITE, ".venv", "bin", "python")
    yield VENV_PY
    import subprocess
    r = subprocess.run(["git", "-C", FERRITE, "rev-parse", "--path-format=absolute", "--git-common-dir"],
                       capture_output=True, text=True)
    if r.returncode == 0 and r.stdout.strip():
        yield os.path.join(os.path.dirname(os.path.dirname(r.stdout.strip())), ".venv", "bin", "python")


try:
    import numpy  # noqa: F401  (bench.py needs it: the DutyLimiter comes from there)
except ImportError:
    for _py in _venv():
        if os.path.exists(_py) and os.path.realpath(sys.executable) != os.path.realpath(_py):
            os.execv(_py, [_py, os.path.abspath(__file__)] + sys.argv[1:])
    sys.exit("tworadio.py needs numpy (bench.py's DutyLimiter): run it with the project's .venv")

import argparse  # noqa: E402
import atexit  # noqa: E402
import contextlib  # noqa: E402
import datetime  # noqa: E402
import fnmatch  # noqa: E402
import html  # noqa: E402
import importlib  # noqa: E402
import json  # noqa: E402
import pkgutil  # noqa: E402
import re  # noqa: E402
import signal  # noqa: E402
import struct  # noqa: E402
import threading  # noqa: E402
import time  # noqa: E402
import shutil  # noqa: E402
import traceback  # noqa: E402

sys.path.insert(0, HERE)
import bench  # noqa: E402
import ferrite_link as fl  # noqa: E402
import uitest  # noqa: E402
from bench import BenchError, DutyLimiter, SafetyError  # noqa: E402
from uitests.core import U, UNSAFE  # noqa: E402
from tworadio_tests import core as trc  # noqa: E402

APPROVED = {bench.mhz10(f) for f in bench.DEFAULTS["test_freqs"]}  # 145.495, 446.475 MHz in 10 Hz
RESULTS = os.path.join(ROOT, "bench-results")
KEY_ANSWER_CABLE_S = bench.KEY_ANSWER_S  # 0.6 s: TX-43
KEY_ANSWER_BLE_S = 2.0
END_TONE = bench.END_TONE  # 0 8 4


def say(*a):
    print(*a, flush=True)


class Fail(Exception):
    pass


class Skip(Exception):
    pass


class LinkLost(Exception):
    """A radio's link timed out (or its port failed): every later test would meet the same, so the run stops at once
    (KEY off to both, what can be put back is, the results kept but never recorded). `res`: the test's result."""

    def __init__(self, role, err, res=None):
        self.role, self.err, self.res = role, err, res
        super().__init__(f"the {role} radio's link: {type(err).__name__}: {err}")


LINK_ERRORS = (TimeoutError, OSError)


# ---- A radio over the link ----


class LinkRadio(fl.Radio):
    """A radio over the link (the cable, ble:ID, or tcp: for the simulator), as the harness drives it: terminal
    lines, remote keys, the readback, SLOT and KEY's remote PTT, each request under one lock (the keepalive thread
    shares the link)."""

    def __init__(self, spec, pin=None):
        self.spec, self.pin = spec, pin
        self.ble, self.sim = spec.startswith("ble:"), spec.startswith("tcp:")
        self.cable = not (self.ble or self.sim)
        self.lock = threading.RLock()
        self.last = time.time()
        super().__init__(spec, pin=pin, name=b"tworadio.py")
        self.version = fl.text(self.link.ok(fl.INFO))[0]
        self.answer_s = KEY_ANSWER_CABLE_S if not self.ble else KEY_ANSWER_BLE_S

    def _nudge(self):
        if time.time() - self.last > 8:  # the link lets the port go 10 s after the last frame (TERM-2): a 0x00 first
            self.port.write(b"\x00")
            time.sleep(0.05)

    def _ok(self, ty, payload=b"", busy_s=3.0):
        with self.lock:
            self._nudge()
            try:
                return super()._ok(ty, payload, busy_s)
            finally:
                self.last = time.time()

    def slot_bin(self, n):
        """LINK-33: slot n's working fields (bench.Radio.slot's layout)."""
        with self.lock:
            self._nudge()
            s, r = self.link.ask(fl.SLOT, bytes([n]))
            self.last = time.time()
        if s != 0:
            raise BenchError(f"SLOT: {fl.why(s, r)}")
        sl, act, ch, f, mode, pw, rxt, txt, off, flags = struct.unpack("<BBHIBBHHiB", r[:19])
        return dict(active=bool(act), channel=None if ch == 0xFFFF else ch, freq10=f, mode=mode, power=pw,
                    rx_tone=rxt, tx_tone=txt, offset=off, split=bool(flags & 1), aprs=bool(flags & 2),
                    scanning=bool(flags & 4))

    # KEY (LINK-20), a step at a time: the Keyer owns the timing.
    def key_on(self, slot_code):
        with self.lock:
            self._nudge()
            if time.time() - self.last > 10:
                with contextlib.suppress(Exception):  # wake the link first (bench.py, Oct 14, 2026)
                    self.link.ask(fl.INFO)  # the port's own time-out (6 s on Bluetooth, 2 s on the cable)
            seq = self.link.send(fl.KEY, bytes([fl.PTT, fl.PTT_ON, slot_code]))
            self.last = time.time()
            end = time.time() + self.answer_s
            while time.time() < end:
                for t, s, status, rest in self.link.answers():
                    if t == fl.KEY | 0x80 and s == seq:
                        return status, ("" if status == 0 else fl.why(status, rest))
        return None

    def key_alive(self):
        with self.lock:
            self.link.send(fl.KEY, bytes([fl.PTT, fl.PTT_ALIVE]))
            self.last = time.time()

    def poll_unkey(self):
        with self.lock:
            for t, s, status, rest in self.link.answers():
                if t == fl.KEY | 0x80 and status != 0:
                    return fl.why(status, rest)
        return None

    def restart(self, settle=4.0, timeout=30.0):
        """REBOOT (LINK-24: the radio saves, resets into the stock bootloader, which starts Ferrite after its 200 ms
        listen), then HELLO again until it answers (as uitest.py's Radio.reopen, without the boot capture)."""
        with self.lock:
            self.link.ok(fl.REBOOT)
            with contextlib.suppress(OSError):
                self.port.close()
            time.sleep(settle)
            end = time.time() + timeout
            while True:
                try:
                    fresh = fl.Radio(self.spec, pin=self.pin, name=b"tworadio.py")
                    break
                except (fl.RadioError, TimeoutError, OSError, SystemExit):
                    if time.time() > end:
                        raise TimeoutError("no HELLO from the radio after the restart")
                    time.sleep(1.0)
            self.port, self.link, self.build, self.bench = fresh.port, fresh.link, fresh.build, fresh.bench
            self.last = time.time()

    def key_off(self):
        with self.lock:
            seq = self.link.send(fl.KEY, bytes([fl.PTT, fl.PTT_OFF]))
            end = time.time() + 3
            while time.time() < end:
                for t, s, status, rest in self.link.answers():
                    if t == fl.KEY | 0x80 and s == seq:
                        self.last = time.time()
                        return True
        return False


# ---- Keying: the gates, the duty limit, keepalives, KEY off ----


def dry_ok(r):
    """LINK-35, TX-48: the PA drive is held at 0: a tx-dry bench build (0x82), or a full-power tx build with Dry TX
    on (tx+dry, 0x03: no bench bit, and nothing here needs a bench command). A plain tx build is never keyed."""
    return r.build == "tx+dry" or r.build == "tx-dry" and r.bench


class KeyUp:
    def __init__(self, purpose, kind):
        self.purpose, self.kind = purpose, kind
        self.accepted = self.refused = self.unkeyed = self.error = None
        self.t_sent = self.t_on = self.t_off = self.t_unkeyed = self.deadline = None
        self.off_answered = None
        self.duty = None

    def ev(self):
        return dict(accepted=self.accepted, refused=self.refused, unkeyed=self.unkeyed, t_on=self.t_on,
                    t_off=self.t_off, t_unkeyed=self.t_unkeyed)


class Keyer:
    """One radio's key-ups. `keyup` is the one way to key: the duty limit admits it, the gates pass right before
    KEY on, keepalives run until its deadline (a thread on a real clock; `pump` on the selftest's virtual one), KEY
    off at the deadline and always in `finally`."""

    def __init__(self, radio, role, duty, clock, log=say, threaded=True):
        self.radio, self.role, self.duty, self.clock, self.log, self.threaded = radio, role, duty, clock, log, threaded
        self.gatelog = []
        self.active = None
        self._off_lock = threading.Lock()
        self.keyups = []

    def need(self, what, ok, detail=""):
        self.gatelog.append(dict(radio=self.role, gate=what, ok=bool(ok), detail=detail))
        if not ok:
            raise SafetyError(f"refused ({self.role} radio): {what}{': ' + detail if detail else ''}")

    def gate(self, purpose, scan_freqs=None):
        r = self.radio
        self.need(f"{purpose}: a tx-dry bench build or Dry TX on (LINK-35, TX-48)", dry_ok(r),
                  f"the radio says {r.build}{' (bench)' if r.bench else ''}")
        if r.build == "tx+dry":
            # TX-48: Dry TX can be turned off on the radio at any time: its bar must still say dry=1.
            bar = fl.parse_ui(r.ui_text()).get("bar") or {}
            self.need(f"{purpose}: Dry TX still on (TX-48)", bar.get("dry") == 1,
                      f"the radio's bar says dry={bar.get('dry')}: Dry TX was turned off on the radio")
        bins = [r.slot_bin(n) for n in range(3)]  # three SLOT reads, the active one among them (Bluetooth: fewer)
        act = [n for n in range(3) if bins[n]["active"]]
        self.need(f"{purpose}: one active slot", len(act) == 1, str(act))
        s = bins[act[0]]
        tx = s["offset"] if s["split"] else s["freq10"] + s["offset"]
        self.need(f"{purpose}: power low", s["power"] == 0, ["low", "mid", "high"][s["power"] % 3])
        self.need(f"{purpose}: not the APRS slot", not s["aprs"])
        if s["scanning"]:
            ok = bool(scan_freqs) and all(f in APPROVED for f in scan_freqs)
            self.need(f"{purpose}: a scanning slot whose every transmit frequency is approved", ok,
                      ", ".join("?" if f is None else f"{f / 1e5:.5f}" for f in sorted(scan_freqs or [], key=str)))
        else:
            self.need(f"{purpose}: transmit frequency approved", tx in APPROVED,
                      f"{tx / 1e5:.5f} MHz; allowed {', '.join(f'{f / 1e5:.3f}' for f in sorted(APPROVED))}")
        return s

    def _off(self, k):
        with self._off_lock:
            try:
                ans = self.radio.key_off()
            except Exception as e:  # keep going: the radio unkeys 2 s after the last keepalive anyway
                ans = False
                k.error = k.error or f"KEY off: {e}"
            if k.t_off is None:
                k.t_off, k.off_answered = self.clock.time(), ans

    def _alive(self, k, stop):
        while not stop.is_set():
            now = self.clock.time()
            if now >= k.deadline:
                self._off(k)  # the key-up's end, whatever the test is doing
                return
            try:
                self.radio.key_alive()
                u = self.radio.poll_unkey()
            except Exception as e:
                k.error = f"keepalive: {e}"
                self._off(k)
                return
            if u and k.unkeyed is None:
                k.unkeyed, k.t_unkeyed = u, now
                self.log(f"  {self.role}: the radio unkeyed: {u}")
                return
            stop.wait(min(0.5, max(0.0, k.deadline - now)))

    def pump(self):
        """The virtual clock's stand-in for the thread: keepalives, the deadline's KEY off."""
        k = self.active
        if self.threaded or k is None or not k.accepted or k.t_off is not None or k.deadline is None:
            return
        now = self.clock.time()
        if now >= k.deadline:
            self._off(k)
            return
        if now - getattr(k, "last_alive", 0) >= 0.5:
            k.last_alive = now
            self.radio.key_alive()
            u = self.radio.poll_unkey()
            if u and k.unkeyed is None:
                k.unkeyed, k.t_unkeyed = u, now

    @contextlib.contextmanager
    def keyup(self, seconds, purpose, kind="burst", expect_refusal=False, scan_freqs=None, relay=None):
        """`relay` (keyer, extra s, kind): the other radio repeats this key-up (the crossband repeater, XB-*): its
        transmission, this key-up's length plus `extra` (the hang and the pass's end) and that radio's unkey tail,
        is admitted on its own DutyLimiter first and recorded there after (kind "relay", or the kind given)."""
        k = KeyUp(purpose, "probe" if expect_refusal else kind)
        planned = self.duty.planned(KEY_ANSWER_CABLE_S if expect_refusal else seconds)
        rk = None
        if relay is not None:
            rkeyer, extra, rkind = relay
            rk = KeyUp(f"relays: {purpose}", rkind)
            rkeyer.duty.admit(rkeyer.duty.planned(seconds + extra), rkind, f"{rkeyer.role}: relays {purpose}")
        self.duty.admit(planned, k.kind, f"{self.role}: {purpose}")
        self.gate(purpose, scan_freqs)  # right before KEY on, after any duty wait
        k.t_sent = self.clock.time()
        stop, th = threading.Event(), None
        self.active = k
        self.keyups.append(k)
        try:
            r = self.radio.key_on(fl.ACTIVE)
            k.t_on = self.clock.time()
            if r is None:
                k.accepted = True  # unknown: counted as keyed
                raise SafetyError(f"{self.role} radio: KEY on wasn't answered within {self.radio.answer_s:g} s (a tx "
                                  "build holds its answers while keyed, TX-43): KEY off, stopping")
            if r[0] != 0:
                k.accepted, k.refused = False, r[1]
            else:
                k.accepted = True
                if expect_refusal:
                    self.log(f"  {self.role}: the radio KEYED a key-up it must refuse: KEY off at once")
                    self._off(k)
                else:
                    # From KEY on sent, less the time KEY off takes to arrive: the duty limit counts from the send,
                    # so a key-up never runs past what was admitted.
                    k.deadline = k.t_sent + seconds - (0.25 if getattr(self.radio, "ble", False) else 0.05)
                    if self.threaded:
                        th = threading.Thread(target=self._alive, args=(k, stop), daemon=True)
                        th.start()
            yield k
        finally:
            stop.set()
            if th is not None:
                th.join(timeout=4)
            self._off(k)
            self.active = None
            k.duty = self.duty.record(f"{self.role}: {purpose}", k.kind, k.ev(), k.t_sent)
            if rk is not None:
                # The repeater's pass: from this KEY on to this key-up's end and the extra (hang, the pass's end).
                rkeyer, extra, _ = relay
                rk.accepted = bool(k.accepted) and not expect_refusal
                rk.t_sent, rk.t_on = k.t_sent, k.t_on
                rk.t_off = (k.t_unkeyed or k.t_off or self.clock.time()) + extra
                rk.duty = rkeyer.duty.record(f"{rkeyer.role}: relays {purpose}", rk.kind, rk.ev(), k.t_sent)
                rkeyer.keyups.append(rk)

    def emergency_off(self):
        with contextlib.suppress(Exception):
            self.radio.key_off()


# ---- The test's view of each radio, and the context ----


class Light:
    """ferrite_link's status() dict seen as the full readback's U for the fields both have: `.id`, `.state` (keyed
    "-" or a letter, remote 0/1, mode), `.cards[L]` (state, rx 1 while its squelch is open, meter, active), `["xb"]`
    (running 0/1, state, passes), `.whole` (always: one line)."""

    whole = True

    def __init__(self, d):
        self.d = d
        self.id = d.get("screen") or ""
        self.state = dict(keyed=d.get("keyed") or "-", remote=int(bool(d.get("remote"))), mode=d.get("mode"),
                          active=d.get("active"))
        self.cards = {L: dict(state=c.get("state", ""), meter=c.get("meter", 0),
                              rx=int(str(c.get("state", "")).startswith("RX")), active=int(L == d.get("active")))
                      for L, c in (d.get("slots") or {}).items()}
        x = d.get("xb")
        self.xb = dict(running=1, **x) if x else dict(running=0)
        self.lines = [repr(d)]

    def __getitem__(self, k):
        return self.xb if k == "xb" else self.d[k]


class Side:
    """One radio as a test drives it: readback, keys, terminal lines, settings (put back by the runner)."""

    def __init__(self, ctx, role, radio, keyer):
        self.ctx, self.role, self.radio, self.keyer = ctx, role, radio, keyer
        self.rb = 0.05 if not getattr(radio, "ble", False) else 1.0  # a readback's time, a running average

    @property
    def fast(self):
        return self.rb < 0.35

    def raw(self):
        """One readback as it comes (for timing)."""
        self.ctx.pump()
        c = self.ctx.clock
        t0 = c.time()
        u = U(fl.parse_ui(self.radio.ui_text()))
        self.rb = 0.8 * self.rb + 0.2 * (c.time() - t0)
        self.ctx.last = u
        self.ctx.pump()
        return u

    def state(self):
        """The radio's state, light: ferrite_link's `Radio.status()` (LINK-19's compact readback, UI what=2: one
        ~80-byte line, no MORE) when the firmware has it, else the full readback (~730 bytes in 4 chunks, ~2.9 s over
        Bluetooth). Either answers `.id`, `.state` (keyed, remote, mode), `.cards[L]` (state, rx, meter) and
        `["xb"]` (running, state, passes): the polls on state go through here; a test that needs frequencies, rows
        or toasts reads `raw()`/`ui()`."""
        st = getattr(self.radio, "status", None)
        if st is None or not getattr(self.radio, "has_status", True):
            return self.raw()
        self.ctx.pump()
        c = self.ctx.clock
        t0 = c.time()
        try:
            d = st()
        except fl.RadioError as e:
            if getattr(e, "status", None) == 3:  # bad argument: firmware before the compact readback
                self.radio.has_status = False
                return self.raw()
            raise
        self.rb = 0.8 * self.rb + 0.2 * (c.time() - t0)
        self.ctx.pump()
        return Light(d)

    def ui(self, timeout=1.0):
        """A settled readback: two whole ones in a row agreeing on the screen, toast, quick menu and hints."""
        end, prev = self.ctx.now() + timeout, None
        while True:
            u = self.raw()
            if u.whole and prev is not None and u.settled == prev.settled:
                return u
            if self.ctx.now() > end:
                return u
            prev = u if u.whole else None
            self.ctx.sleep(0.02)

    def keys(self, seq, settle=0.12):
        self.radio.keys(seq)
        self.ctx.sleep(settle)
        return self.ui()

    def press(self, name, long=False, settle=0.12):
        self.radio.press(name, long=long)
        self.ctx.sleep(settle)
        return self.ui()

    def cmd(self, line):
        self.ctx.pump()
        return self.radio.cmd(line)

    def cmd_err(self, line):
        try:
            return True, self.cmd(line)
        except fl.RadioError as e:
            return False, str(e)

    def get(self, path):
        out = self.cmd(f"get {path}").strip()
        m = re.match(rf"^{re.escape(path)} ([^:]+?): (.*?)( \(not the default\))?\s*$", out)
        if not m:
            raise Fail(f"{self.role}: get {path}: can't read {out!r}")
        return m.group(2)

    def set(self, path, value):
        """A setting for the test; the runner puts it back after."""
        if path in UNSAFE:
            raise Fail(f"{path} is never changed by a test")
        changed = self.ctx.run.changed[self.role]
        was = changed.get(path) if path in changed else self.get(path)
        ok, out = self.cmd_err(f"set {path} {value}")
        if not ok:
            raise Fail(f"{self.role}: set {path} {value}: {out}")
        if path not in changed:
            changed[path] = was
            self.ctx.run.journal_save()  # on disk before the test goes on: a killed run is put back by the next one

    def slot(self, L):
        return self.cmd(f"slot {L}").strip()

    def slot_bin(self, n):
        return self.radio.slot_bin(n)

    def home(self):
        """Home, unlocked, no quick menu: hold BACK (HOME-2), then BACK x4 if that wasn't enough."""
        u = self.raw()
        if u.bar.get("lock"):
            self.radio.press("HASH", long=True)
        for _ in range(2):
            u = self.press("BACK", long=True)
            if u.id == "Home" and not u.qmenu:
                return u
        for _ in range(4):
            u = self.press("BACK")
        if u.id != "Home":
            raise Fail(f"{self.role}: can't get home (on {u.id})")
        return u

    def unmute(self):
        """The active slot unmuted (HOME-15's quick mute): True if it was."""
        u = self.home()
        if "muted" in str(u.active.get("state", "")).lower():
            u = self.keys("STAR")  # HOME-15.1: one STAR unmutes; Off only if it lands within 1 s of the muting one
            if str(u.active.get("state", "")) == "Off":
                self.keys("STAR")
            return True
        return False

    def stop_scans(self):
        """Stop the scans running (SIDE2 on each); the runner starts them again at the end. The slots stopped."""
        stopped = []
        lines = [x for x in self.cmd("slot").splitlines() if x.strip()]
        active = next((x.strip()[0] for x in lines if x.strip()[1:2] == "*"), "A")
        for x in lines:
            L = x.strip()[0]
            if "scanning" in x:
                self.cmd(f"slot {L} active")
                self.home()
                self.press("SIDE2", settle=0.3)
                stopped.append(L)
        if stopped:
            self.cmd(f"slot {active} active")
            self.home()
        return stopped


class Ctx:
    """What a test gets: t.tx and t.rx (Side), t.key (the transmitting radio's key-up), the receiving radio's own
    key-ups (rx_probe, rx_keyup), polling and sampling the readback, checks with evidence, deferred clean-ups."""

    def __init__(self, run):
        self.run, self.clock = run, run.clock
        self.tx = Side(self, "tx", run.radios["tx"], run.keyers["tx"])
        self.rx = Side(self, "rx", run.radios["rx"], run.keyers["rx"])
        self.tx.rb, self.rx.rb = run.rb["tx"], run.rb["rx"]
        self.checks, self.notes, self.evidence, self.defers = [], [], [], []
        self.captures = []
        self.last = None

    def pump(self):
        for k in self.run.keyers.values():
            k.pump()

    def now(self):
        return self.clock.time()

    def sleep(self, s):
        end = self.clock.time() + max(0.0, s)
        while True:
            left = end - self.clock.time()
            if left <= 0:
                return
            self.clock.sleep(min(left, 0.1))
            self.pump()

    def key(self, seconds, purpose, kind="burst"):
        return self.run.keyers["tx"].keyup(seconds, purpose, kind)

    @contextlib.contextmanager
    def relay(self, seconds, purpose, extra, kind="burst", relay_kind="relay"):
        """A key-up of the transmitting radio that the receiving radio repeats (its crossband repeater): both radios'
        duty limits admit it; `extra` is how much longer the repeater stays keyed (its hang and pass end). The
        key-up runs its whole length: the body's end waits for its deadline (an exception ends it at once)."""
        with self.run.keyers["tx"].keyup(seconds, purpose, kind, relay=(self.run.keyers["rx"], extra, relay_kind)) as k:
            yield k
            if k.deadline is not None:
                self.sleep(max(0.0, k.deadline - self.now() + 0.05))

    @contextlib.contextmanager
    def capture(self, mhz, seconds, what, lead=1.0, key=None):
        """The SDR (--sdr) on `mhz` for `seconds` from now; the body runs `lead` s in (rtl_sdr's start-up), and after
        it the capture is waited for: `cap.raw` (u8 IQ), `cap.path`. Skips the test without --sdr. `key` (seconds,
        extra[, the relay's kind]): the key-up the body will make, relayed: both duty limits are waited for before
        the capture starts, so no rest falls inside it."""
        sdr = self.run.sdr
        self.need(sdr is not None, "needs the SDR on the output (--sdr)")
        if key:
            s, ex = key[0], key[1]
            kt = self.run.keyers
            kt["rx"].duty.wait(kt["rx"].duty.planned(s + ex), key[2] if len(key) > 2 else "relay", what, lead)
            kt["tx"].duty.wait(kt["tx"].duty.planned(s), "timeout" if s > 4.5 else "burst", what, lead)
        path = os.path.join(self.run.capdir(), f"{self.run.test_name}-{len(self.captures)}-{bench.slug(what)}.u8")
        cap = type("Capture", (), {})()
        cap.mhz, cap.path, cap.what, cap.raw = float(mhz), path, what, None
        t0 = self.now()
        sdr.start(float(mhz), seconds, path)
        self.captures.append(path)
        try:
            self.sleep(lead)
            yield cap
            self.sleep(t0 + seconds - self.now() + 0.1)
            cap.raw = sdr.finish()
        finally:
            sdr.stop()

    def rx_probe(self, purpose):
        return self.run.keyers["rx"].keyup(0.0, purpose, expect_refusal=True)

    def rx_keyup(self, seconds, purpose, scan_freqs=None):
        return self.run.keyers["rx"].keyup(seconds, purpose, scan_freqs=scan_freqs)

    def poll(self, side, pred, timeout, every=0.0, light=False):
        """Readbacks until pred holds: (readback, its time) or (None, None) after `timeout` s. `light`: the radio's
        compact state (Side.state) where pred needs no more (the crossband line, the cards' rx/state)."""
        end = self.clock.time() + timeout
        every = every or (0.25 if getattr(side.radio, "ble", False) else 0.02)  # Bluetooth: no back-to-back reads
        while True:
            t0 = self.clock.time()
            u = side.state() if light else side.raw()
            at = (t0 + self.clock.time()) / 2
            try:
                ok = pred(u)
            except (KeyError, TypeError, AttributeError):
                ok = False
            if ok:
                return u, at
            if self.clock.time() >= end:
                return None, None
            self.sleep(every)

    def samples(self, side, secs, every=0.0, light=False):
        out, end = [], self.clock.time() + secs
        every = every or (0.25 if getattr(side.radio, "ble", False) else 0.02)
        while self.clock.time() < end:
            t0 = self.clock.time()
            u = side.state() if light else side.raw()
            out.append(((t0 + self.clock.time()) / 2, u))
            self.sleep(every)
        return out

    # -- checks --
    def check(self, cond, what, got=None):
        self.checks.append(dict(what=what, ok=bool(cond), got=None if got is None else str(got)[:300]))
        if self.last is not None and (not self.evidence or self.evidence[-1] is not self.last.lines):
            self.evidence.append(self.last.lines)
            del self.evidence[:-6]
        if not cond:
            raise Fail(f"{what}" + (f": got {str(got)[:300]}" if got is not None else ""))

    def eq(self, got, want, what):
        self.check(got == want, f"{what} is {want!r}", got)

    def has(self, hay, needle, what):
        self.check(needle in (hay or ""), f"{what} has {needle!r}", hay)

    def fail(self, what):
        self.check(False, what)

    def need(self, cond, why):
        if not cond:
            raise Skip(why)

    def note(self, s):
        self.notes.append(s)

    def defer(self, fn):
        self.defers.append(fn)


# ---- The run ----


def load_tests():
    import tworadio_tests
    for m in pkgutil.iter_modules(tworadio_tests.__path__):
        if m.name != "core":
            importlib.import_module(f"tworadio_tests.{m.name}")
    return trc.REGISTRY


def select(tests, a):
    out = []
    for t in tests:
        if a.k and not any(k.lower() in t.name.lower() for k in a.k.split(",")):
            continue
        if a.module and t.module not in a.module.split(","):
            continue
        if a.rules and not any(fnmatch.fnmatch(r, g.strip()) for g in a.rules.split(",") for r in t.rules):
            continue
        out.append(t)
    return out


def slot_lines(side):
    return [x.strip() for x in side.cmd("slot").splitlines() if x.strip()]


def restore_slots(side, want, notes):
    """The slots as in `want` (`slot` lines): the scans running now stopped, each VFO by `slot L vfo ...`, a
    channel by V/M and its number (NAV-4), the scans that ran started again, the active slot. What differs after
    is added to notes; the steps taken are returned."""
    sf = uitest.slot_fields
    now = slot_lines(side)
    if [sf(x)["cmp"] for x in now] == [sf(x)["cmp"] for x in want] and \
            ["scanning" in x for x in now] == ["scanning" in x for x in want]:
        return []
    put = []
    for x in now:
        if "scanning" in x:
            side.cmd(f"slot {x[0]} active")
            side.home()
            side.press("SIDE2", settle=0.3)
    for w in want:
        L = w[0]
        h = next((x for x in slot_lines(side) if x[0] == L), "")
        if " aprs " in f" {w} " or sf(w)["cmp"] == sf(h)["cmp"]:
            continue
        wf = sf(w)
        try:
            if wf["ch"] is not None:
                side.cmd(f"slot {L} active")
                u = side.home()
                if u.active.get("head", "").startswith("VFO"):
                    side.keys("VM")
                side.keys(" ".join(str(wf["ch"])) + " MENU")
                side.home()
                put.append(f"slot {L} channel {wf['ch']}")
            else:
                line = f"slot {L} vfo {wf['freq']} {wf['mode'].lower()} {wf['power']}"
                kv = wf["kv"]
                line += f" tone {kv['tone']}" if "tone" in kv else f" txtone {kv.get('txtone', 'off')} rxtone {kv.get('rxtone', 'off')}"
                line += f" split {kv['split']}" if "split" in kv else f" offset {kv.get('offset', 'off')}"
                side.cmd(line)
                put.append(line)
        except (fl.RadioError, Fail) as e:
            notes.append(f"{side.role} slot {L}: {e}")
    for w in want:
        if "scanning" in w and not any(x[0] == w[0] and "scanning" in x for x in slot_lines(side)):
            side.cmd(f"slot {w[0]} active")
            side.home()
            side.press("SIDE2", settle=0.3)
            put.append(f"slot {w[0]} scan started again")
    act = next((w[0] for w in want if w[1:2] == "*"), None)
    if act:
        side.cmd(f"slot {act} active")
    side.home()
    now = slot_lines(side)
    for w in want:
        h = next((x for x in now if x[0] == w[0]), "")
        if sf(w)["cmp"] != sf(h)["cmp"]:
            notes.append(f"{side.role} slot {w[0]}: was {w!r}, now {h!r}")
    return put


class Run:
    def __init__(self, a, radios, clock, threaded=True, log=say):
        self.a, self.radios, self.clock, self.log = a, radios, clock, log
        self.duty = {r: DutyLimiter(None, clock, out=log) for r in radios}
        self.keyers = {r: Keyer(radios[r], r, self.duty[r], clock, log, threaded) for r in radios}
        self.changed = {r: {} for r in radios}  # role -> {path: value before}
        self.rb = {r: (1.0 if getattr(radios[r], "ble", False) else 0.05) for r in radios}
        self.snap = {}
        self.stopped = None
        self.link_lost = None  # the role whose link timed out, when that stopped the run
        self.sdr = None  # --sdr: bench.Sdr (or the selftest's fake)
        self.cap_dir = None
        self.test_name = ""
        self.base_changed = {}
        # What the run changed, also on disk, by radio (not role): a run killed before its restore (pkill, a closed
        # terminal) is put back by the next run before its snapshot. Oct 15, 2026: a killed run left the black
        # radio's watch Off (0 1 1), and the next run's snapshot took Off as the radio's own, so every restore
        # after it put Off back.
        self.journal = getattr(a, "journal", None) or os.path.join(getattr(a, "results", None) or RESULTS,
                                                                   ".tworadio-pending.json")

    def radio_key(self, role):
        """The radio behind a role, as the journal names it: the cable's port name changes with the USB socket."""
        spec = getattr(self.radios[role], "spec", None) or role
        return "cable" if spec.startswith("/dev/") else spec

    def journal_save(self):
        """The settings changed and not yet put back, per radio, to disk (removed when there are none)."""
        pend = {}
        for role, ch in self.changed.items():
            both = dict(ch, **{k: v for k, v in (self.base_changed if role == "tx" else {}).items() if k not in ch})
            if both:
                pend[self.radio_key(role)] = both
        with contextlib.suppress(OSError):
            if pend:
                os.makedirs(os.path.dirname(self.journal) or ".", exist_ok=True)
                with open(self.journal, "w") as f:
                    json.dump(pend, f)
            elif os.path.exists(self.journal):
                os.remove(self.journal)

    def journal_replay(self, ctx):
        """Settings a killed run left changed on these radios, put back before the snapshot; [what was put]."""
        try:
            with open(self.journal) as f:
                pend = json.load(f)
        except (OSError, ValueError):
            return []
        put = []
        for role in self.radios:
            for path, v in reversed(list(pend.pop(self.radio_key(role), {}).items())):
                ok, out = getattr(ctx, role).cmd_err(f"set {path} {v}")
                put.append(f"{role} {path} -> {v}" + ("" if ok else f" (refused: {out.strip()})"))
        with contextlib.suppress(OSError):
            if pend:  # another radio's, kept for when it is back
                with open(self.journal, "w") as f:
                    json.dump(pend, f)
            else:
                os.remove(self.journal)
        if put:
            self.log("put back from an interrupted run: " + "; ".join(put))
        return put

    def capdir(self):
        """Where this run's SDR captures go: kept for the tests that fail, removed for those that pass."""
        if self.cap_dir is None:
            root = getattr(self.a, "results", None) or RESULTS
            stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
            self.cap_dir = os.path.join(root, f"{stamp}_tworadio-captures")
        os.makedirs(self.cap_dir, exist_ok=True)
        return self.cap_dir

    def verify(self):
        """Both radios say tx-dry with the bench bit, or tx+dry (a tx build with Dry TX on: TX-48), in LINK-35's
        build byte, else nothing goes on."""
        bad = [f"the {r} radio ({x.spec}) says it runs a {x.build} build{' with' if x.bench else ' without'} the bench "
               "bit" for r, x in self.radios.items() if not dry_ok(x)]
        for k in self.keyers.values():
            k.gatelog.append(dict(radio=k.role, gate="a tx-dry bench build or Dry TX on (LINK-35, TX-48)",
                                  ok=dry_ok(k.radio), detail=k.radio.build))
        if bad:
            raise SafetyError("refused: " + "; ".join(bad) + " (LINK-35: both radios must run tx-dry, 0x82, or a tx "
                              "build with Dry TX on, MENU 0 8 8 5, TX-48): nothing was changed or keyed")

    MIN_BATTERY_PCT = 30  # Alex, Oct 14, 2026: the black radio reached 7.0 V, near the cut-off, during a run

    def battery_check(self):
        """Both radios' batteries from the status bar readback: no key-up below MIN_BATTERY_PCT, low or cut-off
        (a low battery refuses or cuts transmit, which would read as a test failure)."""
        bad = []
        for role, x in self.radios.items():
            if not hasattr(x, "ui"):
                continue  # the selftest's fakes
            bar = (x.ui().get("bar") or {})
            pct, low, cut = int(bar.get("battery", 100)), int(bar.get("low", 0)), int(bar.get("cutoff", 0))
            if low or cut or getattr(self.a, "verbose", False):  # quiet by default; a low battery always prints
                self.log(f"battery: {role} {pct}%{' LOW' if low else ''}{' CUT-OFF' if cut else ''}")
            if pct < self.MIN_BATTERY_PCT or low or cut:
                bad.append(f"the {role} radio's battery is at {pct}%{' (low)' if low else ''}{' (cut-off)' if cut else ''}")
        if bad:
            raise SafetyError("refused: " + "; ".join(bad) + f": charge it to at least {self.MIN_BATTERY_PCT}% first")

    def ctx(self):
        return Ctx(self)

    def snapshot(self):
        c = self.ctx()
        for role in self.radios:
            side = getattr(c, role)
            self.snap[role] = slot_lines(side)
        if getattr(self.a, "verbose", False):
            self.log(f"snapshot: tx {' | '.join(self.snap['tx'])}")
            self.log(f"          rx {' | '.join(self.snap['rx'])}")

    def restore(self, ctx, notes, roles=("tx", "rx")):
        """Each radio's settings and slots put back; a radio whose link fails is left (noted), the other still done."""
        put = []
        stop = None
        # The cable radio first: a dead Bluetooth link's time-outs never hold up the healthy one (Oct 15, 2026).
        for role in sorted(roles, key=lambda r: bool(getattr(self.radios[r], "ble", False))):
            try:
                put += self._restore_one(ctx, role, notes)
            except LINK_ERRORS as e:
                notes.append(f"{role}: not put back: {type(e).__name__}: {e}")
            except KeyboardInterrupt as e:  # SIGTERM mid-restore: the other radio still put back, then stop
                notes.append(f"{role}: not put back: interrupted")
                stop = e
        if stop is not None:
            raise stop
        return put

    def _restore_one(self, ctx, role, notes):
        put = []
        side = getattr(ctx, role)
        # The crossband repeater never runs past a test (XB-20): stopped, and read back stopped, before anything
        # is put back. Slots put back under a running repeater would move its sides (Oct 14, 2026: `xb` read
        # "A 146.49500 <-> B 146.52000", the snapshot's slots, while it ran).
        ok, out = side.cmd_err("xb stop")
        if ok:
            put.append(f"{role} crossband stopped")
        ok, out = side.cmd_err("xb")
        if ok and not out.strip().startswith("Crossband: off"):
            notes.append(f"{role}: the crossband repeater still runs after xb stop ({out.strip()}): its slots "
                         "and settings left as they are")
            return put
        for path, v in reversed(list(self.changed[role].items())):
            ok, out = side.cmd_err(f"set {path} {v}")
            if ok:
                put.append(f"{role} {path} -> {v}")
            else:
                notes.append(f"{role} {path}: can't put back {v!r}: {out}")
        self.changed[role].clear()
        self.journal_save()
        try:
            side.home()
            put += [f"{role} {x}" for x in restore_slots(side, self.snap[role], notes)]
        except LINK_ERRORS:
            raise
        except Exception as e:
            notes.append(f"{role} slots: {type(e).__name__}: {e}")
        return put

    def run_one(self, tst):
        t = self.ctx()
        res = dict(name=tst.name, module=tst.module, rules=tst.rules, part=tst.part, doc=tst.doc, where="radio",
                   pages=[])
        t0 = self.clock.time()
        n0 = {r: len(k.keyups) for r, k in self.keyers.items()}
        self.test_name = tst.name
        lost = None
        try:
            if tst.long and not getattr(self.a, "long", False):
                raise Skip("keys longer than the 5 s burst limit (one continuous key-up, the duty limit's "
                           "\"timeout\" exception): --long runs it")
            if tst.sdr and self.sdr is None:
                raise Skip("measures the repeater's output on the air: --sdr (an RTL-SDR on the output frequency)")
            if tst.fast and not t.rx.fast:
                raise Skip(f"times to tenths of a second: the receiving radio's readback takes {t.rx.rb:.1f} s "
                           "(Bluetooth); run it with --swap (the cable radio receiving)")
            t.tx.home()
            t.rx.home()
            tst.fn(t)
            res["result"] = "pass"
        except Skip as e:
            res.update(result="skip", why=str(e))
        except Fail as e:
            res.update(result="fail", why=str(e))
        except SafetyError:
            raise
        except LINK_ERRORS as e:  # a link timed out: every later test would too; the run stops after this one
            lost = LinkLost(self.role_of(e), e)
            res.update(result="error", why=f"link lost: {lost}")
        except (fl.RadioError, BenchError) as e:
            res.update(result="error", why=f"{type(e).__name__}: {e}")
        except Exception as e:  # a bug in the test: shown with its place
            res.update(result="error", why=f"{type(e).__name__}: {e}", trace=traceback.format_exc(limit=-3))
        finally:
            for k in self.keyers.values():  # nothing a test started stays keyed
                if k.active is not None:
                    k.emergency_off()
            notes = []
            if lost is None:
                for fn in reversed(t.defers):
                    try:
                        fn()
                    except LINK_ERRORS as e:
                        notes.append(f"clean-up: {type(e).__name__}: {e}")
                        lost = LinkLost(self.role_of(e), e)
                        break
                    except Exception as e:
                        notes.append(f"clean-up: {type(e).__name__}: {e}")
            # After a lost link only the slots and settings (no clean-ups: they read back, on the slow link); the
            # run's own end puts back what it can.
            put = self.restore(t, notes) if lost is None else []
            if lost is None and any(": not put back: " in n for n in notes):
                lost = LinkLost("rx" if any(n.startswith("rx: not put back") for n in notes) else "tx",
                                TimeoutError(next(n for n in notes if ": not put back: " in n)))
            self.rb = {"tx": t.tx.rb, "rx": t.rx.rb}
        res["secs"] = round(self.clock.time() - t0, 2)
        if t.captures:
            if res["result"] in ("fail", "error"):
                res["captures"] = [c for c in t.captures if os.path.exists(c)]
            else:
                for c in t.captures:
                    with contextlib.suppress(OSError):
                        os.remove(c)
        res["checks"], res["notes"], res["evidence"] = t.checks, t.notes, t.evidence[-3:]
        if res["result"] in ("fail", "error") and t.last is not None and t.last.lines not in t.evidence:
            res["evidence"] = (t.evidence + [t.last.lines])[-3:]
        res["keyups"] = [dict(radio=r, purpose=k.purpose, kind=k.kind, accepted=k.accepted, refused=k.refused,
                              unkeyed=k.unkeyed, keyed_s=round((k.duty or {}).get("keyed_s", 0.0), 2))
                         for r, kk in self.keyers.items() for k in kk.keyups[n0[r]:]]
        if put:
            res["restored"] = put
        if notes:
            res["restore_notes"] = notes
        if lost is not None:
            lost.res = res
            raise lost
        return res

    def role_of(self, err):
        """Which radio's link `err` came from: the deepest frame of its traceback whose `self` is a radio (its
        LinkRadio, Link or port); else the one on Bluetooth."""
        found, tb = None, err.__traceback__
        while tb is not None:
            me = tb.tb_frame.f_locals.get("self")
            for role, r in self.radios.items():
                if me is not None and me in (r, getattr(r, "link", None), getattr(r, "port", None)):
                    found = role
            tb = tb.tb_next
        return found or next((r for r, x in self.radios.items() if getattr(x, "ble", False)), "rx")

    def run(self, tests):
        self.verify()
        self.battery_check()
        c = self.ctx()
        self.journal_replay(c)
        self.snapshot()
        results = []
        left = []
        try:
            # The transmitting radio's end tone off for the run (as bench.py): its carrier ends 400 ms after KEY off.
            c.tx.set(END_TONE, "Off")
            self.base_changed = dict(self.changed["tx"])
            self.changed["tx"].clear()
            self.journal_save()
            for tst in tests:
                try:
                    res = self.run_one(tst)
                except SafetyError as e:  # a gate refused or a KEY went unanswered: KEY off was sent; no more keying
                    self.stopped = str(e)
                    self.log(f"\nSTOPPED in {tst.name}: {e}")
                    break
                except LinkLost as e:  # a link timed out: KEY off to both now; the tests after it would only fail
                    for k in self.keyers.values():
                        k.emergency_off()
                    self.stopped = f"link lost in {tst.name}: {e}"
                    self.link_lost = e.role
                    if e.res is not None:
                        results.append(e.res)
                    self.log(f"  [ERROR] {tst.module}.{tst.name}: {e}")
                    self.log(f"\nSTOPPED in {tst.name}: {e}; KEY off sent to both; {len(tests) - len(results)} "
                             "tests not run. The results are kept but not recorded.")
                    break
                results.append(res)
                mark = {"pass": "pass", "fail": "FAIL", "error": "ERROR", "skip": "skip"}[res["result"]]
                keyed = sum(x["keyed_s"] for x in res["keyups"])
                self.log(f"  [{mark:5s}] {tst.module}.{tst.name} ({', '.join(tst.rules)}) {res['secs']:.0f} s, "
                         f"keyed {keyed:.1f} s" + (f": {res.get('why')}" if res["result"] != "pass" else "")
                         + (f"  [NOT restored: {'; '.join(res['restore_notes'])}]" if res.get("restore_notes") else ""))
                if res["result"] in ("fail", "error") and getattr(self.a, "verbose", False):
                    for line in (res["evidence"][-1] if res["evidence"] else []):
                        self.log(f"         | {line}")
                    if res.get("trace"):
                        self.log(res["trace"])
                if getattr(self.a, "stop", False) and res["result"] in ("fail", "error"):
                    break
        finally:
            for k in self.keyers.values():
                k.emergency_off()  # KEY off to both, whatever happened
            notes = []
            self.changed["tx"].update(self.base_changed)
            try:
                self.restore(c, notes)
                if not any(n.startswith("tx: not put back") for n in notes):
                    self.base_changed = {}
                    self.journal_save()
                for role in ("tx", "rx"):
                    if any(n.startswith(f"{role}: not put back") for n in notes):
                        continue
                    now = slot_lines(getattr(c, role))
                    sf = uitest.slot_fields
                    for w, h in zip(self.snap.get(role, []), now):
                        if sf(w)["cmp"] != sf(h)["cmp"] or ("scanning" in w) != ("scanning" in h):
                            left.append(f"{role} slot: was {w!r}, now {h!r}")
            except Exception as e:
                left.append(f"final restore: {type(e).__name__}: {e}")
            left += notes
        return results, left


def rules_of(results):
    """{rule: result, test, why, scope}: the worst result per rule ("part" when only part-checks cover it)."""
    order = {"fail": 4, "error": 4, "pass": 2, "skip": 1}
    rules = {}
    for res in results:
        for rid in res["rules"]:
            prev = rules.get(rid)
            if prev is None or order[res["result"]] > order[prev["result"]]:
                rules[rid] = dict(result=res["result"], test=res["name"], why=res.get("why", ""))
    for rid, x in rules.items():
        full = any(rid in r["rules"] and rid not in r.get("part", []) and r["result"] != "skip" for r in results)
        x["scope"] = "all" if full else "part"
    return rules


def summary_of(a, run, results, left, started, sim):
    tx, rx = run.radios["tx"], run.radios["rx"]
    commit = a.commit or uitest.host_commit()
    version = rx.version if rx.version == tx.version else f"{tx.version} / {rx.version}"
    counts = {k: sum(1 for x in results if x["result"] == k) for k in ("pass", "fail", "error", "skip", "xfail", "xpass")}
    return dict(
        tool="tworadio.py", started=started.isoformat(timespec="seconds"), port=f"tx={tx.spec} rx={rx.spec}", sim=sim,
        build="tx-dry" if tx.build == rx.build == "tx-dry" else f"{tx.build}/{rx.build}", version=version,
        commit=commit, label=a.build or f"{rx.version}-{commit}-tx-dry",
        roles=dict(tx=dict(port=tx.spec, version=tx.version, build=tx.build),
                   rx=dict(port=rx.spec, version=rx.version, build=rx.build)),
        swap=bool(getattr(a, "swap", False)), stopped=run.stopped, counts=counts, rules=rules_of(results), left_changed=left,
        tests=results, duty={r: d.summary() for r, d in run.duty.items()},
        gates=[g for k in run.keyers.values() for g in k.gatelog])


def page_html(summary, duty_lines):
    css = uitest.bench_css()
    e = html.escape
    head = (f"<h1>Ferrite two-radio tests</h1><p class=sub>{e(summary['label'])} · {e(summary['started'])} · "
            f"TX {e(summary['roles']['tx']['port'])} → RX {e(summary['roles']['rx']['port'])}</p>"
            + "".join(f"<p class=sub>{e(x)}</p>" for x in duty_lines))
    body = uitest.ui_section(summary).replace("<h2>UI tests", "<h2>Two-radio tests", 1)
    return (f"<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content='width=device-width,"
            f"initial-scale=1'><title>Ferrite two-radio tests</title><style>{css}</style></head><body><main>{head}"
            f"{body}</main></body></html>")


def write_results(summary, sess_dir, duty_lines):
    d = os.path.join(sess_dir, "tworadio")
    os.makedirs(d, exist_ok=True)
    path = os.path.join(d, "results.json")
    json.dump(summary, open(path, "w"), indent=1, ensure_ascii=False, default=str)
    page = os.path.join(d, "summary.html")
    open(page, "w").write(page_html(summary, duty_lines))
    return path, page


# ---- The plan ----


def plan_lines(tests, a):
    """What a run would do and how long it keys, simulated on bench.py's FakeClock with the duty limit in force."""
    ble_rx = bool(a.rx and a.rx.startswith("ble:"))
    ble_any = ble_rx or bool(a.tx and a.tx.startswith("ble:"))
    per_key = 6.0 if ble_any else 3.0  # set-up and readbacks around a key-up
    per_test = 25.0 if ble_any else 8.0  # home, slots, settings put back
    clk = bench.FakeClock()
    duty = {r: DutyLimiter(None, clk, out=lambda *_: None) for r in ("tx", "rx")}
    out = [f"two-radio plan: {len(tests)} tests; TX {a.tx or '(--tx)'} keys, RX {a.rx or '(--rx)'} is read back"
           + (" (Bluetooth: about 1 s a readback)" if ble_rx else ""),
           "  safety: both radios tx-dry + bench (HELLO's LINK-35 byte) or nothing happens; keyed only on "
           + ", ".join(f"{f / 1e5:.3f}" for f in sorted(APPROVED)) + " MHz, power low, the active slot; KEY on "
           "answered within 0.6 s (cable) / 2 s (Bluetooth); keepalives 0.5 s; KEY off at each key-up's deadline and "
           "in finally; bench.py's DutyLimiter per radio", ""]
    total_keyed = {"tx": 0.0, "rx": 0.0}
    skipped, nosdr = [], []
    for t in tests:
        if t.fast and ble_rx:
            skipped.append(t)
            continue
        t0 = clk.time()
        clk.sleep(per_test + t.idle)
        if t.long and not getattr(a, "long", False):
            skipped.append(t)
            continue
        if t.sdr and not getattr(a, "sdr", False):
            nosdr.append(t)
        for role, ks in (("tx", t.keyups), ("rx", t.rx_keys)):
            for i, s in enumerate(ks):
                d = duty[role]
                probe = s < 1.0 and role == "rx"
                kind = "probe" if probe else ("timeout" if s > 4.5 else "burst")
                ex = t.relays[min(i, len(t.relays) - 1)] if t.relays and role == "tx" else None
                if ex is not None:  # the repeater's pass: admitted on the receiving radio's limit too
                    duty["rx"].admit(duty["rx"].planned(s + ex), "timeout" if s + ex > 4.5 else "relay", t.name)
                planned = d.planned(KEY_ANSWER_CABLE_S if probe else s)
                d.admit(planned, kind, t.name)
                ts = clk.time()
                clk.sleep(0.6 if probe else s)
                ev = dict(accepted=not probe, t_off=clk.time(), t_unkeyed=None)
                d.record(t.name, kind, ev, ts)
                if ex is not None:
                    duty["rx"].record(t.name, "relay", dict(accepted=True, t_off=ts + s + ex, t_unkeyed=None), ts)
                    total_keyed["rx"] += s + ex
                clk.sleep(per_key)
                total_keyed[role] += 0.0 if probe else s
        wall = clk.time() - t0
        part = f"  (part: {', '.join(t.part)})" if t.part else ""
        out.append(f"  {t.module}.{t.name:38s} {', '.join(r for r in t.rules if r not in t.part)}{part}")
        out.append(f"      {len(t.keyups)} key-ups, {sum(t.keyups):.1f} s keyed"
                   + (f"; RX keys {len(t.rx_keys)}x ({'probe' if all(x < 1 for x in t.rx_keys) else sum(t.rx_keys)})"
                      if t.rx_keys else "")
                   + (f"; RX repeats each (+{', '.join(f'{x:g}' for x in t.relays)} s)" if t.relays else "")
                   + f"; about {wall / 60:.1f} min" + ("  [fast: cable RX]" if t.fast else "")
                   + ("  [SDR]" if t.sdr else "") + ("  [long]" if t.long else ""))
    wall = clk.time() - duty["tx"].t_start
    out.append("")
    if skipped:
        out.append(f"skipped (fast with RX on Bluetooth: --swap runs them; long: --long): "
                   f"{', '.join(t.name for t in skipped)}")
    if nosdr:
        out.append(f"skipped without --sdr (planned here as if it were there): {', '.join(t.name for t in nosdr)}")
    rules = sorted({r for t in tests if t not in skipped for r in t.rules}, key=uitest.rule_key)
    out.append(f"rules: {len(rules)}: {' '.join(rules)}")
    out.append(f"estimated: about {wall / 60:.0f} min; keyed {total_keyed['tx']:.0f} s by TX "
               f"({duty['tx'].summary()['keyups']} key-ups) and {total_keyed['rx']:.1f} s by RX (its probes, PTT and "
               "crossband passes)")
    out.append("TX " + duty["tx"].report())
    out.append("RX " + duty["rx"].report())
    return out


# ---- Selftest ----


def selftest(a):
    import tworadio_fake
    return tworadio_fake.selftest(a, sys.modules[__name__])


def selftest_sim(a):
    import tworadio_fake
    return tworadio_fake.selftest_sim(a, sys.modules[__name__])


# ---- Main ----


def main_args(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--tx", help="the radio that keys: a serial device, ble:ID or tcp:HOST:PORT")
    ap.add_argument("--rx", help="the radio that receives and is read back")
    ap.add_argument("--swap", action="store_true", help="reverse the roles (--tx receives, --rx keys)")
    ap.add_argument("--pin-tx", help="the TX radio's PIN (Bluetooth with a PIN lock)")
    ap.add_argument("--pin-rx", help="the RX radio's PIN")
    ap.add_argument("-k", help="tests whose name has one of these (comma list)")
    ap.add_argument("--module", help="tests in these modules (receive, busy, watch, tones, scan, crossband)")
    ap.add_argument("--rules", help="tests checking a rule matching one of these globs")
    ap.add_argument("--list", action="store_true", help="the tests, their rules and key-ups")
    ap.add_argument("--dry-plan", action="store_true", help="the plan with its keyed time; touches nothing")
    ap.add_argument("--selftest", action="store_true", help="fake radios on a fake air (or --sim: two simulators)")
    ap.add_argument("--sim", action="store_true", help="with --selftest: two simulators (started here)")
    ap.add_argument("--results", default=RESULTS, help="results root (default %(default)s)")
    ap.add_argument("--build", help="a label for the results directory")
    ap.add_argument("--commit", help="the firmware's commit (default: the host checkout's)")
    ap.add_argument("--record", action="store_true", help="a result row per rule into the test plan")
    ap.add_argument("--docs", default=os.path.join(ROOT, "docs"), help="the docs checkout for --record")
    ap.add_argument("--stop", action="store_true", help="stop at the first failure")
    ap.add_argument("--verbose", "-v", action="store_true", help="full output: the radios' header and snapshot lines, a failure's readback "
                    "(default: one line per test and the summary; safety messages always print)")
    ap.add_argument("--sdr", action="store_true", help="an RTL-SDR on the crossband repeater's output (bench.py's "
                    "Sdr, fixed gain 0.9 dB): the tests that measure the air")
    ap.add_argument("--long", action="store_true", help="also the tests with one key-up over the 5 s burst limit "
                    "(XB-9's 30 s pass timeout, XB-12's ID): the duty limit's timeout exception")
    return ap.parse_args(argv)


def roles(a):
    tx, rx = a.tx, a.rx
    if a.swap:
        tx, rx = rx, tx
    return tx, rx


def main(argv=None):
    a = main_args(argv)
    tests = select(load_tests(), a)
    if a.list:
        for t in tests:
            say(f"{t.module}.{t.name:40s} {', '.join(t.rules)}" + (f"  [part: {', '.join(t.part)}]" if t.part else "")
                + f"  key-ups {t.keyups}" + (f" rx {t.rx_keys}" if t.rx_keys else "") + ("  [fast]" if t.fast else "")
                + ("  [sdr]" if t.sdr else "") + ("  [long]" if t.long else ""))
        ids = sorted({r for t in tests for r in t.rules}, key=uitest.rule_key)
        say(f"{len(tests)} tests, {len(ids)} rules: {' '.join(ids)}")
        unknown = [i for i in ids if i not in uitest.manual_rules(a.docs)]
        if unknown:
            say(f"rules the manual doesn't have: {' '.join(unknown)}")
            return 1
        return 0
    if a.selftest:
        return 0 if (selftest_sim(a) if a.sim else selftest(a)) else 1
    if a.swap and a.dry_plan:
        a.tx, a.rx = a.rx, a.tx
        a.swap = False
    if a.dry_plan:
        for line in plan_lines(tests, a):
            say(line)
        return 0
    if not (a.tx and a.rx):
        sys.exit("give --tx PORT and --rx PORT (a serial device, ble:ID or tcp:HOST:PORT), or --dry-plan / --selftest")
    tx_spec, rx_spec = roles(a)
    sim = tx_spec.startswith("tcp:") and rx_spec.startswith("tcp:")
    if a.record and sim:
        sys.exit("--record on simulators: simulator runs are never recorded")
    pins = (a.pin_tx, a.pin_rx) if not a.swap else (a.pin_rx, a.pin_tx)
    radios = {}
    run = None

    def stop_now(*_):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, stop_now)
    try:
        radios["tx"] = LinkRadio(tx_spec, pins[0])
        radios["rx"] = LinkRadio(rx_spec, pins[1])
        if a.verbose:
            say(f"TX {tx_spec}: Ferrite {radios['tx'].version}, {radios['tx'].build}{' bench' if radios['tx'].bench else ''}")
            say(f"RX {rx_spec}: Ferrite {radios['rx'].version}, {radios['rx'].build}{' bench' if radios['rx'].bench else ''}")
        run = Run(a, radios, bench.Clock(), threaded=True)
        if a.sdr:
            if not shutil.which("rtl_sdr"):
                raise SafetyError("--sdr: rtl_sdr not found (brew install librtlsdr): nothing was changed or keyed")
            run.sdr = bench.Sdr(dict(sdr_gain=bench.DEFAULTS["sdr_gain"], sdr_ppm=bench.DEFAULTS["sdr_ppm"]),
                                run.clock)
        atexit.register(lambda: [k.emergency_off() for k in run.keyers.values()])
        started = datetime.datetime.now()
        results, left = run.run(tests)
    except SafetyError as e:
        say(f"\nSTOPPED: {e}")
        return 2
    except LINK_ERRORS as e:  # before the tests (HELLO, the snapshot) or in the final restore
        say(f"\nSTOPPED: a link failed: {type(e).__name__}: {e}; KEY off sent to both; nothing recorded")
        return 2
    except KeyboardInterrupt:
        say("\ninterrupted: KEY off sent to both radios")
        return 130
    finally:
        if run is not None:
            for k in run.keyers.values():
                k.emergency_off()
        for r in radios.values():
            with contextlib.suppress(Exception):
                r.close()
    summary = summary_of(a, run, results, left, started, sim)
    duty_lines = [f"{r.upper()} {d.report()}" for r, d in run.duty.items()]
    sess = uitest.new_session(a.results, "tworadio-" + summary["label"] + ("-sim" if sim else ""))
    path, page = write_results(summary, sess, duty_lines)
    json.dump(dict(tool="tworadio.py", kinds=["tworadio"], commit=summary["commit"],
                   build=dict(label=summary["label"], flavour=summary["build"], version=summary["version"]),
                   source="simulator" if sim else "radio", port=summary["port"], tworadio=summary["counts"]),
              open(os.path.join(sess, "session.json"), "w"), indent=1)
    c = summary["counts"]
    rr = summary["rules"]
    ok = sorted((k for k, v in rr.items() if v["result"] == "pass"), key=uitest.rule_key)
    bad = sorted((k for k, v in rr.items() if v["result"] in ("fail", "error")), key=uitest.rule_key)
    say(f"\ntwo-radio tests: {c['pass']} pass, {c['fail']} fail, {c['error']} error, {c['skip']} skipped; rules: "
        f"{len(ok)} pass ({' '.join(r + ('◐' if rr[r]['scope'] == 'part' else '') for r in ok)})"
        + (f", {len(bad)} fail ({' '.join(bad)})" if bad else ""))
    for line in duty_lines:
        say(line)
    if left:
        say("left changed after the run:\n  " + "\n  ".join(left))
    if run.stopped:
        say(f"STOPPED by a safety check: {run.stopped}")
    say(f"results: {path}\npage: {page}")
    if a.record and run.stopped:
        say("not recorded: the run stopped early (a stopped run's rows would stand for tests it never ran)")
    elif a.record:
        import record_results
        for line in record_results.record_paths([path], a.docs):
            say(line)
    return 2 if run.stopped else 0 if not (c["fail"] or c["error"] or left) else 1


if __name__ == "__main__":
    sys.exit(main())
