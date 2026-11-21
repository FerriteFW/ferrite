"""One radio confirmation for rules whose logic the host and simulator tests carry (`How: host-covered` in the test
plan): readback, the terminal and the link only. Nothing here keys or writes a register; settings a test changes
are put back by the runner, the channel one makes is deleted, and the one restart (the option bytes before and after
a REBOOT, LINK-24) lands home as `restart_lands_home` does.

    tools/uitest.py --port PORT --backup ../backups/blackradio/flash.bin -k hostconfirm    # (--bt, --pin for XB-20.3)
"""
import hashlib
import os
import re
import statistics
import struct
import time

import ferrite_link as fl
from .chips import AWAKE, QUIET_UHF, QUIET_VHF, cal_sector, quiet, samples, set_squelch, sq_regs, squelch_level
from .core import test
from .restart import clear_pin_lock, pin_lock, set_pin_lock, the_pin  # noqa: F401 (clear_pin_lock: pin_lock's)

ASLEEP_37, AWAKE_37 = 0x1D00, 0x9F1F  # PWR-5: awake and receiving, the receive set-up's (SQL-18.9)
TARGET = os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))), "target")


def mem(t, addr, n):
    """`mem ADDR N` (bench builds, TERM-22) as bytes."""
    out = t.cmd(f"mem {addr:08X} {n}")
    data = bytearray()
    for line in out.splitlines():
        parts = line.replace(":", " ").split()
        if not parts or not re.fullmatch(r"[0-9A-Fa-f]{8}", parts[0]):
            continue
        for w in parts[1:17]:
            if not re.fullmatch(r"[0-9A-Fa-f]{2}", w):
                break
            data.append(int(w, 16))
    if len(data) < n:
        t.fail(f"mem {addr:08X} {n}: {out[:200]!r}")
    return bytes(data[:n])


MARKER, MARKER_OFF = 0x080F0000, 0x080F0000 - 0x08003000
# Stock's marker words (decision 338, BACKUP-12), as the bytes would read in either order.
MARKER_WORDS = (b"UV85", b"58VU", b"BJ90", b"09JB", b"\xff\xff\xff\xff")


def installed_image(t):
    """The image the radio runs (About's commit), unpacked from target/'s .BTF and its 0xFF padding cut, or None
    when target/ has no .BTF of that commit."""
    import sys
    sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    import btf
    u = t.keys("MENU 0 9 8")
    version = (u.row("Version") or {}).get("value", "")
    m = re.search(r"([0-9a-f]{7,})", version)
    if not m:
        return None
    commit = m.group(1)[:7]
    for name in ("ferrite-txbench.btf", "ferrite-txdry.btf", "ferrite-tx.btf", "ferrite.btf"):
        p = os.path.join(TARGET, name)
        if os.path.exists(p):
            cand = btf.unpack(open(p, "rb").read())[0]
            if commit.encode() in cand:
                return cand.rstrip(b"\xff")
    return None


def boot_flash(t):
    """The MCU's option bytes (user system data, 48 B), the stock bootloader (0x08000000, 12 KB) as a SHA-256 and the
    marker sector's first 16 bytes (0x080F0000)."""
    usd = mem(t, 0x1FFFF800, 48)
    h = hashlib.sha256()
    for a in range(0x08000000, 0x08003000, 1024):
        h.update(mem(t, a, 1024))
    return usd, h.hexdigest()[:16], mem(t, 0x080F0000, 16)


@test("INST-4", "LINK-24.4", "UPD-6", "SAFE-3", needs=("bench",), where="radio", pages=("Home",))
def option_bytes_and_boot_flash_unchanged(t):
    """The option bytes (`mem 1FFFF800 48`), the stock bootloader and the marker sector read the same before and after
    a REBOOT (LINK-24); each option-byte half-word is erased (0xFFFF) or a byte and its complement, as the factory
    and the stock bootloader leave them (nothing half-written)."""
    t.bench()
    t.need(t.get("0 9 3 1") == "Off", "a PIN lock is on: the restart asks for it")
    before = boot_flash(t)
    usd = before[0]
    bad = [i for i in range(0, 48, 2) if (usd[i], usd[i + 1]) != (0xFF, 0xFF) and usd[i] ^ usd[i + 1] != 0xFF]
    t.eq(bad, [], "every option-byte half-word erased or a byte and its complement")
    t.note(f"option bytes {usd.hex()}; bootloader sha256 {before[1]}; marker {before[2][:4]!r}")
    u = t.restart()
    t.eq(u.id, "Home", "home after the REBOOT")
    after = boot_flash(t)
    t.eq(after[0].hex(), before[0].hex(), "the option bytes unchanged after the REBOOT")
    t.eq(after[1], before[1], "the bootloader unchanged")
    t.eq(after[2].hex(), before[2].hex(), "the marker sector unchanged")
    img = installed_image(t)
    if img is not None and len(img) > MARKER_OFF:
        t.note("the installed image reaches 0x080F0000 (BACKUP-12): the marker sector holds Ferrite's code")
    else:
        t.check(after[2][:4] in MARKER_WORDS, "the marker sector is stock's or erased", after[2][:4])


@test("UPD-1", "UPD-3", needs=("bench",), where="radio", pages=("Info",))
def installed_image_matches_its_btf(t):
    """The installed image is the .BTF's, unpacked (first 1 KB plain, the rest decrypted with the zero key, the last
    block plain: UPD-3): its first, a middle and its last 1 KB read back from 0x08003000 the same, it is at most
    1,012 KB and ends within the chip's 1024 KB, at 0x08100000 at most (UPD-1, decision 338). Skipped when target/
    has no .BTF of the commit the radio runs (About's version)."""
    t.bench()
    img = installed_image(t)
    t.need(img is not None, f"no .BTF of the commit About names in {TARGET}")
    t.check(len(img) <= 1012 * 1024, "the image is at most 1,012 KB", len(img))
    t.check(0x08003000 + len(img) <= 0x08100000, "it ends within MCU flash", hex(0x08003000 + len(img)))
    last = (len(img) - 1) // 1024 * 1024
    for off, what in ((0, "the first 1 KB (plain in the .BTF)"), (last // 2 // 1024 * 1024, "a middle 1 KB"),
                      (last, "the last 1 KB")):
        want = img[off:off + 1024]
        n = (len(want) + 15) // 16 * 16  # whole lines: a short last line's characters could read as hex
        t.eq(mem(t, 0x08003000 + off, n)[:len(want)].hex(), want.hex(), f"{what} as installed")
    t.note(f"installed image: {len(img)} bytes")


BUILT = {fl.HELLO, fl.INFO, fl.AUTH, fl.READ, fl.WRITE, fl.DELETE, fl.BEGIN, fl.END, fl.SLOT, fl.BAUD, fl.REBOOT,
         fl.KEY, fl.CMD, fl.MORE, fl.UI, fl.USED, fl.READ_MANY, fl.WRITE_MANY, fl.LOG_READ,
         fl.SUBSCRIBE, fl.HEARD, fl.MSGS, fl.MSG_SEND, fl.MSG_STOP, fl.MSG_DELETE, fl.AUDIO, fl.STREAM, fl.Z,
         0x60, fl.DATA_READ, fl.DATA_WRITE, fl.CLOCK, fl.STATE, fl.TUNE, fl.BEACON, fl.PHONE_POS}
# EVENT (0x51): radio to PC only. 0x60 STOCK_RESTORE (decision 261): calibration and the update area are refused by
# ferrite_core::restore::allowed (host-tested), and it is cable only (refused here). DATA_READ and DATA_WRITE (LINK-59,
# LINK-60, decision 333): calibration and the update area refused by ferrite_core::link::data::allowed, checked below.
# STATE and TUNE (LINK-67, LINK-68): a read, and the keypad's own paths (no flash beyond what the keys write).
# BEACON (LINK-69): the APRS task's beacon now, as the radio's own; it writes nothing.
# PHONE_POS (LINK-72): the phone's position, kept in RAM for GPS-3.1; it writes nothing.
# CLOCK (LINK-65, decision 340): the clock only, never a store write.


@test("LINK-3", "SAFE-5", "TERM-8", pages=("Home",))
def link_has_no_calibration_write(t):
    """Every link message type the radio doesn't have answers "unknown type" (no CAL_WRITE or any other way to the
    calibration sector or the update area); READ takes the settings and APRS records only (1 and 5, LINK-21, LINK-43);
    the terminal has no command that reaches SPI flash."""
    link = t.r.link
    known = []
    for ty in range(0x00, 0x80):
        if ty in BUILT:
            continue
        s, _ = link.ask(ty, b"", timeout=1.0)
        if s != 2:
            known.append((hex(ty), s))
    t.eq(known, [], "every other type: unknown type")
    for kind in (0, 2, 3, 4, 6, 7, 0x80, 0xFF):
        s, why = link.ask(fl.READ, struct.pack("<BHHB", kind, 0, 0, 200), timeout=1.0)
        t.check(s != 0, f"READ kind {kind}: refused", (s, why))
        if kind in fl.ONE_FRAME:
            continue  # READ_MANY's and WRITE_MANY's kinds
        for ty, p in ((fl.USED, bytes([kind])), (fl.READ_MANY, struct.pack("<BHH", kind, 0, 1)),
                      (fl.WRITE_MANY, struct.pack("<BHB", kind, 0, 1) + b"\x00")):
            s, why = link.ask(ty, p, timeout=1.0)  # LINK-39 - LINK-41: the record kinds only (channel, zone, preset, contact, position)
            t.check(s != 0, f"0x{ty:02X} kind {kind}: refused", (s, why))
    for ty, p in ((fl.DATA_READ, struct.pack("<BIB", 0, 0xF000, 16)), (fl.DATA_READ, struct.pack("<BIB", 0, 0x300000, 16)),
                  (fl.DATA_WRITE, struct.pack("<BI", 0, 0xF000)), (fl.DATA_WRITE, struct.pack("<BI", 0, 0x300000)),
                  (fl.DATA_WRITE, struct.pack("<BI", 1, 0xF000) + b"\x00")):
        s, why = link.ask(ty, p, timeout=1.0)  # LINK-59, LINK-60: refused (4), whatever the port
        t.eq(s, 4, f"0x{ty:02X} at calibration or the update area: refused")
    for c in ("flash", "erase", "spi", "cal", "calibration", "write", "update"):
        ok, out = t.cmd_err(c)
        t.check(not ok or "no such command" in out, f"`{c}`: no such command", out)


@test("SND-2", needs=("bench",), where="radio", pages=("Home",))
def receive_level_is_stocks(t):
    """REG_48 (the receive audio level into the volume knob) is stock's 0xB2BF on the chip in use, VHF and UHF."""
    t.bench()
    stopped = quiet(t)
    try:
        t.cmd("slot A active")
        for mhz, chip in ((QUIET_VHF, 0), (QUIET_UHF, 1)):
            t.cmd(f"slot A vfo {mhz}")
            time.sleep(0.6)
            t.eq(f"{t.bk(chip, 0x48):04X}", "B2BF", f"{mhz} MHz: chip {chip} REG_48")
    finally:
        t.put_back(stopped)


@test("PWR-1", needs=("bench",), where="radio", pages=("Home",))
def battery_save_off_keeps_the_receiver_on(t):
    """Battery Save Off, watch off, A on 2 m: past the 10 s a key holds, the VHF chip still reads receiving (REG_30
    0xBFF1) every time. The levels' own timings start 30 min after the last activity (PWR-3): host-tested."""
    t.bench()
    stopped = quiet(t, bsave="Off")
    try:
        t.cmd(f"slot A vfo {QUIET_VHF}")
        t.cmd("slot A active")
        t.home()
        time.sleep(11)
        vals = {v for _, v in samples(lambda: t.bk(0, 0x30), 3)}
        t.eq(sorted(vals), [AWAKE], "Off: awake throughout")
    finally:
        t.put_back(stopped)


@test("PWR-3", "PWR-5", needs=("bench",), where="radio", pages=("Home",))
def key_wakes_then_the_sleep_registers(t):
    """Battery Save Level 1, watch off, A on 2 m: a key press holds the VHF chip receiving for 10 s, then it cycles
    about 300 ms on, 300 ms off (PWR-3); REG_37 reads 0x1D00 and 0x9F1F in turn, nothing else (PWR-5)."""
    t.bench()
    stopped = quiet(t, bsave="Level 1")
    try:
        t.cmd(f"slot A vfo {QUIET_VHF}")
        t.cmd("slot A active")
        t.home()
        t.press("BACK")  # a key: no change on home
        t0 = time.time()
        s = samples(lambda: t.bk(0, 0x30), 13.5)
        held = [v for at, v in s if at - t0 <= 9.5]
        late = [(at, v) for at, v in s if at - t0 >= 10.5]
        t.check(held and all(v == AWAKE for v in held), "held receiving for 10 s after the key", sorted(set(held)))
        t.check({0, AWAKE} <= {v for _, v in late}, "then asleep and awake in turn", sorted({v for _, v in late}))
        runs, cur, start = [], None, None
        for at, v in late:
            if v != cur:
                if cur is not None:
                    runs.append(at - start)
                cur, start = v, at
        rt = statistics.median(s[i + 1][0] - s[i][0] for i in range(len(s) - 1))
        med = statistics.median(runs[1:]) * 1000 if len(runs) > 2 else None
        t.note(f"runs {[round(r * 1000) for r in runs]} ms, a read every {rt * 1000:.0f} ms")
        t.check(med is not None and abs(med - 300) <= max(150, 2000 * rt), "about 300 ms on, 300 ms off", med)
        # REG_37 alone through whole phases: a REG_37, REG_30, REG_37 triple takes longer over the link than a
        # 300 ms phase, so it straddled the sleep and woke halves and never saw 0x1D00 (Oct 15, 2026). The sleep
        # writes REG_30 0, waits the settle, then REG_37 0x1D00 (PWR-21, host-tested in ferrite-radio).
        s37 = samples(lambda: t.bk(0, 0x37), 6)
        seen = sorted({v for _, v in s37})
        t.note(f"REG_37 read {len(s37)} times: {[f'0x{v:04X}' for v in seen]}")
        t.eq(seen, sorted([ASLEEP_37, AWAKE_37]), "REG_37: 0x1D00 asleep and 0x9F1F awake in turn, nothing else")
    finally:
        t.put_back(stopped)


@test("BCR-20", needs=("bench",), where="radio", pages=("Home", "Fm", "Hf"))
def si_inputs_on_each_receiver(t):
    """PA7/PC5: 1/0 while the BK4829 radio has the antenna once the SI4732 has been used (0/0 from boot until then,
    as stock), 0/0 for FM broadcast (slot A tuned into the band), 1/1 for HF mode's AM (the LNA path)."""
    t.bench()
    from .modes import HfVfo, hf_mode, hf_type
    stopped = quiet(t)
    try:
        t.cmd(f"slot A vfo {QUIET_VHF}")
        t.cmd("slot A active")
        t.home()
        time.sleep(0.5)
        # Boot drives both low and they stay so until the SI4732 is first used, as stock (its master init; 1/0
        # only on leaving broadcast): 0/0 before any broadcast since power-on, 1/0 after.
        first = (t.pin("PA7"), t.pin("PC5"))
        t.check(first in ((1, 0), (0, 0)), "home on 2 m: PA7/PC5 1/0 (0/0 before the SI4732's first use)", first)
        # BCR-1 (decision 352): no FM screen; a slot tuned into the band is the broadcast receiver
        t.cmd("slot A vfo 101.1")
        t.home()
        time.sleep(1.0)
        t.eq((t.pin("PA7"), t.pin("PC5")), (0, 0), "FM broadcast: PA7/PC5 0/0")
        t.cmd(f"slot A vfo {QUIET_VHF}")
        t.home()
        time.sleep(0.5)
        t.eq((t.pin("PA7"), t.pin("PC5")), (1, 0), "home after FM: PA7/PC5 1/0")
        vfo = HfVfo(t)
        try:
            hf_type(t, 9.58)
            hf_mode(t, "AM")
            time.sleep(0.5)
            t.eq((t.pin("PA7"), t.pin("PC5")), (1, 1), "HF AM: PA7/PC5 1/1")
        finally:
            vfo.restore()
        time.sleep(0.5)
        t.eq((t.pin("PA7"), t.pin("PC5")), (1, 0), "home again: 1/0")
    finally:
        t.put_back(stopped)


SQ_CH = 990


@test("SQL-6", needs=("bench",), where="radio", pages=("Home", "Quick"))
def channel_squelch_override(t):
    """A channel with its own squelch (squelch=7) sets the chip's thresholds (REG_78) for 7 whatever the slot's
    level; the same channel at Global uses the slot's level. Thresholds as stock computes them from this radio's
    calibration (--backup)."""
    t.bench()
    cal = cal_sector(t)
    ok, out = t.cmd_err(f"ch {SQ_CH}")
    t.need(not ok or "UITEST" in out or "empty" in out.lower() or "no channel" in out.lower(),
           f"channel {SQ_CH} is in use: {out!r}")
    stopped = quiet(t)
    t.cmd("slot A active")
    t.home()
    level = squelch_level(t)
    t.home()
    f10 = round(float(QUIET_VHF) * 100_000)
    try:
        t.cmd(f'ch {SQ_CH} "UITEST SQL" {QUIET_VHF} squelch={7 if level != 7 else 2}')
        own = 7 if level != 7 else 2
        u = t.ui()
        if u.active.get("head", "").startswith("VFO"):
            t.keys("VM")
        t.keys(" ".join(str(SQ_CH)) + " MENU")
        u = t.home()
        t.eq(u.active.get("name"), "UITEST SQL", f"A on channel {SQ_CH}")
        time.sleep(0.4)
        t.eq(f"{t.bk(0, 0x78):04X}", f"{sq_regs(cal, f10, own)[0]:04X}", f"the channel's level {own}, not the slot's {level}")
        t.cmd(f'ch {SQ_CH} "UITEST SQL" {QUIET_VHF} squelch=global')
        t.home()
        t.keys("VM")
        t.keys(" ".join(str(SQ_CH)) + " MENU")
        u = t.home()
        time.sleep(0.4)
        t.has(t.cmd(f"ch {SQ_CH}"), "squelch=global", "the channel at Global")
        t.eq(f"{t.bk(0, 0x78):04X}", f"{sq_regs(cal, f10, level)[0]:04X}", f"Global: the slot's level {level}")
    finally:
        u = t.home()
        if not u.active.get("head", "").startswith("VFO"):
            t.keys("VM")
        t.home()
        t.cmd_err(f"ch {SQ_CH} delete")
        set_squelch(t, level)
        t.put_back(stopped)


@test("TSRCH-8", "DEC-1", "SAFE-16", pages=("Counter", "Home"))
def counter_and_decoders_never_key(t):
    """The frequency counter (hold SIDE2) and a decoder running (the pager, 0 4 2 1) only receive: the radio's own
    keyed state stays idle throughout (`status`)."""
    t.home()
    u = t.press("SIDE2", long=True)
    t.eq(u.id, "Counter", "hold SIDE2: the counter")
    keyed = set()
    end = time.time() + 3
    while time.time() < end:
        keyed.add(fl.parse_status(t.r.status_text())["keyed"])
    t.eq(keyed, {None}, "the counter: never keyed")
    t.home()
    t.set("0 4 2 1", "On")
    end = time.time() + 3
    keyed = set()
    while time.time() < end:
        keyed.add(fl.parse_status(t.r.status_text())["keyed"])
    t.eq(keyed, {None}, "the pager decoding: never keyed")


@test("TERM-36.6", needs=("bench",), where="radio", pages=("Home",))
def gps_never_saves_or_changes_baud(t):
    """Only queries and standby go to the GPS module: `gps baud 9600`, `gps query 3` and `gps nmea 2` are refused
    (never CAS00, its flash, or CAS01, its baud rate)."""
    t.bench()
    for line in ("gps baud 9600", "gps query 3", "gps nmea 2"):
        ok, out = t.cmd_err(line)
        t.check(not ok or "refused" in out.lower() or "gps " in out.lower() and "usage" in out.lower(),
                f"`{line}`: refused", out)


@test("XB-20.3", needs=("bt", "pin"), pages=("Group", "Pin"))
def xb_needs_the_pin_over_bluetooth(t):
    """PIN lock on: over Bluetooth before AUTH `xb start`, and `xb` alone, are not authorised (the repeater stays
    off); past AUTH `xb` reads its state. Nothing is started."""
    with pin_lock(t):
        set_pin_lock(t)
        raw = t.bt_raw()
        t.eq(raw.link.ask(fl.CMD, b"xb start")[0], 5, "xb start before AUTH: not authorised")
        t.eq(raw.link.ask(fl.CMD, b"xb")[0], 5, "xb before AUTH: not authorised")
        t.check("running" not in t.cmd("xb").lower() or "not running" in t.cmd("xb").lower(),
                "the repeater still off (the cable's xb)", t.cmd("xb"))
        t.eq(raw.link.ask(fl.AUTH, the_pin(t).encode())[0], 0, "AUTH with the PIN")
        s, rest = raw.link.ask(fl.CMD, b"xb")
        t.eq(s, 0, "past AUTH: xb reads")
        t.close_bt()

