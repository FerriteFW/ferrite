"""Home: the cards, their chips, the layouts, shortcuts, quick mute and the quick menu (chapter 6, HOME-*)."""
import time

from . import squelch
from .core import test

LAYOUTS = ["Watch", "Single + scope", "Watch + activity"]


@test("HOME-2", "KEY-5", smoke=True, pages=("Edit", "Entry", "Group", "Home", "Info", "Quick", "Scope", "Status"))
def hold_back_goes_home(t):
    """Hold BACK goes home from a menu, a group, the Status page, entry, the scope and the editor."""
    for seq in ("MENU", "MENU 0 7 4", "MENU 0 0", "1 4", "SLOT:long", "MENU 0 9 8 RIGHT RIGHT"):
        t.home()
        u = t.keys(seq)
        t.check(u.id != "Home", f"{seq}: left home", u.id)
        u = t.press("BACK", long=True)
        t.eq(u.id, "Home", f"hold BACK after {seq}")
    t.home()
    u = t.press("1", long=True)
    t.eq(u.id, "Scope", "hold 1: the scope")
    u = t.press("BACK", long=True)
    t.eq(u.id, "Home", "hold BACK from the scope")


@test("HOME-4", "VFO-20", smoke=True, pages=("Home", "Quick"))
def active_card_fields(t):
    """The active card: letter, VFO/channel and band, frequency, mode chip (FM, NFM, AM, WFM), tones, shift, power."""
    t.cmd("slot A vfo 146.52 fm low tone 100.0 offset -0.6")
    u = t.ui()
    a = u.cards["A"]
    t.eq(a["active"], 1, "A is active")
    t.eq(a["head"], "VFO · VHF", "the header: VFO and the band")
    t.eq(a["freq"], "146.52000", "the frequency")
    t.eq((a["mode"], a["tone"], a["shift"], a["power"]), ("FM", "T 100.0", "-600k", "LOW"),
         "chips: mode, tone, shift; the power shows at Low too")
    for word, chip in (("nfm", "NFM"), ("am", "AM"), ("fm", "FM")):
        t.cmd(f"slot A {word}")
        t.eq(t.ui().cards["A"]["mode"], chip, f"mode {word}: the chip")
    u = t.keys("MENU")
    t.eq(u.num(3)["name"], "Mode", "quick settings row 3 is Mode (the bandwidth)")
    t.home()
    t.cmd("slot A mid")
    t.eq(t.ui().cards["A"]["power"], "MID", "power Mid")
    t.cmd("slot A low")


@test("HOME-4", "VFO-6", pages=("Home",))
def tone_chip_both_ways(t):
    """The tone chip in 7 characters: T 100.0 both ways or sent only; R 88.5 heard only; T100.0* sent with a
    different heard tone; DCS as D023N; nothing with no tones."""
    cases = [("tone 100.0", "T 100.0"), ("txtone 100.0 rxtone off", "T 100.0"), ("txtone off rxtone 88.5", "R 88.5"),
             ("txtone 100.0 rxtone 88.5", "T100.0*"), ("tone D023N", "D023N"), ("txtone off rxtone D023N", "R D023N"),
             ("txtone D754I rxtone D023N", "D754I*"), ("tone off", "")]
    for words, chip in cases:
        t.cmd(f"slot A vfo {words}")
        got = t.ui().cards["A"]["tone"]
        t.eq(got, chip, f"{words}: the chip")
        t.check(len(got) <= 7, f"{words}: 7 characters at most", got)


@test("HOME-3", pages=("Home", "Quick"))
def layouts_keep_the_keys(t):
    """Each of the three layouts: MENU opens quick settings, SLOT moves the active slot, the cards stay A, B, C."""
    for lay in LAYOUTS:
        t.set("0 7 2", lay)
        u = t.home()
        t.eq([c["slot"] for c in u.d["cards"]], ["A", "B", "C"], f"{lay}: the cards")
        u = t.keys("MENU")
        t.eq(u.id, "Quick", f"{lay}: MENU")
        t.home()
        u = t.keys("SLOT")
        t.eq(u.state["active"], "B", f"{lay}: SLOT")
        u = t.keys("SLOT SLOT")
        t.eq(u.state["active"], "A", f"{lay}: SLOT twice more")


@test("HOME-5", "SBAR-16", pages=("Quick", "Status"))
def watch_cards_and_bar(t):
    """Watch A+B+C: TW (watch=3) on home only; A+B: DW (2); Off: nothing; the watched cards show a state."""
    for val, n in (("A+B+C", 3), ("A+B", 2), ("Off", 0)):
        t.set("0 1 1", val)
        u = t.home()
        t.eq(u.bar["watch"], n, f"watch {val}: the bar")
        for L in "ABC":
            t.check(u.cards[L].get("state"), f"watch {val}: card {L} shows a state", u.cards[L])
        u = t.keys("MENU")
        t.eq(u.bar["watch"], 0, f"watch {val}: not in the bar away from home")
        u = t.keys("BACK MENU 0 0")
        t.eq(u.bar["watch"], n, f"watch {val}: on the Status page too")


@test("HOME-8", "KEY-3", pages=("Home",))
def hold_digit_shortcuts(t):
    """Hold 4 (watch on/off) and hold 2 (TX power): a popup names it with the new value, at the long press;
    nothing opens and the release does nothing."""
    w0 = t.get("0 1 1")
    u = t.press("4", long=True)
    t.eq(u.id, "Home", "no page opens")
    t.check(u.popup and u.popup["head"] == "Watch", "the popup names the shortcut", u.popup)
    w1 = t.get("0 1 1")
    t.check(w1 != w0, "watch changed", w1)
    t.eq(u.popup["body"].lower(), w1.lower(), "the popup shows the new value")
    u = t.press("4", long=True)
    w2 = t.get("0 1 1")
    t.check((w2 == "Off") != (w1 == "Off"), "hold 4 again: watch on/off the other way", (w1, w2))
    t.note(f"watch {w0} -> {w1} -> {w2}")
    # Down and up apart: the shortcut runs while still held.
    p0 = t.ui().active["power"]
    t.r.down("2")
    time.sleep(0.9)
    u = t.ui(0)
    t.check(u.popup and u.popup["head"] == "TX power", "hold 2: it ran while held", u.popup)
    t.r.up("2")
    u = t.ui()
    t.eq(u.id, "Home", "the release does nothing")
    p1 = u.active["power"]
    t.check(p1 != p0, "power changed once", p1)
    for _ in range(4):
        if t.ui(0).active["power"] == p0:
            break
        t.press("2", long=True)
    t.eq(t.ui(0.3).active["power"], p0, "power back")


@test("HOME-10", "KEY-17", "SBAR-13", pages=("Info", "Status"))
def hash_opens_help(t):
    """# on home opens the quick help on the holds (decision 376; watch off, so nothing to pause); # goes on
    through the help pages and round, BACK goes home; the Status page is MENU 0 0."""
    t.set("0 1 1", "Off")
    t.home()
    u = t.keys("HASH")
    t.eq((u.id, u.title), ("Info", "Holds"), "# on home: the quick help, the holds first")
    titles = [u.title]
    for _ in range(5):
        titles.append(t.keys("HASH").title)
    t.eq(titles, ["Holds", "Keys", "Side+PTT", "Menu nos", "Manual", "Holds"], "# goes round the help pages")
    t.eq(t.keys("BACK").id, "Home", "BACK: home")
    st = t.keys("MENU 0 0")
    t.check(st.id == "Status" and st.rows and st.rows[0]["name"] == "Transmit", "Status: MENU 0 0", st.rows[:2])


@test("HOME-10", "WATCH-6", pages=("Home", "Quick", "Status"))
def hash_pauses_watch_while_receiving(t):
    """While a watched slot is receiving the hint bar says "# Pause" and # pauses watch instead of the quick help. Slot B is
    made to receive by opening its squelch fully (level 0: the noise floor opens it), so no signal is needed."""
    t.set("0 1 1", "A+B+C")
    was = t.home().state["active"]
    t.cmd("slot B active")
    v0 = squelch.level(t)
    try:
        squelch.set_level(t, "0")
        t.cmd("slot A active")
        t.home()
        u = t.wait(lambda u: any(c["rx"] and not c["active"] for c in u.d["cards"]), timeout=10,
                   what="a watched slot receiving", every=0.2)
        if u.hint("#") != "Pause":
            u = t.ui(0)
        t.need(any(c["rx"] and not c["active"] for c in u.d["cards"]), "the receiving slot went quiet")
        t.eq(u.hint("#"), "Pause", "the hint bar")
        u = t.keys("HASH")
        t.eq(u.id, "Home", "# stays home")
        t.check(u.toast and "paused" in u.toast["head"].lower(), "watch paused", u.toast)
        st = t.keys("MENU 0 0")
        t.has(st.row("Watch")["value"], "paused", "Status: Watch")
    finally:
        t.home()
        t.cmd("slot B active")
        squelch.set_level(t, v0)
        t.cmd(f"slot {was} active")
        t.home()


@test("HOME-11", "NAV-5", pages=("Home",))
def slot_cycles_cards_stay(t):
    """SLOT cycles A -> B -> C -> A; the cards stay stacked A, B, C, only the active one changes."""
    seen = []
    for _ in range(3):
        u = t.keys("SLOT")
        seen.append(u.state["active"])
        t.eq([c["slot"] for c in u.d["cards"]], ["A", "B", "C"], "the order")
        t.eq([c["slot"] for c in u.d["cards"] if c["active"]], [u.state["active"]], "one active card")
    t.eq(seen, ["B", "C", "A"], "SLOT order")


@test("HOME-15", "HOME-15.1", "KEY-18", pages=("Home",))
def star_quick_mute(t):
    """* on home mutes the active slot (card says Muted, toast); * again a second later unmutes it; * * within a
    second turns it off (card says Off, toast); * on an off slot turns it on; muted, the slot keeps receiving."""
    was = t.unmute()
    if was:
        t.note("slot A was muted at the start (put back)")
    u = t.keys("STAR")
    try:
        t.check("muted" in u.active["state"].lower(), "the card says Muted", u.active["state"])
        t.check(u.toast and "muted" in u.toast["head"], "the toast", u.toast)
        time.sleep(1.5)
        u = t.keys("STAR")
        t.check("muted" not in u.active["state"].lower() and u.active["state"] != "Off", "a later *: unmuted", u.active)
        u = t.keys("STAR STAR")
        t.check(u.active["state"] == "Off", "* * quickly: Off", u.active["state"])
        t.check(u.toast and u.toast["head"].endswith(" off"), "the off toast", u.toast)
    finally:
        if u.active["state"] == "Off" or "muted" in u.active["state"].lower():
            u = t.keys("STAR")
    t.check("muted" not in u.active["state"].lower() and u.active["state"] != "Off", "* on Off: on", u.active)
    if was:
        t.keys("STAR")


@test("HOME-17", "HOME-17.1", "KEY-18", pages=("Home", "Scan"))
def quick_menu(t):
    """Hold * on home: the quick menu (PTT A Reverse, PTT B Night/day, PF1 GPS, PF2 Bluetooth, Knob
    Brightness); the knob changes the brightness, PF1 steps GPS (off, auto, on); BACK closes it. Not while scanning."""
    u = t.press("STAR", long=True)
    t.eq([q[:2] for q in u.qmenu], [["Knob", "Brightness"], ["PTT A", "Reverse"], ["PTT B", "Night/day"],
                                     ["PF1", "GPS"], ["PF2", "Bluetooth"]], "the quick menu, the knob first (HOME-17.1)")
    b0 = int(u.qmenu[0][2])
    g0 = u.qmenu[3][2]
    u = t.knob(-1)
    t.eq(int(u.qmenu[0][2]), b0 - 1, "the knob: brightness")
    t.eq(t.get("0 7 4 2"), str(b0 - 1), "the setting follows")
    u = t.knob(+1)
    t.eq(int(u.qmenu[0][2]), b0, "and back")
    # GPS-1: PF1 steps 0 6 1 through Off, Auto and On, so three presses come back.
    u = t.keys("SIDE1")
    t.check(u.qmenu and u.qmenu[3][2] != g0, "PF1 stepped GPS", u.qmenu)
    t.check(u.qmenu[3][2] in ("off", "auto", "on"), "PF1: off, auto or on", u.qmenu)
    u = t.keys("SIDE1")
    t.check(u.qmenu[3][2] not in (g0, ""), "PF1 again: the third state", u.qmenu)
    u = t.keys("SIDE1")
    t.eq(u.qmenu[3][2], g0, "PF1 a third time: GPS back")
    u = t.keys("BACK")
    t.eq(u.qmenu, [], "BACK closes it")
    t.eq(u.id, "Home", "still home")
    # The active slot scanning: hold * does nothing.
    t.keys("SIDE2")
    u = t.ui()
    t.need(u.id in ("Scan", "Home"), f"SIDE2 opened {u.id}")
    t.home()
    if t.ui().active.get("scan"):
        u = t.press("STAR", long=True)
        t.eq(u.qmenu, [], "no quick menu while the active slot scans")
    t.keys("SIDE2")


@test("HOME-17", pages=("Home",))
def quick_menu_brightness_only_while_up(t):
    """The knob is the brightness only while the quick menu is up: after BACK it tunes again."""
    f0 = t.ui().active["freq"]
    t.press("STAR", long=True)
    t.knob(+1)
    t.knob(-1)
    t.keys("BACK")
    u = t.knob(+1)
    t.check(u.active["freq"] != f0, "the knob tunes again", u.active["freq"])
    t.knob(-1)
    t.eq(t.ui().active["freq"], f0, "tuned back")
