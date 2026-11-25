"""Checks read from the cable log (0 9 9 5, TERM-28) while the keys and readback go over Bluetooth: the cable must
carry no byte from the PC (the first one ends the log), so these run with

    tools/uitest.py --port ble:walkie-talkie --cablelog /dev/cu.usbserial-XXXX --module cablelog

(Bluetooth on, 0 9 2 1; the cable log is turned on for each test and off after). Radio only: the simulator has no
cable log. GPS checks need GPS on; the standby one a fix.
"""
import re
import time

from .core import test

SET_NORMAL = "$PCAS03,1,0,5,5,1,0,0,0*02"
SET_SKY = "$PCAS03,1,0,1,1,1,0,0,0*02"
NMEA_OFF = "$PCAS03,0,0,0,0,0,0,0,0*02"
COUNTS = r"gps: (\d+) sentences, (\d+) bytes in 5 s"


def gps_on(t):
    t.need(t.get("0 6 1") == "On", "GPS isn't on (0 6 1)")


@test("GPS-34.1", needs=("cablelog",), where="radio", pages=())
def nmea_set_after_power_up(t):
    """GPS off and on: once the module is heard after the power-up, the cable log shows Ferrite's NMEA set as its
    bytes ("gps: > $PCAS03,1,0,5,5,1,0,0,0*02<CR><LF> (28 bytes)"), and the 5 s counts fall to about a third
    (12-18 sentences against about 50)."""
    gps_on(t)
    log = t.cable_log()
    t.set("0 6 1", "Off")
    time.sleep(2)
    t0 = time.time()
    t.set("0 6 1", "On")
    hit = log.wait(re.escape("gps: > " + SET_NORMAL), t0, 30)
    t.check(hit, "the set, once the module is heard", [ln for _, ln in log.since(t0)][-10:])
    t.check("(28 bytes)" in hit[1], "as its bytes", hit[1])
    c = log.wait(COUNTS, hit[0] + 5.5, 15)
    t.check(c, "a 5 s count after the set", [ln for _, ln in log.since(hit[0])][-10:])
    n = int(re.search(COUNTS, c[1]).group(1))
    t.check(n <= 20, "the counts fall to about a third (12-18 a 5 s)", c[1])


@test("GPS-34.3", needs=("cablelog",), where="radio", pages=("Gps", "Home"))
def sky_set_with_the_satellite_pages(t):
    """Opening the GPS position or satellites page sends the sky set ($PCAS03,1,0,1,1,1,0,0,0*02: GSA and GSV
    every fix); leaving sends the normal one; a waypoint page sends neither."""
    gps_on(t)
    log = t.cable_log()
    t0 = time.time()
    t.keys("MENU 0 6 2")
    t.keys("RIGHT")
    t.check(log.wait(re.escape(SET_SKY), t0, 8), "the satellites page: the sky set", log.since(t0)[-5:])
    t1 = time.time()
    t.home()
    t.check(log.wait(re.escape(SET_NORMAL), t1, 8), "home: the normal set", log.since(t1)[-5:])
    t2 = time.time()
    t.keys("MENU 0 6 7")
    time.sleep(4)
    t.check(not [ln for _, ln in log.since(t2) if "PCAS03" in ln], "a waypoint page: no set", log.since(t2)[-5:])


@test("GPS-31", needs=("cablelog", "fix"), where="radio", slow=True, pages=("Gps", "Home"))
def wake_while_asleep(t):
    """While the module sleeps (the log's "gps: standby N s"), opening the position page wakes it: "gps: wake byte"
    and NMEA within 3 s, or "gps: no NMEA after a wake: power cycled" and the power cycle (records which)."""
    gps_on(t)
    log = t.cable_log()
    t0 = time.time()
    hit = log.wait(r"gps: standby \d+ s", t0, 130)
    t.check(hit, "the module goes into standby (GPS-26)", log.since(t0)[-8:])
    time.sleep(3)
    t1 = time.time()
    t.keys("MENU 0 6 2")
    woke = log.wait(r"gps: wake byte|gps: no NMEA after a wake", t1, 10)
    t.check(woke, "a wake byte (or the fallback's power cycle)", log.since(t1)[-8:])
    if "wake byte" in woke[1]:
        nmea = log.wait(COUNTS, t1, 12)
        t.note(f"wake byte; counts after: {nmea[1] if nmea else None}")
    else:
        t.check(log.wait(r"gps: power (off|on)", t1, 5), "the power cycle", log.since(t1)[-8:])
        t.note("the wake byte didn't work: the PA8 power cycle")


@test("TERM-36.3", needs=("cablelog", "bench"), where="radio", pages=())
def gps_nmea_off_and_on(t):
    """`gps nmea off` sends $PCAS03,0,0,0,0,0,0,0,0*02 (in the log as its 28 bytes) and the 5 s counts fall to 0;
    `gps nmea on` sends Ferrite's set and the counts come back."""
    gps_on(t)
    t.bench()
    log = t.cable_log()
    t0 = time.time()
    t.cmd("gps nmea off")
    hit = log.wait(re.escape("gps: > " + NMEA_OFF), t0, 5)
    t.check(hit and "(28 bytes)" in hit[1], "NMEA off as its bytes", log.since(t0)[-5:])
    c = log.wait(COUNTS, hit[0] + 5.5, 12)
    try:
        t.check(c and re.search(COUNTS, c[1]).group(1) == "0", "the 5 s counts fall to 0 (else the module ignores "
                "CAS03: note it)", c)
    finally:
        t1 = time.time()
        t.cmd("gps nmea on")
    t.check(log.wait(re.escape("gps: > " + SET_NORMAL), t1, 5), "nmea on: Ferrite's set", log.since(t1)[-5:])
    c = log.wait(COUNTS, t1 + 5.5, 12)
    t.check(c and int(re.search(COUNTS, c[1]).group(1)) > 0, "the counts come back", c)


@test("TERM-36.4", needs=("cablelog", "bench"), where="radio", slow=True, pages=())
def gps_quiet_standby(t):
    """`gps standby 40 quiet`: NMEA off, CAS12 100 ms after it, and not NMEA on; the set comes by itself once NMEA
    is back or 43 s on ("gps: bench: over")."""
    gps_on(t)
    t.bench()
    log = t.cable_log()
    t0 = time.time()
    t.cmd("gps standby 40 quiet")
    off = log.wait(re.escape("gps: > " + NMEA_OFF), t0, 5)
    cas = log.wait(r"gps: > \$PCAS12,40\*", t0, 5)
    t.check(off and cas, "NMEA off, then CAS12", log.since(t0)[-6:])
    t.check(0.05 <= cas[0] - off[0] <= 0.5, "about 100 ms apart", cas[0] - off[0])
    t.check(not [ln for ts, ln in log.since(t0) if SET_NORMAL in ln and ts < cas[0] + 1.5], "no NMEA on with it", "")
    over = log.wait(r"gps: bench: over", t0, 50)
    t.check(over, "the test's end", log.since(t0)[-8:])
    t.check(log.wait(re.escape("gps: > " + SET_NORMAL), cas[0], 50), "the set by itself", log.since(cas[0])[-8:])


@test("HOME-13.1", needs=("cablelog",), where="radio", pages=("Home",))
def knob_redraws_light_then_whole(t):
    """A knob spin on home: while it turns each redraw is a light one (a few ms, the active slot's part), and one
    full redraw follows about 400 ms after the last step (the log's "draw N ms, K KB to the panel")."""
    log = t.cable_log()
    t.home()
    time.sleep(1)
    t0 = time.time()
    t.knob(10)
    time.sleep(1.5)
    lines = log.since(t0)
    knobs = [ts for ts, ln in lines if ln.startswith("knob") or ln.startswith("key ") and "knob" in ln]
    draws = [(ts, int(m.group(1)), int(m.group(2))) for ts, ln in lines
             if (m := re.match(r"draw (\d+) ms, (\d+) KB", ln))]
    t.check(draws, "draw lines in the log", lines[-10:])
    last = knobs[-1] if knobs else None
    if last is None:
        t.note("no knob lines in the log: the spin's end taken from the draws")
        last = max(ts for ts, _, _ in draws if ts - t0 < 1.0)
    during = [d for d in draws if d[0] <= last + 0.05]
    after = [d for d in draws if d[0] > last + 0.05]
    t.check(during and max(ms for _, ms, _ in during) < 20, "while turning: light redraws (under 20 ms, HOME-13.1)", during)
    big = [d for d in after if d[2] >= max(kb for _, _, kb in during) * 2 or d[1] >= 10]
    t.check(big, "a full redraw after", after)
    t.check(0.25 <= big[0][0] - last <= 0.6, "about 400 ms after the last step", big[0][0] - last)
    t.knob(-10)
