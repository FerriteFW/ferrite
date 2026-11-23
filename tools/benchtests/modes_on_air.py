#!/usr/bin/env python3
"""The modes-on-air suite (tools/bench.py --suite modes-on-air; tools/benchtests/): VOX, DTMF, APRS, KISS, GPS, watch, HF, broadcast
and the end tone, each checked on air with the RTL-SDR against the black radio on a `tx-dry` build, through
bench.py's gates, duty limit and restore.

    tools/bench.py --suite modes-on-air --dry-plan                 # the plan, the time and the keyed time; touches nothing
    tools/bench.py --suite modes-on-air --i-know-its-tx-dry        # everything that can run here
    tools/bench.py --suite modes-on-air --tests vox,dtmf           # groups (vox dtmf tone aprs kiss gps watch hf bcr)
                                                                   # or test names (dtmf_timing, vox_tail, ...)
    tools/bench.py --suite modes-on-air --kiss-ble ble:walkie-talkie   # the KISS tests (KISS over Bluetooth)
    tools/bench.py --suite modes-on-air --slow                     # also GPS-33's timed beacon without a fix (~4 min)
    tools/bench.py --suite modes-on-air --no-audio                 # without the Mac's speaker (no VOX, no mic tone)
    tools/bench.py --selftest                                      # this suite too, on a fake radio, SDR and speaker

The same safety as bench.py: the LINK-35 tx-dry gate before anything; every key-up only with the keying slot on
145.495 or 446.475 MHz (bench.py's test_freqs), simplex, power low, read back over SLOT right before it; every
key-up through bench.py's DutyLimiter (at most 5 s with its unkey tail, a 2x rest, 25 % of any 5 min), the ones the
radio makes by itself (VOX, the dialer, a beacon, a KISS frame) admitted before whatever starts them, with their
longest possible length, and logged after with the length the radio's status showed; the SDR at bench.py's fixed
gain; everything this suite changes saved first and put back in `finally` (VOX off first of all).

Key-ups the radio makes itself are watched over the link's compact status (LINK-19, `keyed=`): one longer than
admitted is stopped at once (the tone killed, VOX off, KEY off). APRS and KISS key on the APRS frequency (0 5 4),
which must read 145.495 (`get 0 5 4`) before any beacon or frame (and slot C over SLOT): left alone when it
does, else typed by keys once (the terminal can't set it) and put back after, else the test says to set it by hand.
They need a callsign of 6 characters at most (AX.25 carries 6, SETUP-13). KISS on makes C the APRS slot (KISS-9). HF transmit is never keyed: HF-22, HF-23's key-up
and HF-25 need an HF test frequency approved first (a decision); only the refusals run.

The speaker (VOX, and the mic tone the DTMF and end tone checks listen for): a sine written to a WAV and
played with macOS `afplay` (no Python audio package needed). Put the Mac's speaker about 10 cm from the radio's
mic, the Mac's volume at about 60 %. The suite calibrates first, without keying: VOX off, a tone at rising
levels, the radio's `mic` (TERM-32: the PA0 envelope VOX reads) after each; it picks the levels and the VOX level
from that, and measures how long the envelope takes to fall after the tone stops (VOX-5's tail starts there).
If the loudest tone doesn't reach VOX level 1's threshold, the VOX tests say so and don't run.

Results: one session under bench.py's results root (session.json with the extras "suite:modes-on-air",
bench/results.json through bench.rule_tests and tests_of(), summary.html, the captures of failed checks), so
`--record` and tools/record_results.py take it like any bench session.
"""
import contextlib
import json
import math
import os
import re
import shutil
import struct
import subprocess
import tempfile
import time
import wave

import numpy as np

import bench as B
import bench_rules
import ferrite_link as fl
import sdrsuite
import sdrtx

S = dict(
    vox="0 8 2 1", vox_level="0 8 2 2", vox_tail="0 8 2 3", dtmf_speed="0 8 5 3",
    dtmf_delay="0 8 5 5", aprs_mode="0 5 3", beacon_mode="0 5 6 1", after_voice="0 5 6 2",
    beacon_power="0 5 6 3", aprs_freq="0 5 4", tx_delay="0 5 9 3", pos_source="0 5 5 4", fixed="0 5 5 5",
    kiss_port="0 5 8", watch="0 1 1", gps_rxtx="0 6 8", bluetooth="0 9 2 1", end_tone=B.END_TONE, gps="0 6 1",
    bsave="0 7 7",
)

DEFAULTS = {
    "mic_tone_hz": 1800.0,  # the Mac's tone at the mic: no DTMF, end tone or AFSK frequency
    "levels_db": [-42, -36, -30, -24, -18, -12, -6, 0],  # dBFS of the calibration tones
    "cal_tone_s": 1.2,
    "vox_tail_long": "2 s",  # VOX-5: the default
    "vox_tail_short": "0.5 s",
    "vox_margin": 10,  # counts over the threshold the keying tone reads (and under it, the ambient)
    "decay_max_s": 1.5,  # the envelope's fall after the tone stops, at most (else the VOX key-ups don't fit 5 s)
    "vox_hold_s": 1.5,  # VOX-3.5
    "vox_hold_tol_s": 0.35,
    "vox_tail_tol_s": 0.35,
    "boot_hold_s": 5.0,  # VOX-3.3
    "vox_screens": [  # VOX-3.1: screens that hold VOX (keys from home; the readback id the screen must have)
        ["MENU", "Quick"], ["MENU 0", "More"], ["MENU 0 8 2", "Group"], ["1", "Entry"]],  # MENU 0: the full menu's id is More
    "dtmf_digits": "1593",
    "dtmf_runs": [[50, 0], [100, 300], [200, 1000]],  # (speed ms, first digit delay ms)
    "dtmf_ms_tol": 12,
    "dtmf_delay_tol": 60,
    "dialer_keys": "MENU 0 8 5 1",  # opens the dialer (DTMF-13); digits, then MENU sends
    "dialer_id": "Dtmf",  # the dialer's readback id
    "aprs_freq_keys": "MENU 0 5 4",  # opens 0 5 4's entry; digits with STAR for the point, MENU takes it
    "beacon_now_keys": "MENU 0 5 1 MENU:long",  # the heard list's options (MENU-25): row 1 is Beacon now
    "tx_delay_tol_ms": 60,
    "aprs_spacing_s": 30.0,
    "hf_keys": "MENU 0 3 3",  # opens HF mode (HF-2)
    "hf_tune_keys": "4 0 STAR 0 0 0 MENU",  # in HF mode: 40.000 MHz (chip 1's range, outside the band plan: HF-23)
    "bcast_mhz": 100.1,
    "watch_resume_s": 30.0,
    "watch_tol_s": 3.0,
    "gps33_wait_s": 210.0,
    "kiss_txdelay_ms": 500,
}

GROUPS = ["vox", "dtmf", "tone", "aprs", "kiss", "gps", "watch", "hf", "bcr"]


class Skip(Exception):
    """A test that can't run here (what it needs), not a failure."""


# ---- The speaker ----


def tone_wav(path, hz, secs, dbfs, rate=44100):
    n = int(secs * rate)
    t = np.arange(n) / rate
    ramp = np.minimum(1, np.minimum(t, secs - t) / 0.01)  # 10 ms ramps: no clicks
    x = 10 ** (dbfs / 20) * 0.99 * np.sin(2 * np.pi * hz * t) * ramp
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes((x * 32767).astype("<i2").tobytes())


class MacAudio:
    """A tone from the Mac's speaker: a WAV, played by `afplay` in the background (stop() kills it)."""

    def __init__(self, clock):
        self.clock, self.p, self.dir = clock, None, tempfile.mkdtemp(prefix="bench-tone-")
        self.t_start = self.t_end = None

    @staticmethod
    def available():
        return shutil.which("afplay") is not None

    def play(self, hz, secs, dbfs):
        self.stop()
        path = os.path.join(self.dir, f"{hz:.0f}-{secs:.2f}-{dbfs:+.0f}.wav")
        if not os.path.exists(path):
            tone_wav(path, hz, secs, dbfs)
        self.p = subprocess.Popen(["afplay", path], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.t_start, self.t_end, self.secs = self.clock.time(), None, secs

    def playing(self):
        if self.p and self.p.poll() is not None:
            self.t_end = self.t_end or self.clock.time()
            self.p = None
        return self.p is not None

    def wait(self):
        if self.p:
            with contextlib.suppress(Exception):
                self.p.wait(timeout=self.secs + 5)
            self.t_end = self.clock.time()
            self.p = None
        return self.t_end

    def stop(self):
        if self.p:
            with contextlib.suppress(Exception):
                self.p.kill()
                self.p.wait(timeout=2)
            self.t_end = self.clock.time()
            self.p = None
        return self.t_end


class FakeAudio:
    """The selftest's speaker: plays on the virtual clock; the fake radio's mic hears what is playing."""

    def __init__(self, clock):
        self.clock, self.plays = clock, []
        self.t_start = self.t_end = None

    def play(self, hz, secs, dbfs):
        self.stop()
        t = self.clock.time()
        self.plays.append([t, t + secs, hz, dbfs])
        self.t_start, self.t_end = t, None

    def at(self, t):
        for a, b, hz, db in self.plays:
            if a <= t < b:
                return hz, db
        return None

    def playing(self):
        return bool(self.plays) and self.plays[-1][1] > self.clock.time()

    def wait(self):
        if self.plays:
            self.clock.sleep(max(self.plays[-1][1] - self.clock.time(), 0))
            self.t_end = self.plays[-1][1]
        return self.t_end

    def stop(self):
        if self.plays and self.plays[-1][1] > self.clock.time():
            self.plays[-1][1] = self.clock.time()
        if self.plays:
            self.t_end = self.plays[-1][1]
        return self.t_end


# ---- The link beyond bench.Radio: remote keys, the readback, the status (LINK-19) ----


class LinkRadio(B.Radio):
    def _ok(self, ty, payload=b"", busy_s=3.0):
        end = time.time() + busy_s
        while True:
            s, rest = self._ask(ty, payload)
            if s == 0:
                return rest
            if s != 7 or time.time() > end:
                raise B.TermError(f"{ty:#x}: {fl.why(s, rest)}")
            time.sleep(0.05)

    def keys(self, seq):
        for word in seq.split():
            name, _, how = word.partition(":")
            how = how.lower() or "short"
            self._ok(fl.KEY, bytes([fl.key_code(name), fl.ACTIONS[how]]))
            if how == "long":
                time.sleep(fl.LONG_HOLD + 0.05)
            time.sleep(0.12)

    def ui_text(self, what=0):
        out, rest = [], self._ok(fl.UI, bytes([what]))
        while True:
            out.append(rest[1:].decode("utf-8", "replace"))
            if rest[0] == 0:
                return "".join(out)
            rest = self._ok(fl.MORE)

    def ui(self):
        return fl.parse_ui(self.ui_text())

    def status(self):
        return fl.parse_status(self.ui_text(2).strip())

    def reboot(self, after=None):
        """LINK-24: into the stock bootloader, which starts Ferrite again; then HELLO again (the time it answered).
        `after()` runs as soon as REBOOT is answered. A refusal (busy while transmitting, LINK-24.3) is an error."""
        s, rest = None, b""
        with contextlib.suppress(Exception):
            s, rest = self._ask(fl.REBOOT)
        if s not in (None, 0):
            raise B.BenchError(f"REBOOT refused: {fl.why(s, rest)}")
        if after:
            after()
        self.close()
        end = time.time() + 30
        while time.time() < end:
            time.sleep(0.3)
            try:
                self.port = fl.Port(self.spec)
                self.link = fl.Link(self.port)
                self.hello_ans = self.hello()
                return time.time()
            except Exception:
                with contextlib.suppress(Exception):
                    self.port.close()
        raise B.BenchError("the radio didn't answer HELLO within 30 s of the restart")


# ---- Readings ----


def parse_gpio(out):
    ports = {}
    for m in re.finditer(r"P([A-E])\s+in ([01 ]{19})\s+out ([01 ]{19})", out):
        ports[m.group(1)] = (int(m.group(2).replace(" ", ""), 2), int(m.group(3).replace(" ", ""), 2))
    if len(ports) != 5:
        raise B.BenchError(f"gpio: can't read {out[:120]!r}")
    return ports


def pin(ports, name, which):
    """which 0: the input level, 1: the output latch."""
    return ports[name[1]][which] >> int(name[2:]) & 1


def parse_bk(out):
    m = re.search(r"REG_[0-9A-Fa-f]+ = 0x([0-9A-Fa-f]{4})", out)
    if not m:
        raise B.BenchError(f"bk: can't read {out[:120]!r}")
    return int(m.group(1), 16)


def si_prop(out, prop):
    """`si prop P`'s answer (TERM-24): "si 13 00 PP PP -> SS RR VV VV", GET_PROPERTY's status, a reserved byte, then
    the value. None if it can't be read."""
    m = re.search(r"->\s*([0-9A-Fa-f]{2})\s+[0-9A-Fa-f]{2}\s+([0-9A-Fa-f]{2})\s+([0-9A-Fa-f]{2})", out)
    if not m or int(m.group(1), 16) & 0x40:  # ERR set
        return None
    return int(m.group(2), 16) << 8 | int(m.group(3), 16)


def parse_mic(out):
    m = re.search(r"min (\d+), max (\d+), mean (\d+)", out)
    if not m:
        raise B.BenchError(f"mic: can't read {out[:120]!r}")
    return dict(min=int(m.group(1)), max=int(m.group(2)), mean=int(m.group(3)), reading=int(m.group(3)) >> 4)


def vox_threshold(level):
    """VOX-14.1: stock's thresholds, level 1 = 21 (keys easiest) to level 9 = 101 (ferrite-core vox.rs)."""
    return 11 + 10 * max(1, min(9, level))


# ---- Demodulation and decoders ----


def air(raw):
    """(deviation Hz at 64 kS/s, fs, carrier mask per sample, carrier runs in s): the whole capture demodulated,
    the deviation zero where there is no carrier."""
    x = sdrtx.baseband(raw)
    fs = sdrtx.FS / sdrtx.DEC
    fm = np.zeros(len(x))
    fm[1:] = np.angle(x[1:] * np.conj(x[:-1])) * fs / (2 * np.pi)
    runs = B.carrier_runs(raw)
    on = np.zeros(len(x), bool)
    for a, b in runs:
        on[int((a + 0.005) * fs): int(max(b - 0.005, a) * fs)] = True
    if on.any():
        fm[on] -= np.median(fm[on])
    fm[~on] = 0
    fm = sdrtx.conv_same(fm, sdrtx.lowpass(6000, fs))
    return fm, fs, on, runs


DTMF_LOW = [697, 770, 852, 941]
DTMF_HIGH = [1209, 1336, 1477, 1633]
DTMF_KEYS = ["123A", "456B", "789C", "*0#D"]


def env_ms(x, fs, hz, win_ms=10, step_ms=1):
    """A tone's amplitude (Hz of deviation) every step_ms, smoothed over win_ms."""
    e = sdrsuite.tone_env(x, fs, hz, win_ms)
    k = int(fs * step_ms / 1000)
    return e[::k]


def dtmf_decode(dev, fs, min_ms=20):
    """The DTMF digits in a deviation track: [(digit, t_on s, t_off s, low Hz dev, high Hz dev)], from 10 ms tone
    envelopes every ms; a digit is on where its low and high tones both stand over a quarter of the loudest pair
    and over twice every other tone of their group."""
    lo = np.array([env_ms(dev, fs, f) for f in DTMF_LOW])
    hi = np.array([env_ms(dev, fs, f) for f in DTMF_HIGH])
    il, ih = lo.argmax(0), hi.argmax(0)
    ml, mh = lo.max(0), hi.max(0)
    ref = np.percentile(np.minimum(ml, mh), 99.5) if len(ml) else 0
    if ref < 100:
        return []
    second_l = np.sort(lo, 0)[-2]
    second_h = np.sort(hi, 0)[-2]
    on = (ml > ref / 4) & (mh > ref / 4) & (ml > 2 * second_l) & (mh > 2 * second_h)
    code = np.where(on, il * 4 + ih, -1)
    out = []
    d = np.diff(np.concatenate([[-1], code, [-1]]))
    edges = np.nonzero(d)[0]
    for a, b in zip(edges[:-1], edges[1:]):
        c = code[a]
        if c < 0 or b - a < min_ms:
            continue
        out.append((DTMF_KEYS[c // 4][c % 4], a / 1000, b / 1000, float(np.median(ml[a:b])), float(np.median(mh[a:b]))))
    return out


def crc_x25(data):
    crc = 0xFFFF
    for b in data:
        crc ^= b
        for _ in range(8):
            crc = (crc >> 1) ^ 0x8408 if crc & 1 else crc >> 1
    return crc ^ 0xFFFF


def ax25_addr(call, last=False, h=False):
    c, _, ssid = call.upper().partition("-")
    b = bytes((ord(x) << 1) for x in c.ljust(6)[:6])
    return b + bytes([0x60 | (int(ssid or 0) << 1) | (0x80 if h else 0) | (1 if last else 0)])


def ax25_frame(dest, src, path, info):
    """An AX.25 UI frame without its FCS (what KISS carries)."""
    calls = [dest, src] + list(path)
    a = b"".join(ax25_addr(c, i == len(calls) - 1) for i, c in enumerate(calls))
    return a + b"\x03\xf0" + info


def ax25_parse(fr):
    """{dest, src, path, info} of an AX.25 UI frame (no FCS), or None."""
    calls, i = [], 0
    while i + 7 <= len(fr):
        a = fr[i:i + 7]
        c = "".join(chr(x >> 1) for x in a[:6]).strip()
        s = (a[6] >> 1) & 0x0F
        calls.append(c + (f"-{s}" if s else "") + ("*" if a[6] & 0x80 and len(calls) >= 2 else ""))
        i += 7
        if a[6] & 1:
            break
    if len(calls) < 2 or i + 2 > len(fr):
        return None
    return dict(dest=calls[0], src=calls[1], path=calls[2:], control=fr[i], pid=fr[i + 1], info=fr[i + 2:])


def hdlc_bits(frame, flags_before, flags_after=2):
    """The bits on air for one frame: flags, the frame and its FCS (LSB first, bit-stuffed), flags."""
    fcs = crc_x25(frame)
    data = frame + bytes([fcs & 0xFF, fcs >> 8])
    flag = [0, 1, 1, 1, 1, 1, 1, 0]
    bits = flag * flags_before
    ones = 0
    for byte in data:
        for k in range(8):
            b = byte >> k & 1
            bits.append(b)
            ones = ones + 1 if b else 0
            if ones == 5:
                bits.append(0)
                ones = 0
    return bits + flag * flags_after


def afsk_dev(bits, t, t0, amp=3000.0):
    """The deviation (Hz) of AFSK1200 for `bits` (NRZI: a 0 changes the tone) starting at t0, at times t."""
    tones = []
    f = 1200.0
    for b in bits:
        if b == 0:
            f = 2200.0 if f == 1200.0 else 1200.0
        tones.append(f)
    tones = np.array(tones)
    ph = np.concatenate([[0.0], np.cumsum(2 * np.pi * tones / 1200)])
    k = np.floor((t - t0) * 1200).astype(int)
    inside = (k >= 0) & (k < len(bits))
    kc = np.clip(k, 0, len(bits) - 1)
    return np.where(inside, amp * np.sin(ph[kc] + 2 * np.pi * tones[kc] * (t - t0 - kc / 1200)), 0.0)


def afsk_decode(dev, fs, spans):
    """AX.25 frames in a deviation track (AFSK1200 Bell 202): [{frame, t_flag (first flag's start, s), t_data
    (the frame's first bit), crc_ok}] with a good FCS, one per frame (16 clock phases tried, the first that decodes
    it kept). `spans`: the carrier runs (s) to look in."""
    m = sdrsuite.tone_env(dev, fs, 1200, 1000 / 1200)
    s = sdrsuite.tone_env(dev, fs, 2200, 1000 / 1200)
    d = np.log((m + 1) / (s + 1))
    found = []
    for a, b in spans:
        i0, i1 = int(a * fs), int(b * fs)
        if i1 - i0 < fs * 0.05:
            continue
        for ph in range(16):
            pos = i0 + ph / 16 * fs / 1200 + np.arange(int((i1 - i0) / fs * 1200) - 1) * fs / 1200
            pos = pos[pos < i1].astype(int)
            tone = d[pos] > 0  # True: mark
            bits = np.concatenate([[1], (tone[1:] == tone[:-1]).astype(int)])  # NRZI: no change = 1
            t_bits = pos / fs
            for fr, k_flag, k_data in hdlc_frames(bits):
                key, td, tf = bytes(fr), float(t_bits[k_data]), float(t_bits[k_flag])
                same = next((f for f in found if f["frame"] == key and abs(f["t_data"] - td) < 0.1), None)
                if same is None:
                    found.append(dict(frame=key, t_flag=tf, t_data=td))
                else:
                    same["t_flag"] = min(same["t_flag"], tf)
    return sorted(found, key=lambda f: f["t_data"])


def hdlc_frames(bits):
    """[(frame bytes without FCS, index of the first flag of its run, index of its first data bit)] with good FCS."""
    out = []
    n = len(bits)
    s = "".join("1" if b else "0" for b in bits)
    i = s.find("01111110")
    run_start = i
    while i >= 0:
        j = i + 8
        if s.startswith("01111110", j):  # still flags
            i = j
            continue
        k = s.find("01111110", j)
        if k < 0:
            break
        body, ones, out_bits = s[j:k], 0, []
        ok = True
        for c in body:
            if ones == 5:
                if c == "1":
                    ok = False
                    break
                ones = 0
                continue
            out_bits.append(c)
            ones = ones + 1 if c == "1" else 0
        if ok and len(out_bits) >= 16 * 8 and len(out_bits) % 8 == 0:
            by = bytes(int("".join(reversed(out_bits[q:q + 8])), 2) for q in range(0, len(out_bits), 8))
            if crc_x25(by[:-2]) == (by[-2] | by[-1] << 8):
                out.append((by[:-2], run_start, j))
        i = k
        run_start = k
        if i > n:
            break
    return out


def kiss_wrap(cmd, data):
    esc = data.replace(b"\xdb", b"\xdb\xdd").replace(b"\xc0", b"\xdb\xdc")
    return b"\xc0" + bytes([cmd]) + esc + b"\xc0"


# ---- The suite ----


class Modes:
    """The tests, on a bench.Bench (its gates, duty limit, SDR and restore) and a speaker."""

    def __init__(self, bench, cfg, a, audio, sess_dir, kiss_port=None):
        self.b, self.cfg, self.a, self.audio, self.d = bench, cfg, a, audio, sess_dir
        self.r, self.clock, self.out = bench.radio, bench.clock, bench.out
        self.mc = dict(DEFAULTS, **(cfg.get("modes") or {}))
        self.kiss_port = kiss_port
        self.results = []
        self.saved = {}
        self.cal = None
        self.t_last_beacon = None
        self.slots_saved = {}
        self.f_lo, self.f_hi = cfg["test_freqs"][0], cfg["test_freqs"][-1]

    # -- small helpers --
    def say(self, s):
        self.out(s)

    def get(self, path):
        return self.b.setting(path)

    def setv(self, path, value):
        if path not in self.saved:
            self.saved[path] = self.get(path)
        self.r.term(f"set {path} {value}")

    def status(self):
        return self.r.status()

    def ui(self):
        return self.r.ui()

    def home(self):
        for _ in range(2):
            self.r.keys("BACK:long")
            u = self.ui()
            if u.get("screen", {}).get("id") == "Home":
                return u
        self.r.keys("BACK BACK BACK BACK")
        u = self.ui()
        if u.get("screen", {}).get("id") != "Home":
            raise B.BenchError(f"can't get home (on {u.get('screen', {}).get('id')})")
        return u

    def tune(self, letter, mhz, active=True):
        self.r.term(f"slot {letter} vfo {mhz:.5f} fm low tone off")
        if active:
            self.r.term(f"slot {letter} active")

    def gate(self, letter, purpose, allow_aprs=False):
        """bench.py's key gates on slot `letter` (an approved frequency, VFO, simplex, low, not scanning, tx-dry)."""
        n = "ABC".index(letter)
        st = self.r.slot(n)
        g = self.b.gate
        g(f"{purpose}: frequency approved", st["freq10"] in [B.mhz10(x) for x in self.cfg["test_freqs"]],
          f"slot {letter} {st['freq10'] / 1e5:.5f} MHz")
        g(f"{purpose}: on the VFO", st["channel"] is None and (allow_aprs or not st["aprs"]), f"channel {st['channel']}")
        g(f"{purpose}: no offset, no split", st["offset"] == 0 and not st["split"])
        g(f"{purpose}: power low", st["power"] == 0 or (allow_aprs and st["aprs"]), ["low", "mid", "high"][st["power"] % 3])
        g(f"{purpose}: not scanning", not st["scanning"])
        g(f"{purpose}: tx-dry", self.b.tx_dry is not None, self.b.tx_dry or "")
        return st

    def gate_active(self, purpose):
        act = self.status()["active"]
        return self.gate(act, purpose)

    def key_on(self, slot_code):
        """KEY on (LINK-20) with bench.py's TX-43 check: on the cable a tx-dry build answers at once."""
        self.b.keyed = True
        r = self.r.key_on(slot_code)
        if r is None and self.r.cable:
            raise B.SafetyError("KEY on wasn't answered within 0.6 s: a `tx` build holds its answers while keyed "
                                "(TX-43). Unkeyed at once; stopping.")
        return r is None or r[0] == 0, (None if r is None or r[0] == 0 else r[1])

    def mic_tone_then(self, key_s, extra_tail, purpose, lead, secs=5.0):
        """The duty wait for a remote key-up first, then the mic tone (so it plays through the key-up)."""
        self.b.duty.wait(self.b.duty.planned(key_s, extra_tail), "burst", purpose, lead)
        if self.audio:
            db = self.cal["db_key"] if self.cal and self.cal.get("db_key") is not None else -12
            self.audio.play(self.mc["mic_tone_hz"], secs, db)
            self.clock.sleep(0.3)

    def put_back(self):
        """The settings a test changed, back as they were before the suite (the APRS frequency at the end)."""
        for p, v in self.saved.items():
            if p != "aprs_freq":
                with contextlib.suppress(Exception):
                    self.r.term(f"set {p} {v}")

    def keyed_now(self):
        return self.status()["keyed"]

    def watch_keyed(self, until, cap_s, stop, tick=None):
        """Poll the status until `until` (clock time) or a key-up has come and gone: (t_on, t_off, slot). A key-up
        longer than cap_s is stopped at once (stop())."""
        t_on = t_off = slot = None
        while self.clock.time() < until:
            k = self.keyed_now()
            now = self.clock.time()
            if k and t_on is None:
                t_on, slot = now, k
            if t_on is not None and not k:
                t_off = now
                break
            if tick:
                tick(now, t_on)
            if t_on is not None and now - t_on > cap_s:
                self.say(f"  keyed {now - t_on:.1f} s, over the {cap_s:.1f} s admitted: stopping it")
                stop()
                t_off = self.clock.time()
                break
            self.clock.sleep(0.1)
        return t_on, t_off, slot

    def stop_all(self):
        """Whatever keyed: the tone off, VOX off, KEY off."""
        with contextlib.suppress(Exception):
            self.audio.stop()
        if S["vox"] in self.saved:  # only a VOX this suite turned on
            with contextlib.suppress(Exception):
                self.r.term(f"set {S['vox']} Off")
        with contextlib.suppress(Exception):
            self.r.key_off()

    def radio_keyup(self, purpose, planned_s, start, mhz=None, total_s=None, lead=0.8, path=None, cap_s=None,
                    settle_s=0.0, tick=None):
        """A key-up the radio makes itself, started by start(): admitted first with its longest length, watched over
        the status (stopped if longer), logged with what it was; with mhz the SDR records it. Returns (ev, raw)."""
        duty = self.b.duty
        cap = cap_s or (planned_s - duty.c["unkey_tail_s"])
        total = total_s or (lead + planned_s + settle_s + 1.5)
        duty.wait(planned_s, "burst", purpose, lead if mhz else 0.0)
        if mhz:
            self.b.sdr.start(mhz, total, path or os.path.join(self.d, B.slug(purpose) + ".u8"))
        t0 = self.clock.time()
        raw = None
        try:
            if mhz:
                self.clock.sleep(lead)
            duty.admit(planned_s, "burst", purpose)
            t_sent = self.clock.time()
            start()
            t_on, t_off, slot = self.watch_keyed(t_sent + planned_s + settle_s + 1.0, cap, self.stop_all, tick)
            if t_on is not None and t_off is None:
                self.stop_all()
                t_off = self.clock.time()
            ev = dict(accepted=t_on is not None, t_on=t_on, t_off=t_off, t_unkeyed=t_off, slot=slot, t_capture=t0)
            duty.record(purpose, "burst", ev, t_on if t_on is not None else t_sent)
            if mhz:
                self.clock.sleep(max(t0 + total - self.clock.time(), 0))
                raw = self.b.sdr.finish()
        except BaseException:
            self.stop_all()
            if mhz:
                self.b.sdr.stop()
            raise
        return ev, raw

    def add(self, name, checks, summary, extra=None):
        ok = bool(checks) and all(c["ok"] for c in checks)
        bad = [c for c in checks if not c["ok"]]
        scope = bench_rules.tags(name)
        t = dict(name=name, module="bench_modes", rules=list(scope), scope=scope, result="pass" if ok else "fail",
                 checks=checks, summary=summary, why="; ".join(f"{c['what']}: got {c['got']}" for c in bad[:3]))
        if extra:
            t["data"] = B.jsonable(extra)
        self.results.append(t)
        self.say(f"  {'pass' if ok else 'FAIL'}  {name}: {summary}" + (f"  ({t['why']})" if not ok else ""))
        return ok

    def keep(self, path, ok):
        if ok and not self.a.keep_captures:
            with contextlib.suppress(OSError):
                os.remove(path)

    # -- the calibration (no key-up) --
    def calibrate(self):
        """VOX off; the ambient, then a tone at each level: `mic`'s mean (>> 4: what VOX compares); the decay after
        the loudest that keys. Picks the keying level, the VOX level for it and a mid level for VOX-14.2."""
        mc = self.mc
        self.setv(S["vox"], "Off")
        amb = parse_mic(self.r.term("mic"))["reading"]
        table = []
        for db in mc["levels_db"]:
            self.audio.play(mc["mic_tone_hz"], mc["cal_tone_s"], db)
            self.clock.sleep(mc["cal_tone_s"] * 0.6)
            table.append((db, parse_mic(self.r.term("mic"))["reading"]))
            self.audio.wait()
            self.clock.sleep(0.6)
        self.say("  calibration: ambient " + str(amb) + "; " + ", ".join(f"{db:+d} dBFS {v}" for db, v in table))
        cal = dict(ambient=amb, table=table)
        m = mc["vox_margin"]
        # The keying tone: the quietest whose reading is m over level 1's threshold; the VOX level: the highest whose
        # threshold the tone clears by m and the ambient stays m under its off level.
        key = next(((db, v) for db, v in table if v >= vox_threshold(1) + m), None)
        if key is None:
            cal["why"] = (f"the loudest tone reads {max(v for _, v in table)}, under VOX level 1's threshold "
                          f"{vox_threshold(1)} + {m}: move the speaker closer or raise the Mac's volume")
            self.cal = cal
            return cal
        db_key, v_key = max(table, key=lambda x: x[1])  # the loudest: the most margin
        lv = max((L for L in range(1, 10) if vox_threshold(L) + m <= v_key and amb + m < vox_threshold(L) - 8),
                 default=None)
        if lv is None:
            cal["why"] = f"no VOX level between the ambient ({amb}) and the tone ({v_key}) with {m} counts each side"
            self.cal = cal
            return cal
        mids = [(db, v) for db, v in table if vox_threshold(1) + 4 <= v <= vox_threshold(9) - 12]
        mid = min(mids, key=lambda x: abs(x[1] - vox_threshold(5))) if mids else None
        # The envelope's fall after the keying tone stops (VOX off still): to under the level's off threshold.
        self.audio.play(mc["mic_tone_hz"], 1.0, db_key)
        self.audio.wait()
        t_end = self.clock.time()
        decay = None
        while self.clock.time() - t_end < 4.0:
            if parse_mic(self.r.term("mic"))["reading"] < vox_threshold(lv) - 8:
                decay = self.clock.time() - t_end
                break
            self.clock.sleep(0.1)
        cal.update(db_key=db_key, v_key=v_key, level=lv, mid=mid, decay_s=decay)
        if decay is None or decay > mc["decay_max_s"]:
            cal["why"] = f"the mic envelope takes {'over 4' if decay is None else f'{decay:.1f}'} s to fall: VOX key-ups wouldn't fit 5 s"
        self.say(f"  calibration: keying tone {db_key:+d} dBFS (reads {v_key}), VOX level {lv} (threshold "
                 f"{vox_threshold(lv)}), envelope falls in {_s(decay)}; mid tone {mid}")
        self.cal = cal
        return cal

    def need_vox(self):
        if not self.audio:
            raise Skip("no speaker (--no-audio, or afplay not found)")
        if self.cal is None:
            self.calibrate()
        if self.cal.get("why"):
            raise Skip(self.cal["why"])

    def vox_on(self, level=None, tail=None):
        self.setv(S["vox_level"], str(level or self.cal["level"]))
        self.setv(S["vox_tail"], tail or self.mc["vox_tail_short"])
        self.setv(S["vox"], "On")

    def tail_s(self, tail):
        return float(tail.split()[0])

    def hold_s(self):
        """TX-47.2: the carrier held past the unkey (0.5 s on tx-dry); the status reads keyed through it."""
        return self.cfg["sweeps"]["endtones"].get("hold_s", 0.5)

    def vox_planned(self, tone_s, tail):
        """A VOX key-up's longest length: the tone, the envelope's fall, the tail, the unkey's carrier hold and a
        0.3 s margin, plus the duty limit's unkey tail. Admitted whole (the run admitted it without the
        hold and stopped it before the tail ended)."""
        return (tone_s + (self.cal.get("decay_s") or 0) + self.tail_s(tail) + self.hold_s() + 0.3
                + self.b.duty.c["unkey_tail_s"])

    def tone_for(self, tail, want=2.0):
        """The longest tone up to `want` s whose VOX key-up fits the duty limit's burst."""
        room = self.b.duty.c["max_burst_s"] - self.vox_planned(0, tail)
        return min(want, room)

    # -- VOX --
    def t_vox_keys_and_tail(self):
        """VOX-5 (and VOX-13's active slot): VOX keyed by the tone from home; the SDR's carrier ends the tail
        (0 8 2 3) after the envelope falls under the off level; B active on 446.475, A on 145.495: B's frequency."""
        self.need_vox()
        tail = self.mc["vox_tail_long"]
        tone = self.tone_for(tail, 1.0)
        if tone < 0.6:
            tail = "1 s"
            tone = self.tone_for(tail, 1.0)
        if tone < 0.6:
            raise Skip(f"a VOX key-up with a {tail} tail and the envelope's {self.cal['decay_s']:.1f} s fall doesn't fit 5 s")
        self.tune("A", self.f_lo, active=False)
        self.tune("B", self.f_hi)
        self.home()
        self.gate("B", "vox keys B")
        self.vox_on(tail=tail)
        planned = self.vox_planned(tone, tail)
        path = os.path.join(self.d, "vox-tail.u8")
        ev, raw = self.radio_keyup("vox tail", planned, lambda: self.audio.play(self.mc["mic_tone_hz"], tone,
                                                                                self.cal["db_key"]),
                                   mhz=self.f_hi, path=path)
        self.r.term(f"set {S['vox']} Off")
        t_tone_end = self.audio.wait()
        runs = B.carrier_runs(raw)
        c = []
        c.append(_chk("VOX keyed (status)", ev["accepted"], ev.get("slot")))
        c.append(_chk("keyed the active slot B (VOX-13)", ev.get("slot") == "B", ev.get("slot")))
        c.append(_chk("a carrier on 446.475 MHz (B's) on the SDR", bool(runs), runs))
        ok_tail = None
        if runs and t_tone_end:
            drop = ev["t_capture"] + runs[-1][1] - t_tone_end
            hold = self.cfg["sweeps"]["endtones"]["hold_s"]  # TX-47.2 on tx-dry: the carrier 0.5 s past the unkey
            want = self.tail_s(tail) + (self.cal["decay_s"] or 0) + hold
            ok_tail = abs(drop - want) <= self.mc["vox_tail_tol_s"]
            c.append(_chk(f"carrier ends {tail} after the envelope falls, + the {hold:g} s unkey hold ({want:.2f} s after the tone, "
                          f"±{self.mc['vox_tail_tol_s']})", ok_tail, f"{drop:.2f} s"))
        ok = self.add("vox_tail", [x for x in c if "VOX-13" not in x["what"]],
                      f"tail {tail}: carrier ended {_s(ev['t_capture'] + runs[-1][1] - t_tone_end if runs and t_tone_end else None)} "
                      f"after the tone (envelope fall {_s(self.cal['decay_s'])})")
        self.add("vox_active_slot", c[:3], f"VOX keyed {ev.get('slot')}, carrier on 446.475: {bool(runs)}")
        self.keep(path, ok)

    def t_vox_levels(self):
        """VOX-14.2 (VOX-14.1 in part): one tone loudness, VOX level 1 to 9: keys up to some level and not above;
        that level against the one the calibration's reading predicts (VOX-14.1's thresholds)."""
        self.need_vox()
        if not self.cal.get("mid"):
            raise Skip("no calibration tone reads between level 1's and level 9's thresholds")
        db, v = self.cal["mid"]
        tail = self.mc["vox_tail_short"]
        tone = self.tone_for(tail, 1.2)
        self.tune("A", self.f_lo)
        self.home()
        self.gate("A", "vox levels")
        keyed = {}
        for L in range(1, 10):
            self.vox_on(level=L, tail=tail)
            ev, _ = self.radio_keyup(f"vox level {L}", self.vox_planned(tone, tail),
                                     lambda: self.audio.play(self.mc["mic_tone_hz"], tone, db))
            self.audio.wait()
            keyed[L] = bool(ev["accepted"])
            self.r.term(f"set {S['vox']} Off")
            self.clock.sleep(0.5)
        top = max((L for L in keyed if keyed[L]), default=0)
        mono = all(keyed[L] for L in range(1, top + 1)) and not any(keyed[L] for L in range(top + 1, 10))
        pred = max((L for L in range(1, 10) if vox_threshold(L) <= v), default=0)
        self.add("vox_levels", [_chk("keys up to a level and none above it", mono, keyed),
                                _chk("keys at level 1, not at level 9", keyed[1] and not keyed[9], keyed)],
                 f"tone reading {v}: keyed at levels {[L for L in keyed if keyed[L]]}")
        self.add("vox_thresholds", [_chk(f"the last level that keys is the one reading {v} predicts (±1)",
                                         abs(top - pred) <= 1, f"{top}, predicted {pred}")],
                 f"keys to level {top}; reading {v} predicts {pred}")

    def t_vox_home_only(self):
        """VOX-3.1 (part: the screens of config vox_screens): the tone from home keys; in each screen it doesn't, the
        VOX group (0 8 2) included."""
        self.need_vox()
        tail = self.mc["vox_tail_short"]
        tone = self.tone_for(tail, 2.0)
        self.tune("A", self.f_lo)
        self.home()
        self.gate("A", "vox home")
        self.vox_on(tail=tail)
        c = []
        ev, _ = self.radio_keyup("vox from home", self.vox_planned(tone, tail),
                                 lambda: self.audio.play(self.mc["mic_tone_hz"], tone, self.cal["db_key"]))
        self.audio.wait()
        c.append(_chk("from home: keyed", ev["accepted"], ev.get("slot")))
        for keys, want in self.mc["vox_screens"]:
            self.home()
            self.clock.sleep(self.mc["vox_hold_s"] + 0.5)
            self.r.keys(keys)
            sid = self.ui().get("screen", {}).get("id")
            ev, _ = self.radio_keyup(f"vox on {want}", self.vox_planned(tone, tail),
                                     lambda: self.audio.play(self.mc["mic_tone_hz"], tone, self.cal["db_key"]))
            self.audio.wait()
            c.append(_chk(f"`{keys}` ({sid}): no key-up", not ev["accepted"], ev.get("slot") or "none"))
            if sid != want:
                c.append(_chk(f"`{keys}` opened {want}", False, sid))
        self.r.term(f"set {S['vox']} Off")
        self.home()
        self.add("vox_home_only", c, f"keyed from home; held on {', '.join(w for _, w in self.mc['vox_screens'])}")

    def t_vox_hold_after_menu(self):
        """VOX-3.5: the tone kept on as a menu closes: the key-up comes 1.5 s after home."""
        self.need_vox()
        tail = self.mc["vox_tail_short"]
        hold = self.mc["vox_hold_s"]
        keyed_want = 0.8
        tone = 1.0 + hold + keyed_want
        planned = keyed_want + (self.cal["decay_s"] or 0) + self.tail_s(tail) + self.hold_s() + 0.3 + self.b.duty.c["unkey_tail_s"] + 0.4
        self.tune("A", self.f_lo)
        self.home()
        self.gate("A", "vox hold")
        self.vox_on(tail=tail)
        self.r.keys("MENU")
        t_home = []

        def start():
            self.audio.play(self.mc["mic_tone_hz"], tone, self.cal["db_key"])
            self.clock.sleep(1.0)
            self.r.keys("BACK")
            t_home.append(self.clock.time())

        ev, _ = self.radio_keyup("vox hold", planned, start, cap_s=planned)
        self.audio.wait()
        self.r.term(f"set {S['vox']} Off")
        dt = ev["t_on"] - t_home[0] if ev["accepted"] and t_home else None
        self.add("vox_hold", [_chk("keyed once home", ev["accepted"]),
                              _chk(f"the key-up {hold} s after home (±{self.mc['vox_hold_tol_s']})",
                                   dt is not None and abs(dt - hold) <= self.mc["vox_hold_tol_s"], _s(dt))],
                 f"keyed {_s(dt)} after BACK reached home")

    def t_vox_blocked(self):
        """VOX-3.2 (part: the broadcast playing, a slot's monitor open; a beep is too short to hold a tone over):
        the tone at the mic keys nothing."""
        self.need_vox()
        tail = self.mc["vox_tail_short"]
        tone = self.tone_for(tail, 2.0)
        c = []
        self.tune("A", self.f_lo)
        self.r.term(f"slot B vfo {self.mc['bcast_mhz']:.5f}")
        self.home()
        self.gate("A", "vox blocked")
        b = next((x for x in self.ui().get("cards", []) if x.get("slot") == "B"), {})
        self.vox_on(tail=tail)
        ev, _ = self.radio_keyup("vox under the broadcast", self.vox_planned(tone, tail),
                                 lambda: self.audio.play(self.mc["mic_tone_hz"], tone, self.cal["db_key"]))
        self.audio.wait()
        c.append(_chk(f"broadcast playing on B ({b.get('state')}): no key-up", not ev["accepted"] and b.get("state") == "FM",
                      f"{ev.get('slot') or 'none'}; B {b.get('state')}"))
        self.r.term(f"slot B vfo {self.f_hi:.5f}")
        self.r.keys("SIDE1")  # A's monitor: squelch open
        ev, _ = self.radio_keyup("vox under the monitor", self.vox_planned(tone, tail),
                                 lambda: self.audio.play(self.mc["mic_tone_hz"], tone, self.cal["db_key"]))
        self.audio.wait()
        self.r.keys("SIDE1")
        c.append(_chk("A's monitor open: no key-up", not ev["accepted"], ev.get("slot") or "none"))
        self.r.term(f"set {S['vox']} Off")
        self.add("vox_blocked", c, "no key-up under the broadcast or an open monitor")

    def t_vox_boot(self):
        """VOX-3.3: VOX on, the tone playing through a restart: no carrier for 5 s after the restart (the SDR); then
        it keys (the tone is loud enough). The tone is cut 4.5 s after REBOOT unless the slot gated again."""
        self.need_vox()
        if not hasattr(self.r, "reboot"):
            raise Skip("this link can't restart the radio")
        tail = self.mc["vox_tail_short"]
        hold = self.mc["boot_hold_s"]
        self.tune("A", self.f_lo)
        self.home()
        self.gate("A", "vox boot")
        self.vox_on(tail=tail)
        self.clock.sleep(3.0)  # the store writes the slot and VOX before the restart
        keyed_want = 0.5
        planned = (keyed_want + 0.6 + (self.cal["decay_s"] or 0) + self.tail_s(tail) + self.hold_s() + 0.3
                   + self.b.duty.c["unkey_tail_s"])
        tone = 4.5 + hold + keyed_want + 4.0
        path = os.path.join(self.d, "vox-boot.u8")
        mark = {}

        def start():
            # REBOOT first and the tone from its answer on: a tone started before it lets VOX key the radio, and a
            # keyed radio refuses REBOOT (LINK-24.3). The tone plays through the restart.
            if self.keyed_now():
                raise B.BenchError("keyed before the restart (the last key-up's tail?): not restarting")
            mark["reboot"] = self.clock.time()
            mark["hello"] = self.r.reboot(after=lambda: self.audio.play(self.mc["mic_tone_hz"], tone, self.cal["db_key"]))
            if mark["hello"] - mark["reboot"] > 4.5:
                self.audio.stop()
                raise Skip("the radio took over 4.5 s to answer after the restart: the tone was cut")
            self.gate("A", "vox boot, after the restart")

        def tick(now, t_on):
            # The tone stops keyed_want s into the key-up, or keyed_want s after the latest the hold can end.
            if (t_on is not None and now - t_on >= keyed_want) or now >= mark["hello"] + hold + keyed_want:
                self.audio.stop()

        ev, raw = self.radio_keyup("vox boot", planned, start, mhz=self.f_lo, path=path, lead=0.8,
                                   total_s=0.8 + 0.5 + 4.5 + hold + keyed_want + 4.0, settle_s=hold + 5.0,
                                   cap_s=planned + 1.0, tick=tick)
        self.r.term(f"set {S['vox']} Off")
        runs = B.carrier_runs(raw)
        # Only carriers that start at REBOOT or later: the capture's lead can hold the tail of the key-up before
        # (the run counted one 0.86 s before REBOOT).
        t_rb = mark["reboot"] - ev["t_capture"] if "reboot" in mark else 0.0
        before = [r for r in runs if r[0] < t_rb - 0.05]
        after = [r for r in runs if r[0] >= t_rb - 0.05]
        dt = after[0][0] - t_rb if after else None
        ok = self.add("vox_boot", [_chk(f"no carrier within {hold} s of the restart", dt is None or dt >= hold, _s(dt)),
                                   _chk("then VOX keys (the tone reaches it)", dt is not None, after)],
                      f"first carrier {_s(dt)} after REBOOT (HELLO again {mark.get('hello', 0) - mark.get('reboot', 0):.1f} s after)"
                      + (f"; {len(before)} carrier(s) before REBOOT not counted" if before else ""))
        self.keep(path, ok)

    # -- DTMF and the end tone (the mic tone shows the mic muted) --
    def dial(self, digits, purpose, speed, delay, mhz, path, tone=True):
        """The dialer's Send (DTMF-13) with the SDR on; the mic tone playing through it."""
        self.home()
        self.setv(S["dtmf_speed"], f"{speed} ms")
        self.setv(S["dtmf_delay"], f"{delay} ms")
        self.r.keys(self.mc["dialer_keys"])
        sid = self.ui().get("screen", {}).get("id")
        self.r.keys(" ".join(digits))
        planned = (delay + 2 * speed * len(digits)) / 1000 + 0.6 + self.b.duty.c["unkey_tail_s"]

        def start():
            if tone and self.audio:
                self.audio.play(self.mc["mic_tone_hz"], planned + 1.0, self.cal["db_key"] if self.cal and self.cal.get("db_key") is not None else -12)
            self.r.keys("MENU")

        ev, raw = self.radio_keyup(purpose, planned, start, mhz=mhz, path=path, lead=0.8)
        if self.audio:
            self.audio.stop()
        ev["dialer"] = sid
        if not ev["accepted"]:  # a refusal says why in a toast
            ev["toast"] = self.ui().get("toast")
        return ev, raw

    def t_dtmf(self):
        """DTMF-1 (part), DTMF-2, DTMF-3, TONE-11 (part): the dialer at each (speed, first digit delay): the SDR
        decodes the digits; each on and off for the speed; the first the delay after the carrier; both tones of each
        pair; no mic tone between the digits; no end tone after (end tone set to Beep)."""
        self.tune("A", self.f_lo)
        self.home()
        self.setv(S["vox"], "Off")
        self.setv(S["end_tone"], "Beep")
        digits = self.mc["dtmf_digits"]
        mic_ok = None
        if self.audio:
            # The mic tone reaches the air on a voice key-up (so its absence in a dial means the mic is muted).
            self.gate("A", "mic tone")
            self.mic_tone_then(1.5, 0.0, "mic tone", 0.8, 4.0)
            path = os.path.join(self.d, "mic-tone.u8")
            ev, raw = self.b.capture_keyed(self.f_lo, 1.5, path, "mic tone", lead=0.8, tail=1.2)
            self.audio.stop()
            dev, fs, on, runs = air(raw)
            e = env_ms(dev, fs, self.mc["mic_tone_hz"])
            mic_lvl = float(np.percentile(e[on[::int(fs / 1000)][:len(e)]], 75)) if on.any() else 0.0
            mic_ok = mic_lvl > 150
            self.say(f"  mic tone on a voice key-up: {mic_lvl:.0f} Hz deviation")
            self.keep(path, mic_ok)
        decoded_all, timing, pairs, muted, no_end = [], [], [], [], []
        for speed, delay in self.mc["dtmf_runs"]:
            self.gate("A", f"dtmf {speed}/{delay}")
            path = os.path.join(self.d, f"dtmf-{speed}-{delay}.u8")
            ev, raw = self.dial(digits, f"dtmf {speed} ms {delay} ms", speed, delay, self.f_lo, path)
            dev, fs, on, runs = air(raw)
            got = dtmf_decode(dev, fs)
            s = "".join(g[0] for g in got)
            # Which step failed: the dialer not open, Send not keyed (status), no carrier, or nothing decoded.
            if ev.get("dialer") != self.mc["dialer_id"]:
                s = f"(the dialer didn't open: `{self.mc['dialer_keys']}` gave {ev.get('dialer')})"
            elif not ev["accepted"]:
                s = f"(Send didn't key: {ev.get('toast') or 'no toast'})"
            elif not runs:
                s = "(keyed by the status, no carrier on the SDR)"
            decoded_all.append((speed, delay, s))
            ok = s == digits
            r = dict(speed=speed, delay=delay, decoded=s)
            if ok and runs:
                ons = [g[2] - g[1] for g in got]
                offs = [got[i + 1][1] - got[i][2] for i in range(len(got) - 1)]
                first = got[0][1] - runs[0][0]
                tol = self.mc["dtmf_ms_tol"] / 1000
                r.update(on_ms=[round(x * 1000) for x in ons], off_ms=[round(x * 1000) for x in offs],
                         first_ms=round(first * 1000))
                timing.append(_chk(f"{speed} ms: each digit on {speed} ms (±{self.mc['dtmf_ms_tol']})",
                                   all(abs(x - speed / 1000) <= tol for x in ons), r["on_ms"]))
                timing.append(_chk(f"{speed} ms: each gap {speed} ms", all(abs(x - speed / 1000) <= tol for x in offs), r["off_ms"]))
                timing.append(_chk(f"first digit {delay} ms after the carrier (±{self.mc['dtmf_delay_tol']})",
                                   abs(first * 1000 - delay) <= self.mc["dtmf_delay_tol"], r["first_ms"]))
                tw = [20 * math.log10(g[4] / g[3]) for g in got]
                pairs.append(_chk(f"{speed} ms: both tones of each pair (twist within 6 dB)", all(abs(x) <= 6 for x in tw),
                                  [round(x, 1) for x in tw]))
                if mic_ok:
                    e = env_ms(dev, fs, self.mc["mic_tone_hz"])
                    a, b = int(got[0][1] * 1000), int(got[-1][2] * 1000)
                    lvl = float(np.percentile(e[a:b], 90)) if b > a else 0.0
                    muted.append(_chk(f"{speed} ms: no mic tone from the first digit to the last", lvl < 0.1 * mic_lvl,
                                      f"{lvl:.0f} Hz (voice key-up {mic_lvl:.0f})"))
                # TONE-11: nothing after the last digit but the carrier's end
                tail_dev = dev[int(got[-1][2] * fs) + int(0.03 * fs): int(runs[-1][1] * fs) - int(0.03 * fs)]
                rms = float(np.sqrt(np.mean(tail_dev ** 2))) if len(tail_dev) else 0.0
                no_end.append(_chk(f"{speed} ms: no end tone after the digits", rms < 300, f"{rms:.0f} Hz rms"))
            timing_ok = all(x["ok"] for x in timing)
            self.keep(path, ok and timing_ok)
        self.add("dtmf_dial", [_chk(f"{sp}/{dl} ms: decoded {digits}", s == digits, s) for sp, dl, s in decoded_all],
                 "decoded " + ", ".join(f"{s!r} at {sp} ms" for sp, dl, s in decoded_all))
        if timing:
            self.add("dtmf_timing", timing, "on/off and first digit delay per speed")
        if pairs:
            self.add("dtmf_pairs", pairs + (muted if muted else [_chk("the mic tone heard on a voice key-up", False, "no speaker or not heard")]),
                     "pairs" + (", mic muted between digits" if muted else "; mic not checked"))
        if no_end:
            self.add("tone11_no_end_after_dtmf", no_end, "no end tone after DTMF dialling")

    def t_end_tone_order(self):
        """TONE-8 (part), TONE-11 (part): end tone Beep, a remote key-up with the mic tone: after KEY off the end
        tone, no mic tone while it plays, then the carrier drops."""
        self.setv(S["end_tone"], "Beep")
        self.tune("A", self.f_lo)
        self.home()
        self.gate("A", "end tone")
        self.mic_tone_then(1.5, 0.8, "end tone", 0.8, 4.5)
        path = os.path.join(self.d, "end-tone.u8")
        ev, raw = self.b.capture_keyed(self.f_lo, 1.5, path, "end tone", lead=0.8, tail=1.8, extra_tail=0.8)
        if self.audio:
            self.audio.stop()
        dev, fs, on, runs = air(raw)
        e = env_ms(dev, fs, 1000)
        c = []
        t_off = ev["t_off"] - ev["t_capture"] if ev.get("t_off") else None
        # The beep: 1000 Hz standing well over its own level in the keyed part before KEY off (the mic tone's
        # leakage, noise), inside the carrier. Half the capture's peak alone (the run) took noise for it
        # when the beep was weak: "-2.52 s after KEY off".
        beep = np.array([], int)
        if runs and t_off is not None and len(e):
            a, b = int(runs[0][0] * 1000) + 100, int((t_off - 0.1) * 1000)
            base = float(np.percentile(e[a:b], 90)) if b > a else 0.0
            ms = np.arange(len(e)) / 1000
            inside = np.zeros(len(e), bool)
            for r0, r1 in runs:
                inside |= (ms >= r0 + 0.01) & (ms < r1 - 0.01)
            beep = np.nonzero(inside & (e > max(300.0, 3 * base)))[0]
        if len(beep) and runs:
            b0, b1 = beep[0] / 1000, beep[-1] / 1000
            c.append(_chk("the end tone after KEY off", t_off is not None and b0 >= t_off - 0.05, f"{b0 - (t_off or 0):.2f} s"))
            c.append(_chk("the end tone before the carrier drops", b1 < runs[-1][1], f"{runs[-1][1] - b1:.2f} s"))
            if self.audio:
                m = env_ms(dev, fs, self.mc["mic_tone_hz"])
                during = float(np.percentile(m[int(b0 * 1000):int(b1 * 1000)], 90))
                before = float(np.median(m[int(runs[0][0] * 1000) + 200:int((t_off or 0) * 1000) - 100])) if t_off else 0
                c.append(_chk("the mic muted while it plays", during < 0.15 * max(before, 1) and before > 150,
                              f"{during:.0f} Hz during, {before:.0f} Hz before"))
        else:
            c.append(_chk("an end tone on the SDR", False, "none"))
        ok = self.add("end_tone_order", c, "end tone after KEY off, before the drop" + (", mic muted" if self.audio else ""))
        self.keep(path, ok)
        # TONE-11: none after `txtest afsk` (a data key-up)
        self.gate("A", "afsk end tone")
        armed = self.r.term("txtest afsk data 100")
        path = os.path.join(self.d, "afsk-end.u8")
        ev, raw = self.b.capture_keyed(self.f_lo, 1.5, path, "afsk end tone", lead=0.8, tail=1.8, extra_tail=0.8)
        with contextlib.suppress(Exception):
            self.r.term("txtest off")
        dev, fs, on, runs = air(raw)
        t_off = ev["t_off"] - ev["t_capture"] if ev.get("t_off") else 0
        e = env_ms(dev, fs, 1000)
        a = int((t_off + 0.1) * 1000)
        b = int(runs[-1][1] * 1000) - 20 if runs else a
        lvl = float(np.percentile(e[a:b], 90)) if b > a else 0.0
        ok = self.add("tone11_no_end_after_afsk", [_chk("armed", "armed" in armed, armed.strip()[:60]),
                                                   _chk("no 1000 Hz after the AFSK key-up's KEY off", lvl < 300, f"{lvl:.0f} Hz")],
                      "no end tone after txtest afsk")
        self.keep(path, ok)

    def t_reg40(self):
        """TONE-19.3 (part: the end tone): `bk 0 40` read again and again from KEY off: 0x43C (low 12 bits) while
        the end tone plays, 0x50A before KEY off and after it."""
        self.setv(S["end_tone"], "Quindar")
        et = self.get(S["end_tone"])
        if not et.lower().startswith("quindar"):
            raise Skip(f"the end tone reads {et!r}, not Quindar")
        tone_s = sum(ms for _, ms in dict(B.END_TONES)["quindar"]) / 1000
        # Reads fast enough to land inside the tone at least once: twice in its length.
        t_r = self.clock.time()
        for _ in range(4):
            parse_bk(self.r.term("bk 0 40"))
        every = (self.clock.time() - t_r) / 4
        if every > tone_s / 2:
            raise Skip(f"`bk 0 40` takes {every * 1000:.0f} ms a read over this link: too slow for the {tone_s * 1000:.0f} ms "
                       "Quindar tone (run it on the cable)")
        self.tune("A", self.f_lo)
        self.home()
        self.gate("A", "reg40")
        reads = []
        duty = self.b.duty
        planned = duty.planned(1.5, 0.8)
        duty.admit(planned, "burst", "reg40")
        t_sent = self.clock.time()
        ev = dict(accepted=None)
        try:
            ev["accepted"], ev["refused"] = self.key_on(self.b.slot_code)
            end = t_sent + 1.5
            while self.clock.time() < end:
                self.r.key_alive()
                if self.clock.time() > t_sent + 0.8 and not any(x[1] == "keyed" for x in reads):
                    reads.append((self.clock.time(), "keyed", parse_bk(self.r.term("bk 0 40")) & 0xFFF))
                self.clock.sleep(0.3)
        finally:
            self.r.key_off()
            ev["t_off"] = self.clock.time()
            self.b.keyed = False
            duty.record("reg40", "burst", ev, t_sent, 0.8)
        while self.clock.time() < ev["t_off"] + 1.5:
            reads.append((self.clock.time() - ev["t_off"], "after off", parse_bk(self.r.term("bk 0 40")) & 0xFFF))
            self.clock.sleep(0.04)
        late = parse_bk(self.r.term("bk 0 40")) & 0xFFF
        tone = [v for t, k, v in reads if k == "after off" and t <= 1.0]  # the tone's start after KEY off varies
        first = min((t for t, k, _ in reads if k == "after off"), default=None)
        if 0x43C not in tone and (first is None or first > tone_s + self.hold_s()):
            # The cable sends nothing while the radio is keyed (TX-43), the end tone and the carrier hold included:
            # the first answer came after the tone, so it can't be read this way.
            raise Skip(f"the first `bk 0 40` after KEY off answered {_s(first)} after it, past the Quindar tone and the "
                       "carrier hold (the cable waits while keyed, TX-43): not readable over this link")
        self.add("reg40_end_tone", [_chk("keyed (voice): 0x50A", any(v == 0x50A for _, k, v in reads if k == "keyed"),
                                         [hex(v) for _, k, v in reads if k == "keyed"]),
                                    _chk("0x43C read while the end tone plays", 0x43C in tone, [hex(v) for v in tone]),
                                    _chk("0x50A after it", late == 0x50A, hex(late))],
                 f"{len(reads)} reads after KEY off: " + " ".join(sorted({hex(v) for _, _, v in reads})))

    # -- APRS and KISS --
    def type_freq(self, keys_open, mhz):
        self.home()
        self.r.keys(keys_open)
        w = f"{mhz:.3f}".rstrip("0").rstrip(".")
        self.r.keys(" ".join("STAR" if ch == "." else ch for ch in w) + " MENU")
        self.home()

    def aprs_freq_ready(self):
        """The APRS frequency (0 5 4) on 145.495. The terminal can't set it (`set` says it keeps its default) and
        the entry by keys didn't take on the radio (the frequency picker in ui-backlog will fix it): left
        alone when it already reads 145.495 (the radios were left so), else the keys tried once, else a hand step."""
        def reads():
            m = re.match(r"[\d.]+", self.get(S["aprs_freq"]))
            return float(m.group()) if m else None
        if reads() is not None and abs(reads() - self.f_lo) < 1e-6:
            return
        if "aprs_freq" not in self.saved:
            self.saved["aprs_freq"] = self.get(S["aprs_freq"])
        self.type_freq(self.mc["aprs_freq_keys"], self.f_lo)
        now = reads()
        if now is None or abs(now - self.f_lo) > 1e-6:
            raise Skip(f"the APRS frequency reads {now} after the keys {self.mc['aprs_freq_keys']!r}: set MENU 0 5 4 "
                       f"to {self.f_lo:.3f} on the radio by hand, then run it again")

    def ax25_call(self, what):
        """The callsign for AX.25 (INFO's): 6 characters at most, or the test can't run (SETUP-13: APRS refuses a
        longer one, and an AX.25 address holds 6; VE7TEST went out as VE7TES)."""
        call = self.b.info.get("callsign")
        if not call:
            raise Skip(f"no callsign set ({what})")
        if len(call) > 6:
            raise Skip(f"the callsign {call} is over 6 characters: AX.25 carries 6 (SETUP-13); set a 6-character "
                       "call at MENU 0 8 6 1 for the APRS and KISS tests")
        return call

    def aprs_setup(self):
        """APRS mode on, the APRS frequency 145.495 (by keys; read back), a callsign and a position; slot C gated."""
        call = self.ax25_call("APRS-8")
        src = self.get(S["pos_source"])
        if src.lower().startswith("fixed") and self.get(S["fixed"]).lower() == "none":
            raise Skip("position source Fixed with no fixed position")
        if src.lower().startswith("gps") and "fix" not in self.r.term("gps").lower():
            raise Skip("no GPS fix and no fixed position: set 0 5 5 4 Fixed and 0 5 5 5 on the radio, or run by a window")
        self.aprs_freq_ready()
        self.setv(S["beacon_mode"], "Off")
        self.setv(S["after_voice"], "Off")
        self.setv(S["beacon_power"], "Low")
        self.setv(S["aprs_mode"], "On")
        self.gate("C", "aprs: slot C", allow_aprs=True)
        return call

    def check_frame(self, f, call):
        p = ax25_parse(f["frame"])
        if not p:
            return [_chk("an AX.25 frame", False, f["frame"][:20])]
        return [_chk("from our callsign", p["src"].split("-")[0] == call.upper(), p["src"]),
                _chk("a UI frame, no layer 3", p["control"] == 3 and p["pid"] == 0xF0, (p["control"], p["pid"])),
                _chk("a position report", p["info"][:1] in (b"!", b"=", b"/", b"@", b"`", b"'"), p["info"][:30])]

    def beacon_now(self, purpose, mhz, path):
        planned = 2.0 + self.b.duty.c["unkey_tail_s"]

        def start():
            self.home()
            self.r.keys(self.mc["beacon_now_keys"])
            sel = (self.ui().get("sel") or {}).get("name")
            if sel != "Beacon now":
                raise Skip(f"`{self.mc['beacon_now_keys']}` selected {sel!r}, not Beacon now")
            self.r.keys("MENU")

        return self.radio_keyup(purpose, planned, start, mhz=mhz, path=path, lead=0.8, total_s=0.8 + planned + 2.5)

    def t_aprs_beacon(self):
        """APRS-14.1, APRS-20: A active on 446.475, C (APRS) on 145.495; Beacon now: a frame on 145.495 (C's) from
        our callsign, the TX delay (0 5 9 3) between the carrier and the first flag; beacon power Low."""
        call = self.aprs_setup()
        self.tune("A", self.f_hi)
        self.gate("A", "aprs: A elsewhere")
        txd = int(re.match(r"\d+", self.get(S["tx_delay"])).group())
        path = os.path.join(self.d, "beacon.u8")
        ev, raw = self.beacon_now("beacon now", self.f_lo, path)
        self.t_last_beacon = self.clock.time()
        dev, fs, on, runs = air(raw)
        frames = afsk_decode(dev, fs, runs)
        c = [_chk("a carrier on C's 145.495 (A on 446.475)", bool(runs), runs),
             _chk("one frame decoded", len(frames) == 1, len(frames))]
        if frames and runs:
            c += self.check_frame(frames[0], call)
            d = (frames[0]["t_flag"] - runs[0][0]) * 1000
            fl_ms = (frames[0]["t_data"] - frames[0]["t_flag"]) * 1000
            c14 = [_chk(f"the TX delay {txd} ms before the first flag (±{self.mc['tx_delay_tol_ms']})",
                        abs(d - txd) <= self.mc["tx_delay_tol_ms"], f"{d:.0f} ms (then {fl_ms:.0f} ms of flags)"),
                   _chk("the frame decodes after the flags", True, "")]
            self.add("aprs_tx_delay", c14, f"carrier to first flag {d:.0f} ms, flags {fl_ms:.0f} ms (TX delay {txd} ms)")
        c.append(_chk("beacon power reads Low", self.get(S["beacon_power"]).lower() == "low", self.get(S["beacon_power"])))
        ok = self.add("aprs_beacon_on_c", c, f"{len(frames)} frame(s): " + ", ".join(
            f"{(ax25_parse(f['frame']) or {}).get('src')}>{(ax25_parse(f['frame']) or {}).get('dest')}" for f in frames))
        self.keep(path, ok)

    def t_aprs_after_voice(self):
        """APRS-27, APRS-9: beacon after voice on; a remote voice key-up on A (145.495), unkey: a beacon on C;
        another voice key-up 8 s later: its beacon waits until 30 s after the first."""
        call = self.aprs_setup()
        self.setv(S["after_voice"], "On")
        self.tune("A", self.f_lo)
        self.gate("A", "after voice")
        sp = self.mc["aprs_spacing_s"]
        if self.t_last_beacon:  # APRS-9 from the Beacon now before: the first beacon here mustn't wait for it
            self.clock.sleep(max(self.t_last_beacon + sp + 1.0 - self.clock.time(), 0))
        total = 1.0 + 1.0 + 8.0 + sp + 6.0
        path = os.path.join(self.d, "after-voice.u8")
        duty = self.b.duty
        # The first voice key-up and its beacon as one burst (the beacon follows within a second or two).
        duty.wait(duty.planned(1.0, 2.5), "burst", "after voice 1", 1.0)
        self.b.sdr.start(self.f_lo, total, path)
        t0 = self.clock.time()
        try:
            self.clock.sleep(1.0)
            ev1 = self.b.key_session(1.0, "after voice 1", extra_tail=2.5)
            self.clock.sleep(max(ev1["t_off"] + 8.0 - self.clock.time(), 0))
            self.gate("A", "after voice 2")
            ev2 = self.b.key_session(1.0, "after voice 2")
            # The second beacon comes by itself ~30 s after the first: if the duty limit wouldn't take it, cancel it.
            t_b2 = ev1["t_off"] + sp
            w, why = duty.wait_needed(2.0, max(t_b2, self.clock.time()), "burst")
            if w > 0:
                self.say(f"  the second beacon wouldn't fit the duty limit ({why}): beacon after voice off")
                self.r.term(f"set {S['after_voice']} Off")
            self.clock.sleep(max(t0 + total - self.clock.time(), 0))
            raw = self.b.sdr.finish()
        except BaseException:
            self.b.sdr.stop()
            raise
        self.r.term(f"set {S['after_voice']} Off")
        dev, fs, on, runs = air(raw)
        frames = afsk_decode(dev, fs, runs)
        for f in frames:  # the beacons the radio sent by itself, in the duty log
            duty.record("beacon (after voice)", "burst", dict(accepted=True, t_off=t0 + f["t_data"] + 0.8), t0 + f["t_flag"] - 0.35)
        off1 = ev1["t_off"] - t0
        c27 = [_chk("a beacon after the first voice key-up", bool(frames) and 0 < frames[0]["t_flag"] - off1 < 4,
                    _s(frames[0]["t_flag"] - off1) if frames else "none")]
        if frames:
            c27 += self.check_frame(frames[0], call)
        self.add("aprs_after_voice", c27, f"{len(frames)} beacon(s); the first {_s(frames[0]['t_flag'] - off1) if frames else '-'} after the unkey")
        gap = frames[1]["t_flag"] - frames[0]["t_flag"] if len(frames) > 1 else None
        ok = self.add("aprs_spacing", [_chk("two beacons", len(frames) == 2, len(frames)),
                                       _chk(f"at least {sp:g} s apart (the second waited)", gap is not None and gap >= sp - 0.2, _s(gap)),
                                       _chk("and not much later", gap is not None and gap <= sp + 3.0, _s(gap))],
                      f"beacons {_s(gap)} apart after voice key-ups 8 s apart")
        self.keep(path, ok)

    def t_aprs_compose(self):
        """APRS-18 (part: remote PTT only): APRS mode, a remote KEY on C opens compose, no carrier on C's 145.495."""
        self.aprs_setup()
        self.tune("A", self.f_lo)
        self.home()
        self.gate("C", "compose", allow_aprs=True)
        path = os.path.join(self.d, "compose.u8")
        slot_code = self.b.slot_code
        self.b.slot_code = fl.SLOTS["c"]
        try:
            ev, raw = self.b.capture_keyed(self.f_lo, 1.0, path, "aprs compose", lead=0.8, tail=1.0, kind="burst")
        finally:
            self.b.slot_code = slot_code
        sid = self.ui().get("screen", {}).get("id")
        runs = B.carrier_runs(raw)
        ok = self.add("aprs_compose", [_chk("compose opens", bool(re.search(r"ompose|Msg|Message", sid or "")), sid),
                                       _chk("no carrier", not runs, runs)], f"screen {sid}, carrier {runs or 'none'}")
        self.home()
        self.keep(path, ok)

    def t_gps33(self):
        """GPS-33 (part: no position): GPS source, no fix, beacons every 1 min: no key-up for 3.5 min (the status)
        and a "Beacon not sent" toast or event."""
        if not self.a.slow:
            raise Skip("--slow (about 4 minutes)")
        if "fix" in self.r.term("gps").lower() and "no fix" not in self.r.term("gps").lower():
            raise Skip("the GPS has a fix (run it indoors)")
        self.ax25_call("GPS-33")
        self.aprs_freq_ready()
        self.setv(S["pos_source"], "GPS")
        self.setv(S["aprs_mode"], "On")
        self.gate("C", "gps33", allow_aprs=True)
        # Should it key (a bug), the beacon is short: admitted as one 2 s burst; the status watch stops anything longer.
        toasts = []

        def start():
            self.setv(S["beacon_mode"], "Every 1 min")

        end = self.clock.time() + self.mc["gps33_wait_s"]
        keyed = None
        self.b.duty.admit(2.5, "burst", "gps33")
        start()
        while self.clock.time() < end:
            if self.keyed_now():
                keyed = self.clock.time()
                self.r.term(f"set {S['beacon_mode']} Off")
                break
            t = (self.ui().get("toast") or {})
            if t:
                toasts.append(f"{t.get('head')} {t.get('body')}".strip())
            self.clock.sleep(1.0)
        self.r.term(f"set {S['beacon_mode']} Off")
        self.add("gps33_no_position", [_chk("no key-up", keyed is None, keyed),
                                       _chk("\"Beacon not sent\" shown", any("not sent" in x.lower() for x in toasts), toasts[-3:])],
                 f"{self.mc['gps33_wait_s']:.0f} s with beacons every minute and no fix: "
                 + ("keyed" if keyed else "no key-up"))

    def t_kiss(self):
        """KISS-2 (part), KISS-7 (part), KISS-9: KISS on Bluetooth (0 5 8; the cable stays the link): slot C is the
        APRS slot on the APRS frequency (145.495) without APRS mode; a frame from the PC goes out there with the TX
        delay its KISS command set; a second 6 s later goes out too (exempt from APRS-9's 30 s); the frames decode
        byte for byte. No PIN asked (none is sent). Each frame is its own capture, watched over the status (a data
        key-up reads keyed=C): the one 12.8 s capture around both ran ~32 s (the duty waits and the
        status watch, which never saw a data key-up then, inside it) and rtl_sdr was killed."""
        if not self.kiss_port:
            raise Skip("--kiss-ble ble:NAME (KISS over Bluetooth; the cable stays the link)")
        call = self.ax25_call("KISS-2")
        self.aprs_freq_ready()
        self.setv(S["bluetooth"], "On")
        self.setv(S["kiss_port"], "Bluetooth")
        self.setv(S["aprs_mode"], "KISS")  # APRS-29.2: KISS is one of 0 5 3's values; 0 5 8 its port
        port = self.kiss_port() if callable(self.kiss_port) else self.kiss_port
        frames = [ax25_frame("APZFER", call, ["WIDE1-1"], f">bench KISS {i}".encode()) for i in (1, 2)]
        txd = self.mc["kiss_txdelay_ms"]
        got, evs, t_sent = [], [], []
        try:
            self.gate("C", "kiss: slot C (KISS-9)", allow_aprs=True)
            port.write(kiss_wrap(0x01, bytes([txd // 10])))
            self.clock.sleep(0.5)
            for i, fr in enumerate(frames):
                if t_sent:  # the second 6 s after the first (or as soon after as the duty limit lets it)
                    self.clock.sleep(max(t_sent[0] + 6.0 - self.clock.time(), 0))
                path = os.path.join(self.d, f"kiss-{i + 1}.u8")
                planned = 2.5

                def send(fr=fr):
                    t_sent.append(self.clock.time())
                    port.write(kiss_wrap(0x00, fr))

                ev, raw = self.radio_keyup(f"kiss {i + 1}", planned, send, mhz=self.f_lo, path=path, lead=0.8,
                                           total_s=0.8 + planned + 1.5)
                evs.append(ev)
                dev, fs, on, runs = air(raw)
                for g in afsk_decode(dev, fs, runs):
                    got.append(dict(g, t_abs=ev["t_capture"] + g["t_flag"], t_carrier=runs[0][0] if runs else None))
                self.keep(path, any(g["frame"] == fr for g in got))
        finally:
            with contextlib.suppress(Exception):
                self.r.term(f"set {S['kiss']} Off")
        gotb = [g["frame"] for g in got]
        c = [_chk(f"frame {i + 1} keyed slot C (status keyed=C, KISS-9)", ev["accepted"] and ev.get("slot") == "C",
                  ev.get("slot") or "not keyed") for i, ev in enumerate(evs)]
        c += [_chk(f"frame {i + 1} on 145.495, byte for byte", fr in gotb, len(gotb)) for i, fr in enumerate(frames)]
        if len(got) == 2:
            c.append(_chk("the second under 30 s after the first (KISS-2: exempt from APRS-9)",
                          got[1]["t_abs"] - got[0]["t_abs"] < 30, _s(got[1]["t_abs"] - got[0]["t_abs"])))
        if got and got[0]["t_carrier"] is not None:
            d = (got[0]["t_data"] - got[0]["t_carrier"]) * 1000
            c.append(_chk(f"TX delay {txd} ms from the KISS command (carrier to the frame, ±{self.mc['tx_delay_tol_ms']})",
                          abs(d - txd) <= self.mc["tx_delay_tol_ms"] + 20, f"{d:.0f} ms"))
        self.add("kiss_frames", c, f"{len(got)} of 2 KISS frames on the air")
        self.add("kiss_no_pin", [_chk("frames went out with no PIN or session (KISS-7)", bool(got), len(got))],
                 "no AUTH sent; frames on air")

    # -- GPS, watch, HF, broadcast: remote key-ups with readings while keyed --
    def keyed_readings(self, purpose, key_s, mhz, readers, after_s=0.6, slot="A"):
        """A remote key-up of key_s with the SDR on; readers {name: fn} read at its middle and 0.1/after_s after KEY
        off. Returns ({name: [before, keyed, unkey wait, after]}, ev, runs)."""
        vals = {k: [f()] for k, f in readers.items()}
        b, duty = self.b, self.b.duty
        path = os.path.join(self.d, B.slug(purpose) + ".u8")
        planned = duty.planned(key_s)
        duty.wait(planned, "burst", purpose, 0.8)
        b.sdr.start(mhz, 0.8 + key_s + after_s + 2.0, path)
        t0 = self.clock.time()
        ev = dict(purpose=purpose, accepted=None)
        try:
            self.clock.sleep(0.8)
            duty.admit(planned, "burst", purpose)
            t_sent = self.clock.time()
            ev["accepted"], ev["refused"] = self.key_on(fl.SLOTS[slot.lower()])
            end = t_sent + key_s
            done = False
            while self.clock.time() < end:
                self.r.key_alive()
                if not done and self.clock.time() > t_sent + key_s / 2 - 0.3:
                    for k, f in readers.items():
                        vals[k].append(f())
                    done = True
                self.clock.sleep(min(0.4, max(end - self.clock.time(), 0)))
        finally:
            self.r.key_off()
            ev["t_off"] = self.clock.time()
            b.keyed = False
            duty.record(purpose, "burst", ev, t_sent)
        for k, f in readers.items():
            vals[k].append(f())
        self.clock.sleep(after_s)
        for k, f in readers.items():
            vals[k].append(f())
        self.clock.sleep(max(t0 + 0.8 + key_s + after_s + 2.0 - self.clock.time(), 0))
        raw = b.sdr.finish()
        runs = B.carrier_runs(raw)
        self.keep(path, bool(runs))
        return vals, ev, runs

    def t_gps_pins(self):
        """GPS-5, GPS-22 (part), GPS-14.1: a remote key-up: PB10 (USART3 TX to the GPS) reads low while keyed and
        idles high again after; PA8 (the GPS's power) on throughout with GPS during RX/TX Always on; the NMEA keeps
        coming (`gps`'s age stays under 2 s while keyed)."""
        # GPS-14.1 is about the module's power while keyed, so the GPS must be on (0 6 1; put back after). PA8 then
        # reads on, standby or not (VCC kept, GPS-26); the run had it off and read PA8 0 throughout.
        if not self.get(S["gps"]).lower().startswith("on"):
            self.setv(S["gps"], "On")
        end = self.clock.time() + 5.0
        while pin(parse_gpio(self.r.term("gpio")), "PA8", 1) != 1:
            if self.clock.time() > end:
                raise Skip("GPS on (0 6 1), but PA8 (the GPS's power) still reads off after 5 s")
            self.clock.sleep(0.5)
        self.tune("A", self.f_lo)
        self.home()
        self.gate("A", "gps pins")
        rxtx = self.get(S["gps_rxtx"])

        def age():
            m = re.search(r"age (\d+(?:\.\d+)?) ?(ms|s)", self.r.term("gps"))
            return None if not m else float(m.group(1)) / (1000 if m.group(2) == "ms" else 1)

        g = lambda: parse_gpio(self.r.term("gpio"))  # noqa: E731
        vals, ev, runs = self.keyed_readings("gps pins", 3.0, self.f_lo, {"gpio": g, "age": age}, after_s=1.0)
        pb10 = [pin(x, "PB10", 0) for x in vals["gpio"]]
        pa8 = [pin(x, "PA8", 1) for x in vals["gpio"]]
        self.add("gps_pb10", [_chk("keyed (carrier on the SDR)", bool(runs) and ev["accepted"], runs),
                              _chk("PB10 high before (UART idle)", pb10[0] == 1, pb10),
                              _chk("PB10 low while keyed", pb10[1] == 0, pb10),
                              _chk("PB10 high again after the unkey", pb10[3] == 1, pb10)], f"PB10 {pb10} (before, keyed, unkey, after)")
        ages = vals["age"]
        c = [_chk("GPS during RX/TX reads Always on", rxtx.lower() == "always on", rxtx),
             _chk("PA8 (GPS power) on throughout", all(x == pa8[0] for x in pa8) and pa8[0] == 1, pa8)]
        # The NMEA only when it was flowing before the key-up: in the power manager's standby (GPS-26) none comes.
        flowing = ages[0] is not None and ages[0] < 2.0
        if flowing:
            c.append(_chk("NMEA still coming while keyed (age under 2 s)", ages[1] is not None and ages[1] < 2.0, ages))
        self.add("gps_during_tx", c, f"PA8 {pa8}, NMEA age {ages}" + ("" if flowing else " (no NMEA before the key-up: standby, not checked)"))

    def t_watch_relay(self):
        """WATCH-12: `gpio` PC4 (the band relay) before, while keyed, in the unkey wait and after: unchanged, on
        each test frequency."""
        c = []
        for L, f in (("A", self.f_lo), ("B", self.f_hi)):
            self.tune(L, f)
            self.home()
            self.gate(L, f"relay {f}")
            g = lambda: pin(parse_gpio(self.r.term("gpio")), "PC4", 1)  # noqa: E731
            vals, ev, runs = self.keyed_readings(f"relay {f:.3f}", 2.0, f, {"pc4": g}, after_s=0.5, slot=L)
            p = vals["pc4"]
            c.append(_chk(f"{f:.3f}: keyed", bool(runs) and ev["accepted"], runs))
            c.append(_chk(f"{f:.3f}: PC4 unchanged (before, keyed, unkey wait, after)", len(set(p)) == 1, p))
        self.add("watch_relay_keyup", c, "PC4 the same through each key-up")

    def t_watch_pause(self):
        """WATCH-16 (part: not the key press that ends it): watch A+B, A keyed on 145.495, B on 446.475 watched:
        B's chip (1) REG_30 differs from watching while keyed and stays so until ~30 s after the unkey."""
        self.tune("B", self.f_hi, active=False)
        self.tune("A", self.f_lo)
        self.setv(S["watch"], "A+B")
        # Battery Save off (put back after): asleep, B's chip reads REG_30 0 whatever the watch does.
        self.setv(S["bsave"], "Off")
        self.home()
        self.gate("A", "watch pause")
        # Watch is paused 30 s after any key-up (this rule): the last one's end + 32 s before the baseline.
        last = max((e["t_end"] for e in self.b.duty.log if e["keyed_s"] > 0), default=0)
        self.clock.sleep(max(last + self.mc["watch_resume_s"] + 2.0 - self.clock.time(), 1.0))
        w = parse_bk(self.r.term("bk 1 30"))
        vals, ev, runs = self.keyed_readings("watch pause", 2.0, self.f_lo, {"r30": lambda: parse_bk(self.r.term("bk 1 30"))},
                                             after_s=0.3)
        paused = vals["r30"][1]
        t_off = ev["t_off"]
        resumed = None
        while self.clock.time() < t_off + self.mc["watch_resume_s"] + 8:
            v = parse_bk(self.r.term("bk 1 30"))
            if v != paused:
                resumed = self.clock.time() - t_off
                break
            self.clock.sleep(0.5)
        tol = self.mc["watch_tol_s"]
        self.add("watch_pause_after_tx", [
            _chk("keyed", bool(runs) and ev["accepted"], runs),
            _chk("B's chip changes from watching while A transmits", paused != w, f"0x{w:04X} -> 0x{paused:04X}"),
            _chk(f"resumes {self.mc['watch_resume_s']:g} s after the unkey (±{tol:g})",
                 resumed is not None and abs(resumed - self.mc["watch_resume_s"]) <= tol, _s(resumed))],
            f"REG_30 on chip 1: watching 0x{w:04X}, paused 0x{paused:04X}, back after {_s(resumed)}")

    def t_hf(self):
        """HF-2: keyed on 145.495, HF mode by remote keys is refused with a reason. HF-24 (part: AM below 18 MHz):
        HF mode at its frequency there, a remote KEY refused "Receive only here". HF-23 (part: the refusal, staying
        in HF mode): at 40 MHz (chip 1, outside the band plan) refused with the band plan's reason, still in HF
        mode, no carrier."""
        self.tune("A", self.f_lo)
        self.home()
        self.gate("A", "hf-2")
        sid = []

        def try_hf():
            # Remote keys are refused `invalid` while the radio transmits ("remote keys wait for the unkey",
            # LINK-19): that refusal is the reason; a key the radio took is checked on its screen.
            try:
                self.r.keys(self.mc["hf_keys"])
                refused = ""
            except B.TermError as e:
                refused = str(e)
            u = self.ui()
            sid.append((u.get("screen", {}).get("id"), (u.get("toast") or {}), refused))

        planned = self.b.duty.planned(3.0)
        self.b.duty.admit(planned, "burst", "hf-2")
        t_sent = self.clock.time()
        e2 = dict(accepted=None)
        try:
            e2["accepted"], e2["refused"] = self.key_on(self.b.slot_code)
            self.r.key_alive()
            try_hf()
            self.r.key_alive()
            self.clock.sleep(0.4)
        finally:
            self.r.key_off()
            e2["t_off"] = self.clock.time()
            self.b.keyed = False
            self.b.duty.record("hf-2", "burst", e2, t_sent)
        scr, toast, refused = sid[0]
        self.add("hf_refused_while_keyed", [_chk("keyed", e2["accepted"]),
                                            _chk("HF mode didn't open", scr != "Hf", scr),
                                            _chk("a reason given (the keys' refusal, or a toast)",
                                                 bool(refused or toast.get("head") or toast.get("body")), refused or toast)],
                 f"screen {scr}, " + (f"keys refused: {refused}" if refused else f"toast {toast}"))
        self.home()
        # HF-24: HF mode at a SI4732 frequency
        self.r.keys(self.mc["hf_keys"])
        u = self.ui()
        if u.get("screen", {}).get("id") != "Hf":
            self.add("hf_receive_only", [_chk("HF mode opens", False, u.get("screen"))], "HF mode didn't open")
            return
        st = self.status()
        mhz = _hf_mhz(u)
        if mhz is None:
            self.home()
            raise Skip("HF mode's frequency isn't in the readback: no HF probe (HF-23, HF-24) without it")
        if mhz >= 18:
            self.add("hf_receive_only", [_chk("HF mode on an SI4732 frequency (< 18 MHz)", False, mhz)], "not below 18 MHz")
        else:
            ev, raw = self.b.capture_keyed(self.f_lo, B.KEY_ANSWER_S, os.path.join(self.d, "hf-24.u8"), "hf-24",
                                           lead=0.6, tail=0.8, kind="probe", expect_refusal=True)
            said = ev.get("refused") or ""
            self.add("hf_receive_only", [_chk("refused", not ev.get("accepted"), said or "KEYED"),
                                         _chk("\"Receive only here\"", "receive only" in said.lower(), said),
                                         _chk("nothing keyed (status)", not self.status()["keyed"], st)],
                     f"{mhz:.3f} MHz: {said!r}")
        # HF-23: 40 MHz on chip 1, outside the band plan
        self.r.keys(self.mc["hf_tune_keys"])
        u = self.ui()
        mhz = _hf_mhz(u)
        if mhz is None or abs(mhz - 40.0) > 0.01:
            self.add("hf_chip1_refusal", [_chk("HF mode tuned to 40.000 MHz", False, mhz)], f"`{self.mc['hf_tune_keys']}` gave {mhz}")
        else:
            ev, raw = self.b.capture_keyed(self.f_lo, B.KEY_ANSWER_S, os.path.join(self.d, "hf-23.u8"), "hf-23",
                                           lead=0.6, tail=0.8, kind="probe", expect_refusal=True)
            said = ev.get("refused") or ""
            sid2 = self.ui().get("screen", {}).get("id")
            self.add("hf_chip1_refusal", [_chk("refused", not ev.get("accepted"), said or "KEYED"),
                                          _chk("the band plan's reason", "band plan" in said.lower(), said),
                                          _chk("still in HF mode (doesn't go home first)", sid2 == "Hf", sid2)],
                     f"40.000 MHz: {said!r}")
        self.home()

    def t_bcast(self):
        """BCR-16, BCR-29.3, BCR-26.4 (part: during a remote key-up): B plays the broadcast (100.1), A keyed on
        145.495: the SI4732 muted (`si prop 4001`) and B's card off while keyed; FM and unmuted after; the carrier."""
        self.r.term(f"slot B vfo {self.mc['bcast_mhz']:.5f}")
        self.tune("A", self.f_lo)
        self.home()
        self.gate("A", "broadcast")
        self.clock.sleep(1.5)

        def card():
            # The compact status's per-slot state (LINK-19): the cards' words, read on any screen (the full
            # readback has no cards on the Transmit screen, so it gave None while keyed).
            return (self.status().get("slots", {}).get("B") or {}).get("state")

        def mute():
            v = si_prop(self.r.term("si prop 4001"), 0x4001)  # RX_HARD_MUTE: bits 0-1 the two sides
            return None if v is None else v & 3

        vals, ev, runs = self.keyed_readings("broadcast", 3.0, self.f_lo, {"card": card, "mute": mute}, after_s=1.5)
        cd, mu = vals["card"], vals["mute"]
        self.add("bcast_muted_keyed", [_chk("keyed (carrier)", bool(runs) and ev["accepted"], runs),
                                       _chk("playing before (card FM)", cd[0] == "FM", cd),
                                       _chk("SI4732 muted while keyed", mu[1] == 3, mu)], f"mute {mu}, card {cd}")
        self.add("bcast_resumes", [_chk("off while keyed", cd[1] == "off", cd),
                                   _chk("FM again after", cd[3] == "FM", cd),
                                   _chk("unmuted after", mu[3] == 0, mu)], f"card {cd}")
        self.add("bcast_card_keyed", [_chk("the card reads off while keyed, FM before and after",
                                           cd[1] == "off" and cd[0] == "FM" and cd[3] == "FM", cd)], f"card {cd}")
        self.r.term(f"slot B vfo {self.f_hi:.5f}")

    # -- running them --
    def save_slots(self):
        """Slots B and C as they are (bench.py saves A): put back by restore()."""
        for L in "BC":
            self.slots_saved[L] = B.parse_slot_line(self.r.term(f"slot {L}"))

    def restore_slots(self, notes):
        for L, v in self.slots_saved.items():
            if v.get("aprs"):
                continue  # APRS mode's own slot: 0 5 3 and 0 5 4 are put back instead
            if v["channel"] is not None:
                notes.append(f"slot {L} was on channel {v['channel']}: press V/M on the radio if it shows its VFO")
                continue
            words = [f"slot {L} vfo {v['freq']}", v.get("mode", "").lower(), v.get("power", "")]
            if v.get("txtone"):
                words += ["txtone", v["txtone"], "rxtone", v.get("rxtone", "off")]
            try:
                self.r.term(" ".join(w for w in words if w))
            except Exception as e:
                notes.append(f"slot {L}: {e}")

    def restore(self):
        """Everything this suite changed, VOX off first."""
        notes = []
        with contextlib.suppress(Exception):
            self.audio and self.audio.stop()
        for p in [S["vox"]] + [k for k in self.saved if k not in (S["vox"], "aprs_freq")]:
            if p in self.saved:
                try:
                    self.r.term(f"set {p} {self.saved[p]}")
                except Exception as e:
                    notes.append(f"{p}: {e}")
        if "aprs_freq" in self.saved:
            try:
                f = float(re.match(r"[\d.]+", self.saved["aprs_freq"]).group())
                self.type_freq(self.mc["aprs_freq_keys"], f)
                if self.get(S["aprs_freq"]) != self.saved["aprs_freq"]:
                    notes.append(f"APRS frequency reads {self.get(S['aprs_freq'])}, was {self.saved['aprs_freq']}: set 0 5 4 on the radio")
            except Exception as e:
                notes.append(f"APRS frequency (0 5 4, was {self.saved['aprs_freq']}): {e}")
        self.restore_slots(notes)
        with contextlib.suppress(Exception):
            self.home()
        return notes


def _hf_mhz(u):
    for k in ("hf", "freq", "value"):
        v = u.get(k)
        if isinstance(v, dict):
            for x in list(v.values()) + list(v.get("args", [])):
                m = re.fullmatch(r"(\d+\.\d+)", str(x))
                if m:
                    return float(m.group(1))
    return None


def _chk(what, ok, got=""):
    if isinstance(got, (bytes, bytearray)):
        got = repr(bytes(got))
    return dict(what=what, ok=bool(ok), got=B.jsonable(got) if not isinstance(got, str) else got)


def _s(x):
    return "-" if x is None else f"{x:.2f} s"


# (name, method, groups, needs, est wall s, est keyed s, what)
TESTS = [
    ("vox_tail", "t_vox_keys_and_tail", "vox", "audio", 25, 4.5, "VOX keyed by the tone: carrier on B (active); the tail after the envelope falls"),
    ("vox_levels", "t_vox_levels", "vox", "audio", 110, 18, "one loudness, VOX levels 1-9: keys up to a level, none above"),
    ("vox_home_only", "t_vox_home_only", "vox", "audio", 80, 4, "from home it keys; in quick, menu, 0 8 2 and entry it doesn't"),
    ("vox_hold", "t_vox_hold_after_menu", "vox", "audio", 20, 3, "the tone on as the menu closes: the key-up 1.5 s after home"),
    ("vox_blocked", "t_vox_blocked", "vox", "audio", 30, 0, "the tone under the broadcast and an open monitor: no key-up"),
    ("vox_boot", "t_vox_boot", "vox", "audio", 45, 4, "the tone through a restart: no carrier for 5 s, then VOX keys"),
    ("dtmf", "t_dtmf", "dtmf", "", 60, 8, "mic tone on a voice key-up; dial 1593 at 50/0, 100/300, 200/1000 ms: digits, timing, pairs, mic muted, no end tone"),
    ("end_tone", "t_end_tone_order", "tone", "", 30, 5, "end tone Beep after KEY off, before the drop, mic muted; none after txtest afsk"),
    ("reg40", "t_reg40", "tone", "", 15, 2.3, "bk 0 40 while keyed (0x50A), during the Quindar end tone (0x43C), after (0x50A)"),
    ("aprs_beacon", "t_aprs_beacon", "aprs", "", 30, 2, "Beacon now with C on 145.495, A on 446.475: frame, TX delay, C's frequency"),
    ("aprs_after_voice", "t_aprs_after_voice", "aprs", "", 55, 5, "beacon after voice: a beacon after a voice key-up; the next waits 30 s"),
    ("aprs_compose", "t_aprs_compose", "aprs", "", 15, 0, "APRS mode, remote KEY on C: compose, no carrier"),
    ("kiss", "t_kiss", "kiss", "ble", 30, 4, "two KISS frames over Bluetooth 6 s apart: on air byte for byte, TX delay 500 ms"),
    ("gps_pins", "t_gps_pins", "gps", "", 15, 3.5, "keyed: PB10 low, PA8 on, NMEA age fresh; after: PB10 back"),
    ("gps33", "t_gps33", "gps", "slow", 240, 0, "no fix, beacons every 1 min: no key-up for 3.5 min, Beacon not sent"),
    ("watch_relay", "t_watch_relay", "watch", "", 25, 5, "PC4 unchanged through key-ups on 145.495 and 446.475"),
    ("watch_pause", "t_watch_pause", "watch", "", 50, 2.5, "watched B's chip paused while A keyed, back 30 s after the unkey"),
    ("hf", "t_hf", "hf", "", 40, 3.5, "keyed: HF mode refused; HF at 9.58 MHz: Receive only here; at 40 MHz: band plan, stays in HF"),
    ("bcast", "t_bcast", "bcr", "", 20, 3.5, "broadcast on B, A keyed: SI4732 muted, card off; FM after"),
]


SUITE = "modes-on-air"


def select(a):
    want = [x.strip() for x in (getattr(a, "tests", None) or "").split(",") if x.strip()]
    out = []
    for t in TESTS:
        if want and t[0] not in want and t[2] not in want:
            continue
        out.append(t)
    return out


def why_not(t, a, has_audio):
    need = t[3]
    if need == "audio" and not has_audio:
        return "no speaker (--no-audio or no afplay)"
    if need == "ble" and not getattr(a, "kiss_ble", None):
        return "needs --kiss-ble ble:NAME"
    if need == "slow" and not getattr(a, "slow", False):
        return "needs --slow"
    return ""


def plan(cfg, a):
    has_audio = not getattr(a, "no_audio", False) and MacAudio.available()
    B.say("modes-on-air: dry plan (nothing is touched)")
    B.say(f"  port {cfg['port']}; keying only on {', '.join(f'{f:.3f}' for f in cfg['test_freqs'])} MHz; SDR gain "
          f"{cfg['sdr_gain']} dB; speaker: {'afplay' if has_audio else 'none'}")
    B.say("  connect: HELLO (LINK-35 tx-dry with the bench bit, else refused), INFO, txtest; save slots, end tone, "
          "TX timeout and every setting a test changes (VOX first)")
    if any(t[3] == "audio" for t in select(a)) and has_audio:
        B.say("  calibrate (VOX off, no key-up): ambient and 8 tones (-42 to 0 dBFS, 1.2 s) read by `mic`; the "
              "envelope's fall: about 25 s")
    wall = keyed = 25.0 if has_audio else 0.0
    keyed = 0.0
    for t in select(a):
        w = why_not(t, a, has_audio)
        B.say(f"  {'skip' if w else 'run '} {t[0]:16s} ~{t[4]:3.0f} s, ~{t[5]:4.1f} s keyed: {t[6]}" + (f"  [{w}]" if w else ""))
        if not w:
            wall += t[4]
            keyed += t[5]
    B.say(f"  restore in finally: VOX off, settings, slots, APRS frequency by keys, home")
    B.say(f"about {wall / 60:.1f} min, about {keyed:.0f} s keyed (each key-up through the duty limit: <= 5 s, rest 2x, "
          f"<= 25 % of 5 min); rtl_sdr {'found' if shutil.which('rtl_sdr') else 'NOT found'}; afplay "
          f"{'found' if MacAudio.available() else 'NOT found'}")


def run(cfg, a, radio=None, sdr=None, clock=None, audio=None, kiss_port=None):
    """The suite on air (or on fakes): returns (ok, session dir, results)."""
    clock = clock or B.Clock()
    radio = radio or LinkRadio(cfg["port"])
    if audio is None and not getattr(a, "no_audio", False) and MacAudio.available():
        audio = MacAudio(clock)
    if kiss_port is None and getattr(a, "kiss_ble", None):
        spec = a.kiss_ble
        kiss_port = lambda: fl.Port(spec)  # noqa: E731 (opened when the KISS test runs)
    bench = B.Bench(cfg, a, radio=radio, sdr=sdr or B.Sdr(cfg, clock), clock=clock)
    B._CURRENT.append(bench)
    hello = radio.hello() if hasattr(radio, "hello") else {}
    info = radio.info()
    bench.info = info
    label = getattr(a, "build", None) or f"{info['version']}-modes"
    session, sd = B.new_session(cfg, label)
    B.say(f"session {sd}")
    m = Modes(bench, cfg, a, audio, sd, kiss_port)
    skipped = {}
    errors = {}
    try:
        bench.check_build(hello, info)
        bench.save_state()
        m.save_slots()
        for t in select(a):
            w = why_not(t, a, audio is not None)
            if w:
                skipped[t[0]] = w
                continue
            B.say(f"\n== {t[0]}: {t[6]}")
            try:
                radio.term(f"set {B.END_TONE} Off")  # bench.py put it back; a test that wants one sets it
                getattr(m, t[1])()
            except Skip as e:
                skipped[t[0]] = str(e)
                B.say(f"  not run: {e}")
            except B.SafetyError:
                raise
            except (B.BenchError, B.TermError) as e:
                errors[t[0]] = str(e)
                B.say(f"  ERROR: {e}")
            finally:
                m.stop_all() if bench.keyed else None
                m.put_back()
    except B.SafetyError as e:
        errors["gate"] = str(e)
        B.say(f"\n{e}")
        B.say("restoring")
    finally:
        bench.emergency_unkey()
        with contextlib.suppress(Exception):
            m.stop_all()
        notes = m.restore()
        bench.restore()
        B._CURRENT.remove(bench)
        if bench.duty.log:
            B.say(bench.duty.report())
    if errors.get("gate"):
        raise B.SafetyError(errors["gate"])
    items = [dict(name=t["name"], label=t["name"], result=t["result"], ok=t["result"] == "pass", text=t["summary"],
                  why=t["why"], checks=t["checks"], summary=t["summary"], rules=t["rules"]) for t in m.results]
    items += [dict(name=k, label=k, result="skip", ok=True, text=f"not run: {v}") for k, v in skipped.items()]
    items += [dict(name=k, label=k, result="fail", ok=False, text=f"error: {v}", why=v) for k, v in errors.items()]
    ran = [x for x in items if x["result"] != "skip"]
    extras = {f"suite:{SUITE}": dict(name=SUITE, total=len(select(a)), items=items, complete=not skipped and not errors,
                                     ok=bool(ran) and all(x["ok"] for x in ran), restore_notes=notes, skipped=skipped,
                                     errors=errors, calibration=m.cal)}
    meta = dict(build=dict(label=label, version=info.get("version"), host=B.host_build()), source="on air",
                port=getattr(radio, "spec", "fake"), slot=cfg["slot"], commit=B.host_build(), kinds=["rf"],
                cfg=dict(sweeps=cfg["sweeps"]), sim=not isinstance(radio, B.Radio) or not radio.cable, suite=SUITE,
                gates=bench.gatelog, warnings=bench.warnings, restore_notes=bench.restore_notes + notes,
                tx_dry=bench.tx_dry, duty=bench.duty.summary())
    page = B.write_session(cfg, sd, session, [], extras, meta)
    B.say("\nmodes-on-air:")
    for t in m.results:
        B.say(f"  {'pass' if t['result'] == 'pass' else 'FAIL'}  {t['name']:26s} {', '.join(t['rules'])}")
    for k, v in skipped.items():
        B.say(f"  skip  {k:26s} {v}")
    for k, v in errors.items():
        B.say(f"  ERR   {k:26s} {v}")
    B.say(f"\npage: {page}")
    return extras, page, m


def tests_of(ex, add):
    """bench.rule_tests' hook: each check of a modes-on-air run, under its own name (bench_rules tags it)."""
    for x in (ex.get(f"suite:{SUITE}") or {}).get("items", []):
        if x.get("result") in ("pass", "fail") and x.get("checks"):
            add(x["name"], x["checks"], x.get("summary") or x.get("text") or "")


# ---- The selftest's fakes: a radio with VOX, the dialer, beacons, KISS, pins; the SDR from its air log ----


class FakeModesRadio(B.FakeRadio):
    """bench.FakeRadio plus what this suite reads and presses: screens by remote keys, the compact status, VOX on
    the fake speaker's level, the dialer, Beacon now and beacon after voice, KISS frames, gpio, bk, si, mic, gps,
    HF mode and a broadcast slot. Every key-up goes in `air` (on, off, MHz, deviation function)."""

    def __init__(self, clock, audio, version="v0.1.0 TX DRY", callsign="VA7FER", **kw):
        super().__init__(clock, version=version, build="tx-dry", **kw)
        self.audio, self.callsign = audio, callsign
        self.set.update({S["vox"]: "Off", S["vox_level"]: "5", S["vox_tail"]: "2 s", S["dtmf_speed"]: "100 ms",
                         S["dtmf_delay"]: "300 ms", S["aprs_mode"]: "Off",
                         S["beacon_mode"]: "Off", S["after_voice"]: "Off", S["beacon_power"]: "Low",
                         S["aprs_freq"]: "144.390", S["tx_delay"]: "300 ms", S["pos_source"]: "Fixed",
                         S["fixed"]: "49.2827 -123.1207", S["kiss_port"]: "Bluetooth", S["watch"]: "Off", S["gps_rxtx"]: "Always on",
                         S["bluetooth"]: "Off", S["gps"]: "Off", S["bsave"]: "Level 1"})
        self.set0 = dict(self.set)
        self.screen, self.entry = ["Home"], ""
        self.hf_mhz, self.monitor = 9.58, False
        self.air = []
        self.vkey = None  # the radio's own key-up: dict(on, kind, slot, mhz, until)
        self.vox_on, self.vox_last, self.vox_free = False, 0.0, 0.0
        self.t_boot, self.t_eval = clock.time() - 60, clock.time()
        self.toast = None
        self.last_beacon = -1e9
        self.owed = None
        self.watch_until = 0.0
        self.kiss_buf = b""
        self.txd_kiss = 300

    # -- time --
    def _mic(self, t):
        a = self.audio.at(t)
        v = 3 if not a else max(0, min(255, int(130 + 2.0 * a[1])))
        return v

    def _blocked(self):
        b = self.slots[1]
        return self.monitor or 87.5 <= b["freq"] <= 108.0 or self.screen[-1] != "Home"

    def _advance(self, now=None):
        now = self.clock.time() if now is None else now
        t = self.t_eval
        while t < now:
            t = min(t + 0.05, now)
            self._step(t)
        self.t_eval = now

    sched = None

    def _step(self, t):
        if self.sched and t >= self.sched + 120.0:  # GPS-33: no fresh fix within 2 min: skipped
            self.toast = {"head": "Beacon not sent", "body": "no position"}
            self.sched += 60.0
        vk = self.vkey
        if vk and vk["kind"] != "vox" and t >= vk["until"]:
            self._end(vk["until"])
        if self.owed and t >= self.owed and not self.key and not self.vkey:
            self.owed = None
            self._beacon(t)
        lvl = int(self.set[S["vox_level"]]) if self.set[S["vox"]] == "On" else 0
        if self._blocked():
            self.vox_free = t + 1.5
            self.vox_on = False
            return
        if not lvl or t - self.t_boot < 5.0 or t < self.vox_free:
            self.vox_on = False
            return
        m = self._mic(t)
        thr = vox_threshold(lvl)
        above = m + 8 >= thr if self.vox_on else m >= thr
        tail = float(self.set[S["vox_tail"]].split()[0])
        if above:
            self.vox_on, self.vox_last = True, t
        elif self.vox_on and t - self.vox_last >= tail:
            self.vox_on = False
        if self.vox_on and not self.vkey and not self.key:
            s = self.slots[self.active]
            self.vkey = dict(on=t, kind="vox", slot="ABC"[self.active], mhz=s["freq"], parts=[self._voice(t)])
        elif not self.vox_on and self.vkey and self.vkey["kind"] == "vox":
            self._end(t)

    def _end(self, t):
        vk = self.vkey
        self.air.append((vk["on"], t + 0.5, vk["mhz"], vk["parts"]))
        self.vkey = None
        if vk["kind"] == "vox" and self.set[S["after_voice"]] == "On":
            self._owe(t)

    def _owe(self, t):
        self.owed = max(t + 0.8, self.last_beacon + 30.0)

    def _voice(self, t0, until=1e18):
        aud = self.audio

        def f(t):
            out = np.zeros(len(t))
            for a, b, hz, db in aud.plays:
                m = (t >= a) & (t < min(b, until))
                out[m] += 1500 * 10 ** (db / 40) * np.sin(2 * np.pi * hz * t[m])
            return out
        return f

    def info(self):
        return dict(super().info(), callsign=self.callsign)

    # -- the link --
    def term(self, line):
        self._advance()
        w = line.split()
        if w[0] == "mic":
            v = self._mic(self.clock.time()) * 16
            return f"  mic PA0: 256 readings in 32000 us: min {v - 8}, max {v + 8}, mean {v} (12-bit)\n"
        if w[0] == "gpio":
            keyed = bool(self.key or self.vkey)
            pb = 0 if keyed else 1 << 10
            pa8 = (1 << 8) if self.set[S["gps"]] == "On" else 0  # the GPS's power follows 0 6 1
            row = lambda v: " ".join(f"{v:016b}"[i:i + 4] for i in range(0, 16, 4))  # noqa: E731
            return "".join(f"P{p}  in {row(i)}  out {row(o)}\n" for p, i, o in (
                ("A", pa8, pa8), ("B", pb, 0), ("C", 0, 1 << 4), ("D", 0, 0), ("E", 0, 0)))
        if w[0] == "bk":
            reg = int(w[2], 16)
            v = 0
            if reg == 0x40:
                v = 0x350A
                if self.endtone_until and self.endtone_from <= self.clock.time() < self.endtone_until:
                    v = 0x343C
            if reg == 0x30:
                paused = (self.key or self.vkey or self.clock.time() < self.watch_until) and self.set[S["watch"]] == "A+B"
                v = 0 if paused else 0xBFF1
            return f"  REG_{reg:02X} = 0x{v:04X}\n"
        if w[0] == "si":
            muted = bool(self.key or self.vkey) and 87.5 <= self.slots[1]["freq"] <= 108
            return f"si 13 00 40 01 -> 80 00 00 {3 if muted else 0:02X}\n"
        if w[0] == "gps":
            return "  gps: $GPGGA,... no fix, age 0.4 s\n"
        if w[0] == "set" and line.startswith(f"set {S['aprs_freq']} "):
            raise B.TermError(f"{line}: not a setting the terminal sets")
        if (w[0] == "get" or w[0] == "slot" and len(w) <= 2) and (self.key or self.vkey):
            k, self.key, vk, self.vkey = self.key, None, self.vkey, None
            try:
                return super().term(line)
            finally:
                self.key, self.vkey = k, vk
        if self.vkey and w[0] not in ("set",):
            raise B.TermError(f"{line}: transmitting: nothing changes while keyed (TX-22)")
        out = super().term(line)
        if line.startswith(f"set {S['beacon_mode']} Every"):
            self.sched = self.clock.time() + 60.0
        elif line.startswith(f"set {S['beacon_mode']} "):
            self.sched = None
        if w[0] == "set" and line.startswith(f"set {S['vox']} Off") and self.vkey and self.vkey["kind"] == "vox":
            self._end(self.clock.time())
        if w[0] == "slot" and len(w) > 2 and w[1].upper() == "C" and self._c_aprs():
            raise B.TermError(f"slot C: slot C held by {self._c_aprs()}")
        return out

    def _c_aprs(self):
        """What holds C as the APRS slot (decision 181): APRS mode (APRS-19), KISS (KISS-9) or automatic beacons
        (APRS-29), as the firmware's lock message names it; None when nothing does."""
        if self.set[S["aprs_mode"]] == "On":
            return "APRS mode (APRS-19)"
        if self.set[S["aprs_mode"]] == "KISS":
            return "KISS (KISS-9)"
        if self.set[S["aprs_mode"]] == "Tracker" and self.set[S["beacon_mode"]] not in ("Off", "Manual"):
            return "Tracker (APRS-29.2)"
        return None

    def slot(self, n):
        self._advance()
        d = super().slot(n)
        if n == 2 and self._c_aprs():
            d.update(freq10=B.mhz10(float(self.set[S["aprs_freq"]])), aprs=True, channel=None, offset=0, power=0)
        return d

    def keys(self, seq):
        for word in seq.split():
            self._advance()
            if self.key or self.vkey and self.vkey["kind"] in ("vox", "dial"):
                # LINK-19 as the firmware has it: keys are refused while a voice key-up holds the radio
                raise B.TermError("0x30: invalid: transmitting: remote keys wait for the unkey")
            name, _, how = word.partition(":")
            self._key(name.upper(), how)
            self.clock.sleep(0.12)
        self._advance()

    def _key(self, k, how):
        top = self.screen[-1]
        if k == "BACK":
            if how == "long":
                self.screen = ["Home"]
            elif len(self.screen) > 1:
                self.screen.pop()
            self.entry = ""
            return
        if k == "SIDE1" and top == "Home":
            self.monitor = not self.monitor
            return
        if top == "Home":
            if k == "MENU":
                self.screen.append("Quick")
            elif k.isdigit():
                self.screen.append("Entry")
                self.entry = k
            return
        if top == "Quick" and k == "0":
            self.screen[-1], self.entry = "More", ""  # the full menu's readback id
            return
        if top == "More":
            if k.isdigit():
                self.entry += k
                leaf = {"8 6 1": "Dtmf", "5 7 1": "FreqEdit", "3 4": "Hf", "5 1": "Heard"}.get(" ".join(self.entry))
                if leaf:
                    self.screen[-1], self.entry = leaf, ""
                elif " ".join(self.entry) == "8 2":
                    self.screen[-1], self.entry = "Group", ""
            return
        if top == "Heard" and k == "MENU" and how == "long":
            self.screen.append("Options")
            return
        if top == "Options" and k == "MENU":
            self.screen.pop()
            self._beacon(self.clock.time() + 0.2)
            return
        if top in ("Dtmf", "FreqEdit", "Hf", "Entry"):
            if k.isdigit() or k == "STAR":
                self.entry += "." if k == "STAR" else k
            elif k == "MENU" and top == "Dtmf":
                self._dial(self.entry)
            elif k == "MENU" and top == "FreqEdit":
                self.set[S["aprs_freq"]] = f"{float(self.entry):.3f}"
                self.screen.pop()
                self.entry = ""
            elif k == "MENU" and top == "Hf":
                self.hf_mhz = float(self.entry)
                self.entry = ""

    def ui(self):
        self._advance()
        b = self.slots[1]
        bstate = ("off" if (self.key or self.vkey) else "FM") if 87.5 <= b["freq"] <= 108 else "RX"
        sel = {"name": "Beacon now"} if self.screen[-1] == "Options" else None
        t, self.toast = self.toast, None
        u = {"screen": {"id": self.screen[-1]}, "cards": [{"slot": "A", "state": "RX"}, {"slot": "B", "state": bstate}],
             "toast": t, "sel": sel, "rows": []}
        if self.screen[-1] == "Hf":
            u["hf"] = {"args": [f"{self.hf_mhz:.3f}"]}
        return u

    def status(self):
        self._advance()
        k = self.key and "ABC"[self.active] or (self.vkey and self.vkey["slot"])
        b = self.slots[1]
        bstate = ("off" if (self.key or self.vkey) else "FM") if 87.5 <= b["freq"] <= 108 else "idle"
        return {"screen": self.screen[-1], "active": "ABC"[self.active], "keyed": k or None, "remote": bool(self.key),
                "mode": "off", "slots": {"A": {"state": "idle", "meter": 0}, "B": {"state": bstate, "meter": 0}},
                "xb": None}

    def reboot(self, after=None):
        if self.key or self.vkey:
            raise B.BenchError("REBOOT refused: busy")  # LINK-24.3
        if after:
            after()
        self.key = self.vkey = None
        self.screen = ["Home"]
        self.clock.sleep(1.2)
        self.t_boot = self.clock.time() - 0.4
        self.t_eval = self.clock.time()
        return self.clock.time()

    # -- key-ups --
    def key_on(self, slot_code):
        self._advance()
        if self.screen[-1] == "Hf":
            self.calls.append("KEY on")
            if self.hf_mhz < 18:
                return 9, "invalid: Receive only here"
            why = B.tx_expect(self.hf_mhz, self.region)
            return 9, f"invalid: {'outside the band plan' if why == 'band plan' else why}"
        if slot_code == 2 and self.set[S["aprs_mode"]] == "On":
            self.calls.append("KEY on")
            self.screen.append("Compose")
            return 9, "invalid: APRS mode: compose opened"
        r = super().key_on(slot_code)
        if self.key:
            self.key["t_voice"] = self.clock.time()
        return r

    def key_off(self):
        self._advance()
        k = self.key
        t = self.clock.time()
        if k:
            parts = []
            mic_from = t
            sp = int(self.set[S["dtmf_speed"]].split()[0]) / 1000
            mic_from = k["on"]
            end = t
            parts.append(self._voice_window(mic_from, t))
            et = self.set[S["end_tone"]]
            if k["kind"] == "carrier" and et != "Off":
                steps = dict(B.END_TONES)[dict(B.ENDTONE_SETTINGS)[et]]
                t1 = end + 0.2
                self.endtone_from, self.endtone_until = t1, t1 + sum(ms for _, ms in steps) / 1000
                parts.append(self._steps_fn(steps, t1))
                end = self.endtone_until
            if k["kind"] == "afsk":
                parts.append(lambda tt, a=k["on"], b=t: np.where((tt >= a) & (tt < b), 2500 * np.sin(2 * np.pi * 1200 * tt), 0))
            self.air.append((k["on"], end + 0.5, k["mhz"], parts))
            self.key = None
            self.watch_until = end + 30.0
            if self.set[S["after_voice"]] == "On" and self.set[S["aprs_mode"]] == "On" and k["kind"] == "carrier":
                self._owe(end + 0.5)
            self.calls.append("KEY off")
            return True
        return super().key_off()

    endtone_from = endtone_until = 0.0

    def _voice_window(self, a, b):
        aud = self.audio

        def f(t):
            out = np.zeros(len(t))
            for p0, p1, hz, db in aud.plays:
                m = (t >= max(a, p0)) & (t < min(b, p1))
                out[m] += 1500 * 10 ** (db / 40) * np.sin(2 * np.pi * hz * t[m])
            return out
        return f

    def _steps_fn(self, steps, t1):
        def f(t):
            out = np.zeros(len(t))
            s = t1
            for hz, ms in steps:
                m = (t >= s) & (t < s + ms / 1000)
                out[m] += B.TONE_19_4.get(hz, 1400) * np.sin(2 * np.pi * hz * (t[m] - s))
                s += ms / 1000
            return out
        return f

    def _dtmf_fn(self, digits, t0, sp):
        def f(t):
            out = np.zeros(len(t))
            for i, d in enumerate(digits):
                r = next(j for j, row in enumerate(DTMF_KEYS) if d in row)
                c = DTMF_KEYS[r].index(d)
                a = t0 + 2 * i * sp
                m = (t >= a) & (t < a + sp)
                out[m] += 1000 * (np.sin(2 * np.pi * DTMF_LOW[r] * t[m]) + np.sin(2 * np.pi * DTMF_HIGH[c] * t[m]))
            return out
        return f

    def _dial(self, digits):
        t = self.clock.time()
        sp = int(self.set[S["dtmf_speed"]].split()[0]) / 1000
        dl = int(self.set[S["dtmf_delay"]].split()[0]) / 1000
        s = self.slots[self.active]
        end = t + dl + 2 * sp * len(digits)
        self.vkey = dict(on=t, kind="dial", slot="ABC"[self.active], mhz=s["freq"], until=end,
                         parts=[self._dtmf_fn(digits, t + dl, sp)])

    def _beacon(self, t, info=None):
        if self.set[S["aprs_mode"]] != "On" and info is None:
            return
        frame = ax25_frame("APZFER", self.callsign + "-9", ["WIDE1-1", "WIDE2-1"], info or b"!4916.96N/12307.24W>Ferrite")
        txd = (int(self.set[S["tx_delay"]].split()[0]) if info is None else self.txd_kiss) / 1000
        bits = hdlc_bits(frame, 4)
        dur = txd + len(bits) / 1200
        mhz = float(self.set[S["aprs_freq"]])
        self.air.append((t, t + dur + 0.1, mhz, [lambda tt, b=bits, a=t + txd: afsk_dev(b, tt, a)]))
        self.vkey = dict(on=t, kind="beacon", slot="C", mhz=mhz, until=t + dur, parts=[])
        self.vkey_air = True
        if info is None:
            self.last_beacon = t

    def kiss_write(self, data):
        self._advance()
        self.kiss_buf += data
        while self.kiss_buf.count(b"\xc0") >= 2:
            a = self.kiss_buf.index(b"\xc0")
            b = self.kiss_buf.index(b"\xc0", a + 1)
            body = self.kiss_buf[a + 1:b].replace(b"\xdb\xdc", b"\xc0").replace(b"\xdb\xdd", b"\xdb")
            self.kiss_buf = self.kiss_buf[b + 1:]
            if not body or self.set[S["aprs_mode"]] != "KISS" or self.set[S["kiss_port"]] != "Bluetooth":
                continue
            if body[0] == 1:
                self.txd_kiss = body[1] * 10
            elif body[0] == 0:
                fr = body[1:]
                p = ax25_parse(fr)
                self._beacon(self.clock.time() + 0.05, info=p["info"])
                self.air[-1] = (self.air[-1][0], self.air[-1][1], self.air[-1][2],
                                [lambda tt, b=hdlc_bits(fr, 4), a=self.clock.time() + 0.05 + self.txd_kiss / 1000: afsk_dev(b, tt, a)])

    def _end_air(self):
        pass


class FakeKissPort:
    def __init__(self, radio):
        self.r = radio

    def write(self, b):
        self.r.kiss_write(b)


class FakeModesSdr:
    """IQ from the fake radio's air log: each key-up's deviation functions, FM onto a carrier 700 Hz off."""

    def __init__(self, radio, clock, seed=11):
        self.r, self.clock, self.rng = radio, clock, np.random.default_rng(seed)

    def start(self, mhz, seconds, path):
        self.mhz, self.seconds, self.path, self.t0 = mhz, seconds, path, self.clock.time()

    def finish(self):
        self.r._advance()
        fs = sdrtx.FS
        n = int(self.seconds * fs)
        fd = fs // sdrtx.DEC
        td = self.t0 + np.arange(n // sdrtx.DEC + 1) / fd
        dev = np.zeros(len(td))
        on = np.zeros(len(td), bool)
        for a, b, mhz, parts in list(self.r.air) + [(c[0], c[1], c[2], []) for c in self.r.carriers]:
            if abs(mhz - self.mhz) > 0.01 or b <= self.t0 or a >= self.t0 + self.seconds:
                continue
            m = (td >= a) & (td < b)
            on |= m
            for p in parts:
                dev[m] += p(td[m])
        dev += np.where(on, self.rng.normal(0, 10, len(td)), 0)
        raw = np.empty(2 * n, np.uint8)
        chunk, ph = 1 << 20, 0.0
        for a in range(0, n, chunk):
            idx = np.arange(a, min(a + chunk, n))
            j = idx // sdrtx.DEC
            d = np.where(on[j], dev[j] + 700.0, 0.0) - sdrtx.OFF
            p = ph + 2 * np.pi * np.cumsum(d) / fs
            ph = p[-1] % (2 * np.pi)
            x = np.where(on[j], 60.0, 0.0) * np.exp(1j * p) + self.rng.normal(0, 1.5, len(idx)) + 1j * self.rng.normal(0, 1.5, len(idx))
            raw[2 * a: 2 * idx[-1] + 2: 2] = np.clip(np.round(x.real + 127.5), 0, 255)
            raw[2 * a + 1: 2 * idx[-1] + 2: 2] = np.clip(np.round(x.imag + 127.5), 0, 255)
        return raw

    def stop(self):
        pass


def selftest(cfg, args_fn, check, tmp):
    """Every test of the suite on the fake radio, SDR and speaker; then the decoders on their own; then the duty
    log and the restore."""
    import argparse
    B.say("\n== modes-on-air: every test on the fake radio, SDR and speaker (virtual clock)")
    clk = B.FakeClock()
    aud = FakeAudio(clk)
    r = FakeModesRadio(clk, aud)
    r.region = 1
    r.slots[1] = dict(r.vfo_b)  # B on its VFO (a channel can't be put back by `slot`: the restore notes say so)
    before = (json.dumps(r.slots), dict(r.set), r.active)
    a = argparse.Namespace(**dict(vars(args_fn()), tests=None, kiss_ble="fake", slow=True, no_audio=False, record=False,
                                  keep_captures=False, build="selftest"))
    c2 = json.loads(json.dumps(cfg))
    c2["results"] = os.path.join(tmp, "modes-results")
    ex, page, m = run(c2, a, radio=r, sdr=FakeModesSdr(r, clk), clock=clk, audio=aud, kiss_port=FakeKissPort(r))
    sd = os.path.dirname(page)
    s0 = ex[f"suite:{SUITE}"]
    names = {t["name"]: t for t in m.results}
    for t in m.results:
        check(f"modes: {t['name']} ({', '.join(t['rules'])})", t["result"] == "pass", t["why"] or t["summary"])
    check("modes: no test skipped or in error on the fake", not s0["skipped"] and not s0["errors"],
          {**s0["skipped"], **s0["errors"]})
    missing = [n for n in TAGS if n not in names]
    check("modes: every tagged check ran", not missing, missing)
    after = (json.dumps(r.slots), dict(r.set), r.active)
    check("modes: everything put back (slots, settings, APRS frequency, active slot)", after == before,
          [{k: (v, after[1].get(k)) for k, v in before[1].items() if after[1].get(k) != v}, before[0], after[0],
           before[2], after[2]] if after != before else "")
    s = json.load(open(os.path.join(sd, "session.json")))
    d = s["duty"]
    check("modes: duty kept (bursts <= 5 s, window <= 25 %)", d["longest_s"] <= 5.0 + 1e-6 and d["peak_window_frac"] <= 0.25 + 1e-6,
          f"longest {d['longest_s']:.2f} s, peak {d['peak_window_frac']:.0%}, {d['keyups']} key-ups")
    keys = [x for x in r.air if True]
    bad = [x[2] for x in keys if all(abs(x[2] - f) > 1e-6 for f in cfg["test_freqs"])]
    check("modes: every carrier on an approved frequency", not bad, bad)
    rr = json.load(open(os.path.join(sd, "bench", "results.json")))
    check("modes: results.json carries the rules", set(rr["rules"]) >= {rid for n in TAGS for rid in TAGS[n]}, sorted(rr["rules"])[:8])
    # A failing radio: VOX that keys from a menu, a dialer that drops a digit.
    B.say("\n== modes-on-air: a broken fake radio fails the right checks")
    clk = B.FakeClock()
    aud = FakeAudio(clk)
    r = FakeModesRadio(clk, aud)
    r._blocked = lambda: False  # VOX keys anywhere
    a2 = argparse.Namespace(**dict(vars(a), tests="vox_home_only,vox_blocked"))
    ex2, page2, m2 = run(c2, a2, radio=r, sdr=FakeModesSdr(r, clk), clock=clk, audio=aud)
    res2 = m2.results
    check("modes: VOX keying from a menu fails vox_home_only and vox_blocked",
          not ex2[f"suite:{SUITE}"]["ok"] and all(t["result"] == "fail" for t in res2) and len(res2) == 2,
          [(t["name"], t["result"]) for t in res2])
    check("modes: ... and the stray VOX key-ups stopped and logged within the duty limit",
          json.load(open(os.path.join(os.path.dirname(page2), "session.json")))["duty"]["longest_s"] <= 5.0 + 1e-6)
    # The decoders on synthetic audio.
    fs = 64000
    tt = np.arange(int(fs * 2.0)) / fs
    fr = ax25_frame("APZFER", "VA7FER-9", ["WIDE1-1"], b"!4916.96N/12307.24W>test")
    x = afsk_dev(hdlc_bits(fr, 30), tt, 0.3) + np.random.default_rng(3).normal(0, 80, len(tt))
    got = afsk_decode(x, fs, [(0.25, 1.9)])
    check("decoder: AFSK1200 frame decoded byte for byte, its flags found", got and got[0]["frame"] == fr
          and abs(got[0]["t_flag"] - 0.3) < 0.01, got[:1])


# Names this suite's checks carry (bench_rules.RULE_TAGS has them).
TAGS = {k: v for k, v in bench_rules.RULE_TAGS.items() if k in {
    "vox_tail", "vox_active_slot", "vox_levels", "vox_thresholds", "vox_home_only", "vox_hold", "vox_blocked", "vox_boot",
    "dtmf_dial", "dtmf_timing", "dtmf_pairs", "tone11_no_end_after_dtmf", "end_tone_order",
    "tone11_no_end_after_afsk", "reg40_end_tone", "aprs_tx_delay", "aprs_beacon_on_c", "aprs_after_voice",
    "aprs_spacing", "aprs_compose", "gps33_no_position", "kiss_frames", "kiss_no_pin", "gps_pb10", "gps_during_tx",
    "watch_relay_keyup", "watch_pause_after_tx", "hf_refused_while_keyed", "hf_receive_only", "hf_chip1_refusal",
    "bcast_muted_keyed", "bcast_resumes", "bcast_card_keyed"}}
