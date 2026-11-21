"""Watch and the band relay (chapter 14, WATCH-*; the Status page's Watch row, SBAR-17)."""
from .core import test


def relay_mismatch(t):
    """Watch A+B+C, A tuned to 28.4 MHz by keys (it needs the relay) with B and C on VHF/UHF. Returns the readback
    straight after the retune (its toast) and the Status page's Watch row; checks the cancel and that watch resumes
    by itself once A is back on 2 m (WATCH-11.1)."""
    t.set("0 1 1", "A+B+C")
    t.cmd("slot A vfo 146.55 fm low tone off offset off")
    t.cmd("slot B vfo 146.52 fm low tone off offset off")
    t.cmd("slot C vfo 446.0 fm low tone off offset off")
    u = t.home()
    u = t.ui(0.5)
    t.eq(u.bar["watch"], 3, "watch on: TW")
    t.keys("2 8 STAR 4 MENU", settle=0.05)
    # the cancel's toast comes with the Radio task's retune, about 0.5 s on the radio
    u = t.wait(lambda u: u.toast is not None, timeout=2, what="the relay toast")
    toast = u.toast
    t.eq(u.active["freq"], "28.40000", "A tuned to 28.4 MHz by keys")
    u = t.ui(0.5)
    t.eq(u.bar["watch"], 0, "cancelled: no TW or DW")
    t.eq((u.cards["B"]["state"], u.cards["C"]["state"]), ("relay", "relay"), "the other cards say relay")
    u = t.keys("MENU 0 0")
    row = u.row("Watch")["value"]
    t.home()
    t.keys("1 4 6 STAR 5 5 MENU")
    u = t.ui(0.5)
    t.eq(u.bar["watch"], 3, "back on 2 m: watch resumes by itself")
    t.check(u.cards["B"]["state"] != "relay", "B watched again", u.cards["B"])
    return toast, row


@test("WATCH-11.1", pages=("Home", "Status"))
def relay_cancels_watch(t):
    """Watch A+B+C with the active slot tuned to 28 MHz (relay) and the others on VHF: watch is cancelled (no TW,
    the other cards say "relay"); tuned back to 2 m, watch resumes by itself."""
    stopped = t.stop_scans()
    try:
        relay_mismatch(t)
    finally:
        t.put_back(stopped)


@test("WATCH-11.2", pages=("Home", "Status"))
def relay_cancel_toast(t):
    """The same retune: a toast "Watch off: HF needs the relay"."""
    stopped = t.stop_scans()
    try:
        toast, _ = relay_mismatch(t)
        words = toast and toast["head"] + " " + toast["body"]
        t.check(words and "Watch off" in words and "HF needs the relay" in words,
                "a toast: Watch off: HF needs the relay", toast)
    finally:
        t.put_back(stopped)


@test("WATCH-11.3", "SBAR-17", pages=("Home", "Status"))
def relay_cancel_status_row(t):
    """The same retune: Status's Watch row "Off: HF needs the relay"."""
    stopped = t.stop_scans()
    try:
        _, row = relay_mismatch(t)
        t.eq(row, "Off: HF needs the relay", "Status's Watch row says why")
    finally:
        t.put_back(stopped)
