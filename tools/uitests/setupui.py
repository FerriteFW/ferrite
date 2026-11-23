"""First-run setup by remote keys (chapter 4: SETUP-*) and the factory reset (BOOT-14, SAFE-9, SAFE-14), read
back with setup's `setup` line, its rows and the summary's note.

"Run setup again" (MENU 0 9 7 1) goes through every page with OK, keeping what the radio has. The reset wipes
the settings, the PIN, channels, zones, presets, the home channel, messages, the event log and waypoints: on the
radio it runs only with --with reset, and the test writes the `dump` back after (settings, zones, presets,
channels, the home channel); messages, the log and waypoints aren't in a dump and stay gone.
"""
import datetime
import os
import time

from .core import Fail, test

REGIONS = 8


def setup_line(u):
    return u["setup"] or {}


def ok_through(t, u, region=None):
    """OK on each page from here to the summary (on the region page, OK on `region` if given, else the focused
    row); returns the summary's readback."""
    for _ in range(10):
        s = setup_line(u)
        if s.get("page") == s.get("of"):
            return u
        if s.get("head") == "Region" and region is not None:
            names = [r["name"] for r in u.rows][:REGIONS]
            t.check(region in names, f"region {region!r} on the page", names)
            cur = next((i for i, r in enumerate(u.rows) if r["sel"]), 0)
            want = names.index(region)
            if want != cur:
                u = t.keys(" ".join(["DOWN" if want > cur else "UP"] * abs(want - cur)))
        u = t.keys("MENU")
    t.fail("setup didn't reach its summary")


@test("SETUP-3", "SETUP-10", pages=("Home", "Setup"))
def setup_again_ok_through(t):
    """Run setup again (MENU 0 9 7 1): eight pages, OK on each keeps a working radio; the region page lists all
    eight regions on one screen (and Extend TX after them); the summary shows the transmit state (region,
    Extend TX, callsign), the channels and how to add them; OK there goes home with every setting as it was."""
    before = [x for x in t.cmd("dump").splitlines() if not x.startswith("#")]
    region = t.get("0 9 4")
    u = t.keys("MENU 0 9 7 1")
    t.eq(u.id, "Setup", "MENU 0 9 7 1")
    t.eq((setup_line(u)["page"], setup_line(u)["of"]), (1, 8), "page 1 of 8")
    pages = []
    for _ in range(8):
        s = setup_line(u)
        pages.append(s["head"])
        if s["head"] == "Region":
            names = [r["name"] for r in u.rows]
            t.eq(len(names), REGIONS + 1, "eight regions and Extend TX, on one page")
            t.eq(names[-1], "Extend TX", "Extend TX after the regions")
            chosen = [r["name"] for r in u.rows if r["value"] == "chosen"]
            t.eq(chosen, [region], "the radio's region is the one chosen")
        if s["page"] == s["of"]:
            break
        u = t.keys("MENU")
    t.eq(pages, ["Welcome to Ferrite", "Your callsign", "Region", "Owner card", "Time", "Look", "Battery Save",
                 "All set"], "the eight pages")
    rows = {r["name"]: r["value"] for r in u.rows}
    t.eq(list(rows), ["Callsign", "Region", "Extend TX", "Time zone", "Home", "Battery Save", "Channels"],
         "the summary's rows")
    t.eq(rows["Region"], region, "the region")
    note = " ".join((u["help"] or {}).get("args", [])[1:])
    t.check("Add channels" in note and "MENU 0 9 7 1" in note, "how to add channels, and how to run it again", note)
    if rows["Callsign"] == "none":
        t.check("No callsign" in note, "no callsign: automatic transmissions stay off", note)
    u = t.keys("MENU")
    t.eq(u.id, "Home", "OK on the summary: home")
    after = [x for x in t.cmd("dump").splitlines() if not x.startswith("#")]
    t.eq(after, before, "every setting as it was")


def about_rows(t):
    t.home()
    u = t.keys("MENU 0 9 8")
    a = {r["name"]: r["value"] for r in u.rows}
    t.home()
    u = t.keys("MENU 0 9 9 2 MENU")
    a.update({"cal " + r["name"]: r["value"] for r in u.rows})
    t.home()
    u = t.keys("MENU 0 9 6 MENU")
    a.update({"ssb " + r["name"]: r["value"] for r in u.rows})
    t.home()
    return a


def dump_lines(d):
    """A dump's command lines as the link sends them (comments, blank lines and `end` left out; a `set` line's
    trailing `# name` dropped)."""
    out = []
    for line in d.splitlines():
        if not line.strip() or line.startswith("#") or line.strip() == "end":
            continue
        out.append(line.split("#")[0].strip() if line.startswith("set ") else line)
    return out


def save_dump(t, d0):
    """The dump before the reset written to the run's results directory (--session, else --results/saved-dumps):
    the raw dump (the terminal's :load takes it pasted) and its command lines (one per line, for ferrite_link.py
    term). Returns (dump path, lines path); both printed."""
    root = getattr(t.opts, "session", None) or getattr(t.opts, "results", None) or "."
    d = os.path.join(root, "saved-dumps")
    os.makedirs(d, exist_ok=True)
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    path = os.path.abspath(os.path.join(d, f"{stamp}_before-reset.dump"))
    lines = path[:-len(".dump")] + "_lines.txt"
    with open(path, "w") as f:
        f.write(d0)
    with open(lines, "w") as f:
        f.write("\n".join(dump_lines(d0)) + "\n")
    t.log(f"    the dump before the reset saved: {path}")
    t.note(f"the dump before the reset saved: {path}")
    return path, lines


def how_to_load(path, lines):
    return (f"the dump before the reset is saved in {path}: load it with "
            f"`tr '\\n' '\\0' < {lines} | xargs -0 python3 tools/ferrite_link.py PORT term` (line by line), or "
            f"the terminal's :load with {path} pasted")


def write_back(t, d0, tries=3):
    """The dump's lines sent again (each up to `tries` times: a time-out resyncs and sends it again; set, ch, zone
    and preset lines are the same twice); returns the lines not taken, with why."""
    from ferrite_link import RadioError
    bad = []
    for line in dump_lines(d0):
        for i in range(tries):
            try:
                t.r.cmd(line)
                break
            except RadioError as e:
                bad.append(f"{line}: {e}")
                break
            except (TimeoutError, OSError) as e:
                if i == tries - 1:
                    bad.append(f"{line}: {e}")
                    break
                t.resync(what=f"the radio again after no answer to {line!r}")
    return bad


@test("BOOT-14.4", "BOOT-14.5", "SAFE-9.1", "SAFE-14.5", "SETUP-3", "PIN-8", needs=("reset",), pages=(
    "Confirm", "Group", "Home", "Info", "Setup"))
def reset_then_setup(t):
    """The reset (MENU 0 9 7 2: OK, hold # 3 s) by remote keys: setup's first page follows, with no region until
    setup picks one (none chosen on the region page); VOX, KISS and the digipeater read off; calibration (Valid,
    the CRC at install matching) and the SSB patch (its state) as before. Setup is then run with the radio's
    region and the dump written back; the dump after matches the one before. The dump is saved to a file under
    the results directory before the reset, and written back whatever happens after it."""
    t.need(t.get("0 9 3 1") == "Off", "a PIN lock is on")
    d0 = t.cmd("dump")
    path, lines = save_dump(t, d0)
    region = t.get("0 9 4")
    a0 = about_rows(t)
    t.keys("MENU 0 9 7 2")
    u = t.ui()
    t.eq(u.id, "Confirm", "MENU 0 9 7 2: the reset's question")
    reset = False
    done = False
    try:
        t.keys("MENU")
        reset = True
        t.r.down("HASH")
        try:
            time.sleep(3.4)
        finally:
            try:
                t.r.up("HASH")
            except (TimeoutError, OSError):
                pass  # the radio restarting: the key went with it
        t.resync(timeout=40, what="the radio after the reset")
        u = t.wait(lambda u: u.id == "Setup", timeout=15, what="setup after the reset")
        t.eq(setup_line(u)["page"], 1, "setup's first page (BOOT-14.5)")
        t.check(t.get("0 9 4") in ("None", "Not set", "-", ""), "no region until setup picks one", t.get("0 9 4"))
        for p, name in (("0 8 2 1", "VOX"), ("0 5 3", "APRS mode and KISS"), ("0 5 7 1", "digipeater")):
            t.eq(t.get(p), "Off", f"{name} off after the reset (SAFE-14.5)")
        u = t.keys("MENU MENU")
        t.eq(setup_line(u)["head"], "Region", "the region page")
        t.eq([r["name"] for r in u.rows if r["value"] == "chosen"], [], "none chosen until OK on one (SETUP-3)")
        u = ok_through(t, u, region=region)
        # The summary's Region row, or the setting itself when ok_through stopped short of the summary (Oct 15, 2026:
        # KeyError 'Region' on the radio after the region page).
        got = {r["name"]: r["value"] for r in u.rows}.get("Region") or t.get("0 9 4")
        t.eq(got, region, "the region picked")
        t.keys("MENU")
        t.home()
        a1 = about_rows(t)
        for k in ("Calibration", "cal Status", "cal CRC at install", "SSB patch", "ssb State"):
            if k in a0:
                t.eq(a1.get(k), a0[k], f"{k} as before the reset (BOOT-14.4, SAFE-9.1)")
        done = True
    finally:
        if reset:
            restore(t, d0, path, lines, region, done)
    # What the reset seeded that the dump before didn't have (load merges: it deletes nothing).
    keep = {tuple(x.split()[:2]) for x in d0.splitlines() if x.split()[:1] in (["ch"], ["preset"], ["zone"])}
    extra = [tuple(x.split()[:2]) for x in t.cmd("dump").splitlines()
             if x.split()[:1] in (["ch"], ["preset"], ["zone"]) and tuple(x.split()[:2]) not in keep]
    for kind, n in extra:
        if kind == "preset":
            continue
        ok, why = t.cmd_err(f"{kind} {n} delete")
        t.check(ok, f"{kind} {n} (seeded by the reset) deleted", why)
    # Range presets have no terminal delete: by keys on the list (MENU 0 1 4), * then OK, the last first.
    presets = [int(x.split()[1]) for x in t.cmd("dump").splitlines() if x.startswith("preset ")]
    for kind, n in sorted((e for e in extra if e[0] == "preset"), key=lambda e: -int(e[1])):
        t.home()
        t.keys("MENU 0 1 4")
        i = presets.index(int(n))
        u = t.keys(" ".join(["DOWN"] * i + ["STAR"]))
        t.eq(u.id, "Confirm", f"* on preset {n}: delete it?")
        t.keys("MENU")
        presets.remove(int(n))
        t.check(f"preset {n} " not in t.cmd("dump"), f"preset {n} (seeded by the reset) deleted", n)
    t.put_back([])
    d1 = t.cmd("dump")
    t.eq([x for x in d1.splitlines() if not x.startswith("#")], [x for x in d0.splitlines() if not x.startswith("#")],
         "the dump after is the one before")


def restore(t, d0, path, lines, region, done):
    """After the reset, whatever happened: the radio answering again, setup finished if it still shows (with the
    region from before), then the dump written back and checked line by line. Anything left undone fails the test
    with where the saved dump is and how to load it."""
    try:
        u = t.resync(timeout=40, what="the radio after the reset")
        if not done:
            for _ in range(3):
                if u.id != "Setup":
                    break
                u = ok_through(t, u, region=region)
                u = t.keys("MENU")
            t.home()
        bad = write_back(t, d0)
        if bad:
            t.fail(f"the dump written back: {len(bad)} line(s) not taken ({'; '.join(bad[:5])}); " + how_to_load(path, lines))
        after = set(t.cmd("dump").splitlines())
        missing = [x for x in d0.splitlines() if not x.startswith("#") and x.strip() and x not in after]
        if missing:
            t.fail(f"after the write-back the dump lacks {len(missing)} line(s) ({'; '.join(missing[:5])}); "
                   + how_to_load(path, lines))
        if done:
            t.check(True, "the dump written back, every line taken", path)
        else:
            t.note(f"the test stopped after the reset; the dump was written back ({path})")
    except Exception as e:
        if how_to_load(path, lines) in str(e):
            raise
        raise Fail(f"{e}; {how_to_load(path, lines)}") from e
