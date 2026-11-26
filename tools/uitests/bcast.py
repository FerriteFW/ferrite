"""Broadcast FM on the home slots (chapter 20, BCR-*): a slot in 87.5-108 MHz plays broadcast on the SI4732.

The readback has the broadcast card's kind, state, frequency and S-meter; its RDS text, STEREO and SNR, and the FM
screen's own lines, aren't in it yet (BCR-2 and BCR-26 wait on them).
"""
from .core import test

QUIET = "146.55"  # a 2 m frequency with nothing on it (the simulator's band noise stays under squelch 4)


def card(t, L, settle=0.5):
    return t.ui(settle).cards[L]


def mhz_of_slot(t, L):
    for w in t.slot(L).split():
        if "." in w and w.replace(".", "").isdigit():
            return float(w)
    t.fail(f"slot {L}: no frequency in {t.slot(L)!r}")


@test("BCR-21", "BCR-26.8", pages=("Home",))
def slot_in_the_band_is_broadcast(t):
    """A VFO in 87.5-108.0 MHz (ends included) is a broadcast slot; just outside it, and tuned back out, it is a
    BK4829 slot again (the card's kind and header say which)."""
    region = t.get("0 9 4")
    t.need(region != "Japan", "region Japan: the band is 76-95 MHz")
    for f, kind in (("87.4", "slot"), ("87.5", "broadcast"), ("98.1", "broadcast"), ("108.0", "broadcast"),
                    ("108.025", "slot"), (QUIET, "slot")):
        t.cmd(f"slot A vfo {f}")
        a = card(t, "A")
        t.eq(a["kind"], kind, f"A on {f} MHz")
        if kind == "broadcast":
            # BCR-26.8: a station sending its programme type names it there instead ("VFO · Jazz")
            t.check(a["head"].startswith("VFO · ") and len(a["head"]) > len("VFO · "),
                    f"{f} MHz: the header names the band, or the station's programme type", a["head"])
            if a["head"] != "VFO · Broadcast":
                t.note(f"{f} MHz: the station sends its programme type: {a['head']}")
    t.check(a["head"] != "VFO · Broadcast", "tuned out: an ordinary slot", a["head"])


@test("BCR-22", pages=("Home", "Status"))
def one_broadcast_slot_and_handover(t):
    """One slot plays broadcast at a time: the one that came into the band last. The other reads "WFM on A"; when
    the playing slot leaves, the one still in the band takes the broadcast."""
    t.cmd(f"slot A vfo {QUIET}")
    t.cmd("slot B vfo 100.1")
    b = card(t, "B")
    t.eq(b["kind"], "broadcast", "B in the band")
    t.check(b["state"] in ("WFM", "muted"), "B plays (or is muted by a slot heard)", b["state"])
    t.cmd("slot A vfo 98.5")
    u = t.ui(0.5)
    t.eq(u.cards["B"]["state"], "WFM on A", "A came in last: B says the broadcast is on A")
    t.check(u.cards["A"]["state"] in ("WFM", "muted"), "A has it", u.cards["A"]["state"])
    t.cmd(f"slot A vfo {QUIET}")
    u = t.ui(0.5)
    t.eq(u.cards["A"]["kind"], "slot", "A tuned out")
    t.check(u.cards["B"]["state"] in ("WFM", "muted"), "B, still in the band, takes the broadcast back",
            u.cards["B"]["state"])
    u = t.keys("MENU 0 0")
    t.eq(u.row("Broadcast")["value"], "WFM 100.1 on B", "Status: the broadcast and its slot")


@test("BCR-23", "BCR-15", pages=("Home", "Scan"))
def broadcast_ignores_watch_and_drops_relay_slots(t):
    """The broadcast slot plays whatever Watch says (Off too); a slot needing the band relay (28 MHz) isn't
    listened to while it plays and its card says "Watch off: relay"."""
    stopped = t.stop_scans()
    try:
        t.cmd("slot A vfo 98.1")
        t.cmd("slot B vfo 28.4")
        t.set("0 1 1", "Off")
        # A slot heard just before has the speaker for 2 s more (BCR-24): wait for the broadcast to play.
        u = t.wait(lambda u: u.cards["A"]["state"] == "WFM", timeout=5, what="watch off: A plays")
        t.eq(u.cards["A"]["kind"], "broadcast", "watch off: A still the broadcast slot")
        t.eq(u.cards["B"]["state"], "Watch off: relay", "B on 28 MHz")
        t.set("0 1 1", "A+B+C")
        u = t.wait(lambda u: u.cards["A"]["state"] == "WFM", timeout=5, what="watch A+B+C: A plays")
        t.eq(u.cards["B"]["state"], "Watch off: relay", "B on 28 MHz still isn't watched")
        t.eq(u.cards["B"]["rx"], 0, "and isn't heard")
        t.cmd("slot B vfo 146.52")
        u = t.ui(0.6)
        t.check(u.cards["B"]["state"] != "Watch off: relay", "B on 2 m: watched again", u.cards["B"]["state"])
    finally:
        t.put_back(stopped)


@test("BCR-25", pages=("Home",))
def knob_tunes_and_arrows_seek_the_broadcast_card(t):
    """On the broadcast card the knob and ▲▼ tune by the FM step (0 2 6 1) and ◀▶ seek, the station found taken
    into the VFO."""
    step = t.get("0 2 6 1")
    t.need(step.endswith("kHz"), f"FM step {step!r}")
    khz = float(step.split()[0])
    t.cmd("slot A vfo 98.5")
    u = t.home()
    t.eq(u.active["kind"], "broadcast", "A in the band")
    t.eq(u.hint("<>"), "Seek", "the hint bar: ◀▶ seek")
    f0 = mhz_of_slot(t, "A")
    t.knob(+1)
    t.check(abs(mhz_of_slot(t, "A") - (f0 + khz / 1000)) < 1e-4, f"knob +1: one {step} step up", t.slot("A"))
    t.knob(-2)
    t.check(abs(mhz_of_slot(t, "A") - (f0 - khz / 1000)) < 1e-4, "knob -2: two steps down", t.slot("A"))
    t.keys("UP")
    t.check(abs(mhz_of_slot(t, "A") - f0) < 1e-4, "▲ one step up", t.slot("A"))
    t.keys("RIGHT")
    u = t.wait(lambda u: abs(mhz_of_slot(t, "A") - f0) > 1e-4, timeout=6, what="▶ seeks to another station")
    f1 = mhz_of_slot(t, "A")
    t.check(87.5 <= f1 <= 108.0, "the station found is in the band", f1)
    t.eq(u.active["kind"], "broadcast", "and the slot plays it")


@test("BCR-7", pages=("More", "Group"))
def no_volume_setting(t):
    """The SI4732's volume is fixed (the knob on top is the volume): no volume setting in any menu group, only the
    beep volume (0 7 5 2) and the message sound's (0 7 5 6, APRS-6.2): local sounds, not the radio's audio."""
    # Every setting the menus have is in `dump` (one table: MENU-7), with its name: the run's snapshot.
    found = sorted((p, n) for p, n in t.state.names.items() if "volume" in n.lower())
    t.eq(found, [("0 7 5 2", "Beep volume"), ("0 7 5 6", "Message volume")], "the only volume settings are the local sounds' (beep, message sound)")
    for g in range(1, 10):
        t.home()
        u = t.keys(f"MENU 0 {g}")
        t.check(not any("volume" in r["name"].lower() for r in u.rows), f"group {g}: no volume row", u.names())
    t.home()
    t.cmd("slot A vfo 98.1")
    u = t.home()
    u = t.press("MENU", long=True)
    t.check(u.id in ("More", "Options"), "hold MENU on a broadcast card", u.id)
    t.check(not any("volume" in r["name"].lower() for r in u.rows), "no volume there either", u.names())
