"""Bluetooth beside the cable (chapter 29: LINK-7, LINK-20.14, LINK-24.2, LINK-25, LINK-28 - LINK-31, LINK-36; SBAR-8,
SAFE-6, KISS-6, TERM-4.3): the tests drive the radio on the cable as always and open a second link on Bluetooth
(--bt ble:walkie-talkie on the radio, with 0 9 2 1 on, its owner's setting; the simulator's own Bluetooth port with
--sim auto, which the test turns on). PIN-lock checks set a PIN by keys (restart.py's helpers; --with pin on the
radio) and turn the lock off after.
"""
import re
from pathlib import Path
import struct
import time

import ferrite_link as fl

from .core import test
from .restart import clear_pin_lock, pin_lock, power_on_pin, set_pin_lock, the_pin

# The workspace's version, as the terminal's top line shows it (TERM-4.3).
VERSION = re.search(r'^version = "([^"]+)"', (Path(__file__).resolve().parents[2] / "Cargo.toml").read_text(), re.M)[1]

HELLO = bytes([fl.PROTOCOL]) + bytes(8) + b"uitest.py bt"


def status_of(link, ty, payload=b""):
    s, rest = link.ask(ty, payload)
    return s, rest


@test("SBAR-8", needs=("bt",), pages=("Home",))
def bt_icon_while_connected(t):
    """The Bluetooth icon is grey while Bluetooth is on and blue while a device is connected (it has sent something
    in the last 2 minutes): a HELLO over Bluetooth makes it blue; Bluetooth off (the simulator) takes it away, 2
    minutes quiet (the radio, --slow) turns it grey (bar bt_on=1 bt=0)."""
    bt = t.bt()
    u = t.wait(lambda u: u.bar.get("bt") == 1, timeout=3, what="the icon with a device connected")
    t.eq(u.bar["bt"], 1, "connected: the icon")
    if t.sim:
        t.owner_set("0 9 2 1", "Off")
        u = t.ui(0.3)
        t.eq((u.bar["bt_on"], u.bar["bt"]), (0, 0), "Bluetooth off: no icon")
        return
    t.need(t.opts.slow, "the 2 minutes' quiet: --slow")
    t.close_bt()
    u = t.wait(lambda u: u.bar.get("bt") == 0, timeout=130, what="2 minutes quiet: the icon turns grey", every=5.0)
    t.eq(u.bar.get("bt_on"), 1, "still on: the grey icon")
    del bt


@test("LINK-7", needs=("bt",), pages=("Group", "Home"))
def bt_reads_and_writes_without_pin(t):
    """With no PIN lock, a Bluetooth connection reads and writes as the cable does, with no prompt on the radio:
    channel 1 read and written back as it was; the cable's readback shows no question; 0 9 2 2 is the link's
    status (there is no list of approved devices)."""
    t.need(t.get("0 9 3 1") == "Off", "a PIN lock is on")
    bt = t.bt()
    rec = bt.link.read(fl.CHANNEL, 0)
    s, _ = bt.link.ask(fl.BEGIN, struct.pack("<H", 1))
    t.eq(s, 0, "a write session from Bluetooth")
    s, rest = bt.link.ask(fl.WRITE_MANY, bytes([fl.CHANNEL]) + struct.pack("<HB", 0, len(rec)) + rec)
    t.eq(s, 0, "the channel written back")
    t.eq(bt.link.ask(fl.END)[0], 0, "the session ends")
    t.eq(bt.link.read(fl.CHANNEL, 0), rec, "channel 1 as it was")
    u = t.ui()
    t.check(u.id not in ("Confirm", "Pin") and not u.popup, "no prompt on the radio", u.id)
    u = t.keys("MENU 0 9 2")
    names = u.names()
    t.check(len(names) > 1 and "Link" in names[1], "0 9 2 2: the link's status", names)
    t.check(not any("device" in n.lower() or "approved" in n.lower() for n in names), "no device list", names)


@test("LINK-30", "LINK-20.14", "SAFE-6", needs=("bt", "pin"), pages=("Group", "Home", "Pin", "TxRefused"))
def bt_needs_the_pin_with_a_lock(t):
    """PIN lock on: a Bluetooth HELLO says AUTH first (its third byte 0); every other request, a write and a KEY
    among them, is not authorised until AUTH with the radio's PIN; past it a write is taken and a KEY reaches the
    transmit checks (an AM slot on 145.495: refused for AM, not for the PIN); a write outside the band is refused;
    a new connection asks again; the cable is never asked."""
    with pin_lock(t):
        t.cmd("slot A vfo 145.495 am offset off")
        t.cmd("slot A active")
        set_pin_lock(t)
        raw = t.bt_raw()
        s, rest = raw.hello
        t.eq((s, rest[2]), (0, 0), "HELLO: AUTH first")
        t.eq(raw.link.ask(fl.INFO)[0], 5, "INFO: not authorised")
        t.eq(raw.link.ask(fl.BEGIN, struct.pack("<H", 1))[0], 5, "a write: not authorised")
        t.eq(raw.link.ask(fl.KEY, bytes([fl.PTT, fl.PTT_ON, fl.ACTIVE]))[0], 5, "KEY: not authorised")
        raw.link.ask(fl.KEY, bytes([fl.PTT, fl.PTT_OFF]))
        t.eq(raw.link.ask(fl.AUTH, the_pin(t).encode())[0], 0, "AUTH with the PIN")
        t.eq(raw.link.ask(fl.INFO)[0], 0, "INFO now")
        rec = raw.link.read(fl.CHANNEL, 0)
        s, _ = raw.link.ask(fl.BEGIN, struct.pack("<H", 1))
        t.eq(s, 0, "a write session now")
        bad = bytearray(rec)
        bad[4:8] = struct.pack("<I", 2_000_000_000)  # 20 GHz: no band has it (protocol 3: RX after the 2-byte zone)
        try:
            s, why = raw.link.ask(fl.WRITE_MANY, bytes([fl.CHANNEL]) + struct.pack("<HB", 0, len(bad)) + bytes(bad))
            t.check(s != 0, "a channel outside every band: refused", (s, why))
        finally:
            raw.link.ask(fl.END)
            if raw.link.read(fl.CHANNEL, 0) != rec:  # taken after all: channel 1 put back from the cable
                t.r.link.ask(fl.BEGIN, struct.pack("<H", 1))
                t.r.link.write(fl.CHANNEL, 0, rec)
                t.r.link.ask(fl.END)
        t.eq(raw.link.read(fl.CHANNEL, 0), rec, "channel 1 unchanged")
        if t.sim or t.build in ("tx-dry", "tx+dry"):
            s, why = raw.link.ask(fl.KEY, bytes([fl.PTT, fl.PTT_ON, fl.ACTIVE]), timeout=1.0)
            raw.link.ask(fl.KEY, bytes([fl.PTT, fl.PTT_OFF]))
            t.check(s == 9 and b"AM" in why, "past the PIN: KEY meets the transmit checks (AM refused)", (s, why))
        if t.sim:
            t.owner_set("0 9 2 1", "Off")  # the connection ends (LINK-29: or 2 minutes quiet)
            t.owner_set("0 9 2 1", "On")
            raw = t.bt_raw()
            t.eq(raw.hello[1][2], 0, "a new connection: AUTH first again")
        elif t.opts.slow:
            t.close_bt()
            time.sleep(125)  # LINK-29: 2 minutes quiet ends the connection
            raw = t.bt_raw()
            t.eq(raw.hello[1][2], 0, "a new connection: AUTH first again")
        else:
            t.note("a new connection asking again: after 2 minutes quiet (--slow)")
        s, rest = t.r.link.ask(fl.HELLO, bytes([fl.PROTOCOL]) + bytes(8) + b"uitest.py")
        t.eq(rest[2], 1, "the cable is never asked")
        t.close_bt()
        clear_pin_lock(t)
        raw = t.bt_raw()
        t.eq(raw.hello[1][2], 1, "PIN lock off: nothing asked")


@test("LINK-31", needs=("bt", "pin"), slow=True, pages=("Group", "Home", "Pin"))
def bt_wrong_pins_count_with_the_keypads(t):
    """Wrong PINs over Bluetooth show on the radio ("Wrong PIN, Bluetooth") and count with the keypad's: the third
    starts the 30 s wait (AUTH answers busy; the keypad's PIN entry waits too); after it the right PIN passes."""
    with pin_lock(t):
        set_pin_lock(t)
        raw = t.bt_raw()
        for i in range(3):
            s, _ = raw.link.ask(fl.AUTH, b"9876" if the_pin(t) != "9876" else b"1234")
            u = t.ui(0.1)
            t.check(s != 0, f"wrong PIN {i + 1}: refused", s)
            if i < 2:
                t.check(u.toast and "Bluetooth" in u.toast["head"] + u.toast["body"], "the radio shows it", u.toast)
        t.eq(raw.link.ask(fl.AUTH, the_pin(t).encode())[0], 7, "after the third: busy, even the right PIN")
        t.home()
        u = t.keys("MENU 0 9 3 1 MENU")
        if u.id == "Pin":
            t.check((u["pin"] or {}).get("note", "").startswith("Wait"), "the keypad's PIN entry waits too", u["pin"])
        t.home()
        time.sleep(31)
        raw.port.write(b"\x00")  # quiet past 10 s the port went back to the terminal: a sync claims it (TERM-2)
        time.sleep(0.1)
        t.eq(raw.link.ask(fl.AUTH, the_pin(t).encode())[0], 0, "after the wait: the right PIN passes")


@test("LINK-24.2", needs=("bt", "pin"), pages=("Group", "Home", "Pin"))
def bt_reboot_needs_the_pin(t):
    """REBOOT over Bluetooth with a PIN lock on: not authorised until AUTH; after it the radio restarts (and asks
    for its PIN at power on). The restart drops the Bluetooth connection, often before REBOOT's answer is out (the
    link is paced, LINK-36): a drop or no answer then is the restart. Bluetooth connects again once the module
    advertises: a new session, not authorised until AUTH. The PIN lock goes back off after (pin_lock)."""
    with pin_lock(t):
        set_pin_lock(t)
        raw = t.bt_raw()
        t.eq(raw.link.ask(fl.REBOOT)[0], 5, "REBOOT before AUTH: not authorised")
        t.eq(raw.link.ask(fl.AUTH, the_pin(t).encode())[0], 0, "AUTH")
        try:
            t.eq(raw.link.ask(fl.REBOOT)[0], 0, "REBOOT: answered")
        except (TimeoutError, OSError) as e:
            t.note(f"REBOOT over Bluetooth: no answer before the link dropped ({e}): the restart")
        t.close_bt()
        t.r.reopen()
        u = t.wait(lambda u: u.id == "Pin", timeout=15, what="restarted: the PIN asked at power on")
        raw = t.bt_raw_again(timeout=20)
        s, rest = raw.hello
        t.eq(s, 0, "HELLO over Bluetooth after the restart")
        t.eq(rest[2], 0, "a new Bluetooth session: not authorised until AUTH (LINK-30)")
        t.eq(raw.link.ask(fl.AUTH, the_pin(t).encode())[0], 0, "AUTH again")
        t.close_bt()
        t.wait(lambda u: not (u["pin"] or {}).get("note", "").startswith("Wait"), timeout=35, what="no wait left",
               every=2.0)
        power_on_pin(t, the_pin(t))
        t.wait(lambda u: u.id == "Home", timeout=5, what="the PIN typed: home")


@test("LINK-28", needs=("bt",), pages=())
def a_session_on_each_port(t):
    """The cable and Bluetooth each run their own session at once: readbacks on both answer, each on its own port;
    a write session on one makes the other's write answer busy until it ends."""
    t.need(t.get("0 9 3 1") == "Off", "a PIN lock is on")
    bt = t.bt()
    for _ in range(5):
        t.check(t.r.ui()["screen"]["id"], "the cable's readback", "")
        t.check(bt.ui()["screen"]["id"], "Bluetooth's readback", "")
    rec = t.r.link.read(fl.CHANNEL, 0)
    t.eq(t.r.link.ask(fl.BEGIN, struct.pack("<H", 1))[0], 0, "the cable's write session")
    try:
        s, _ = bt.link.ask(fl.BEGIN, struct.pack("<H", 1))
        t.eq(s, 7, "Bluetooth's: busy meanwhile")
    finally:
        t.r.link.ask(fl.WRITE_MANY, bytes([fl.CHANNEL]) + struct.pack("<HB", 0, len(rec)) + rec)
        t.r.link.ask(fl.END)
    s, _ = bt.link.ask(fl.BEGIN, struct.pack("<H", 1))
    t.eq(s, 0, "the cable's ended: Bluetooth's taken")
    bt.link.ask(fl.END)
    t.note("the remote key-up half (one remote keyer, LINK-20.5) keys a transmitter: host-tested only")


def bt_text(port, send, wait=0.6):
    port.write(send)
    end, out = time.time() + wait, b""
    while time.time() < end:
        out += port.read()
    return out.decode("ascii", "replace")


ESC_SEQ = re.compile(r"\x1b(\[[0-9;?]*[A-Za-z]|[78()=>][0-9A-Za-z]?)")


def bt_until(port, send, want, wait=4.0):
    """Send, then read until every string in `want` shows in the text with the VT100 escapes taken out (Bluetooth
    sends in paced bursts, so a whole terminal screen takes a few seconds); returns that plain text."""
    port.write(send)
    end, out = time.time() + wait, b""
    while time.time() < end:
        out += port.read()
        plain = ESC_SEQ.sub("", out.decode("ascii", "replace"))
        if all(w in plain for w in want):
            break
    return ESC_SEQ.sub("", out.decode("ascii", "replace"))


@test("LINK-25", "TERM-4.3", needs=("bt",), pages=("Home", "Scan"))
def bt_terminal_and_link(t):
    """HELLO and INFO answer over Bluetooth; Enter there opens the terminal (its header says bluetooth), which
    runs commands; a scan keeps stepping while the terminal is open (the cable's readback shows the slot moving).
    On a bench build `bt stat` shows bytes both ways and no framing errors."""
    t.need(t.get("0 9 3 1") == "Off", "a PIN lock is on")
    raw = t.bt_port()
    t.cmd("slot A active")
    t.home()
    if "scanning" not in t.slot("A"):
        t.keys("SIDE2")
        t.home()  # the scan goes on; home's card shows where it is
    try:
        want = (f"Ferrite {VERSION}", "bluetooth", "Press Enter for the menu")
        head = bt_until(raw, b"\x03\r", want)  # the top line names the port; the banner under it (TERM-11, TERM-20)
        t.check(all(w in head for w in want), "Enter on Bluetooth: the terminal's top line and banner", head[-300:])
        steps = set()
        end = time.time() + 4
        while time.time() < end:
            bt_text(raw, b"\x1b:get 0 7 1\r", wait=0.3)
            steps.add(t.ui(0.05).cards["A"]["freq"])
        t.check("scanning" in t.slot("A") and len(steps) >= 2, "the scan steps while the terminal runs commands",
                sorted(steps))
        if t.r.bench and not t.sim:
            stat = t.cmd("bt stat")
            t.check("framing 0" in stat, "no framing errors", stat)
    finally:
        bt_text(raw, b"\x03", wait=0.2)  # Ctrl-C: the terminal closes (TERM-2)
        t.close_bt()
        t.home()
        if "scanning" in t.slot("A"):
            t.keys("SIDE2")
            t.home()
    bt = t.bt()
    ver, _ = fl.text(bt.link.ok(fl.INFO))
    t.check(ver, "HELLO and INFO over Bluetooth", ver)


@test("LINK-36", needs=("bt", "bench"), where="radio", pages=())
def bt_paced_readback_arrives_whole(t):
    """Bluetooth sends in paced bursts (`bt pace B G`, default 20 B and a 50 ms gap): a whole UI readback arrives
    over Bluetooth; back to back (`bt pace 0 0`) the BT513 drops it (no whole answer); the pace put back, it
    arrives whole again; the cable answers all along. `bt stat` reads the pace (`bt pace` alone is a usage error)."""
    stat = t.cmd("bt stat")
    m = re.search(r"pace: (\d+) B bursts, (\d+) ms gaps", stat)
    t.check(m, "bt stat reads the pace", stat)
    burst, gap = (m.group(1), m.group(2)) if m else ("20", "50")
    bt = t.bt()
    t.check(bt.ui()["screen"]["id"], "the readback whole over Bluetooth", "")
    try:
        t.cmd("bt pace 0 0")
        try:
            bt.ui()
            whole = True
        except (TimeoutError, fl.RadioError):
            whole = False
        t.check(not whole, "unpaced: the readback doesn't arrive whole", whole)
        t.check(t.cmd("get 0 7 1"), "the cable answers meanwhile", "")
    finally:
        t.cmd(f"bt pace {burst} {gap}")
    t.close_bt()
    bt = t.bt()
    t.check(bt.ui()["screen"]["id"], "paced again: whole", "")


@test("KISS-6", needs=("bt",), where="sim", pages=())
def kiss_on_the_bluetooth_port(t):
    """KISS on Bluetooth (0 5 3 KISS, 0 5 8 Bluetooth): that port carries raw KISS bytes, not link frames (a HELLO there gets no
    link answer) while the cable keeps the link; KISS off: Bluetooth speaks the link protocol again. Simulator only:
    KISS on is the owner's setting on the radio (it can transmit, KISS-7)."""
    t.owner_set("0 9 2 1", "On")
    try:
        t.cmd("set 0 5 8 Bluetooth")
        t.cmd("set 0 5 3 KISS")
        try:
            t.bt_raw()
            t.fail("a HELLO over Bluetooth with KISS on was answered as a link frame")
        except TimeoutError:
            pass
        t.close_bt()
        t.check(t.cmd("get 0 7 1"), "the cable keeps the link", "")
    finally:
        t.cmd("set 0 5 3 Off")
    raw = t.bt_raw()
    t.eq(raw.hello[0], 0, "KISS off: the link answers on Bluetooth")
