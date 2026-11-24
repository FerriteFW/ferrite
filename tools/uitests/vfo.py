"""VFO tuning, frequency entry, channels and the slot editor (chapters 9 and 10, VFO-*, NAV-*, EDIT-*)."""
from .core import test


def tune(t, f="146.52", rest="fm low tone 100.0 offset -0.6"):
    t.cmd(f"slot A vfo {f} {rest}")
    return t.home()


@test("VFO-11", "VFO-24", "VFO-19", smoke=True, pages=("Entry", "Home"))
def star_enters_a_frequency(t):
    """A digit opens entry; * is the point; 146 * 52 * tunes 146.520 (the * after a decimal is OK); # does
    nothing in entry."""
    tune(t, "145.00")
    u = t.keys("1")
    t.eq((u.id, u["entry"]["args"][0]), ("Entry", "1"), "a digit opens entry")
    u = t.keys("4 6 STAR 5")
    t.eq(u["entry"]["args"][0], "146.5", "* is the point")
    u = t.keys("HASH")
    t.eq((u.id, u["entry"]["args"][0]), ("Entry", "146.5"), "# does nothing")
    u = t.keys("2 STAR")
    t.eq(u.id, "Home", "* after a decimal enters it")
    t.eq(u.active["freq"], "146.52000", "tuned")


@test("VFO-17", pages=("Entry", "Home", "Quick"))
def whole_frequency_goes_in_without_ok(t):
    """With a 5 kHz step 146.940 goes in at its last 0; five decimals always go in."""
    tune(t, "145.00")
    u = t.keys("MENU 6")
    t.need(u.sel["value"] == "5 kHz", f"step is {u.sel['value']}, not 5 kHz")
    t.home()
    u = t.keys("1 4 6 STAR 9 4")
    t.eq(u.id, "Entry", "146.94: still typing")
    u = t.keys("0")
    t.eq((u.id, u.active.get("freq")), ("Home", "146.94000"), "146.940 went in by itself")
    u = t.keys("1 4 6 STAR 5 2 3 4 5")
    t.eq((u.id, u.active.get("freq")), ("Home", "146.52345"), "five decimals go in")


@test("VFO-12", "NAV-2", pages=("Home",))
def off_step_is_tuned_as_typed(t):
    """146.5234 OK tunes exactly that (never snapped); the knob then steps from there by the step."""
    tune(t, "145.00")
    u = t.keys("1 4 6 STAR 5 2 3 4 MENU")
    t.eq((u.id, u.active["freq"]), ("Home", "146.52340"), "what's typed is what's tuned")
    u = t.knob(+1)
    t.eq(u.active["freq"], "146.52840", "the knob steps 5 kHz from there")
    u = t.keys("DOWN")
    t.eq(u.active["freq"], "146.52340", "▼ one step back")


@test("VFO-14", "VFO-21", pages=("Entry", "Home"))
def entry_ranges(t):
    """Below 18 MHz: "< 18 MHz: HF mode, MENU 0 3 3"; 630-756 and above 1300 MHz refused with the range;
    18, 630, 756 and 1300 MHz accepted."""
    tune(t)
    u = t.keys("1 0 STAR 5 MENU")
    t.check(u.toast and u.toast["head"] == "< 18 MHz" and "MENU 0 3 3" in u.toast["body"], "10.5 MHz", u.toast)
    tune(t)
    u = t.keys("7 0 0 MENU")
    t.check(u.toast and u.toast["head"] == "Can't tune" and "756-1300" in u.toast["body"], "7 0 0: refused", u.toast)
    t.check(" 700.0" not in t.slot("A"), "7 0 0: not tuned there", t.slot("A"))
    # Above 1300 MHz can't be typed (decision 154): * * exists only after 100-130, so after 140 the second * is
    # ignored and the entry stays "140.".
    tune(t)
    u = t.keys("1 4 0 STAR STAR")
    t.eq((u.id, u["entry"]["args"][0] if u["entry"] else None), ("Entry", "140."), "1 4 0 * *: the second * ignored")
    t.home()
    for f, want in (("1 8", "18.00000"), ("6 2 9 STAR 9 9", "629.99000"), ("7 5 6", "756.00000")):
        t.home()
        u = t.keys(f"{f} MENU")
        t.eq((u.id, (u.cards.get("A") or {}).get("freq")), ("Home", want), f"{f}: tuned")
    tune(t)


@test("VFO-21", pages=("Home",))
def entry_range_edges(t):
    """630 and 1300 MHz, the range's own ends, tune when typed; four digits are airband (1 2 1 5 is 121.5,
    1 3 0 0 is 130.0, 1 0 5 0 is 105.0), and 1000-1300 MHz goes in only by * * on 100-130 (1 0 5 * * 0 is
    1050, above 1000 MHz receive only)."""
    for f, want in (("6 3 0", "630.00000"), ("1 3 0 STAR STAR 0", "1300.00000"), ("1 0 5 STAR STAR 0", "1050.00000"),
                    ("1 2 1 5", "121.50000"), ("1 3 0 0", "130.00000"), ("1 0 5 0", "105.00000")):
        tune(t)
        u = t.keys(f"{f} MENU")
        t.eq((u.id, (u.cards.get("A") or {}).get("freq")), ("Home", want), f"{f}: tuned")
    tune(t)


@test("VFO-21", "KEY-9", pages=("Entry", "Home"))
def four_digit_mhz_hints(t):
    """After 1 2 9 * the hint bar says "* again: 4-digit MHz"; after the second * it names the fourth digit's
    range, "0-9 1290-1299 MHz"; 1 2 9 * * 6 OK tunes 1296. Off 100-130 (146 *) the hint stays "* Point"."""
    tune(t)
    u = t.keys("1 2 9 STAR")
    t.eq((u.id, u.hint("*")), ("Entry", "again: 4-digit MHz"), "129.: * again")
    u = t.keys("STAR")
    t.eq(u.hint("0-9"), "1290-1299 MHz", "129 after * *: the fourth digit's range")
    u = t.keys("6 MENU")
    t.eq((u.id, (u.cards.get("A") or {}).get("freq")), ("Home", "1296.00000"), "1 2 9 * * 6 OK")
    tune(t)
    u = t.keys("1 4 6 STAR")
    t.eq(u.hint("*"), "Point", "146.: no * again")
    tune(t)


@test("NAV-2", "VFO-5", pages=("Home",))
def knob_steps_and_bands(t):
    """VFO: knob and ▲▼ step by the step; ◀▶ step bands and come back."""
    u = tune(t)
    u = t.knob(+2)
    t.eq(u.active["freq"], "146.53000", "knob +2 at 5 kHz")
    u = t.keys("UP")
    t.eq(u.active["freq"], "146.53500", "▲")
    u = t.knob(-3)
    t.eq(u.active["freq"], "146.52000", "back")
    h0 = u.active["head"]
    u = t.keys("RIGHT")
    t.check(u.active["head"] != h0 and float(u.active["freq"]) > 146.52, "▶: the next band up", u.active)
    u = t.keys("LEFT")
    t.eq(u.active["head"], h0, "◀: back to the band")


@test("NAV-1", "VFO-23", "KEY-23", pages=("Confirm", "Home"))
def channel_mode_and_copy_to_vfo(t):
    """V/M: channel mode (header the channel number, hint "Zone"); knob steps channels; hold 9 on a channel asks
    "Copy to VFO?" (BACK cancels), OK copies it to the VFO ("Copied to VFO") and V/M goes back to the channel
    untouched."""
    tune(t)
    u = t.keys("VM")
    a = u.active
    t.check(a["head"][:3].isdigit(), "V/M: a channel", a["head"])
    t.eq(u.hint("<>"), "Zone", "◀▶ step zones in channel mode")
    ch0 = a["head"]
    u = t.knob(+1)
    t.check(u.active["head"] != ch0 and u.active["head"][:3].isdigit(), "knob: the next channel", u.active["head"])
    u = t.knob(-1)
    t.eq(u.active["head"], ch0, "knob back")
    name, freq, mode = u.active["name"], u.active["freq"], u.active["mode"]
    u = t.press("9", long=True)
    t.eq((u.id, u.hint("OK")), ("Confirm", "Copy"), "hold 9 on a channel asks")
    t.check('ask "Copy to VFO?"' in u.text(), "the question", u.text())
    u = t.keys("BACK")
    t.eq((u.id, u.active["head"]), ("Home", ch0), "BACK: still on the channel")
    u = t.press("9", long=True)
    u = t.keys("MENU")
    t.check(u.toast and u.toast["head"] == "Copied to VFO", "the toast", u.toast)
    t.check(u.active["head"].startswith("VFO"), "the slot is on its VFO", u.active["head"])
    t.eq((u.active["freq"], u.active["mode"]), (freq, mode), "with the channel's frequency and mode")
    u = t.keys("VM")
    t.eq((u.active["head"], u.active["name"]), (ch0, name), "V/M: back to the channel, untouched")
    t.keys("VM")


@test("VFO-9", "KEY-23", pages=("Home", "Save"))
def hold_9_saves_a_vfo(t):
    """Hold 9 on a VFO opens the save screen (BACK cancels, nothing saved).
    (On 18-64 MHz it saves an HF channel, CHAN-22/CHAN-26: not checked here.)"""
    tune(t)
    n0 = t.cmd("dump").count("\nch ")
    u = t.press("9", long=True)
    t.eq(u.id, "Save", "hold 9 on a VFO")
    u = t.keys("BACK")
    t.eq(u.id, "Home", "BACK cancels")
    t.eq(t.cmd("dump").count("\nch "), n0, "nothing saved")


@test("KEY-23", pages=("Home", "Zones"))
def hold_vm_opens_zones(t):
    """Hold V/M opens the zone list in VFO mode and in channel mode alike; the slot stays where it was."""
    tune(t)
    u = t.press("VM", long=True)
    t.eq(u.id, "Zones", "hold V/M on a VFO")
    u = t.home()
    t.check(u.active["head"].startswith("VFO"), "still on the VFO", u.active["head"])
    u = t.keys("VM")
    ch = u.active["head"]
    t.check(ch[:3].isdigit(), "V/M: a channel", ch)
    u = t.press("VM", long=True)
    t.eq(u.id, "Zones", "hold V/M on a channel")
    u = t.home()
    t.eq(u.active["head"], ch, "still on the channel")
    tune(t)


@test("TX-36", "HF-30", pages=("Home",))
def hf_starts_on_low(t):
    """Tuning onto an HF frequency (28.4 MHz) puts the power at Low, whatever it was."""
    t.cmd("slot A vfo 146.52 high")
    t.eq(t.ui().active["power"], "HIGH", "High on 2 m")
    u = t.keys("2 8 STAR 4 MENU")
    t.eq(u.active["freq"], "28.40000", "28.4 MHz")
    # The card says "No TX" where this region can't transmit (HOME-4.2); the level itself shows in quick settings.
    t.check(u.active["power"] in ("LOW", "No TX"), "28.4 MHz: the card", u.active["power"])
    t.eq(t.keys("MENU").row("TX power")["value"], "Low", "28.4 MHz: Low")
    t.home()
    tune(t)


@test("TX-9", "LINK-33", pages=("Quick",))
def power_levels_on_125m(t):
    """1.25 m: the setting is kept like any band's (quick settings' TX power isn't greyed, and `slot A high`, as the
    link's SLOT, goes in), but the card shows LOW: 1.25 m transmits at Low only (TX-9, decision 260); or No TX
    where the region has no 1.25 m (HOME-4.2)."""
    t.cmd("slot A vfo 223.5 low")
    u = t.keys("MENU")
    p = u.row("TX power")
    t.check(not p["grey"], "TX power not greyed", p)
    t.home()
    ok, why = t.cmd_err("slot A high")
    t.check(ok, "`slot A high` goes in", why)
    u = t.home()
    t.check(u.active["power"] in ("LOW", "No TX"), "the card: LOW (TX-9), or No TX outside the region's bands",
            u.active["power"])
    tune(t)


@test("TX-37", "LINK-33", "TERM-35", pages=("Home",))
def split_and_tune_refusals(t):
    """`slot` refuses a split across 64 MHz and a frequency out of range with the reason, changing nothing;
    a good change shows on the card at once."""
    before = t.slot("A")
    ok, why = t.cmd_err("slot A split 28.4")
    t.check(not ok and why, "split 146.52 -> 28.4 refused", why)
    ok, why = t.cmd_err("slot A 700")
    t.check(not ok and why, "700 MHz refused", why)
    t.eq(t.slot("A"), before, "nothing changed")
    t.cmd("slot A 146.94")
    t.eq(t.ui().cards["A"]["freq"], "146.94000", "a good change shows on the card")
    tune(t)


@test("EDIT-1", "EDIT-3", "EDIT-6", pages=("Edit", "Home"))
def hold_slot_edits_in_place(t):
    """Hold SLOT: the edit screen, status bar "EDIT A", hint "<^v> Pick, Knob Change"; BACK ends it."""
    u = t.press("SLOT", long=True)
    t.eq((u.id, u.title), ("Edit", "EDIT A"), "hold SLOT")
    # "* Undo" takes Pick's place for 5 s after a change (the runner's restore counts as one).
    t.check(("Knob", "Change") in u.hints and (("<^v>", "Pick") in u.hints or ("*", "Undo") in u.hints),
            "the hint bar: Pick, Knob Change", u.hints)
    t.eq(u.hints[-1], ("BACK", "Done"), "BACK Done")
    u = t.keys("BACK")
    t.eq(u.id, "Home", "BACK ends it")
    u = t.press("SLOT", long=True)
    u = t.keys("SLOT")
    t.eq(u.id, "Home", "SLOT ends it")


@test("EDIT-6", slow=True, pages=("Edit", "Home"))
def edit_ends_after_idle(t):
    """The slot editor ends by itself after 8 s idle."""
    u = t.press("SLOT", long=True)
    t.eq(u.id, "Edit", "hold SLOT")
    t.wait(lambda u: u.id == "Home", timeout=11, what="home after 8 s idle", every=0.5)


@test("CHAN-22", "TERM-35")
def channel_edges_over_the_terminal(t):
    """`ch` (the mini-CPS) refuses 700 MHz and 0.1 MHz with a reason, takes 28.4 MHz (an HF channel) and
    64.5 MHz; the channel then reads back; delete removes it."""
    n = 990
    ok, out = t.cmd_err(f"ch {n}")
    t.need(not ok and "no such channel" in out, f"channel {n} isn't empty: {out.strip()}")
    try:
        for f in ("700", "0.1"):
            ok, why = t.cmd_err(f'ch {n} "UITEST" {f}')
            t.check(not ok and why, f"{f} MHz refused", why)
        for f in ("28.4 mode=am", "64.5", "146.52"):
            ok, why = t.cmd_err(f'ch {n} "UITEST" {f}')
            t.check(ok, f"{f} MHz accepted", why)
            t.has(t.cmd(f"ch {n}"), "UITEST", f"ch {n} reads back")
    finally:
        t.cmd_err(f"ch {n} delete")
    ok, out = t.cmd_err(f"ch {n}")
    t.check(not ok and "no such channel" in out, "deleted", out)


# VFO-2's band table as the UI shows it (slot.rs BANDS): a frequency in each band, its name and default mode
# (wide FM in the Americas, AU and NZ; NFM elsewhere; AM on HF, NAM on the airband, VFO-4.2).
BAND_CASES = [("21.2", "HF", "AM"), ("28.4", "HF", "AM"), ("51.0", "6 m", "FM"), ("121.5", "Airband", "NAM"),
              ("146.52", "VHF", "FM"), ("156.8", "Marine", "FM"), ("223.5", "1.25 m", "FM"), ("446.0", "UHF", "FM")]


@test("VFO-2", "VFO-4", "VFO-4.2", "SETUP-7", pages=("Home", "Quick"))
def band_follows_the_frequency(t):
    """The band comes from the frequency by a fixed table: tuning A into each band gives its name in the card's
    header and its default mode (FM, NFM, AM or NAM), and the power chip its level; the same frequency always gives
    the same band."""
    region = t.get("0 9 4")
    wide = region in ("USA", "Canada", "Americas", "Australia", "New Zealand", "US", "AU", "NZ")
    for f, band, mode in BAND_CASES:
        t.cmd(f"slot A vfo {f}")
        u = t.ui()
        a = u.active
        t.eq(a["head"], f"VFO · {band}", f"{f} MHz: the band")
        want = mode if mode in ("AM", "NAM") or wide else "NFM"
        t.eq(a["mode"], want, f"{f} MHz: the band's mode ({region})")
        want = "LOW"
        t.check(a["power"] in (want, "No TX"), f"{f} MHz: the power chip ({want}, or No TX where it can't transmit)",
                a["power"])
    t.note(f"region {region}")
    # The mode list on a home VFO: FM, NFM and AM only (USB, LSB, CW are HF mode's).
    t.cmd("slot A vfo 146.52 fm")
    t.home()
    u = t.keys("MENU 3")
    t.eq(u.sel and u.sel["name"], "Mode", "MENU 3")
    seen = []
    for _ in range(6):
        seen.append(u.sel["value"])
        u = t.knob(+1)
    t.eq(sorted(set(seen)), ["AM", "FM", "NAM", "NFM", "WFM"], f"the knob goes round FM, NFM, AM, NAM and WFM (BCR-33, VFO-4.2) ({seen})")
    tune(t)
