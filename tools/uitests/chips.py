"""The chips and pins under the screens, read with the bench commands (`bk`, `gpio`, `mem32`, `si`, `rx`, `gps
query`: TERM-21, TERM-38): the tx-dry and bring-up builds only, on the radio (the simulator's fake registers don't
model the receive path). Nothing here writes a register or keys: `bk` and `gpio` read, `si` asks for the chip's
revision and reads a property, `rx both` and `bk settle` are RAM-only bench switches put back after.

Pins (firmware/src/board.rs): PC4 the band relay, PB0 VHF RX and PE12 UHF RX selects, PA7/PC5 the SI4732's inputs,
PA8 the GPS's power (low = on), PB8/PE4 the speaker amp, PC10 the cable's TX, PC14 the green LED. REG_30 0 is a
chip asleep, 0xBFF1 receiving; REG_37 0x1D00 asleep, 0x9D1F awake (PWR-5).
"""
import os
import re
import statistics
import time

from .core import test

QUIET_VHF, QUIET_UHF = "146.55", "446.10"
AWAKE = 0xBFF1


def quiet(t, watch="Off", bsave="Off"):
    """Scans stopped, watch and Battery Save as asked: one chip in use, nothing hopping. Returns the scans to
    restart (`t.put_back`)."""
    stopped = t.stop_scans()
    t.set("0 1 1", watch)
    t.set("0 7 7 1", bsave)
    return stopped


def chip_of(mhz):
    """VFO-3: chip 0 for VHF and 200-300 MHz, chip 1 from 300 MHz and for 18-64 MHz (through the relay)."""
    return 1 if mhz >= 300 or mhz < 64 else 0


def samples(fn, secs, every=0.0):
    out, end = [], time.time() + secs
    while time.time() < end:
        out.append((time.time(), fn()))
        if every:
            time.sleep(every)
    return out


@test("VFO-3", "WATCH-2", needs=("bench",), where="radio", pages=("Home",))
def chip_by_frequency(t):
    """A on 146.52, 250, 446 and 28.4 MHz in turn (watch and Battery Save off): the chip serving it is receiving
    (REG_30 0xBFF1) and the other asleep: chip 0 for VHF and 200-300 MHz, chip 1 for 300 MHz up and 18-64 MHz."""
    t.bench()
    stopped = quiet(t)
    try:
        t.cmd("slot A active")
        for f in (146.52, 250.0, 446.0, 28.4):
            t.cmd(f"slot A vfo {f}")
            time.sleep(0.6)
            c = chip_of(f)
            t.eq(t.bk(c, 0x30), AWAKE, f"{f} MHz: chip {c} receiving")
            t.eq(t.bk(1 - c, 0x30), 0, f"{f} MHz: chip {1 - c} asleep")
    finally:
        t.put_back(stopped)


@test("VFO-15", "WATCH-10", "SAFE-20", "SAFE-11", needs=("bench",), where="radio", pages=("Home", "Scan"))
def relay_follows_the_active_slot(t):
    """The relay (PC4) follows the active slot only: A on 28.4 MHz active, set; B (2 m) active, clear; A again,
    set; a scan running on B meanwhile never moves it."""
    t.bench()
    stopped = quiet(t, watch="A+B")
    try:
        t.cmd("slot A vfo 28.4")
        t.cmd(f"slot B vfo {QUIET_VHF}")
        for active, want in (("A", 1), ("B", 0), ("A", 1)):
            t.cmd(f"slot {active} active")
            time.sleep(0.5)
            t.eq(t.pin("PC4"), want, f"{active} active: PC4 {want}")
        t.cmd("slot B active")
        t.home()
        t.keys("SIDE2")  # a scan on B (2 m)
        t.home()
        t.cmd("slot A active")
        seen = {t.pin("PC4") for _ in range(10)}
        t.eq(seen, {1}, "A (28.4) active, B scanning 2 m: PC4 stays set")
        t.cmd("slot B active")
        t.home()
        t.keys("SIDE2")
        t.home()
    finally:
        t.put_back(stopped)


# ---- Squelch: stock's thresholds from this radio's calibration (SQL-15, SQL-16; ferrite_core::sqcal) ----

def cal_sector(t):
    path = getattr(t.opts, "backup", "")
    t.need(path and os.path.exists(path), f"no backup with the calibration sector ({path}); --backup FILE")
    b = open(path, "rb").read()
    t.need(len(b) >= 0xF220, "the backup is too short")
    return b[0xF000:0xF220]


def sq_regs(cal, mhz10, level):
    """ferrite_core::sqcal::SqCal::regs in Python: (REG_78, REG_4F) for f (10 Hz units) at squelch `level`."""
    if cal[0xE0] == 0xFF:
        opn = [0x00, 0x59, 0x5B, 0x5D, 0x5F, 0x61, 0x63, 0x66, 0x69, 0x6B]
        cls = [0x00, 0x57, 0x59, 0x5B, 0x5D, 0x5F, 0x61, 0x63, 0x66, 0x69]
        uhf, vhf, mid, hf = [4] * 13, [4] * 8 + [2, 2], [0] * 32, [0] * 17
        noise, hyst = [0x2A, 0x2B, 0x38, 0x32], 0x02
    else:
        opn, cls = list(cal[0xC0:0xCA]), list(cal[0xD0:0xDA])
        uhf, vhf, mid, hf = list(cal[0xE0:0xED]), list(cal[0xF0:0xFA]), list(cal[0x100:0x120]), list(cal[0x170:0x181])
        noise, hyst = list(cal[0x215:0x219]), cal[0x219]
    f = mhz10
    l = min(level, 9)
    mhz = f // 100_000
    o, c = opn[l], cls[l]

    def with_off(off):
        return (o - off, (c - off) & 0xFF) if o > off else (o, 0)

    clamp = lambda v, a, b: max(a, min(b, v))  # noqa: E731
    if f >= 40_000_000:
        (ro, rc), k = with_off(uhf[clamp((mhz - 400) // 10, 0, 12)]), 0
    elif f >= 30_000_000:
        (ro, rc), k = with_off(mid[16 + clamp((mhz - 350) // 5, 0, 9)]), 3
    elif 13_000_000 <= f < 18_000_000:
        (ro, rc), k = with_off(vhf[min((mhz - 130) // 5, 9)]), 1
    elif f >= 20_000_000:
        (ro, rc), k = with_off(mid[clamp((mhz - 200) // 5, 0, 19)]), 2
    elif 10_800_000 <= f < 13_600_000:
        (ro, rc), k = (c, (c - 10) & 0xFF), 1
    elif 1_500_000 <= f < 10_000_000:
        (ro, rc), k = with_off(hf[min((mhz - 15) // 5, 16)]), 1
    else:
        (ro, rc), k = (0, 0), 1
    n = noise[k] & 0x7F
    r4f = 0x2322 if 2_602_000 <= f < 2_611_000 else n << 8 | ((n - hyst) & 0xFF)
    return ro << 8 | rc, r4f


def squelch_level(t):
    u = t.keys("MENU")
    return int(u.row("Squelch")["value"].split()[0])


def set_squelch(t, level):
    """The active slot's squelch by the quick menu's row (the knob), back home."""
    t.home()
    u = t.keys("MENU")
    now = int(u.row("Squelch")["value"].split()[0])
    if u.sel["name"] != "Squelch":
        t.keys("1")
    if level != now:
        t.knob(level - now)
    t.home()


@test("SQL-1", "SQL-15", "SQL-16.1", "SQL-16.2", needs=("bench",), where="radio", pages=("Home", "Quick"))
def squelch_registers_from_calibration(t):
    """Each squelch level 0-9 on 446.000, 146.520, 223.500, 350.000 and 26.050 MHz: REG_78 (RSSI open and close)
    and REG_4F (noise, with the hysteresis) are stock's from this radio's calibration (read from the backup's
    0xF000 sector, never from the radio): the open and close thresholds less the band's RSSI offset, the noise by
    band class, CB's fixed 0x2322."""
    t.bench()
    cal = cal_sector(t)
    stopped = quiet(t)
    t.cmd("slot A active")
    t.home()
    level0 = squelch_level(t)
    t.home()
    bad = []
    try:
        for mhz in ("446.0", "146.52", "223.5", "350.0", "26.05"):
            t.cmd(f"slot A vfo {mhz}")
            f10 = round(float(mhz) * 100_000)
            c = chip_of(float(mhz))
            for level in range(10):
                set_squelch(t, level)
                time.sleep(0.2)
                want = sq_regs(cal, f10, level)
                got = (t.bk(c, 0x78), t.bk(c, 0x4F))
                if got != want:
                    bad.append(f"{mhz} MHz level {level}: REG_78/4F {got[0]:04X}/{got[1]:04X}, want "
                               f"{want[0]:04X}/{want[1]:04X}")
        t.eq(bad, [], "every level's registers as stock computes them")
    finally:
        t.cmd("slot A vfo 146.52")
        set_squelch(t, level0)
        t.put_back(stopped)


@test("WATCH-8", "PWR-19", needs=("bench",), where="radio", pages=("Home",))
def watched_chips_follow_battery_save(t):
    """Battery Save Level 1 and watch A+B (A on 2 m, B on 70 cm): each watched chip goes through the sleep cycle
    (REG_30 0 and 0xBFF1 both seen over a few seconds). One chip awake at a time (WATCH-2, PWR-7) isn't checked
    here: each `bk` read is a link round trip (about 150 ms on the cable), longer than the hop's 70 ms
    dwell, so a read pair spans hops and both read awake (4 of 16 on the bench) without the two ever overlapping.
    That check needs both chips' REG_30 read in one Radio-task request."""
    t.bench()
    stopped = quiet(t, watch="A+B", bsave="Level 1")
    try:
        t.cmd(f"slot A vfo {QUIET_VHF}")
        t.cmd(f"slot B vfo {QUIET_UHF}")
        t.home()
        time.sleep(2.5)
        s = samples(lambda: (t.bk(0, 0x30), t.bk(1, 0x30)), 5)
        for c in (0, 1):
            vals = {v[c] for _, v in s}
            t.check(AWAKE in vals and 0 in vals, f"chip {c}: awake and asleep in turn", sorted(vals))
        both = sum(1 for _, v in s if v == (AWAKE, AWAKE))
        pair = statistics.median(s[i + 1][0] - s[i][0] for i in range(len(s) - 1)) * 1000
        t.note(f"{len(s)} read pairs, one every {pair:.0f} ms; {both} read both awake (pairs span hops: not checked)")
    finally:
        t.put_back(stopped)


def turns(t, reset=False):
    """WATCH-17.1 (`bk turns`, bench builds since decision 262): each slot's whole turns on a chip and their total
    ms, as the Radio task timed them at each retune: {"A": (n, ms), ...}."""
    line = "bk turns reset" if reset else "bk turns"
    out = t.cmd(line)
    # Only the turns: the scan steps that follow ("; steps: A 0 0 ms ...") have the same shape.
    head = out.split("steps:", 1)[0]
    got = {m.group(1): (int(m.group(2)), int(m.group(3))) for m in re.finditer(r"\b([ABC]) (\d+) (\d+) ms", head)}
    if len(got) != 3:
        t.fail(f"{line}: {out!r} (a build with bk turns, decision 262)")
    return got


@test("WATCH-17", "WATCH-17.1", needs=("bench",), where="radio", smoke=True, pages=("Home",))
def turns_on_a_shared_chip(t):
    """A and B both on chip 0 (146.55 and 147.00 MHz), watch A+B, Battery Save off, nothing heard: the Radio task's
    own turn counts (`bk turns`, WATCH-17.1), read 6 s apart, give mean turns of 150 ms on A (active) and 70 ms on
    B, each within 20 ms. Reading REG_38 over the link can't time them: one read takes about 160 ms."""
    t.bench()
    stopped = quiet(t, watch="A+B")
    try:
        t.cmd("slot A vfo 146.55")
        t.cmd("slot B vfo 147.0")
        t.cmd("slot A active")
        t.home()
        time.sleep(1)
        t0, s0 = turns(t, reset=True), time.time()
        time.sleep(6)
        t1, s1 = turns(t), time.time()
        # After the reset the counts start at 0, so t1 is the difference; t0 (since boot) is only noted.
        n = {s: t1[s][0] for s in "AB"}
        mean = {s: t1[s][1] / n[s] if n[s] else 0.0 for s in "AB"}
        busy = sum(t1[s][1] for s in "ABC") / ((s1 - s0) * 1000)
        t.note(f"in {s1 - s0:.1f} s: A {n['A']} turns, mean {mean['A']:.1f} ms; B {n['B']} turns, mean {mean['B']:.1f} ms; "
               f"C {t1['C'][0]}; chip 0 timed {busy:.0%} of the time (since boot before: A {t0['A']}, B {t0['B']})")
        t.check(n["A"] >= 10 and n["B"] >= 10, "both slots get turns", n)
        t.check(abs(mean["A"] - 150) <= 20, "150 ms turns on the active slot", round(mean["A"], 1))
        t.check(abs(mean["B"] - 70) <= 20, "70 ms turns on the watched one", round(mean["B"], 1))
    finally:
        t.put_back(stopped)


@test("TONE-20.3", needs=("bench",), where="radio", pages=("Home",))
def dcs_receive_polarity(t):
    """A on D023N then D023I (receive tone): REG_51 reads 0xA... (normal) then 0x8... (inverted), stock's."""
    t.bench()
    stopped = quiet(t)
    try:
        t.cmd("slot A active")
        for tone, top in (("D023N", 0xA), ("D023I", 0x8)):
            t.cmd(f"slot A vfo {QUIET_VHF} rxtone {tone}")
            time.sleep(0.5)
            v = t.bk(0, 0x51)
            t.eq(v >> 12, top, f"{tone}: REG_51 {v:04X}")
    finally:
        t.put_back(stopped)


@test("TX-43.3", needs=("bench",), where="radio", pages=())
def cable_tx_pin_high_when_quiet(t):
    """Between lines PC10 (the cable's TX, pin 11) is a GPIO output held high."""
    t.bench()
    time.sleep(0.3)
    g = t.gpio()
    t.eq(g["C"][1] >> 10 & 1, 1, "PC10 driven high")
    crh = t.mem32(0x40011004)[0]
    mode = crh >> 8 & 0x3
    t.check(mode != 0, "PC10 an output (CFGHR mode bits not input)", f"{crh:08X}")


@test("BCR-14", "HF-16", needs=("bench",), where="radio", pages=("Home", "Hf"))
def si_input_pins_by_mode(t):
    """PA7/PC5 (the SI4732's inputs): 0/0 for FM broadcast, 1/1 for HF mode's AM."""
    t.bench()
    from .modes import HfVfo, hf_mode, hf_type
    t.cmd("slot A vfo 98.5")  # BCR-21: a slot in the band plays broadcast (decision 352: no FM screen)
    u = t.home()
    time.sleep(1.0)
    t.eq((t.pin("PA7"), t.pin("PC5")), (0, 0), "FM broadcast: PA7/PC5 0/0")
    vfo = HfVfo(t)
    try:
        hf_type(t, 9.58)
        hf_mode(t, "AM")
        time.sleep(0.5)
        t.eq((t.pin("PA7"), t.pin("PC5")), (1, 1), "HF AM: PA7/PC5 1/1")
    finally:
        vfo.restore()
    del u


SI_DOWN = "refused: the SI4732 is powered down: FM broadcast or HF mode powers it up"  # TERM-24, decision 162


@test("HF-3", "HF-4", "TERM-24", "HF-11", "SAFE-23", needs=("bench",), where="radio", pages=("Hf", "Home"))
def hf_mode_relay_and_si(t):
    """Entering HF mode the SI4732 powers up (`si 10 00` answers) and the relay follows the receiver (released for
    the SI4732, energised on chip 1, HF-11); leaving, the SI4732 is off (its answer says so) and PC4 follows the
    active slot again (A on 28.4 MHz: set); `si prop` reads a property while it runs."""
    t.bench()
    from .modes import HfVfo, hf_mode, hf_type
    stopped = quiet(t)
    try:
        t.cmd("slot A vfo 28.4")
        t.cmd("slot A active")
        t.home()
        off = t.cmd_err("si 10 00")
        t.check(off[1].strip() == SI_DOWN, "the SI4732 off at home: the answer says so (TERM-24)", off)
        vfo = HfVfo(t)
        try:
            hf_type(t, 9.58)
            hf_mode(t, "AM")
            time.sleep(0.5)
            ok, out = t.cmd_err("si 10 00")
            t.check(ok and "->" in out, "HF mode: the SI4732 answers", out)
            ok, out = t.cmd_err("si prop 4000")
            t.check(ok, "si prop reads one", out)
            t.eq(t.pin("PC4"), 0, "the SI4732 in use: the relay released")
            hf_type(t, 28.4)
            if hf_mode(t, "FM") is not None:
                time.sleep(0.5)
                t.eq(t.pin("PC4"), 1, "chip 1 in use: the relay energised")
        finally:
            vfo.restore()
        t.home()
        time.sleep(0.5)
        ok, out = t.cmd_err("si 10 00")
        t.check(out.strip() == SI_DOWN, "left: the SI4732 off (TERM-24)", out)
        t.eq(t.pin("PC4"), 1, "PC4 follows the active slot (28.4) again")
    finally:
        t.put_back(stopped)


@test("HF-33", "BCR-19", "PWR-6", needs=("bench",), where="radio", slow=True, pages=("Hf", "Home"))
def battery_save_beside_the_si4732(t):
    """Battery Save Level 1 with the SI4732 playing: the BK4829s keep their sleep cycle (REG_30 0 and 0xBFF1 seen),
    the SI4732 stays awake (it answers); with a broadcast slot playing the band's receive select (PB0/PE12) stays
    on through the sleeps, and the broadcast plays on (40 s without a key)."""
    t.bench()
    stopped = quiet(t, watch="A+B", bsave="Level 1")
    try:
        t.cmd("slot A vfo 100.1")
        t.cmd(f"slot B vfo {QUIET_VHF}")
        t.cmd("slot A active")
        t.home()
        time.sleep(40)
        s = samples(lambda: (t.bk(0, 0x30), t.pin("PB0"), t.pin("PE12")), 4)
        vals = {v[0] for _, v in s}
        t.check(AWAKE in vals and 0 in vals, "chip 0 (B, watched) sleeps and wakes", sorted(vals))
        t.check(all(v[1] or v[2] for _, v in s), "a receive select stays on through the sleeps", s[:6])
        u = t.ui()
        t.check(u.state.get("fm") == 1, "the broadcast plays on", u.state)
        ok, out = t.cmd_err("si 10 00")
        t.check(ok and "->" in out, "the SI4732 awake", out)
    finally:
        t.put_back(stopped)


@test("TSRCH-10.2", "SCOPE-5", "SAFE-20", needs=("bench",), where="radio", pages=("Counter", "Home", "Scope"))
def modes_never_move_the_relay(t):
    """PC4 before, during and after the frequency counter and the band scope, with A on 2 m and on 28.4 MHz:
    unchanged throughout (SAFE-20)."""
    t.bench()
    stopped = quiet(t)
    try:
        t.cmd("slot A active")
        for f, want in ((QUIET_VHF, 0), ("28.4", 1)):
            t.cmd(f"slot A vfo {f}")
            t.home()
            time.sleep(0.4)
            t.eq(t.pin("PC4"), want, f"{f}: PC4 {want} at home")
            for keys in ("MENU 0 3 2", "1:long"):
                t.home()
                t.keys(keys, settle=0.5)
                seen = {t.pin("PC4") for _ in range(6)}
                t.eq(seen, {want}, f"{f}, {keys}: PC4 unchanged")
            t.home()
            t.eq(t.pin("PC4"), want, f"{f}: after")
    finally:
        t.put_back(stopped)


@test("SCOPE-11", needs=("bench",), where="radio", pages=("Home", "Scope"))
def scope_listen_tunes_the_chip(t):
    """SIDE1 in the scope listens at the marker: the chip's frequency (REG_38/39) is the marker's."""
    t.bench()
    stopped = quiet(t)
    try:
        t.cmd(f"slot A vfo {QUIET_VHF}")
        t.cmd("slot A active")
        t.home()
        t.press("1", long=True)
        t.wait(lambda u: (u["scope"] or {}).get("sweeps", 0) >= 1, timeout=10, what="a sweep")
        t.keys("RIGHT RIGHT RIGHT")
        m = t.ui()["scope"]["marker"]
        t.keys("SIDE1", settle=0.5)
        f = t.bk(0, 0x39) << 16 | t.bk(0, 0x38)
        t.eq(f, round(float(m) * 100_000), f"listening: the chip on the marker ({m})")
        t.keys("SIDE1")
    finally:
        t.put_back(stopped)


@test("APRS-16", "DEC-8", needs=("bench",), where="radio", pages=("Home",))
def kept_chips_stay_awake(t):
    """APRS mode on with watch off: C (144.39) is watched, its chip kept awake beside the active one through
    Battery Save (REG_30 never 0 on it over a few seconds); a pager decoder running (0 4 2 1 on, a slot on its
    frequency) keeps its chip out of the sleep likewise."""
    t.bench()
    stopped = quiet(t, watch="Off", bsave="Level 1")
    try:
        t.cmd("slot A vfo 446.1")
        t.cmd("slot A active")
        t.set("0 5 3", "On")
        u = t.home()
        t.eq(u.cards["C"]["kind"], "aprs", "APRS mode: C is the APRS card")
        time.sleep(2)
        s = [t.bk(0, 0x30) for _ in range(30)]
        t.check(0 not in s, "C's chip (0) never asleep", s)
        t.set("0 5 3", "Off")
        t.home()
        t.set("0 4 2 1", "On")
        f = t.get("0 4 2 2")
        t.cmd(f"slot B vfo {f}")
        t.set("0 1 1", "A+B")
        time.sleep(2)
        c = chip_of(float(f))
        s = [t.bk(c, 0x30) for _ in range(30)]
        t.check(0 not in s, f"the decoder's chip ({c}) never asleep", s)
    finally:
        t.put_back(stopped)


@test("GPS-1.1", "TERM-36.7", needs=("bench",), where="radio", pages=())
def gps_off_powers_the_module_down(t):
    """0 6 1 Off: PA8 high (the module unpowered, as stock) and the terminal's GPS commands refused; On again:
    PA8 low."""
    t.bench()
    t.need(t.get("0 6 1") == "On", "GPS isn't on")
    t.set("0 6 1", "Off")
    time.sleep(1)
    t.eq(t.pin("PA8"), 1, "GPS off: PA8 high (powered down)")
    for line in ("gps query 0", "gps nmea off", "gps standby 10"):
        ok, out = t.cmd_err(line)
        t.check(not ok or "off" in out.lower(), f"{line}: refused with GPS off", out)
    t.set("0 6 1", "On")
    time.sleep(1)
    t.eq(t.pin("PA8"), 0, "GPS on: PA8 low")


@test("TERM-36.5", needs=("bench",), where="radio", pages=())
def gps_commands_refused_during_a_standby(t):
    """While quiet (`gps nmea off`) and in `gps standby 60`, the terminal's other GPS commands are refused; GPS off
    and on ends the test (the module's RAM settings gone)."""
    t.bench()
    t.need(t.get("0 6 1") == "On", "GPS isn't on")
    try:
        t.cmd("gps nmea off")
        t.cmd("gps standby 60")
        ok, out = t.cmd_err("gps query 0")
        # The terminal prints its refusal as text ("refused: a bench standby is running: ..."); CMD itself succeeds.
        t.check(not ok or "refused" in out.lower(), "gps query 0 refused during the standby", out)
    finally:
        t.set("0 6 1", "Off")
        time.sleep(1)
        t.set("0 6 1", "On")


@test("SND-9", needs=("bench",), where="radio", pages=())
def amp_off_when_quiet(t):
    """With nothing playing the speaker amp (PE4, PB8) is off; after a restart too (the safe state)."""
    t.bench()
    stopped = quiet(t)
    try:
        t.cmd(f"slot A vfo {QUIET_VHF}")
        t.home()
        time.sleep(3)  # past any receive tail and key beep
        s = [(t.pin("PE4"), t.pin("PB8")) for _ in range(5)]
        t.eq(set(s), {(0, 0)}, "nothing playing: PE4 and PB8 low")
    finally:
        t.put_back(stopped)
    if t.get("0 9 3 1") == "Off":
        t.restart()
        time.sleep(3)
        s = [(t.pin("PE4"), t.pin("PB8")) for _ in range(3)]
        t.eq(set(s), {(0, 0)}, "after a restart: the amp off")


@test("UPD-9", needs=("bench",), where="radio", pages=())
def swd_pins_never_remapped(t):
    """PA13/PA14 are never remapped: IOMUX's SWJ setting isn't "all off" (100) and GPIOA's CFGHR leaves PA13 and
    PA14 out of the output modes."""
    t.bench()
    crh = t.mem32(0x40010804)[0]
    remap = t.mem32(0x40010004)[0]
    for pin in (13, 14):
        nib = crh >> ((pin - 8) * 4) & 0xF
        t.check(nib & 0x3 == 0 or nib & 0xC in (0x8, 0xC), f"PA{pin}: not a GPIO output ({nib:X})", f"{crh:08X}")
    t.check(remap >> 24 & 0x7 != 0b100, "SWJ not turned off", f"{remap:08X}")


@test("DISP-16.1", needs=("bench",), where="radio", pages=())
def brightness_duty_table(t):
    """Brightness 1-10 (0 7 4 2): the screen's PWM (TMR8 C1DT) reads 216, 312, 432, 600, 864, 1200, 1704, 2400, 3408
    and 4800 (4.5 % to 100 % of 4800); the keypad light's (TMR2 C2DT, 0 7 4 5 "with the screen") the same curve at
    half (DISP-22.1: thousandths x 500 / 1000, then of 4800)."""
    t.bench()
    t.need(t.get("0 7 4 5") == "With screen", "the keypad light isn't 'with the screen'")
    want = [216, 312, 432, 600, 864, 1200, 1704, 2400, 3408, 4800]
    got = []
    for n in range(1, 11):
        t.set("0 7 4 2", str(n))
        t.keys("UP")  # a key: the screen (and the keypad light) lit
        time.sleep(0.2)
        got.append((t.mem32(0x40013434)[0] & 0xFFFF, t.mem32(0x40000038)[0] & 0xFFFF))
        t.keys("DOWN")
    t.eq([g[0] for g in got], want, "the screen's duty")
    curve = [45, 65, 90, 125, 180, 250, 355, 500, 710, 1000]  # lights::BRIGHTNESS, thousandths
    t.eq([g[1] for g in got], [c * 500 // 1000 * 4800 // 1000 for c in curve], "the keypad light's (DISP-22.1)")


@test("DISP-15.5", needs=("bench",), where="radio", slow=True, pages=("Home",))
def broadcast_never_wakes_the_screen(t):
    """A broadcast slot playing, the timeout at 10 s: the screen goes dark and stays dark (broadcast isn't
    receiving); the green LED (PC14) stays off. LEDs on "RX and TX" for the test: the default heartbeat flashes the
    green LED on a dark screen every 5 s (DISP-21), which is not receiving."""
    t.bench()
    stopped = quiet(t)
    try:
        t.set("0 7 6", "RX and TX")
        t.set("0 7 4 1", "10 s")
        t.cmd("slot A vfo 100.1")
        t.cmd("slot A active")
        t.home()
        t.wait(lambda u: u.state.get("dark") == 1, timeout=16, what="dark after 10 s", every=1.0)
        leds = []
        end = time.time() + 30
        while time.time() < end:
            leds.append(t.pin("PC14"))
            t.check(t.ui(0).state.get("dark") == 1, "still dark", "")
            time.sleep(1.5)
        t.eq(set(leds), {0}, "the green LED off throughout")
    finally:
        t.put_back(stopped)


@test("TERM-29.1", "TERM-29.2", needs=("bench",), where="radio", pages=("Hf", "Home"))
def rx_both_selects(t):
    """`rx both` holds PB0 and PE12 on; `rx one` back to the receiving band's alone; `rx` says which; refused in HF
    mode and with an HF slot active (the relay energised), saying why; off after a restart."""
    t.bench()
    stopped = quiet(t)
    try:
        t.cmd(f"slot A vfo {QUIET_VHF}")
        t.cmd("slot A active")
        t.home()
        t.check("one" in t.cmd("rx"), "rx: one by default", t.cmd("rx"))
        t.cmd("rx both")
        time.sleep(0.3)
        t.eq((t.pin("PB0"), t.pin("PE12")), (1, 1), "rx both: PB0 and PE12 on")
        t.cmd("rx one")
        time.sleep(0.3)
        t.eq((t.pin("PB0"), t.pin("PE12")), (1, 0), "rx one, A on 2 m: PB0 alone")
        t.cmd("slot A vfo 28.4")
        ok, out = t.cmd_err("rx both")
        t.check(not ok or "relay" in out.lower(), "an HF slot active: refused, saying why", out)
        t.cmd(f"slot A vfo {QUIET_VHF}")
        t.keys("MENU 0 3 3")
        t.wait(lambda u: "freq" in (u["hf"] or {}), timeout=20, what="HF mode")
        ok, out = t.cmd_err("rx both")
        t.check(not ok or "hf" in out.lower(), "HF mode: refused, saying why", out)
        t.home()
        t.cmd("rx both")
        if t.get("0 9 3 1") == "Off":
            t.restart()
            t.check("one" in t.cmd("rx"), "after a restart: one", t.cmd("rx"))
    finally:
        t.cmd_err("rx one")
        t.put_back(stopped)


@test("TERM-31", needs=("bench",), where="radio", pages=())
def bk_settle_sequence(t):
    """`bk settle` reads the waits (2 and 1 ms at boot); `bk settle 5`, `bk settle 10 0`, `bk settle 0` (stock's: no
    wait) set them; 21 and three numbers are refused with the usage line; a restart is back at 2 and 1 ms."""
    t.bench()
    try:
        first = t.cmd("bk settle")
        t.check(re.search(r"wake \d+ ms, sleep \d+ ms", first), "bk settle reads", first)
        t.cmd("bk settle 5")
        t.has(t.cmd("bk settle"), "wake 5 ms, sleep 5 ms", "bk settle 5")
        t.cmd("bk settle 10 0")
        t.has(t.cmd("bk settle"), "wake 10 ms, sleep 0 ms", "bk settle 10 0")
        out = t.cmd("bk settle 0")
        t.check("stock" in (out + t.cmd("bk settle")).lower(), "0: stock's (no wait)", out)
        for bad in ("bk settle 21", "bk settle 2 1 0"):
            ok, why = t.cmd_err(bad)
            t.check(not ok, f"{bad}: refused", why)
    finally:
        t.cmd_err("bk settle 2 1")
    if t.get("0 9 3 1") == "Off":
        t.restart()
        t.has(t.cmd("bk settle"), "wake 2 ms, sleep 1 ms", "after a restart")
