#!/usr/bin/env python3
"""Ferrite's on-radio UI tests: remote keys and the screen read back over the link (LINK-19), checked against the
manual's rules. The tests are in tools/uitests/*.py, each tagged with the rule IDs it checks.

    tools/uitest.py --sim tcp:127.0.0.1:7300           # against the simulator (the reference)
    tools/uitest.py --sim auto                          # start a headless simulator for the run, then stop it
    tools/uitest.py                                     # against the radio on the cable (port from --port)
    tools/uitest.py --port ble:walkie-talkie            # over Bluetooth (bleak; 0 9 2 1 on; slower: 9600 baud)
    tools/uitest.py --list                              # the tests and their rules
    tools/uitest.py -k quick --rules MENU-1,HOME-*      # a selection (name substring, rule globs, --module menus)
    tools/uitest.py --sim auto --page zones,browse      # the tests that visit these screens (id or screen file)
    tools/uitest.py --sim auto --check-pages            # run all; report tests whose pages= tags differ from what
                                                        #   they visited (--fix-pages: write the tags from the run)
    tools/uitest.py --smoke                             # the short smoke set (bench.py runs it beside the SDR suite)
    tools/uitest.py --slow                              # on the radio, also the tests that wait on timers (menu
                                                        #   timeout, auto lock); the simulator always runs them
    tools/uitest.py --record                            # write a result row per rule into docs/test-plan/*.md
    tools/uitest.py --with reset,fm-station             # also the tests that need these (needs=, uitests/core.py
                                                        #   CAPS: a factory reset, a local FM station, ...)
    tools/uitest.py --bt ble:walkie-talkie              # a Bluetooth link beside the cable (0 9 2 1 on): needs=("bt",)
    tools/uitest.py --port ble:walkie-talkie --cablelog /dev/cu.usbserial-XXXX     # keys over Bluetooth while the
                                                        #   cable carries the cable log (0 9 9 5): needs=("cablelog",)
    tools/uitest.py --flash before --module install     # around a flash (tools/flash.py): what the install and
    tools/uitest.py --flash after --module install      #   update checks compare (needs=("flash",))

A run: connect (HELLO), read the build flavour (receive, tx, tx-dry), snapshot every setting (`dump`) and the slots
(`slot`), then each test from home (hold BACK); after each test the settings and slots are compared with the
snapshot and anything changed is put back (`set`, `slot`, V/M for a slot that was on a channel). At the end the
whole `dump` is compared again and any difference is reported (and fails the run).

Safe by construction: remote keys never transmit (LINK-19) but for the crossband repeater's # (XB-20.2), which only
the crossband tests press, and only on tx-dry (PA held at 0) with both sides on 145.495 and 446.475 MHz
(needs=("tx-dry", ...)); no test changes a setting that could transmit or take the cable away (uitests/core.py
UNSAFE). Remote PTT (KEY 0x80) is sent only by `T.keyup_refused`, for the
checks that a key-up is refused (needs=("keyup",)): on a tx-dry build (PA held at 0) or the simulator, never a real
transmit build, KEY off at once whatever the answer; a key-up that isn't refused fails the test.

Results: <results>/<date-time>_<build>/ui/results.json (each test: rules, pass/fail/skip, the checks, the readback
lines it looked at), ui/summary.html. --session DIR puts them under an existing session (bench.py does). With
--record each rule a run covered gets a row in its test-plan entry (date, firmware = commit + build, ✓/◐/✗, a short
note; tools/record_results.py, which replaces the rule's older automated row and never touches rows written by
hand); entries not covered are never touched. Simulator runs are never recorded.
"""
import argparse
import datetime
import fnmatch
import html
import importlib
import json
import os
import pkgutil
import re
import socket
import subprocess
import sys
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
FERRITE = os.path.dirname(HERE)
ROOT = os.path.dirname(FERRITE)
sys.path.insert(0, HERE)
import ferrite_link as fl  # noqa: E402
from uitests import core  # noqa: E402

RESULTS = os.path.join(ROOT, "bench-results")


def say(*a):
    print(*a, flush=True)


def load_tests():
    import uitests
    for m in pkgutil.iter_modules(uitests.__path__):
        if m.name != "core":
            importlib.import_module(f"uitests.{m.name}")
    return core.REGISTRY


def git(*a):
    r = subprocess.run(["git", "-C", FERRITE, *a], capture_output=True, text=True)
    return r.stdout.strip() if r.returncode == 0 else ""


def host_commit():
    if os.environ.get("FERRITE_COMMIT"):  # tools/remote.sh: the copy has no .git of its own
        return os.environ["FERRITE_COMMIT"]
    h = git("rev-parse", "--short=7", "HEAD") or "unknown"
    return h + ("+" if git("status", "--porcelain", "--untracked-files=no") else "")


class Radio(fl.Radio):
    """ferrite_link's Radio, keeping the cable: the link lets it go 10 s after the last frame (TERM-2)."""

    def __init__(self, port, pin=None):
        self.spec, self.pin = port, pin
        super().__init__(port, pin=pin, name=b"uitest.py")
        self.last = time.time()

    def reopen(self, settle=3.0, capture=0.0, timeout=25.0):
        """After a restart (REBOOT): the port closed, `capture` s of what the cable says read without a byte sent
        (the boot report: the stock bootloader's 200 ms listen must hear nothing, so the first write waits), then
        HELLO again until the radio answers. Returns the captured text (the simulator says nothing there)."""
        try:
            self.port.close()
        except OSError:
            pass
        text = b""
        if capture and not self.spec.startswith(("tcp:", "ble:")):
            import serial
            end = time.time() + capture
            with serial.Serial(self.spec, 115200, timeout=0.1) as s:
                while time.time() < end:
                    text += s.read(4096)
        else:
            time.sleep(settle)
        end = time.time() + timeout
        while True:
            try:
                fresh = fl.Radio(self.spec, pin=self.pin, name=b"uitest.py")
                break
            except (fl.RadioError, TimeoutError, OSError, SystemExit):
                if time.time() > end:
                    raise TimeoutError("no HELLO from the radio after the restart")
                time.sleep(1.0)
        self.port, self.link, self.build, self.bench = fresh.port, fresh.link, fresh.build, fresh.bench
        self.last = time.time()
        return text.decode("ascii", "replace")

    def _ok(self, ty, payload=b"", busy_s=3.0):
        if time.time() - self.last > 8:
            self.port.write(b"\x00")
            time.sleep(0.05)
        try:
            return super()._ok(ty, payload, busy_s)
        finally:
            self.last = time.time()


# ---- The snapshot: settings and slots, and putting them back ----

SET_LINE = re.compile(r"^set ((?:\d )*\d) ?(.*?)\s+# (.*)$")


class State:
    """Every setting (from `dump`), the slots (`slot`) and the active one, as at the run's start."""

    def __init__(self, r):
        self.r = r
        self.dump = self.r.cmd("dump")
        self.paths = {}  # path -> value
        self.names = {}  # path -> name
        for line in self.dump.splitlines():
            p = self.resolve(line)
            if p:
                path, val, name = p
                self.paths[path], self.names[path] = val, name
        self.slots = self.slot_lines()

    def resolve(self, line):
        """A `set` line's (path, value, name): the path is 0 + two or three digits; `get` settles which."""
        m = re.match(r"^set (0(?: \d){2,3})(?: (.*?))?\s+# (.*)$", line.rstrip())
        if not m:
            return None
        toks = line.split("#")[0].split()[1:]
        name = m.group(3).strip()
        for n in (3, 4):
            path = " ".join(toks[:n])
            if len(toks) < n or not all(t.isdigit() and len(t) == 1 for t in toks[:n]):
                continue
            if n == 4 and not toks[3].isdigit():
                break
            known = getattr(self, "names", {}).get(path)
            if known is not None:
                if known == name:
                    return path, " ".join(toks[n:]), name
                continue
            try:
                out = self.r.cmd(f"get {path}").strip()
            except fl.RadioError:
                continue
            g = core.GET.match(out)
            if g and g.group(1) == path and g.group(2) == name:
                return path, " ".join(toks[n:]), name
        return None

    def slot_lines(self):
        return [x.strip() for x in self.r.cmd("slot").splitlines() if x.strip()]

    def nondefault(self):
        out = {}
        for line in self.r.cmd("get").splitlines():
            m = re.match(r"^set (.*?)\s+# (.*)$", line.rstrip())
            if not m:
                continue
            body = m.group(1)
            for path in sorted(self.paths, key=len, reverse=True):
                if body.startswith(path + " ") or body == path:
                    out[path] = body[len(path):].strip()
                    break
        return out

    def settings_now(self):
        """Each setting's value now: the defaults from the snapshot's dump where `get` lists nothing."""
        nd = self.nondefault()
        return nd

    def restore(self, notes, base_nd):
        """Settings and slots back as at the start; what couldn't be is added to notes."""
        put = []
        now = self.nondefault()
        for path in sorted(set(now) | set(base_nd)):
            if now.get(path) == base_nd.get(path):
                continue
            want = self.paths.get(path)
            if want is None:
                notes.append(f"{path}: changed and not in the snapshot")
                continue
            try:
                self.r.cmd(f"set {path} {want}")
                put.append(f"{path} {self.names.get(path, '')} -> {want}")
            except fl.RadioError as e:
                notes.append(f"{path} {self.names.get(path, '')}: can't put back {want!r}: {e}")
        put += self.restore_slots(notes)
        return put

    def restore_slots(self, notes):
        now = self.slot_lines()
        same = lambda x, y: slot_fields(x)["cmp"] == slot_fields(y)["cmp"]  # noqa: E731
        if len(now) == len(self.slots) and all(same(w, h) for w, h in zip(self.slots, now)):
            return []
        put = []
        active = None
        for want, have in zip(self.slots, now):
            if want.startswith(want[0] + "*"):
                active = want[0]
            if same(want, have):
                continue
            L = want[0]
            w = slot_fields(want)
            h = slot_fields(have)
            if w["ch"] is not None:
                if h["ch"] != w["ch"]:
                    # Back to its channel: V/M on that slot (VFO-23: V/M goes back to the channel).
                    self.r.cmd(f"slot {L} active")
                    self.r.keys("BACK:long")
                    self.r.press("VM")
                    time.sleep(0.15)
                    h = slot_fields(self.r.cmd(f"slot {L}").strip())
                    if h["ch"] is not None and h["ch"] != w["ch"]:
                        # On another channel (its last one was lost, as after a reset): type its number (NAV-4).
                        self.r.keys(" ".join(str(w["ch"])) + " MENU")
                        time.sleep(0.3)
                        h = slot_fields(self.r.cmd(f"slot {L}").strip())
                    if h["ch"] != w["ch"]:
                        notes.append(f"slot {L}: was on channel {w['ch']}, now {h['line']}")
                        continue
                put.append(f"slot {L} channel {w['ch']}")
                continue
            line = f"slot {L} vfo {w['freq']} {w['mode'].lower()} {w['power']}"
            if "tone" in w["kv"]:
                line += f" tone {w['kv']['tone']}"
            else:
                line += f" txtone {w['kv'].get('txtone', 'off')} rxtone {w['kv'].get('rxtone', 'off')}"
            if "split" in w["kv"]:
                line += f" split {w['kv']['split']}"
            else:
                line += f" offset {w['kv'].get('offset', 'off')}"
            try:
                # The VFO's step isn't in the `slot` line, so a step left by a test (hold 9's copy of a 12.5 kHz
                # channel, VFO-23) went unseen and the next test on this simulator stepped 12.5 kHz (Nov 13, 2026:
                # knob_steps_and_bands saw 146.545). Through another band first: coming back brings the band's
                # step and mode for the region (NAV-2), then the line puts the rest back.
                self.r.cmd(f"slot {L} vfo {'146.52' if float(w['freq']) >= 400 else '446.0'}")
                self.r.cmd(line)
                put.append(line)
            except fl.RadioError as e:
                notes.append(f"slot {L}: {line}: {e}")
        if active:
            self.r.cmd(f"slot {active} active")
        now = self.slot_lines()
        for want, have in zip(self.slots, now):
            if slot_fields(want)["cmp"] != slot_fields(have)["cmp"]:
                notes.append(f"slot {want[0]}: was {want!r}, now {have!r}")
        return put


def slot_fields(line):
    m = re.match(r'([ABC])(\*?)\s+(?:(vfo|aprs)|ch (\d+)(?: "[^"]*")?)\s+(\d+\.\d+)\s*(.*)$', line)
    if not m:
        return dict(ch=None, freq=None, mode="", power="", kv={}, line=line, cmp=line)
    kv = dict(re.findall(r"(\w+)=(\S+)", m.group(6)))
    # "scanning" is the slot's own state (it may stop or start by itself on the simulator): not compared.
    cmp = re.sub(r"\s*scanning", "", line)
    if m.group(4):
        cmp = f"{m.group(1)}{m.group(2)} ch {m.group(4)}"
    return dict(ch=int(m.group(4)) if m.group(4) else None, freq=m.group(5), mode=kv.get("mode", "fm"),
                power=kv.get("power", "low"), kv=kv, line=line, cmp=cmp)


def dump_diff(a, b):
    """Lines of two dumps that differ (the header aside)."""
    la = [x for x in a.splitlines() if x and not x.startswith("#")]
    lb = [x for x in b.splitlines() if x and not x.startswith("#")]
    sa, sb = set(la), set(lb)
    return [f"- {x}" for x in la if x not in sb] + [f"+ {x}" for x in lb if x not in sa]


# ---- Pages: the screens a test visits ----

SCREENS_MOD = os.path.join(FERRITE, "crates", "ferrite-app", "src", "screens", "mod.rs")


def screen_modules():
    """{screen id: its file under crates/ferrite-app/src/screens/}: the `Screen` enum's variants (the readback's ids,
    uistate.rs) and the module each comes from; Home is a plain variant (home.rs), LowPower its id in Low Power Mode
    (PWR-24.7)."""
    src = open(SCREENS_MOD).read()
    body = src[src.index("pub enum Screen {"):]
    body = body[:body.index("\n}")]
    out = {"Home": "home", "LowPower": "home"}
    for m in re.finditer(r"^\s+(\w+)\((\w+)::\w+\),", body, re.M):
        out[m.group(1)] = m.group(2)
    # the readback names a few variants otherwise (uistate.rs `id`: Advanced reads "More")
    ids = open(os.path.join(os.path.dirname(SCREENS_MOD), "..", "uistate.rs")).read()
    for m in re.finditer(r'Screen::(\w+)\(_\) => "(\w+)"', ids):
        if m.group(1) != m.group(2) and m.group(1) in out:
            out[m.group(2)] = out.pop(m.group(1))
    return out


def page_ids(names, mods=None):
    """Screen ids for page names: an id in any case ("zones", "Browse"), or a screen file's stem ("groups" is Group,
    "transmit" is Transmit, TxRefused and Emergency)."""
    mods = mods or screen_modules()
    ids = set()
    for n in (x.strip() for x in names if x.strip()):
        hit = {i for i in mods if i.lower() == n.lower()} or {i for i, m in mods.items() if m == n.lower()}
        if not hit:
            raise SystemExit(f"--page {n}: no such screen; screens: {', '.join(sorted(mods))}")
        ids |= hit
    return ids


# ---- Running ----


# ---- --auto: the tests what changed touches ----

# (glob, test-name keywords or "module:NAME"/"page:NAME"/"full", why): every match adds. Paths in the ferrite checkout.
# A firmware or app file no row names runs everything (the safe side); host-only files (tests, simulators, PC tools,
# docs) run nothing but the smoke set.
AREAS = [
    ("firmware/src/tasks/ui.rs", ["backlight", "wake", "redraw", "dark", "brightness"], "the UI task: drawing, lights"),
    ("firmware/src/bus.rs", ["backlight", "wake", "redraw", "hint_bars"], "the display bus"),
    ("firmware/src/display.rs", ["backlight", "wake", "redraw", "hint_bars"], "the display"),
    ("drivers/st7789/*", ["backlight", "wake", "redraw", "hint_bars"], "the display driver"),
    ("firmware/src/hal/backlight.rs", ["backlight", "light", "key_lock"], "the backlight"),
    ("firmware/src/tasks/input.rs", ["module:keys", "knob"], "keys and the knob"),
    ("drivers/keypad/*", ["module:keys", "knob"], "the keypad"),
    ("firmware/src/tasks/gps.rs", ["gps_off", "nmea", "gga", "sky_set", "gps_icon"], "GPS"),
    ("crates/ferrite-core/src/gpspower.rs", ["module:gpsui", "gps", "nmea"], "GPS power"),
    ("crates/ferrite-radio/src/gps.rs", ["gps", "nmea"], "GPS pins"),
    ("drivers/nmea/*", ["module:gpsui", "gga", "sky"], "NMEA"),
    ("firmware/src/hal/uart.rs", ["gps_off", "nmea", "bt_terminal", "baud_switch", "one_request_in_flight"], "the UARTs"),
    ("firmware/src/tasks/link.rs", ["module:bluetooth", "module:linkproto"], "the link"),
    ("firmware/src/hal/audio.rs", ["aprs", "kept_chips", "decod", "pocsag", "same"], "audio capture and playback"),
    ("firmware/src/tasks/dsp.rs", ["aprs", "kept_chips", "decod", "pocsag", "same"], "DSP"),
    ("firmware/src/tasks/aprs.rs", ["aprs"], "APRS"),
    ("firmware/src/tasks/decode.rs", ["decod", "pocsag", "same"], "decoders"),
    ("firmware/src/hal/adc.rs", ["about_rows", "battery", "temp", "vox"], "the ADC"),
    ("firmware/src/hal/tone.rs", ["beep", "tone"], "beeps"),
    ("firmware/src/tasks/radio/*", ["battery_save", "si_input", "broadcast_never", "kept_chips", "amp_off", "relay_follows", "turns_on_a_shared"], "the Radio task"),
    ("crates/ferrite-radio/*", ["module:chips", "module:squelch"], "chip recipes"),
    ("drivers/bk4829/*", ["module:chips", "module:squelch", "module:watch"], "the BK4829"),
    ("drivers/si4732/*", ["module:bcast", "fm_", "broadcast"], "the SI4732"),
    ("crates/ferrite-core/src/rxsleep.rs", ["key_wakes_then_the_sleep", "battery_save", "watched_chips"], "Battery Save"),
    ("firmware/src/hal/clock.rs", ["module:restart", "option_bytes", "baud_switch", "bt_terminal"], "clocks"),
    ("firmware/src/board.rs", ["module:restart", "option_bytes"], "boot"),
    ("firmware/src/main.rs", ["module:restart", "option_bytes"], "boot"),
    ("crates/ferrite-app/src/screens/*", ["page:"], "a screen"),
    ("tools/uitests/*", ["changed:"], "UI tests: the tests whose code changed"),
    # The app, drawing and core logic run the same in the simulator: its full suite (`--sim auto`) covers them, so
    # on the radio only the tests of the rules on their changed lines (above) and the smoke set run. Run the sim
    # suite first.
    ("crates/ferrite-app/*", [], "app logic (sim-covered)"),
    ("crates/ferrite-ui/*", [], "drawing (sim-covered)"),
    ("crates/ferrite-core/*", [], "core logic (sim-covered)"),
    ("fonts/*", [], "fonts (sim-covered)"),
    ("assets/*", [], "assets (sim-covered)"),
    ("firmware/Cargo.toml", [], "features"),
    ("tools/*", [], "PC tools"),
    ("*/tests/*", [], "host tests"),
    ("*/tests.rs", [], "host tests"),
    ("*/tests_*.rs", [], "host tests"),
    ("sim/*", [], "the simulator"),
    ("sim-web/*", [], "the web simulator"),
    ("crates/ferrite-checks/*", [], "rule checks"),
    (".github/*", [], "CI"),
    ("*.md", [], "docs"),
]
RULE_ID = re.compile(r"\b([A-Z]{2,6}-\d+(?:\.\d+)*)\b")


def git(*args):
    r = subprocess.run(["git", "-C", FERRITE, *args], capture_output=True, text=True)
    return r.stdout if r.returncode == 0 else None


def last_ui_commit(results):
    """The commit of the newest on-radio UI run (session.json, source radio)."""
    best = None
    for name in sorted(os.listdir(results)) if os.path.isdir(results) else []:
        try:
            m = json.load(open(os.path.join(results, name, "session.json")))
        except (OSError, ValueError):
            continue
        if m.get("source") == "radio" and "ui" in (m.get("kinds") or []) and m.get("commit"):
            best = m["commit"].rstrip("+")
    return best


def changed_tests(base, path):
    """The test functions a change to a test module touched: a new or edited `def`, or a hunk inside one (git's
    hunk header names the function it falls in)."""
    names = set()
    diff = (git("diff", "-U0", f"{base}..HEAD", "--", path) or "") + (git("diff", "-U0", "HEAD", "--", path) or "")
    for line in diff.splitlines():
        m = re.match(r"^@@ .* @@\s*def (\w+)", line) or re.match(r"^\+\s*def (\w+)", line)
        if m:
            names.add(m.group(1))
    return names


def auto_select(tests, a):
    """(tests, lines of why): the smoke set; tests checking a rule ID on a changed line (or its parent rule); each
    changed file's area (AREAS); everything when a firmware/app file isn't in AREAS or there is no base."""
    base = a.since or last_ui_commit(a.results)
    why = []
    if not base or git("cat-file", "-e", f"{base}^{{commit}}") is None:
        return tests, [f"no base commit ({base or 'no earlier on-radio UI run'}): everything"]
    files = sorted({f for f in ((git("diff", "--name-only", f"{base}..HEAD") or "") + (git("diff", "--name-only", "HEAD") or "")).splitlines() if f})
    diff = (git("diff", "-U0", f"{base}..HEAD") or "") + (git("diff", "-U0", "HEAD") or "")
    rules = set()
    for line in diff.splitlines():
        if line[:1] in "+-" and not line.startswith(("+++", "---")):
            for r in RULE_ID.findall(line):
                rules.add(r)
                rules.add(r.split(".")[0])
    picked = {f"{t.module}.{t.name}": "smoke" for t in tests if t.smoke}
    full = []
    for f in files:
        rows = [(kw, w) for g, kw, w in AREAS if fnmatch.fnmatch(f, g)]
        if not rows:
            if f.startswith(("firmware/", "crates/", "drivers/")):
                full.append(f)
            continue
        for kws, w in rows:
            for kw in kws:
                stem = os.path.splitext(os.path.basename(f))[0]
                for t in tests:
                    tid = f"{t.module}.{t.name}"
                    hit = (kw == "page:" and stem in t.pages) or (kw == "changed:" and t.module == stem and t.name in changed_tests(base, f)) \
                        or (kw.startswith("module:") and len(kw) > 7 and t.module == kw[7:]) \
                        or (not kw.endswith(":") and not kw.startswith("module:") and kw.lower() in t.name.lower())
                    if hit:
                        picked.setdefault(tid, f"{w} ({f})")
    for t in tests:
        hit = [r for r in t.rules if r in rules or r.split(".")[0] in rules]
        if hit:
            picked.setdefault(f"{t.module}.{t.name}", "rules on changed lines: " + ", ".join(hit))
    why.append(f"since {base}: {len(files)} files changed, {len(rules)} rule IDs on changed lines")
    if any(fnmatch.fnmatch(f, g) for f in files for g in ("crates/ferrite-app/*", "crates/ferrite-ui/*", "crates/ferrite-core/*")):
        why.append("app/drawing/core changes count on the simulator's full suite (`--sim auto`) having passed on this commit")
    if full:
        why.append("firmware/app files no area names, so everything: " + ", ".join(full[:8]) + (" ..." if len(full) > 8 else ""))
        return tests, why
    sel = [t for t in tests if f"{t.module}.{t.name}" in picked]
    why += [f"  {k}: {v}" for k, v in sorted(picked.items())]
    return sel, why


def select(tests, a, sim):
    out = []
    pages = page_ids(a.page.split(",")) if getattr(a, "page", None) else None
    only = set(a.only.split(",")) if getattr(a, "only", None) else None
    for t in tests:
        if only is not None and f"{t.module}.{t.name}" not in only:
            continue
        if pages is not None and not pages & set(t.pages):
            continue
        if a.k and not any(k.lower() in t.name.lower() for k in a.k.split(",")):
            continue
        if a.module and t.module not in a.module.split(","):
            continue
        if a.rules and not any(fnmatch.fnmatch(r, g.strip()) for g in a.rules.split(",") for r in t.rules):
            continue
        if a.smoke and not t.smoke:
            continue
        if sim and core.radio_only(t):
            continue  # never the simulator's: not collected, so not a skip (--list marks them)
        out.append(t)
    return out


def run_one(r, state, tst, sim, build, a):
    base_nd = state.base_nd
    t = core.T(r, sim, build)
    t.state, t.opts = state, a
    unmet = core.unmet(tst.needs, build, sim, r.bench, a)
    res = dict(name=tst.name, module=tst.module, rules=tst.rules, where=tst.where, doc=tst.doc, pages=list(tst.pages))
    t0 = time.time()
    if tst.where == "radio" and sim:
        res.update(result="skip", why="radio only: the simulator doesn't model it")
    elif tst.where == "sim" and not sim:
        res.update(result="skip", why="simulator only")
    elif tst.slow and not a.slow and not sim:
        res.update(result="skip", why="slow (waits on a timer): --slow")
    elif unmet:
        res.update(result="skip", why=unmet)
    else:
        try:
            t.home()
            tst.fn(t)
            res["result"] = "pass"
        except core.Skip as e:
            res.update(result="skip", why=str(e))
        except core.Fail as e:
            res.update(result="fail", why=str(e))
        except (fl.RadioError, TimeoutError, OSError) as e:
            res.update(result="error", why=f"{type(e).__name__}: {e}")
        except Exception as e:  # a bug in the test: shown with its place
            res.update(result="error", why=f"{type(e).__name__}: {e}",
                       trace=traceback.format_exc(limit=-3))
        finally:
            t.end_test()
    if tst.xfail and res["result"] in ("fail", "pass"):
        res["result"] = "xfail" if res["result"] == "fail" else "xpass"
        res["known"] = tst.xfail
    res["secs"] = round(time.time() - t0, 2)
    res["visited"] = sorted(t.visited)
    res["checks"] = t.checks
    res["notes"] = t.notes
    res["evidence"] = t.evidence[-3:]
    if res["result"] in ("fail", "error") and t.last is not None:
        res["evidence"] = (t.evidence + [t.last.lines])[-3:] if t.last.lines not in t.evidence else t.evidence[-3:]
    # Put back what the test changed, from home.
    notes = []
    try:
        core.T(r, sim, build).home()
    except Exception as e:
        notes.append(f"home after the test: {e}")
    try:
        put = state.restore(notes, base_nd)
        if put:
            res["restored"] = put
    except Exception as e:
        notes.append(f"restore: {type(e).__name__}: {e}")
    if notes:
        res["restore_notes"] = notes
    return res


def run(a, port, sim, tests, sess_dir=None, log=say):
    r = Radio(port, pin=a.pin)
    build = r.build
    if a.expect_build and build != a.expect_build:
        log(f"warning: the radio runs a {build} build, not {a.expect_build}")
    info = r.link.ok(fl.INFO)
    version, _ = fl.text(info)
    commit = a.commit or host_commit()
    vlog = log if a.verbose else (lambda *_: None)  # quiet by default: the header lines are --verbose detail
    vlog(f"radio: Ferrite {version}, build {build}{' (simulator)' if sim else ''}; host commit {commit}")
    state = State(r)
    state.base_nd = state.nondefault()
    vlog(f"snapshot: {len(state.paths)} settings, slots {' | '.join(state.slots)}")
    results = []
    started = datetime.datetime.now()
    try:
        for tst in tests:
            res = run_one(r, state, tst, sim, build, a)
            results.append(res)
            mark = {"pass": "pass", "fail": "FAIL", "error": "ERROR", "skip": "skip", "xfail": "xfail",
                    "xpass": "XPASS"}[res["result"]]
            log(f"  [{mark:5s}] {tst.module}.{tst.name} ({', '.join(tst.rules)}) {res['secs']:.1f} s"
                + (f": {res.get('why')}" if res["result"] not in ("pass", "xpass") else "")
                + (f" [known: {res['known']}]" if res.get("known") else "")
                + (f"  [restored: {'; '.join(res['restored'])}]" if res.get("restored") and a.verbose else "")
                + (f"  [NOT restored: {'; '.join(res['restore_notes'])}]" if res.get("restore_notes") else ""))
            if res["result"] in ("fail", "error") and a.verbose:
                for line in (res["evidence"][-1] if res["evidence"] else []):
                    log(f"         | {line}")
                if res.get("trace"):
                    log(res["trace"])
            if a.stop and res["result"] in ("fail", "error"):
                break
    finally:
        left = []
        try:
            core.T(r, sim, build).home()
            notes = []
            state.restore(notes, state.base_nd)
            left = dump_diff(state.dump, r.cmd("dump")) + notes
        except Exception as e:
            left = [f"final check: {type(e).__name__}: {e}"]
        r.close()
    rules = {}
    for res in results:
        for rid in res["rules"]:
            prev = rules.get(rid)
            order = {"fail": 4, "error": 4, "xfail": 3, "xpass": 2, "pass": 2, "skip": 1}
            if prev is None or order[res["result"]] > order[prev["result"]]:
                rules[rid] = dict(result=res["result"], test=res["name"], why=res.get("why", ""))
    summary = dict(
        tool="uitest.py", started=started.isoformat(timespec="seconds"), port=port, sim=sim, build=build,
        version=version, commit=commit, label=a.build or f"{version}-{commit}-{build}",
        counts={k: sum(1 for x in results if x["result"] == k) for k in ("pass", "fail", "error", "skip", "xfail", "xpass")},
        rules=rules, left_changed=left, tests=results)
    return summary


def new_session(results_root, label):
    os.makedirs(results_root, exist_ok=True)
    while True:
        stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
        if not any(x.startswith(stamp) for x in os.listdir(results_root)):
            break
        time.sleep(0.3)
    session = f"{stamp}_{re.sub(r'[^A-Za-z0-9._-]+', '-', label)[:48]}"
    d = os.path.join(results_root, session)
    os.makedirs(d)
    return d


def write_results(summary, sess_dir):
    d = os.path.join(sess_dir, "ui")
    os.makedirs(d, exist_ok=True)
    json.dump(summary, open(os.path.join(d, "results.json"), "w"), indent=1, ensure_ascii=False)
    page = os.path.join(d, "summary.html")
    open(page, "w").write(page_html(summary))
    return d, page


def ui_section(summary, link_prefix=""):
    """The UI tests as a section of a summary page (bench.py's page uses it too)."""
    e = html.escape
    c = summary["counts"]
    cls = {"pass": "ok", "xpass": "ok", "fail": "bad", "error": "bad"}

    def pill(s):
        return f"<span class='pill {cls.get(s, 'warn')}'>{e(s)}</span>"
    out = [f"<h2>UI tests <span class=muted>· {e(summary['build'])} build{' · simulator' if summary['sim'] else ''}</span></h2>",
           "<div class=tiles>" + "".join(
               f"<div class=tile><div class=k>{k}</div><div class=v>{v}</div></div>"
               for k, v in (("Passed", c["pass"]), ("Failed", c["fail"] + c["error"]), ("Known deviations", c["xfail"]),
                            ("Skipped", c["skip"]),
                            ("Rules checked", sum(1 for x in summary["rules"].values() if x["result"] != "skip")))) + "</div>"]
    if summary.get("left_changed"):
        out.append("<div class=card><b>Left changed after the run</b><ul class=notes>"
                   + "".join(f"<li>{e(x)}</li>" for x in summary["left_changed"]) + "</ul></div>")
    rows = []
    for t in summary["tests"]:
        ev = ""
        if t["result"] in ("fail", "error") and t.get("evidence"):
            ev = "<details><summary>readback</summary><pre>" + e("\n".join(t["evidence"][-1])) + "</pre></details>"
        rows.append(f"<tr><td>{e(t['module'])}.{e(t['name'])}</td><td>{e(', '.join(t['rules']))}</td><td>{pill(t['result'])}</td>"
                    f"<td>{e(t.get('why', '') or t.get('doc', ''))}{' (known: ' + e(t['known']) + ')' if t.get('known') else ''}{ev}</td></tr>")
    out.append("<div class=card><div class=wrap><table><tr><th>test</th><th>rules</th><th></th><th>note</th></tr>"
               + "".join(rows) + "</table></div></div>")
    return "".join(out)


def page_html(summary):
    sys.path.insert(0, HERE)
    css = bench_css()
    head = (f"<h1>Ferrite UI tests</h1><p class=sub>{html.escape(summary['label'])} · {html.escape(summary['started'])} · "
            f"{html.escape(summary['port'])}</p>")
    return (f"<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'>"
            f"<title>Ferrite UI tests</title><style>{css}</style></head><body><main>{head}{ui_section(summary)}</main></body></html>")


def bench_css():
    """bench.py's page style (read from its source, so this tool doesn't need numpy)."""
    src = open(os.path.join(HERE, "bench.py")).read()
    m = re.search(r'^CSS = """(.*?)"""', src, re.S | re.M)
    return m.group(1) if m else "body{font:14px system-ui}"


def rule_key(rid):
    p, _, n = rid.partition("-")
    a, _, b = n.partition(".")  # a sub-rule, TX-30.1 (decision 160)
    return p, int(a) if a.isdigit() else 0, int(b) if b.isdigit() else -1


def manual_rules(docs):
    import pathlib
    import testplan
    rules, _, _ = testplan.parse_manual(pathlib.Path(docs) / "manual.md")
    return {r["id"] for r in rules}


# ---- Checking the pages= tags against a run ----


def page_report(summary):
    """(bad, unchecked): tests whose tags differ from the screens they read back, as (module, name, tagged,
    visited); and the tests the run couldn't check (skipped) with why."""
    known = set(screen_modules())
    bad, unchecked = [], []
    for t in summary["tests"]:
        tagged = set(t.get("pages") or ())
        unknown = tagged - known
        if t["result"] == "skip":
            unchecked.append((t["module"], t["name"], sorted(tagged), t.get("why", "")))
            if unknown:
                bad.append((t["module"], t["name"], sorted(tagged), [f"unknown: {', '.join(sorted(unknown))}"]))
            continue
        visited = set(t.get("visited") or ())
        if t["result"] in ("fail", "error") and visited <= tagged:
            continue  # stopped early: what it reached is all tagged
        if tagged != visited:
            bad.append((t["module"], t["name"], sorted(tagged), sorted(visited)))
    return bad, unchecked


def fix_pages(summary):
    """Writes each checked test's visited screens into its @test(... pages=(...)) in tools/uitests/<module>.py;
    returns how many changed."""
    n = 0
    for t in summary["tests"]:
        if t["result"] in ("skip", "fail", "error"):
            continue
        visited = sorted(t.get("visited") or ())
        if visited == sorted(t.get("pages") or ()):
            continue
        path = os.path.join(HERE, "uitests", f"{t['module']}.py")
        src = open(path).read()
        m = re.search(r"^@test\((.*)\)\n((?:@.*\n)*)def " + re.escape(t["name"]) + r"\(", src, re.M)
        if not m:
            say(f"  {t['module']}.{t['name']}: no single-line @test to fix")
            continue
        args = re.sub(r",?\s*pages=\([^)]*\)", "", m.group(1)).strip()
        tag = "pages=(" + ", ".join(f'"{p}"' for p in visited) + ("," if len(visited) == 1 else "") + ")"
        new = f"@test({args}{', ' if args else ''}{tag})"
        src = src[:m.start()] + new + "\n" + src[m.start(2):]
        open(path, "w").write(src)
        n += 1
    return n


# ---- Recording into the test plan ----


def record(results_json, docs):
    """--record: tools/record_results.py on this run (a newer automated row replaces older ones; hand rows kept)."""
    sys.path.insert(0, HERE)
    import record_results
    return record_results.record_paths([results_json], docs)


# ---- The simulator ----


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def start_sim(secs=3600, quiet=True, build=True, ports=None, extra=()):
    exe = os.path.join(FERRITE, "target", "debug", "ferrite-sim")
    if build:
        if not quiet:
            say("building the simulator (cargo build -p ferrite-sim) ...")
        b = subprocess.run(["cargo", "build", "-q", "-p", "ferrite-sim"], cwd=FERRITE)
        if b.returncode != 0 or not os.path.exists(exe):
            sys.exit("can't build the simulator")
    port, bt = ports or (free_port(), free_port())
    p = subprocess.Popen([exe, "--term-headless", str(secs), "--term-tcp", str(port), "--bt-tcp", str(bt), *extra],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for _ in range(50):
        try:
            socket.create_connection(("127.0.0.1", port), timeout=0.2).close()
            break
        except OSError:
            time.sleep(0.1)
    time.sleep(2.5)  # the link answers once the simulated radio has booted (0.5 s timed out HELLO)
    return p, f"tcp:127.0.0.1:{port}", f"tcp:127.0.0.1:{bt}"


def units(sel):
    """The -j work units: one test each, except modules that share radio state (SHARE), which go as one unit."""
    out, groups = [], {}
    for t in sel:
        g = next((g for g in SHARE if t.module in g), None)
        if g:
            groups.setdefault(g, []).append(t)
        else:
            out.append([t])
    return out + list(groups.values())


SHARE = [("zones",)]  # groups of modules that must run in one unit (channels 991-995 etc.)


def last_times():
    """Each test's seconds in the newest few results (to schedule the slowest first); 5 s when unknown."""
    import glob
    times = {}
    for f in sorted(glob.glob(os.path.join(RESULTS, "*", "ui", "results.json")), key=os.path.getmtime)[-6:]:
        try:
            for x in json.load(open(f)).get("tests", []):
                if x.get("secs"):
                    times[(x.get("module"), x.get("name"))] = x["secs"]
        except (OSError, ValueError):
            pass
    return times


def run_parallel(a, sel):
    """--sim auto -j N: N headless simulators kept up for the run; N workers take units (a test, or a SHARE group)
    from one queue, slowest first by the last runs' times, each running a uitest subprocess on its own simulator."""
    import tempfile
    import threading
    tmp = tempfile.mkdtemp(prefix="uitest-j-")
    order = {(t.module, t.name): i for i, t in enumerate(sel)}
    times = last_times()
    queue = sorted(units(sel), key=lambda u: -sum(times.get((t.module, t.name), 40 if t.slow else 5) for t in u))
    total = len(queue)
    lock, done, procs = threading.Lock(), [], []
    b = subprocess.run(["cargo", "build", "-q", "-p", "ferrite-sim"], cwd=FERRITE)  # once, before the workers start
    if b.returncode != 0:
        sys.exit("can't build the simulator")

    base = free_port()
    taken = set()

    def ports():
        # distinct ports for every simulator: free_port() alone can hand two workers the same one
        with lock:
            out = []
            p = base
            while len(out) < 2:
                p += 1
                if p in taken:
                    continue
                try:
                    with socket.socket() as sk:
                        sk.bind(("127.0.0.1", p))
                except OSError:
                    continue
                taken.add(p)
                out.append(p)
            return tuple(out)

    def worker(w):
        try:
            sim_proc, port, bt = start_sim(quiet=True, build=False, ports=ports())
        except BaseException as e:  # noqa: BLE001 (a worker that can't start says so; the others go on)
            with lock:
                done.append((f"w{w}-start", [f"worker {w} start"], f"worker {w}: {e!r}"))
            return
        with lock:
            procs.append(sim_proc)
        k = 0
        while True:
            with lock:
                if not queue:
                    return
                u = queue.pop(0)
            k += 1
            tag = f"w{w}-{k}"
            extra = next((t.sim_args for t in u if t.sim_args), ())
            own = None
            if extra:
                # The state the test needs (sim_args): a simulator of its own, for this unit alone.
                try:
                    own = start_sim(quiet=True, build=False, ports=ports(), extra=extra)
                except BaseException as e:  # noqa: BLE001
                    with lock:
                        done.append((tag, [t.name for t in u], f"simulator {' '.join(extra)}: {e!r}"))
                    continue
            u_port, u_bt = (own[1], own[2]) if own else (port, bt)
            cmd = [sys.executable, os.path.abspath(__file__), "--sim", u_port, "--bt", u_bt,
                   "--only", ",".join(f"{t.module}.{t.name}" for t in u),
                   "--session", os.path.join(tmp, tag), "--results", tmp]
            for flag, opt in (("page", "--page"), ("build", "--build"), ("commit", "--commit"),
                              ("expect_build", "--expect-build"), ("with_", "--with")):
                if getattr(a, flag, None):
                    cmd += [opt, getattr(a, flag)]
            for flag in ("smoke", "slow", "stop", "verbose"):
                if getattr(a, flag, False):
                    cmd.append("--" + flag)
            try:
                r = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
            finally:
                if own:
                    own[0].terminate()
            out = r.stdout + ("" if r.returncode in (0, 1) else f"\n(exit {r.returncode})")
            with lock:
                done.append((tag, [t.name for t in u], out))

    try:
        threads = [threading.Thread(target=worker, args=(w,)) for w in range(max(1, min(a.jobs, len(queue))))]
        for th in threads:
            th.start()
        for th in threads:
            th.join()
        merged = None
        rank = {"fail": 4, "error": 4, "xfail": 3, "xpass": 2, "pass": 2, "skip": 1}
        for tag, names, out in done:
            lines = out.splitlines()
            rj = os.path.join(tmp, tag, "ui", "results.json")
            if not os.path.exists(rj):
                say(f"unit {','.join(names)} gave no results; its output ends:\n  " + "\n  ".join(lines[-8:]))
                continue
            for x in lines:
                if x.startswith("  [") or x.startswith(("warning", "left changed")) or (
                        a.verbose and not x.startswith(("ui tests:", "results:", "page:"))):
                    say(x)
            sm = json.load(open(rj))
            if merged is None:
                merged = sm
                merged["left_changed"] = [f"[{names[0]}] {x}" for x in sm["left_changed"]]
                continue
            merged["tests"] += sm["tests"]
            merged["left_changed"] += [f"[{names[0]}] {x}" for x in sm["left_changed"]]
            for rid, v in sm["rules"].items():
                prev = merged["rules"].get(rid)
                if prev is None or rank[v["result"]] > rank[prev["result"]]:
                    merged["rules"][rid] = v
        ran = sum(1 for tag, _, _ in done if not tag.endswith("-start"))
        if ran < total:
            say(f"warning: {total - ran} of {total} units never ran (see the worker lines above)")
        if merged is None:
            sys.exit("no unit produced results")
        merged["tests"].sort(key=lambda r: order.get((r["module"], r["name"]), 1 << 30))
        merged["counts"] = {k: sum(1 for x in merged["tests"] if x["result"] == k)
                            for k in ("pass", "fail", "error", "skip", "xfail", "xpass")}
        merged["port"] = f"auto -j {a.jobs}"
        return merged
    finally:
        for sp in procs:
            sp.terminate()
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)


def main_args(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", help="the cable, ble:NAME_OR_ADDRESS for Bluetooth, or a radio's name (radios.toml); default: "
                    "FERRITE_PORT, radios.toml's default, the only cable")
    ap.add_argument("--sim", metavar="tcp:HOST:PORT|auto", help="the simulator's link instead of the radio")
    ap.add_argument("--pin", help="the radio's PIN (Bluetooth with a PIN lock)")
    ap.add_argument("-k", help="tests whose name has one of these (comma list)")
    ap.add_argument("--module", help="tests in these modules (comma list: menus, home, keys, ...)")
    ap.add_argument("--only", help=argparse.SUPPRESS)  # exact module.name list (the -j workers)
    ap.add_argument("--rules", help="tests checking a rule matching one of these globs (MENU-*,HOME-17)")
    ap.add_argument("--page", help="tests that visit these screens (comma list: readback ids or screen files, "
                    "e.g. zones,browse)")
    ap.add_argument("--check-pages", action="store_true", help="after the run: tests whose pages= tags differ from "
                    "the screens they read back (exit 1 if any)")
    ap.add_argument("--fix-pages", action="store_true", help="after the run: write the screens each test visited "
                    "into its pages= tag")
    ap.add_argument("--smoke", action="store_true", help="the smoke set only")
    ap.add_argument("--auto", action="store_true", help="only the tests the changes since the last on-radio run "
                    "touch: the smoke set, tests of rule IDs in the changed lines, and each changed area's tests")
    ap.add_argument("--since", help="--auto: the commit to compare with (default: the last on-radio UI run's)")
    ap.add_argument("--slow", action="store_true", help="also the tests that wait on timers (on the radio; the "
                    "simulator always runs them)")
    ap.add_argument("--list", action="store_true", help="list the tests and the rules they check")
    ap.add_argument("--results", default=RESULTS, help="results root (default %(default)s)")
    ap.add_argument("--session", help="an existing session directory to write ui/ into")
    ap.add_argument("--build", help="the build's label for the results directory")
    ap.add_argument("--commit", help="the firmware's commit (default: the host checkout's)")
    ap.add_argument("--expect-build", choices=["receive", "tx", "tx-dry", "tx+dry"], help="warn when the radio runs another")
    ap.add_argument("--record", action="store_true", help="a result row per rule into the test plan")
    ap.add_argument("--docs", default=os.path.join(ROOT, "docs"), help="the docs checkout for --record")
    ap.add_argument("--with", dest="with_", default="", help="also run the tests needing these (comma list: "
                    "uitests/core.py CAPS, e.g. reset,fm-station,signal)")
    ap.add_argument("--bt", help="a Bluetooth link beside the cable for needs=(\"bt\",) tests: ble:NAME (the radio's "
                    "0 9 2 1 on) or tcp:HOST:PORT (a simulator's --bt-tcp; --sim auto starts one)")
    ap.add_argument("--cablelog", metavar="PORT", help="the cable, read without a byte sent, for the cable log "
                    "(0 9 9 5) while --port ble:... has the keys: needs=(\"cablelog\",) tests")
    ap.add_argument("--flash", choices=["before", "after"], help="the install and update checks around a flash: "
                    "`before` keeps what they compare (--flash-state), `after` compares (needs=(\"flash\",))")
    ap.add_argument("--flash-state", default=os.path.join(RESULTS, "flash-state.json"),
                    help="where --flash before keeps its readings (default %(default)s)")
    ap.add_argument("--backup", default=os.path.join(ROOT, "backups", "blackradio", "flash.bin"),
                    help="this radio's whole-flash backup, read (never written) for the calibration bytes that "
                    "squelch checks compare with (default %(default)s)")
    ap.add_argument("--stop", action="store_true", help="stop at the first failure")
    ap.add_argument("--verbose", "-v", action="store_true", help="full output: header lines, restore detail, "
                    "the readback of a failure (default: one line per test and the summary)")
    ap.add_argument("-j", "--jobs", type=int, default=1, help="with --sim auto: run the modules in N parallel "
                    "simulators (a module is never split); default 1")
    return ap.parse_args(argv)


def main(argv=None):
    a = main_args(argv)
    tests = load_tests()
    if a.auto:
        tests, why = auto_select(tests, a)
        for w in why:
            say(w)
    if a.list:
        sel = select(tests, a, False)
        for t in sel:
            say(f"{t.module}.{t.name:40s} {', '.join(t.rules)}{'  [radio only]' if core.radio_only(t) else ''}"
                f"{'  [slow]' if t.slow else ''}{'  [smoke]' if t.smoke else ''}"
                f"  pages: {', '.join(t.pages) if t.pages else '(untagged)'}")
        ids = sorted({r for t in sel for r in t.rules}, key=rule_key)
        say(f"{len(sel)} tests, {len(ids)} rules: {' '.join(ids)}")
        unknown = [i for i in ids if i not in manual_rules(a.docs)]
        if unknown:
            say(f"rules the manual doesn't have: {' '.join(unknown)}")
            return 1
        known = set(screen_modules())
        bad_pages = sorted({p for t in sel for p in t.pages if p not in known})
        if bad_pages:
            say(f"pages that aren't screens: {' '.join(bad_pages)}")
            return 1
        return 0
    proc = None
    try:
        a.port = (a.port or "") if a.sim else fl.resolve_port(a.port)  # a name, FERRITE_PORT, the default, the cable
        if a.cablelog:
            a.cablelog = fl.resolve_port(a.cablelog, cable_only=True)
    except fl.PortError as e:
        say(str(e))
        return 2
    port, sim = a.port, a.port.startswith("tcp:") or bool(a.sim)
    sel = select(tests, a, sim)
    # --sim auto: in parallel simulators with -j N, and whenever a test needs a simulator of its own (sim_args).
    parallel = a.sim == "auto" and (a.jobs > 1 or any(t.sim_args for t in sel))
    if a.sim:
        if a.sim == "auto":
            if parallel:
                port = "auto -j %d" % a.jobs
            else:
                proc, port, sim_bt = start_sim(quiet=not a.verbose)
                a.bt = a.bt or sim_bt
        else:
            port = a.sim
    if a.record and sim:
        sys.exit("--record on the simulator: simulator runs are never recorded (the test plan holds radio results)")
    if sim:
        left_out = [t for t in select(tests, a, False) if core.radio_only(t)]
        if left_out:
            say(f"{len(left_out)} radio-only tests not collected on the simulator (`--list` marks them [radio only])")
    try:
        summary = run_parallel(a, sel) if parallel else run(a, port, sim, sel)
    finally:
        if proc:
            proc.terminate()
    sess = a.session or new_session(a.results, summary["label"] + ("-sim" if sim else ""))
    d, page = write_results(summary, sess)
    if not a.session:
        json.dump(dict(tool="uitest.py", kinds=["ui"], commit=summary["commit"], build=dict(label=summary["label"],
                       flavour=summary["build"], version=summary["version"]), source="simulator" if sim else "radio",
                       port=port, ui=summary["counts"]), open(os.path.join(sess, "session.json"), "w"), indent=1)
    c = summary["counts"]
    rules_ok = sorted(k for k, v in summary["rules"].items() if v["result"] in ("pass", "xpass"))
    rules_bad = sorted(k for k, v in summary["rules"].items() if v["result"] in ("fail", "error"))
    rules_known = sorted(k for k, v in summary["rules"].items() if v["result"] == "xfail")
    say(f"\nui tests: {c['pass']} pass, {c['fail']} fail, {c['error']} error, {c['skip']} skipped"
        + (f", {c['xfail']} known deviations (xfail)" if c["xfail"] else "") + (f", {c['xpass']} XPASS (drop the mark)" if c["xpass"] else "")
        + "; "
        f"rules: {len(rules_ok)} pass" + (f", {len(rules_bad)} fail ({' '.join(rules_bad)})" if rules_bad else "")
        + (f", {len(rules_known)} with a known deviation ({' '.join(rules_known)})" if rules_known else ""))
    if summary["left_changed"]:
        say("left changed after the run:\n  " + "\n  ".join(summary["left_changed"]))
    say(f"results: {d}/results.json\npage: {page}")
    pages_bad = False
    if a.check_pages or a.fix_pages:
        bad, unchecked = page_report(summary)
        if a.fix_pages:
            say(f"pages: {fix_pages(summary)} tags written from the run")
        elif bad:
            pages_bad = True
            say(f"pages: {len(bad)} tests' tags differ from what they visited:")
            for mod, name, tagged, visited in bad:
                say(f"  {mod}.{name}: tagged {', '.join(tagged) or '(none)'}; visited {', '.join(visited) or '(none)'}")
        else:
            say(f"pages: every checked test's tags match what it visited ({len(summary['tests']) - len(unchecked)} tests)")
        if unchecked:
            say(f"pages: {len(unchecked)} not checked here (skipped): "
                + "; ".join(f"{m}.{n} [{', '.join(p) or 'untagged'}]" for m, n, p, _ in unchecked))
    if a.record:
        for line in record(os.path.join(d, "results.json"), a.docs):
            say(line)
    return 0 if not (c["fail"] or c["error"] or summary["left_changed"] or pages_bad) else 1


if __name__ == "__main__":
    sys.exit(main())
