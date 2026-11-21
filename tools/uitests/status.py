"""The status bar and the Status page (chapter 8, SBAR-*), About and its help pages (HELP-*)."""
import re

from .core import test

BAR_KEYS = {"lock", "scan", "watch", "inbox", "alert", "gps", "bt_on", "bt", "battery", "low", "cutoff", "dry", "lpm", "clock", "utc", "call", "mcu", "switches", "held_us", "why", "vox"}  # dry: SBAR-22; bt_on: SBAR-8; lpm: SBAR-24; vox: SBAR-25; mcu ... why: PWR-26.6
CPU_KEYS = {"cpu", "cpu24", "cpu120", "at24"}  # SELF-9.1, SELF-9.3: once measured, and each speed only if it stayed there


@test("SBAR-14", "PWR-8", "SBAR-10", pages=("Status",))
def status_page_lists_states(t):
    """The Status page lists the states in words and follows them (Battery Save, GPS, VOX, Key lock); Battery
    Save shows there and never in the status bar."""
    u = t.keys("MENU 0 0")
    for name in ("Transmit", "Region", "Watch", "Scan", "Battery Save", "GPS", "Bluetooth", "Battery", "Key lock",
                 "VOX", "Weather alerts"):
        t.check(u.row(name) is not None, f"a {name} row", u.names())
    t.eq(u.row("Battery Save")["value"], t.get("0 7 7 1"), "Battery Save follows the setting")
    t.check(u.row("Transmit")["value"], "the transmit state in words", u.row("Transmit"))
    t.check(u.row("Battery")["value"].endswith("%"), "the battery as a percentage", u.row("Battery"))
    t.set("0 7 7 1", "Off")
    u = t.ui()
    t.eq(u.row("Battery Save")["value"], "Off", "Battery Save off: the row follows")
    t.set("0 7 7 1", "Level 2")
    u = t.ui()
    t.eq(u.row("Battery Save")["value"], "Level 2", "Level 2: the row follows")
    t.eq(set(u.bar) - {"args"} - CPU_KEYS, BAR_KEYS, "the status bar carries nothing for it")
    if t.sim:
        t.note("GPS off: the simulator's GPS ignores 0 6 1 (radio only)")
        return
    t.set("0 6 1", "Off")
    u = t.ui(0.5)
    t.check(u.row("GPS")["value"].startswith("Off"), "GPS off: the row", u.row("GPS"))
    t.eq(u.bar["gps"], "off", "GPS off: no GPS icon")


@test("SBAR-15", "SBAR-13", pages=("Gps", "Group", "Quick", "Status"))
def status_rows_open_their_setting(t):
    """OK on a Status row opens that row's setting: Battery Save, VOX, GPS; BACK returns to the row."""
    for name, want in (("Battery Save", "Battery Save"), ("GPS", "GPS"), ("VOX", "VOX")):
        t.home()
        u = t.keys("MENU 0 0")
        i = u.names().index(name)
        u = t.keys(" ".join(["DOWN"] * i) + " MENU" if i else "MENU")
        t.check(u.id in ("Group", "Quick", "Value", "Gps"), f"{name}: a setting opens", u.id)
        if u.id != "Gps":  # the GPS row opens the GPS page (on/off, fix, position)
            t.eq(u.sel["name"] if u.sel else (u["value"] or {}).get("args", [None])[0], want, f"{name}: on its setting")
        u = t.keys("BACK")
        t.eq((u.id, u.sel and u.sel["name"]), ("Status", name), f"{name}: BACK to the row")


@test("SBAR-11", "SBAR-4", pages=("Home", "Scan"))
def bar_shows_lock_and_scan(t):
    """States that change what a key does are in the bar: the padlock while locked, SCN while the active slot
    scans; each appears only when it applies."""
    u = t.ui()
    t.eq((u.bar["lock"], u.bar["scan"]), (0, 0), "nothing at rest")
    t.keys("SIDE2")
    u = t.ui(0.3)
    t.eq(u.bar["scan"], 1, "SIDE2 scan: SCN")
    t.keys("SIDE2")
    t.home()
    t.eq(t.ui(0.3).bar["scan"], 0, "scan stopped: no SCN")
    u = t.press("HASH", long=True)
    t.eq(u.bar["lock"], 1, "locked: the padlock")
    u = t.press("HASH", long=True)
    t.eq(u.bar["lock"], 0, "unlocked")


@test("SBAR-19", "SBAR-9", pages=("Group", "Home", "Info", "Status"))
def battery_icon_status_about(t):
    """The battery: an icon in the bar (always), the percentage on Status, the volts on System (SELF-9); no "Battery as"
    anywhere in Display."""
    u = t.ui()
    t.check(isinstance(u.bar.get("battery"), int), "the bar always has the battery", u.bar)
    u = t.keys("MENU 0 0")
    t.check(u.row("Battery")["value"].endswith("%"), "Status: %", u.row("Battery"))
    t.home()
    u = t.keys("MENU 0 9 8 RIGHT")
    t.check(u.row("Battery")["value"].endswith("V"), "System: volts", u.row("Battery"))
    for path in ("MENU 0 7", "MENU 0 7 3"):
        t.home()
        u = t.keys(path)
        t.check(not any("Battery as" in n for n in u.names()), f"{path}: no Battery as", u.names())


@test("HELP-1", "SBAR-5", "SELF-9", pages=("Group", "Info"))
def about_pages_go_round(t):
    """◀▶ on About go round its pages: About, System, Manual, Keys, Side+PTT, Holds, Menu nos, Self-test; the hint
    bar names the page ▶ goes to; every name fits 8 characters; BACK leaves."""
    u = t.keys("MENU 0 9 8")
    pages = [u]
    for _ in range(8):
        pages.append(t.keys("RIGHT"))
    titles = [p.title for p in pages]
    t.eq(titles, ["About", "System", "Manual", "Keys", "Side+PTT", "Holds", "Menu nos", "Checks", "About"], "the pages")
    t.check(all(len(x) <= 8 for x in titles), "names fit 8 characters", titles)
    odd = []
    for cur, nxt in zip(pages, pages[1:]):
        label = cur.hint("<>")
        if label is None and cur.hint("#"):
            continue  # a help page's hint names #'s next help page (HOME-10)
        if label not in (nxt.name, nxt.title):
            odd.append(f"{cur.title}: hint {label!r}, next {nxt.name!r}")
    t.eq(odd, [], "each hint names the next page")
    u = t.keys("LEFT")
    t.eq(u.title, "Checks", "◀ goes back")
    u = t.keys("BACK")
    t.eq((u.id, u.sel and u.sel["name"]), ("Group", "About"), "BACK leaves to System, on About")


@test("BOOT-11", "SELF-5", "SELF-8", "SELF-9", pages=("Info",))
def about_rows(t):
    """About: version, MCU, the unique ID (one row, base32), SPI flash, calibration, the transmit state, self-test, the
    SI4732's revisions, SSB patch (installed or the path to install it); System (▶): CPU, RAM, stack free, MCU temp,
    battery, uptime."""
    u = t.keys("MENU 0 9 8")
    want = ["Version", "MCU", "ID", "SPI flash", "Calibration", "Transmit", "Self-test", "SI4732", "SSB patch"]
    t.eq(u.names(), want, "the About rows")
    t.check(re.fullmatch(r"(?:[0-9A-HJKMNP-TV-Z]{5} ){3}[0-9A-HJKMNP-TV-Z]{5}|Not read", u.row("ID")["value"] or ""),
            "the unique ID, base32 on one row", u.row("ID"))
    t.check(u.row("Version")["value"].startswith("v"), "the version", u.row("Version"))
    t.check(u.row("Transmit")["value"], "the transmit state", u.row("Transmit"))
    t.check(u.row("SSB patch")["value"], "the SSB patch state", u.row("SSB patch"))
    t.check(re.fullmatch(r"FW \w\.\w, rev \w, cmp \w\.\w|Not read", u.row("SI4732")["value"] or ""), "the SI4732's revisions",
            u.row("SI4732"))
    t.note(f"About: {[(r['name'], r['value']) for r in u.rows]}")
    u = t.keys("RIGHT")
    t.eq(u.names(), ["CPU", "RAM", "Stack free", "MCU temp", "Battery", "Uptime", "Clock"], "the System rows")
    t.note(f"System: {[(r['name'], r['value']) for r in u.rows]}")


@test("SBAR-7", pages=("Status",))
def gps_icon_follows_the_fix(t):
    """The GPS icon shows only with GPS on: outline (gps=searching) while it searches, solid (gps=fix) with a
    fix, none (gps=off) with GPS off; the Status page's GPS row says the same."""
    u = t.keys("MENU 0 0")
    row = u.row("GPS")["value"]
    gps = u.bar["gps"]
    want = "off" if row.startswith("Off") else "fix" if row.startswith("Fix") else "searching"
    t.eq(gps, want, f"the bar's icon for GPS {row!r}")
    t.eq(t.get("0 6 1") == "Off", gps == "off", "the icon is off exactly when GPS is off")
    if t.sim:
        t.note("GPS off and on: the simulator's GPS ignores 0 6 1 (radio only)")
        return
    t.set("0 6 1", "Off")
    u = t.ui(0.8)
    t.eq((u.bar["gps"], u.row("GPS")["value"]), ("off", "Off"), "GPS off: no icon")
    t.set("0 6 1", "On")
    u = t.wait(lambda u: u.bar["gps"] != "off", timeout=5, what="GPS on: the icon back")
    t.check(u.bar["gps"] in ("searching", "fix"), "outline or solid", u.bar)


@test("BOOT-8", "SELF-2", "BACKUP-4", "TX-15", pages=("Info", "Status"))
def transmit_lock_follows_the_checks(t):
    """Transmit is Ready only when the MCU and calibration checks pass (Self-test page), and About and Status say
    the same; the calibration page's CRC at install matches while it reads Valid, and a lock names its reason."""
    u = t.keys("MENU 0 9 8")
    tx = u.row("Transmit")["value"]
    cal = u.row("Calibration")["value"]
    u = t.keys("LEFT")
    t.eq(u.title, "Checks", "◀ from About: the self-test page")
    checks = {r["name"]: r["value"] for r in u.rows}
    t.check({"MCU", "Calibration"} <= set(checks), "the MCU and calibration checks", checks)
    passed = checks["MCU"] == "OK" and checks["Calibration"] == "OK"
    if tx == "Ready":
        t.check(passed, "Ready: the MCU and calibration checks passed", checks)
    else:
        t.check(len(tx) > 5 and tx != "Locked", "locked: the reason in words", tx)
    if not passed:
        t.check(tx != "Ready", "a failed MCU or calibration check locks transmit", (tx, checks))
    t.home()
    u = t.keys("MENU 0 0")
    t.eq(u.row("Transmit")["value"], tx, "Status says what About says")
    t.home()
    u = t.keys("MENU 0 9 9 2 MENU")
    t.eq(u.id, "Info", "OK on 0 9 9 2: the calibration page")
    page = {r["name"]: r["value"] for r in u.rows}
    t.eq(page.get("Status"), cal, "its status is About's")
    if cal == "Valid":
        t.eq(page.get("CRC at install"), "Matches", "valid: the CRC recorded at install matches")
    t.note(f"transmit {tx!r}, calibration {cal!r}, checks {checks}")


@test("UPD-8", "SAFE-2", "UPD-7", pages=("Group", "Info"))
def no_pending_update_on_a_clean_radio(t):
    """About on a radio with nothing at 0x300000: no "pending stock update" line; there is no update screen in
    the menus (System has no update row)."""
    u = t.keys("MENU 0 9 8")
    t.check(not any("pending" in (r["name"] + r["value"]).lower() or "update" in r["name"].lower() for r in u.rows),
            "no pending-update line", [(r["name"], r["value"]) for r in u.rows])
    t.home()
    u = t.keys("MENU 0 9")
    t.check(not any("update" in r["name"].lower() for r in u.rows), "System: no update row", u.names())


@test("GPS-28", where="radio", slow=True, pages=("Gps",))
def gps_screen_keeps_the_module_running(t):
    """With the GPS position page (0 6 2) open the module keeps running: its sentences keep coming (the terminal's
    `gps` age stays a few seconds) for two minutes; no standby while the page is open."""
    t.need(t.get("0 6 1") != "Off", "GPS is off")
    import re
    import time
    u = t.keys("MENU 0 6 2")
    t.eq(u.id, "Gps", "MENU 0 6 2")
    worst = 0
    end = time.time() + 120
    while time.time() < end:
        out = t.cmd("gps")
        m = re.search(r"(\d+) s ago", out)
        t.check(m, "the GGA sentence and its age", out)
        worst = max(worst, int(m.group(1)))
        t.check(worst <= 5, "a sentence within 5 s all along (no standby)", out)
        time.sleep(10)
    t.eq(t.ui().id, "Gps", "still on the page")
    t.note(f"oldest sentence seen: {worst} s")
