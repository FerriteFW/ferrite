"""Squelch and counter checks with a real radio as the source: the second radio (over Bluetooth, Dry TX on, its
active slot already on 446.475 sending CTCSS 88.5, or on 145.495 for --counter) keys by remote PTT while the radio on
the cable is watched as sqreg.py does (card states, and the chip's audio from REG_47).

  realradio.py CABLE_PORT SOURCE_PORT [--counter] [--seconds 3]

Checks: an 88.5 slot opens on the real tone and shuts after (with the time from the carrier's end to the mute), a
100.0 slot hears the carrier but never plays it, a no-tone slot opens and closes; --counter: the counter reads the
source on 145.495 (chip 0's range, TSRCH-10). Black's slots are put back at the end.
"""
import argparse, os, subprocess, sys, time

sys.path.insert(0, os.path.dirname(__file__))
from sqreg import Bench, episodes, final, restore_slots, selects, states, counter_line  # noqa: E402

LINK = os.path.join(os.path.dirname(__file__), "..", "ferrite_link.py")


def key(src, seconds):
    return subprocess.Popen([sys.executable, LINK, src, "key", "--seconds", str(seconds)],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def last_t(trace, state):
    ts = [t for t, st, _ in trace if st == state]
    return ts[-1] if ts else None


def audio_trace(b, src, seconds):
    """Watch slot A with the audio as 'audio', and the carrier ('RX') sampled from the card in the same poll."""
    out, t0, done_at = [], time.time(), None
    proc = key(src, seconds)
    while True:
        now = time.time()
        if done_at is None and proc.poll() is not None:
            done_at = now
        if done_at is not None and now - done_at > 2.0 or now - t0 > 40:
            break
        s = b.status()["slots"].get("A", {})
        out.append((now - t0, s.get("state"), b.speaker()))
        time.sleep(0.05)
    return out


def summarize(tr):
    rx = [(t, st, None) for t, st, _ in tr]
    au = [(t, "audio" if a else "-", None) for t, _, a in tr]
    return rx, au


def t_tones(b, src, seconds):
    b.set("0 1 1", "Off")
    for tone, want in (("ctcss 88.5", 1), ("ctcss 100.0", 0), ("off", 1)):
        b.cmd(f"slot A vfo 446.475 nfm low tone off offset off")
        if tone != "off":
            b.cmd(f"slot A rxtone {tone}")
        b.cmd("slot A active")
        b.home()
        tr = audio_trace(b, src, seconds)
        rx, au = summarize(tr)
        heard = episodes(rx) >= 1
        opens = episodes(au, "audio")
        shut = "audio" not in final(au) and "RX" not in final(rx)
        la, lr = last_t(au, "audio"), last_t(rx, "RX")
        lag = f", audio mutes {round(lr - la, 2)} s before the carrier drops" if la and lr else ""
        ok = heard and opens == want and shut
        b.record(f"real radio, slot rxtone {tone}: {'plays' if want else 'stays muted'}", ok,
                 f"carrier opens {episodes(rx)}, audio opens {opens}, shut after {shut}{lag}")
        time.sleep(3)


def t_counter_vhf(b, src, seconds):
    b.set("0 1 1", "Off")
    b.cmd("slot A vfo 446.475 nfm low tone off offset off")
    b.cmd("slot A active")
    b.home()
    b.r.keys("MENU 0 3 2")
    time.sleep(0.8)
    sel = selects(b)
    proc = key(src, seconds)
    got, keyed, t0 = None, False, time.time()
    while time.time() - t0 < seconds + 15 and (proc.poll() is None or time.time() - t0 < 3):
        c = counter_line(b) or {}
        f = float(c.get("freq", "0") or 0)
        keyed |= b.status()["keyed"] is not None
        if f and c.get("live") == "1":
            got = f if got is None or abs(f - 145.495) < abs(got - 145.495) else got
        time.sleep(0.1)
    proc.wait()
    b.home()
    ok = got is not None and abs(got - 145.495) <= 0.005 and not keyed
    b.record("counter reads a real radio on 145.495 (chip 0)", ok, f"read {got}, keyed {keyed}, PB0/PE12 {sel}")


def main():
    a = argparse.ArgumentParser()
    a.add_argument("port")
    a.add_argument("source")
    a.add_argument("--counter", action="store_true")
    a.add_argument("--seconds", type=float, default=3)
    o = a.parse_args()
    b = Bench(o.port)
    slots0 = b.r.cmd("slot")
    try:
        if o.counter:
            t_counter_vhf(b, o.source, o.seconds)
        else:
            t_tones(b, o.source, o.seconds)
    finally:
        for k, (path, v) in reversed(b.put_back):
            b.cmd(f"set {path} {v}")
        restore_slots(b, slots0)
        b.home()
    bad = [r for r in b.results if not r["ok"]]
    print(f"{len(b.results) - len(bad)} pass, {len(bad)} fail")


if __name__ == "__main__":
    main()
