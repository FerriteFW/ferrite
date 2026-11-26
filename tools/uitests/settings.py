"""Every setting in the settings table, by its menu number (Appendix A; MENU-7, MENU-12, MENU-15, MENU-16, MENU-21)."""
import re

from .core import UNSAFE, test

def ascii_(s):
    """The terminal sends ASCII (° comes as ?): compare without the rest."""
    return re.sub(r"[^\x20-\x7e]|\?", "", s or "")


def unbuilt_group(t, path):
    """Whether path's third-level group isn't built yet: OK on its row says "setting comes next". Leaves the
    screen on the opened group otherwise."""
    p = path.split()
    t.home()
    u = t.keys(f"MENU {' '.join(p[:3])} MENU", settle=0.15)
    return bool(u.toast and "comes next" in u.toast["body"])


# Settings that act as they change and leave the list: 0 7 7 2 enters Low Power Mode and goes to its home
# (PWR-24; lowpower.py tests it).
ACTS = {"0 7 7 2"}
# Settings typed or picked on a page of their own (OK opens them): the knob doesn't step them in the group.
TYPED = {"0 4 2 3", "0 5 9 1", "0 5 9 2", "0 5 4", "0 5 7 2", "0 4 2 4", "0 5 5 3"}


@test("MENU-31", "TERM-12", pages=("Entry", "Group"))
def frequency_setting_typed(t):
    """MENU 0 5 4 opens the frequency entry box; 1 4 5 4 9 5 types 145.495 and keeps it (back on the list);
    446 is refused with its digits left; the terminal's `set 0 5 4` takes MHz too."""
    old = t.get("0 5 4")
    t.home()
    u = t.keys("MENU 0 5 4")
    t.eq(u.id, "Entry", "the frequency entry box")
    u = t.keys("1 4 5 4 9 5", settle=0.3)
    if u.id == "Entry":
        u = t.keys("MENU", settle=0.3)
    t.eq(u.id, "Group", "kept: back on the list")
    t.eq(t.get("0 5 4"), "145.495", "the setting has it")
    t.home()
    u = t.keys("MENU 0 5 4 4 4 6 MENU", settle=0.3)
    t.eq(u.id, "Entry", "446 refused: still typing")
    t.eq(t.get("0 5 4"), "145.495", "unchanged")
    t.home()
    t.set("0 5 4", old)
    t.eq(t.get("0 5 4"), old, "set puts it back")


GROUPS = "13456789"  # the settings table's top-level groups (MENU 0 G); one test each, so -j runs them side by side


def walk(t, paths):
    """Each setting in paths (from the table, `dump`): MENU 0 and its number lands on its row (name as `get`,
    value, help with the path); the knob changes it at once (`get` has it), and back; # leaves defaults alone."""
    changed = unchanged = 0
    odd, via, misrouted, unbuilt = [], [], [], []
    for path in paths:
        name, value, nd = t.get_full(path)
        t.home()
        u = t.keys("MENU " + path, settle=0.15)
        if u.id == "Group" and (u["help"] or {}).get("args", [""])[0] != "MENU " + path and len(path.split()) == 4:
            # The typed path went elsewhere (a row with a value and a sub-group: its digit picks it, OK opens it).
            p = path.split()
            if unbuilt_group(t, path):
                # Its group isn't built yet: the row says "setting comes next" (the value is in the record).
                unbuilt.append(f"{path} {name}")
                continue
            misrouted.append(f"{path} -> {(u['help'] or {}).get('args', ['?'])[0]}")
            u = t.keys(p[3], settle=0.15)  # unbuilt_group left it on the opened group
        if path in TYPED and u.id in ("Text", "Entry", "Compose"):  # 0 5 5 3: the comment in the compose editor (APRS-50)
            via.append(f"{path} {name} (typed)")
            continue
        if u.id != "Group":
            # A tool's own settings (0 2 6 x, FM broadcast): its number opens the tool; hold MENU there has them.
            # 0 5 9 9 opens auto reply's own page (APRS-57).
            t.check(u.id in ("Fm", "Scope", "Hf", "AutoReply"), f"{path} {name}: a group or its tool's screen", u.id)
            via.append(f"{path} {name} ({u.id})")
            continue
        t.eq(u.sel and u.sel["name"], name, f"{path}: the row is {name}")
        t.eq(u["help"]["args"][0], "MENU " + path, f"{path}: the help names its path")
        t.eq(ascii_(u.sel["value"]), ascii_(value), f"{path} {name}: the row's value")
        t.eq(u.sel["dot"], nd, f"{path} {name}: the dot")
        if path in TYPED:
            continue
        u = t.knob(+1)
        v1 = u.sel["value"]
        if v1 == value:
            u = t.knob(-1)
            v1 = u.sel["value"]
            if v1 == value:
                unchanged += 1
                odd.append(f"{path} {name}")
                continue
            back = +1
        else:
            back = -1
        t.eq(ascii_(t.get(path)), ascii_(v1), f"{path} {name}: the change is in the settings at once")
        u = t.knob(back)
        t.eq(ascii_(u.sel["value"]), ascii_(value), f"{path} {name}: the knob back")
        t.eq(ascii_(t.get(path)), ascii_(value), f"{path} {name}: back in the settings")
        changed += 1
    t.note(f"{len(paths)} settings reached; {changed} changed by the knob and back"
           + (f"; the knob didn't step {', '.join(odd)}" if odd else "")
           + (f"; reached on their tool's screen: {', '.join(via)}" if via else "")
           + (f"; typed paths landing elsewhere (reached with OK): {', '.join(misrouted)}" if misrouted else "")
           + (f"; in groups not built yet (\"setting comes next\"): {', '.join(unbuilt)}" if unbuilt else ""))
    t.check(not misrouted, "every typed path lands on its row", misrouted)


def settings_group_test(g):
    def fn(t):
        paths = [p for p in t.state.paths if p not in UNSAFE and p not in ACTS and p.startswith(f"0 {g} ")]
        t.check(paths, f"settings in MENU 0 {g}", len(paths))
        walk(t, paths)
    fn.__name__ = f"settings_0_{g}_reachable_and_live"
    fn.__doc__ = f"MENU 0 {g}'s settings: each reached by its number and changed live by the knob, then back (walk())."
    return test("MENU-5", "MENU-7", "MENU-12", "MENU-15", "MENU-16", "MENU-21", pages=("Fm", "Group", "Text"))(fn)


for _g in GROUPS:
    settings_group_test(_g)


@test("MENU-5", "MENU-7", pages=("Group",))
def settings_in_other_groups_reachable_and_live(t):
    """Settings outside GROUPS (a group added later): walked the same way, so none goes untested; and the table
    as a whole still has its settings."""
    t.check(len([p for p in t.state.paths if p not in UNSAFE]) >= 40, "the settings table", len(t.state.paths))
    walk(t, [p for p in t.state.paths if p not in UNSAFE and p.split()[1] not in GROUPS])


@test("TX-2", "SETUP-8", pages=("ExtendTx", "Group", "Status"))
def extend_tx_warns_first_and_shows_on_status(t):
    """Extend TX is off by default; turning it on (0 8 8) shows its warning first, where BACK keeps it off; its
    state is on the Status page, never in the status bar. The test never turns it on: OK on the warning would mark
    it seen for good (a flag no `set` puts back), so only the warning and BACK are checked."""
    name, v0, nd = t.get_full("0 8 8 4")
    t.eq(name, "Extend TX", "0 8 8 4")
    t.need(v0 == "Off", f"Extend TX is {v0} on this radio")
    t.check(not nd, "off is the default", t.cmd("get 0 8 8 4"))
    u = t.keys("MENU 0 0")
    t.eq(u.row("Extend TX")["value"], "Off", "Status shows it")
    bar_keys = set(u.bar)
    t.home()
    u = t.keys("MENU 0 8 8 4")
    t.eq(u.sel and u.sel["name"], "Extend TX", "MENU 0 8 8 4")
    u = t.knob(+1)
    if u.id != "ExtendTx":
        # Seen before on this radio (the flag persists): the knob turned it on; put it back off.
        t.eq(u.sel and u.sel["value"], "On", "warned before: the knob turns it on")
        u = t.knob(-1)
        t.eq(t.get("0 8 8 4"), "Off", "and off again")
        t.need(False, "the warning was seen before on this radio (it shows once)")
    t.eq(u.hints, [("OK", "Turn on"), ("BACK", "Keep off")], "the warning: OK turns on, BACK keeps off")
    t.eq(t.get("0 8 8 4"), "Off", "still off while the warning shows")
    u = t.keys("BACK")
    t.eq((u.id, u.sel and u.sel["name"], u.sel and u.sel["value"]), ("Group", "Extend TX", "Off"),
         "BACK: back on the row, still off")
    t.eq(t.get("0 8 8 4"), "Off", "the setting is off")
    t.home()
    u = t.keys("MENU 0 0")
    t.eq(set(u.bar), bar_keys, "the status bar has nothing for it")


@test("TONE-9", pages=("Group", "Quick"))
def tail_elimination_is_a_transmit_setting(t):
    """Tail removal (tail elimination) lives in the Transmit group: its mode and phase in Transmit's Advanced (0 8 8 2, 0 8 8 3;
    MENU-32), the end tone a main row (0 8 4); it is not a squelch option (0 9 9, System's Advanced, and the quick
    settings have no tail elimination row)."""
    u = t.keys("MENU 0 8 8")
    t.eq(u.title, "Advanced", "MENU 0 8 8: Transmit's Advanced")
    t.check(u["help"]["args"][0].startswith("MENU 0 8 8 "), "its rows are MENU 0 8 8 N", u["help"])
    names = u.names()
    t.check({"Tail removal", "Phase"} <= set(names), "mode and phase", names)
    u = t.keys("BACK")
    t.eq((u.title, u.sel and u.sel["name"]), ("Transmit", "Advanced"), "in the Transmit group")
    t.home()
    t.check("End tone" in t.keys("MENU 0 8").names(), "the end tone is Transmit's own row", t.keys("MENU 0 8").names())
    for path in ("MENU 0 9 9", "MENU"):
        t.home()
        u = t.keys(path)
        t.check(not any("tail elim" in n.lower() for n in u.names()), f"{path}: no tail elimination row", u.names())


@test("SETUP-1", "SETUP-4", pages=("Group", "Setup"))
def setup_runs_from_the_menu(t):
    """MENU 0 9 7 1 runs setup: OK goes to the next page, BACK to the previous, BACK on the first page leaves it
    with nothing changed (only the welcome page is passed: no answer is given)."""
    call = t.get("0 8 6 1")
    u = t.keys("MENU 0 9 7 1")
    t.eq(u.id, "Setup", "MENU 0 9 7 1")
    t.eq(u.hints, [("OK", "Next"), ("BACK", "Previous"), ("#", "Skip")], "OK next, BACK previous, # skips")
    f0 = u.state["frame"]
    u = t.keys("MENU")
    t.eq(u.id, "Setup", "OK on the welcome page: the next page")
    t.check(u.state["frame"] != f0, "a new page drawn", u.state)
    u = t.keys("BACK")
    t.eq(u.id, "Setup", "BACK: the previous page (the welcome)")
    u = t.keys("BACK")
    t.eq((u.id, u.sel and u.sel["name"]), ("Group", "Run setup again"), "BACK on the first page leaves setup")
    t.eq(t.get("0 8 6 1"), call, "the callsign untouched")
