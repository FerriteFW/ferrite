"""Low Power Mode (chapter 33: PWR-24, SBAR-24)."""
import time

from .core import test


@test("PWR-24", "PWR-24.2", "PWR-24.6", "PWR-24.7", "SBAR-24", pages=("Group", "Home", "LowPower", "Status"))
def low_power_on_from_the_menu(t):
    """0 7 7 is a list (Battery Save, Low Power Mode, Auto below); turning 0 7 7 2 On goes home with the toast,
    LOW in the bar (`lpm=1`), Bluetooth and GPS shown off, the Status page saying so; Off puts it all back."""
    u = t.keys("MENU 0 7 7")
    t.eq(u.names()[:3], ["Battery Save", "Low Power Mode", "Auto below"], "0 7 7's list")
    t.eq((u.row("Low Power Mode")["value"], u.row("Auto below")["value"]), ("Auto", "20 %"), "the defaults (decision 236)")
    t.set("0 7 7 2", "On")
    u = t.ui(0.3)
    t.eq(u.id, "LowPower", "its own home on entering (PWR-24.7)")
    t.check(u.toast and u.toast["head"] == "Low Power Mode", "the toast", u.toast)
    t.eq(u.bar["lpm"], 1, "LOW in the bar")
    t.eq(u.bar["bt_on"], 0, "Bluetooth off")
    if not t.sim:  # the simulator's GPS ignores 0 6 1 (radio only); the receiver takes about 2 s to stop
        g = t.wait(lambda u: u.bar["gps"] == "off", timeout=4, what="GPS off")
        t.eq(g.bar["gps"], "off", "GPS off")
    u = t.keys("MENU 0 0")
    t.eq(u.row("Battery Save")["value"], "Low Power Mode", "Status says so")
    t.home()
    t.set("0 7 7 2", "Off")
    u = t.ui(0.3)
    t.eq(u.bar["lpm"], 0, "out: no LOW")
    t.check(u.toast and u.toast["body"] == "off", "the leaving toast", u.toast)


@test("PWR-24.4", pages=("Group", "LowPower"))
def low_power_keeps_one_vfo(t):
    """In the mode: scan (SIDE2) and FM broadcast are refused with a toast; the menus, tuning and the active slot's
    editing stay."""
    t.set("0 7 7 2", "On")
    u = t.press("SIDE2", settle=0.3)
    t.eq((u.id, u.bar["scan"]), ("LowPower", 0), "SIDE2: no scan")
    t.check(u.toast and "SIDE1+2" in u.toast["body"], "refused, naming the way out (PWR-24.6)", u.toast)
    f0 = float(u.active["freq"])
    u = t.knob(+1, settle=0.3)
    t.check(float(u.active["freq"]) != f0, "the knob still tunes", u.active["freq"])
    t.knob(-1)
    u = t.keys("MENU 0 7")
    t.eq(u.id, "Group", "the menus stay")
    t.home()
    t.set("0 7 7 2", "Off")


@test("PWR-24.4", pages=("Home", "LowPower"))
def side_keys_leave_low_power(t):
    """SIDE1 + SIDE2 held 1 s leave the mode (0 7 7 2 back to Off); out of the mode the same hold does nothing
    (it never enters); half a second isn't enough."""
    t.r.down("SIDE1")
    t.r.down("SIDE2")
    time.sleep(1.3)
    t.r.up("SIDE2")
    t.r.up("SIDE1")
    t.home()
    t.eq(t.ui(0.2).bar["lpm"], 0, "the hold never enters")
    t.set("0 7 7 2", "On")
    t.home()
    t.r.down("SIDE1")
    t.r.down("SIDE2")
    time.sleep(0.4)
    leave = int(t.ui(0).state.get("leave", 0))
    t.check(200 <= leave < 1000, "the Leaving bar part way (state leave=)", leave)
    t.r.up("SIDE2")
    t.r.up("SIDE1")
    t.eq(t.ui(0.2).bar["lpm"], 1, "half a second: still in")
    t.r.down("SIDE1")
    t.r.down("SIDE2")
    time.sleep(1.4)
    t.r.up("SIDE2")
    t.r.up("SIDE1")
    u = t.ui(0.3)
    t.eq((u.id, u.bar["lpm"]), ("Home", 0), "held 1 s: out, the normal home, no counter")
    t.eq(t.get("0 7 7 2"), "Off", "On became Off")
