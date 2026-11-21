"""Squelch regression on a radio with the bladeRF (SQL-*, TONE-*, WATCH-17.5, PWR-*): the radio on the cable is
polled over the link (its slots' card states, about 10 a second) while the Xeon's bladeRF sends sqgen.py's files
(already in XEON:/tmp/sq/, see sqgen.py) on 446.475 MHz.

  sqreg.py PORT [--only NAME,NAME] [--levels 1,3,5,7,9]

Tests: quiet, chip and soft (the level sweep: needs a quiet floor; a bench at -98 dBm puts every level's RSSI
threshold under it, so only the noise threshold decides), level0, monitor, ctcss and dcs (the chip's audio, REG_47),
save and watch (Battery Save Off-3, single slot and watch A+B), scan, scanresume, counter (TSRCH-10: 70 cm reads;
33 and 23 cm reach chip 1 too weak through the front end), xb, bcast (BCR-24), busy.

Every setting and slot it changes is put back at the end. Remote PTT (busy lockout) only with Dry TX on.
"""
import argparse, json, os, subprocess, sys, time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from ferrite_link import Radio, parse_status  # noqa: E402

XEON = os.environ.get("FERRITE_XEON", "infamy@192.168.7.162")
MHZ = 446.475
# Power steps (dB, txvga1 + txvga2) for the level sweep.
STEPS = [-35, -30, -26, -22, -19, -16, -13, -11, -9, -7, -5, -4, 0, 4, 8, 12, 16, 21]


# The level sweep: the signal from -75 dB (relative to c4 at the lowest gains being -35) up, 3 dB a step: the
# weaker files (c4a20, c4a40: 20 and 40 dB down) carry it below the bladeRF's lowest gain.
SWEEP = list(range(-75, -3, 3))


def sweep_file(eff):
    for att, name in ((40, "c4a40"), (20, "c4a20"), (0, "c4")):
        if -35 <= eff + att <= -4:
            return name, eff + att
    raise ValueError(eff)


def gains(p):
    g1 = max(-35, min(-4, p))
    return g1, max(0, min(25, p - g1))


def blade(name, p, repeat=1, mhz=None):
    g1, g2 = gains(p)
    f = int(round((mhz or MHZ) * 1e6)) - 200_000
    cmd = (f"set frequency tx {f}; set samplerate tx 1000000; set bandwidth tx 1500000; set txvga1 {g1}; "
           f"set txvga2 {g2}; tx config file=/tmp/sq/{name}.sc16 format=bin repeat={repeat}; tx start; tx wait")
    return subprocess.Popen(["ssh", XEON, f"bladeRF-cli -e '{cmd}'"], stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL)


class Bench:
    def __init__(self, port):
        self.r = Radio(port)
        self.put_back = []  # (kind, arg)
        self.results = []

    def cmd(self, line):
        out = self.r.cmd(line)
        if "!" in out.split("\n", 1)[0][:3]:
            raise RuntimeError(f"{line}: {out}")
        return out

    def get(self, path):
        out = self.r.cmd(f"get {path}").strip()
        return out.split(": ", 1)[1].strip()

    def set(self, path, value):
        old = self.get(path)
        if not any(k == "set" and a[0] == path for k, a in self.put_back):
            self.put_back.append(("set", (path, old)))
        if old != value:
            self.cmd(f"set {path} {value}")

    def status(self):
        return parse_status(self.r.status_text())

    def home(self):
        self.r.keys("BACK:long")
        time.sleep(0.3)

    def level(self):
        self.home()
        self.r.keys("MENU")  # the active slot's quick settings: the Squelch row
        v = None
        for row in self.r.ui().get("rows", []):
            if row.get("name") == "Squelch":
                v = row.get("value")
        if v is None:  # the readback's shape: look the row up in the text
            t = self.r.ui_text()
            for ln in t.splitlines():
                if "Squelch" in ln and "value=" in ln:
                    v = ln.split('value="', 1)[1].split('"', 1)[0]
        self.home()
        return v

    def set_level(self, n):
        self.home()
        self.r.keys("MENU")
        self.r.keys(f"MENU {n} MENU")
        self.home()

    def speaker(self, chip=1):
        """The chip's receive audio (REG_47[11:8]: 0 muted, 1 the demodulated audio, as the Radio task's af_out):
        True while it plays. Chip 1 serves 446.475 (VFO-3)."""
        v = int(self.r.cmd(f"bk {chip} 47").split("=")[1].split()[0], 16)
        return (v >> 8) & 15 == 1

    def watch(self, proc, slot="A", after=2.0, max_s=60, spk=False):
        """Poll until the bladeRF is done and `after` more seconds: [(t, state, meter)] for the slot."""
        out, t0, done_at = [], time.time(), None
        while True:
            now = time.time()
            if done_at is None and proc.poll() is not None:
                done_at = now
            if done_at is not None and now - done_at > after or now - t0 > max_s:
                break
            try:
                s = self.status()["slots"].get(slot, {})
                st = s.get("state")
                if spk:  # the tone tests: the speaker, not the card (the card's RX is the carrier, SQL-13.1)
                    st = "audio" if self.speaker() else st
                out.append((now - t0, st, s.get("meter")))
            except Exception as e:  # noqa: BLE001
                out.append((now - t0, f"err {e}", None))
            time.sleep(0.08)
        if proc.poll() is None:
            proc.kill()
        return out, (done_at or time.time()) - t0

    def record(self, name, ok, detail):
        print(f"{'PASS' if ok else 'FAIL'}  {name}: {detail}", flush=True)
        self.results.append({"test": name, "ok": ok, "detail": detail})


def episodes(trace, state="RX"):
    n, prev = 0, False
    for _, st, _ in trace:
        on = st == state
        n += on and not prev
        prev = on
    return n


def states(trace):
    return sorted({st for _, st, _ in trace})


def peak(trace):
    return max((m or 0) for _, _, m in trace)


def final(trace, tail_s=0.5):
    end = trace[-1][0]
    return {st for t, st, _ in trace if t >= end - tail_s}


def play(b, name, p, slot="A", repeat=1, after=2.0, spk=False):
    proc = blade(name, p, repeat)
    return b.watch(proc, slot, after, spk=spk)


# ---- the tests ---------------------------------------------------------------------------------------------------

def single_slot(b, tone="off"):
    b.set("0 1 1", "Off")  # watch off
    b.cmd(f"slot A vfo {MHZ} nfm low tone {tone} offset off")
    b.cmd("slot A active")
    b.home()


def t_quiet(b):
    """No signal: stays shut for 6 s at the level now (no false opens)."""
    proc = subprocess.Popen(["sleep", "6"])
    tr, _ = b.watch(proc, after=0)
    b.record("quiet", episodes(tr) == 0, f"states {states(tr)}")


def sweep(b, mode, levels):
    """The weakest signal that opens, per level: rises with the level; every open closes after the signal."""
    b.set("0 9 5", mode)
    single_slot(b)
    th = {}
    for lv in levels:
        b.set_level(lv)
        th[lv] = None
        for eff in SWEEP:
            name, p = sweep_file(eff)
            tr, _ = play(b, name, p)
            if episodes(tr):
                th[lv] = eff
                shut = "RX" not in final(tr)
                b.record(f"{mode} level {lv}: closes after", shut, f"open at {eff} dB, final {sorted(final(tr))}")
                n2, p2 = sweep_file(min(eff + 6, SWEEP[-1]))  # well above the threshold: one clean open
                tr2, _ = play(b, n2, p2)
                b.record(f"{mode} level {lv}: opens again 6 dB up", episodes(tr2) == 1,
                         f"{episodes(tr2)} opens, states {states(tr2)}")
                break
        print(f"  {mode} level {lv}: opens at {th[lv]} dB", flush=True)
    got = [th[lv] for lv in levels]
    mono = all(a is not None and c is not None and a <= c for a, c in zip(got, got[1:])) and got[0] < got[-1] and got[0] > SWEEP[0]
    b.record(f"{mode} thresholds rise with the level", mono, json.dumps(th))
    return th


def t_level0(b):
    b.set_level(0)
    proc = subprocess.Popen(["sleep", "3"])
    tr, _ = b.watch(proc, after=0)
    b.record("level 0 is open with no signal", "RX" in states(tr), f"states {states(tr)}")


def t_ctcss(b, p):
    single_slot(b, "off")
    b.cmd("slot A rxtone ctcss 88.5")
    b.home()
    tr, _ = play(b, "ct885", p, spk=True)
    b.record("CTCSS 88.5 on an 88.5 slot opens", episodes(tr, 'audio') == 1 and "audio" not in final(tr),
             f"{episodes(tr, 'audio')} opens, states {states(tr)}, final {sorted(final(tr))}")
    tr, _ = play(b, "ct100", p, spk=True)
    b.record("CTCSS 100.0 on an 88.5 slot stays shut", episodes(tr, 'audio') == 0, f"states {states(tr)}, peak {peak(tr)}")
    tr, _ = play(b, "c4", p, spk=True)
    b.record("no tone on an 88.5 slot stays shut", episodes(tr, 'audio') == 0, f"states {states(tr)}, peak {peak(tr)}")
    tr, _ = play(b, "ct885gap", p, spk=True)
    b.record("88.5 with a 60 ms tone gap each second: one open (ToneHold)", episodes(tr, 'audio') == 1,
             f"{episodes(tr, 'audio')} opens, states {states(tr)}")
    tr, _ = play(b, "ct885t55", p, spk=True)
    b.record("88.5 with a 55 Hz tail: opens, then shut", episodes(tr, 'audio') == 1 and "audio" not in final(tr),
             f"{episodes(tr, 'audio')} opens, final {sorted(final(tr))}")


def t_dcs(b, p):
    single_slot(b, "off")
    seen = {}
    for code in ("dcs-n 023", "dcs-i 023"):
        b.cmd(f"slot A rxtone {code}")
        b.home()
        for f in ("d023n", "d023i"):
            tr, _ = play(b, f, p, spk=True)
            seen[(code, f)] = episodes(tr, 'audio')
            print(f"  slot {code}, sent {f}: {episodes(tr, 'audio')} opens, states {states(tr)}", flush=True)
    # Polarity on air is a convention: one file opens the N slot, the other opens the I slot, never both.
    n_ok = (seen[("dcs-n 023", "d023n")] == 1) != (seen[("dcs-n 023", "d023i")] == 1)
    i_ok = (seen[("dcs-i 023", "d023n")] == 1) != (seen[("dcs-i 023", "d023i")] == 1)
    cross = seen[("dcs-n 023", "d023n")] != seen[("dcs-i 023", "d023n")]
    b.record("DCS 023: each polarity opens on one word only, and the two slots differ", n_ok and i_ok and cross,
             json.dumps({f"{k[0]}/{k[1]}": v for k, v in seen.items()}))


def t_watch(b, p, save):
    b.set("0 7 7 1", save)
    b.set("0 1 1", "A+B")
    b.cmd(f"slot A vfo {MHZ} nfm low tone off offset off")
    b.cmd("slot B vfo 146.52 nfm low tone off offset off")
    b.cmd("slot B active")
    b.home()
    time.sleep(3)  # let Battery Save settle into its cycle
    tr, _ = play(b, "burst", p, repeat=3, after=1.0)
    b.record(f"watch A+B, Battery Save {save}: 3 bursts on watched A open 3 times", episodes(tr) == 3,
             f"{episodes(tr)} opens, states {states(tr)}")


def t_save_single(b, p, save):
    b.set("0 7 7 1", save)
    single_slot(b)
    time.sleep(3)
    tr, _ = play(b, "burst", p, repeat=3, after=1.0)
    b.record(f"single slot, Battery Save {save}: 3 bursts open 3 times", episodes(tr) == 3,
             f"{episodes(tr)} opens, states {states(tr)}")


def t_monitor(b):
    single_slot(b)
    b.r.keys("SIDE1:down")
    time.sleep(1.2)
    s1 = b.status()["slots"]["A"]["state"]
    b.r.keys("SIDE1:up")
    time.sleep(0.6)
    s2 = b.status()["slots"]["A"]["state"]
    b.record("SIDE1 held opens a quiet channel, let go shuts it", s1 in ("RX", "MON") and s2 == "idle",
             f"held {s1}, let go {s2}")


def t_busy(b, p):
    """Busy lockout: PTT refused on a carrier, allowed on a quiet channel (Dry TX)."""
    from ferrite_link import KEY, PTT, PTT_ON, PTT_OFF, SLOTS, why
    if not b.get("0 8 8 5").startswith("On"):
        b.record("busy lockout", False, "Dry TX isn't on: skipped")
        return
    b.set("0 8 7", "On")
    single_slot(b)

    def key_once():
        link = b.r.link
        on = link.send(KEY, bytes([PTT, PTT_ON, SLOTS["a"]]))
        res, end = None, time.time() + 1.0
        while time.time() < end and res is None:
            for t, s, status, rest in link.answers():
                if t == KEY | 0x80 and s == on:
                    res = "keyed" if status == 0 else f"refused: {why(status, rest)}"
        link.send(KEY, bytes([PTT, PTT_OFF]))
        end = time.time() + 2
        while time.time() < end:
            for _ in link.answers():
                pass
            time.sleep(0.1)
        return res or "keyed (no answer: held while transmitting)"

    quiet = key_once()
    proc = blade("c4", p)
    t0 = time.time()
    while time.time() - t0 < 8 and b.status()["slots"]["A"]["state"] != "RX":
        time.sleep(0.1)
    busy = key_once()
    proc.wait()
    b.record("busy lockout: quiet keys, carrier refused", quiet.startswith("keyed") and busy.startswith("refused"),
             f"quiet: {quiet}; carrier: {busy}")


def t_scan(b, p):
    """A VFO scan up from 446.300 stops on the carrier at 446.475 and opens there."""
    b.set("0 1 1", "Off")
    b.cmd("slot A vfo 446.300 nfm low tone off offset off")
    b.cmd("slot A active")
    b.home()
    proc = blade("c4", p, repeat=3)
    time.sleep(2.0)
    b.r.keys("SIDE2")
    b.r.keys("BACK")  # the Scan screen opened: back to home, where the card shows the scan's frequency
    time.sleep(0.3)
    hit, t0 = None, time.time()
    while time.time() - t0 < 8 and hit is None:
        u = b.r.ui()
        c = u["cards"]
        card = next((x for x in c if x.get("slot") == "A"), None) if isinstance(c, list) else c.get("A")
        st = b.status()["slots"]["A"]["state"]
        f = card.get("freq") if card else None
        if st == "RX":
            hit = f
        time.sleep(0.1)
    if "scanning" in b.r.cmd("slot A") or b.status()["slots"]["A"]["state"] in ("scanning", "RX"):
        b.r.keys("SIDE2")
    proc.wait()
    b.home()
    ok = hit is not None and abs(float(hit) - MHZ) < 0.002
    b.record("scan stops and opens on the carrier", ok, f"opened at {hit}")


def t_xb(b, p):
    """The crossband repeater passes once on a carrier on A (446.475) to B (145.495, Dry TX), then stops."""
    if not b.get("0 8 8 5").startswith("On"):
        b.record("crossband", False, "Dry TX isn't on: skipped")
        return
    b.set("0 1 1", "Off")
    b.cmd(f"slot A vfo {MHZ} nfm low tone off offset off")
    b.cmd("slot B vfo 145.495 nfm low tone off offset off")
    b.cmd("slot A active")
    b.home()
    print("  xb start:", b.r.cmd("xb start").strip(), flush=True)
    try:
        time.sleep(1.5)
        x0 = b.status()["xb"]
        proc = blade("c4", p)
        keyed, t0 = set(), time.time()
        while proc.poll() is None and time.time() - t0 < 15:
            k = b.status()["keyed"]
            if k:
                keyed.add(k)
            time.sleep(0.1)
        time.sleep(3)
        x1, k1 = b.status()["xb"], b.status()["keyed"]
        passes = (x1 or {}).get("passes", 0) - (x0 or {}).get("passes", 0)
        b.record("crossband: one pass A to B on a carrier, unkeyed after", passes == 1 and "B" in keyed and not k1,
                 f"passes {passes}, keyed {sorted(keyed)}, after {k1}, xb {x1}")
    finally:
        print("  xb stop:", b.r.cmd("xb stop").strip(), flush=True)
        b.home()


def t_bcast(b, p):
    """BCR-24: B plays FM broadcast, A (446.475) watched: a burst on A mutes the broadcast at once; it plays again
    about 2 s after A's audio ends."""
    b.set("0 1 1", "A+B")
    b.set("0 7 7 1", "Off")
    b.cmd(f"slot A vfo {MHZ} nfm low tone off offset off")
    b.cmd("slot B vfo 101.1")
    b.cmd("slot B active")
    b.home()
    time.sleep(2)
    before = b.status()["slots"]["B"]["state"]
    proc = blade("c4", p)
    tr, t_end = [], None
    t0 = time.time()
    while time.time() - t0 < 14:
        if t_end is None and proc.poll() is not None:
            t_end = time.time() - t0
        sl = b.status()["slots"]
        tr.append((time.time() - t0, sl["A"]["state"], sl["B"]["state"]))
        time.sleep(0.08)
    a_open = [t for t, a, _ in tr if a == "RX"]
    muted_while = all(bs != before for t, a, bs in tr if a == "RX") if a_open else False
    back = next((t for t, a, bs in tr if a_open and t > a_open[-1] and bs == before), None)
    gap = back - a_open[-1] if back and a_open else None
    ok = bool(a_open) and muted_while and gap is not None and 1.5 <= gap <= 3.0
    b.record("broadcast muted by a call on a watched slot, back about 2 s after", ok,
             f"B before {before!r}, B while A open {sorted({bs for t, a, bs in tr if a == 'RX'})}, back {gap and round(gap, 2)} s after A shut")
    b.cmd("slot B vfo 146.52")


def selects(b):
    """VHF RX (PB0) and UHF RX (PE12), the band receive selects (SAFE-24): (pb0, pe12) as 0/1."""
    bits = {}
    for ln in b.r.cmd("gpio").splitlines():
        w = ln.split()
        if w and w[0] in ("PB", "PE") and "out" in w:
            bits[w[0]] = "".join(w[w.index("out") + 1:])
    return int(bits["PB"][15 - 0]), int(bits["PE"][15 - 12])


def counter_line(b):
    for ln in b.r.ui_text().splitlines():
        if ln.startswith("counter "):
            return dict(x.split("=", 1) for x in ln.split()[1:] if "=" in x)
    return None


def t_counter(b, p):
    """TSRCH-10: the counter reads a carrier on chip 1's range (each within 5 kHz; amateur bands only), never keys (TSRCH-8); # holds and OK copies the frequency (TSRCH-11, TSRCH-7); both band receive paths on while
    it is open, one again after it closes (TSRCH-10.3, SAFE-24)."""
    b.set("0 1 1", "Off")
    b.cmd(f"slot A vfo {MHZ} nfm low tone off offset off")
    b.cmd("slot A active")
    b.home()
    b.r.keys("MENU 0 3 2")
    time.sleep(0.8)
    scr = b.r.ui_text().splitlines()[0]
    sel_open = selects(b)
    b.record("counter opens (MENU 0 3 2), both band receive paths on", "Counter" in scr and sel_open == (1, 1),
             f"{scr[:40]}, PB0/PE12 {sel_open}")
    # amateur bands only (70 cm, 33 cm, 23 cm); the counter is for a transmitter close by: full bladeRF gain
    p = max(p, 21)
    for mhz, want in ((433.92, True), (446.475, True), (915.0, True), (1270.0, True)):
        proc = blade("c4", p, mhz=mhz)
        got, keyed, t0 = None, False, time.time()
        while proc.poll() is None and time.time() - t0 < 12:
            c = counter_line(b) or {}
            f = float(c.get("freq", "0") or 0)
            keyed |= b.status()["keyed"] is not None
            if f and c.get("live") == "1":
                got = f if got is None or abs(f - mhz) < abs(got - mhz) else got
            time.sleep(0.1)
        proc.wait()
        if want:
            ok = got is not None and abs(got - mhz) <= 0.005 and not keyed
            b.record(f"counter reads {mhz} MHz", ok, f"read {got}, keyed {keyed}")
        else:
            ok = (got is None or abs(got - mhz) > 0.05) and not keyed
            b.record(f"counter: no reading in chip 1's gap ({mhz} MHz)", ok, f"read {got}")
        time.sleep(2.5)  # the strongest of the last 2 s shows: let it age
    # hold and copy
    proc = blade("c4", p, mhz=446.475)
    t0 = time.time()
    while time.time() - t0 < 8 and not float((counter_line(b) or {}).get("freq", "0") or 0):
        time.sleep(0.1)
    b.r.keys("HASH")
    time.sleep(0.3)
    h = counter_line(b) or {}
    proc.wait()
    time.sleep(2.5)
    h2 = counter_line(b) or {}
    held = h.get("hold") == "1" and h2.get("freq") == h.get("freq")
    b.r.keys("MENU")
    time.sleep(0.5)
    b.home()
    a = b.r.cmd("slot A")
    copied = h.get("freq", "x")[:7] in a
    b.record("counter: # holds the reading, OK copies it to the slot", held and copied, f"held {h}, 2.5 s later {h2}; {a.strip()}")
    b.home()
    sel = selects(b)
    b.record("counter closed: one band receive path again", sel in ((0, 1), (1, 0)), f"PB0/PE12 {sel}")
    b.cmd(f"slot A vfo {MHZ} nfm low tone off offset off")


def t_scan_resume(b, p):
    """A VFO scan stopped on the carrier resumes after it ends (0 1 2 Resume), and passes on."""
    b.set("0 1 1", "Off")
    b.cmd("slot A vfo 446.300 nfm low tone off offset off")
    b.cmd("slot A active")
    b.home()
    proc = blade("c4", p)
    time.sleep(1.8)
    b.r.keys("SIDE2")
    b.r.keys("BACK")
    stopped = resumed = None
    t0 = time.time()
    while time.time() - t0 < 20:
        c = next((x for x in b.r.ui()["cards"] if x.get("slot") == "A"), {})
        st, f = c.get("state"), c.get("freq")
        if stopped is None and st == "RX":
            stopped = (round(time.time() - t0, 1), f)
        if stopped and proc.poll() is not None and st == "scanning" and f and abs(float(f) - MHZ) > 0.002:
            resumed = (round(time.time() - t0, 1), f)
            break
        time.sleep(0.15)
    if b.status()["slots"]["A"]["state"] in ("scanning", "RX"):
        b.r.keys("SIDE2")
    proc.wait()
    b.home()
    b.record("scan: stops on the carrier, resumes after it ends", bool(stopped and resumed), f"stopped {stopped}, resumed {resumed}")


def restore_slots(b, text):
    """Put the slots back from `slot`'s lines ("A* vfo 446.55750 mode=FM power=low tone=off offset=-4.50000")."""
    active = None
    for ln in text.splitlines():
        w = ln.strip().lstrip("=").split()
        if len(w) < 3 or w[0][0] not in "ABC" or w[1] != "vfo":
            print(f"  can't put back: {ln!r}")
            continue
        s = w[0][0]
        active = s if w[0].endswith("*") else active
        kv = dict(x.split("=", 1) for x in w[3:] if "=" in x)
        line = f"slot {s} vfo {w[2]} {kv.get('mode', 'fm').lower()} {kv.get('power', 'low').lower()}"
        for k in ("tone", "txtone", "rxtone"):
            if k in kv:
                line += f" {k} {kv[k]}"
        line += f" offset {kv['offset']}" if "offset" in kv else " offset off"
        if "tx" in kv:
            line += f" split {kv['tx']}"
        b.cmd(line)
    if active:
        b.cmd(f"slot {active} active")


def main():
    a = argparse.ArgumentParser()
    a.add_argument("port")
    a.add_argument("--only")
    a.add_argument("--levels", default="1,3,5,7,9")
    a.add_argument("--strong", type=int, default=-4, help="dB for the tone, watch and busy tests")
    a.add_argument("--out", default="sqreg.json")
    o = a.parse_args()
    only = set(o.only.split(",")) if o.only else None
    levels = [int(x) for x in o.levels.split(",")]
    b = Bench(o.port)
    slots0 = b.r.cmd("slot")
    lv0 = b.level()
    print(f"before: level {lv0}\n{slots0}", flush=True)
    run = lambda n: only is None or n in only  # noqa: E731
    try:
        if run("chip"):
            b.set("0 9 5", "Chip")
            single_slot(b)
            b.set_level(4)
            t_quiet(b)
            sweep(b, "Chip", levels)
        if run("soft"):
            sweep(b, "Software", [1, 5, 9])
            b.set("0 9 5", "Chip")
        if run("level0"):
            b.set("0 9 5", "Chip")
            single_slot(b)
            t_level0(b)
        b.set("0 9 5", "Chip")
        single_slot(b)
        b.set_level(4)
        if run("monitor"):
            t_monitor(b)
        if run("ctcss"):
            t_ctcss(b, o.strong)
        if run("dcs"):
            t_dcs(b, o.strong)
        b.cmd("slot A tone off")
        if run("save"):
            for s in ("Off", "Level 1", "Level 2", "Level 3"):
                t_save_single(b, o.strong, s)
        if run("watch"):
            for s in ("Off", "Level 2", "Level 3"):
                t_watch(b, o.strong, s)
            for s in ("Off", "Level 3"):
                b.set("0 9 5", "Software")
                t_watch(b, o.strong, s)
                b.set("0 9 5", "Chip")
        if run("scan"):
            t_scan(b, o.strong)
        if run("scanresume"):
            t_scan_resume(b, o.strong)
        if run("counter"):
            t_counter(b, o.strong)
        if run("xb"):
            t_xb(b, o.strong)
        if run("bcast"):
            t_bcast(b, o.strong)
        if run("busy"):
            t_busy(b, o.strong)
    finally:
        for k, (path, v) in reversed(b.put_back):
            try:
                b.cmd(f"set {path} {v}")
            except Exception as e:  # noqa: BLE001
                print(f"couldn't put back {path} {v}: {e}")
        try:
            restore_slots(b, slots0)
            print("put back the settings and the slots:\n" + b.r.cmd("slot"), flush=True)
        except Exception as e:  # noqa: BLE001
            print(f"couldn't put the slots back ({e}); they were:\n{slots0}", flush=True)
        try:
            if lv0:
                b.set_level(lv0.split()[0])
        except Exception as e:  # noqa: BLE001
            print(f"couldn't put the level back: {e}")
        json.dump(b.results, open(o.out, "w"), indent=1)
        bad = [r for r in b.results if not r["ok"]]
        print(f"\n{len(b.results) - len(bad)} pass, {len(bad)} fail", flush=True)


if __name__ == "__main__":
    main()
