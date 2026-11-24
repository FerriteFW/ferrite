#!/usr/bin/env python3
"""Ferrite link protocol v1, PC side: the reference for the web app (manual chapter 29, Appendix "Link protocol v1";
the radio's side is ferrite-core's link.rs and ferrite-app's link.rs).

    pip install pyserial
    python3 tools/ferrite_link.py PORT hello
    python3 tools/ferrite_link.py PORT info
    python3 tools/ferrite_link.py PORT read-settings settings.bin
    python3 tools/ferrite_link.py PORT write-settings settings.bin
    python3 tools/ferrite_link.py PORT read-channel 12            # channel 12 as shown on the radio (1-4000)
    python3 tools/ferrite_link.py PORT write-channel 12 --name "Club rpt" [--rx 146.94]
    python3 tools/ferrite_link.py PORT delete-channel 12
    python3 tools/ferrite_link.py PORT used [channel|zone|preset]  # which numbers hold a record (LINK-39)
    python3 tools/ferrite_link.py PORT readmany [channel|zone|preset] [FIRST [COUNT]]  # whole records, several a frame (LINK-40)
    python3 tools/ferrite_link.py PORT log [N]                    # the event log, newest first (LINK-42)
    python3 tools/ferrite_link.py PORT baud 230400
    python3 tools/ferrite_link.py PORT reboot                     # into the bootloader, for flash.py
    python3 tools/ferrite_link.py PORT term "LINE" ["LINE" ...]   # terminal command lines over the link (LINK-34)
    python3 tools/ferrite_link.py PORT slot [A|B|C]             # a slot's working fields (LINK-33)
    python3 tools/ferrite_link.py PORT key [--seconds N] [--slot active|a|b|c]   # remote PTT (LINK-20)
    python3 tools/ferrite_link.py PORT keys "MENU 0 8 2 1 OK" [--long]   # remote keys (LINK-19); NAME:long|down|up
    python3 tools/ferrite_link.py PORT knob +3                    # knob steps (LINK-19)
    python3 tools/ferrite_link.py PORT ui [keys]                  # what the screen shows (LINK-19's readback)
    python3 tools/ferrite_link.py PORT status                     # the compact one-line status (LINK-19, what=2)
    python3 tools/ferrite_link.py PORT aprs-cfg                   # the APRS settings record (LINK-43)
    python3 tools/ferrite_link.py PORT heard                      # the APRS heard list (LINK-44)
    python3 tools/ferrite_link.py PORT msgs                       # APRS messages in and out, newest first (LINK-44)
    python3 tools/ferrite_link.py PORT msg-send CALL "TEXT"       # queue an APRS message as compose does (LINK-45)
    python3 tools/ferrite_link.py PORT msg-delete in|out CALL "TEXT"  # delete the newest such message (LINK-52)
    python3 tools/ferrite_link.py PORT subscribe [heard,msgs,ui,log] [--seconds N] [--screen]   # print pushed events (log: LINK-57)
    python3 tools/ferrite_link.py PORT audio [SECONDS [FILE.wav]]   # receive audio over the cable to a WAV (LINK-13.4)
    python3 tools/ferrite_link.py PORT audio SECONDS FILE.wav --raw [12000|16000]   # bench builds: the decoders' samples, 12-bit PCM (LINK-13.9)
    python3 tools/ferrite_link.py --pin 1234 PORT ...             # Bluetooth while a PIN lock is on (LINK-30)

PORT is a serial device (the programming cable, 115200 8N1), tcp:HOST:PORT for the simulator
(`cargo run -p ferrite-sim -- --term-headless 120 --term-tcp 7300`, then `tcp:127.0.0.1:7300`), or ble:NAME_OR_ADDRESS
for the radio's Bluetooth (0 9 2 1 on; the BT513 advertises as "walkie-talkie"; on macOS the address is the
CoreBluetooth UUID): `ble:walkie-talkie`. Needs bleak (`pip install bleak`). The link runs on service FFE0,
characteristic FFE1 (write without response, notify), in 20-byte writes paced for the module's 9600-baud UART
(LINK-25): about 0.8 KB/s each way, so a UI readback takes a second or so. FF31 (the module's own control) is never
touched. The cable's session and a Bluetooth one run at the same time (LINK-28).

Frames: COBS(type, seq, payload, CRC-16/CCITT-FALSE little-endian) then 0x00. Each request is answered with
type | 0x80, the same seq, and a payload starting with a status byte.

The cable carries the terminal or the link, one at a time (TERM-2; LINK-28 for the cable against Bluetooth). With
0 9 2 3 Cable port on Auto (or Link) this tool takes it: it sends Ctrl-C first, which closes an open terminal (quit
term.py or any terminal program holding the serial device first: one program per device), then a 0x00, which gives
the cable to the link. The link keeps it while frames come and lets go 10 s after the last; then Enter in a terminal
program opens the terminal again. With Cable port on Terminal or KISS the link never answers.

`term "LINE"` (LINK-34) runs terminal command lines (`slot`, `set`, `get`, `bk`, `mic`, `txtest`, `help`, ...) on this
one link session, as the terminal's command line would, and prints their output; no switching to the terminal.

`key` (LINK-20): keys like a PTT press, with every transmit check, never on an emergency frequency; sends a keepalive
every 0.5 s (the radio unkeys after 2 s without one) and KEY off after --seconds (default 5) or Ctrl-C, always.
A refusal prints the radio's reason. On the cable a `tx` build sends nothing while keyed (TX-43): the answers wait on
the radio and arrive after the unkey, so this tool doesn't wait for them while keyed (a `tx-dry` build answers at
once). A PTT pressed on the radio ends the remote transmission (the radio's operator wins).

`keys` and `knob` (LINK-19) press the keypad, the side keys and the knob as a person would: the same path as the
keys (long press, lights, wake, lock). Names: MENU (OK), BACK (EXIT), UP, DOWN, LEFT, RIGHT, SLOT (ABC), VM, 0-9,
STAR (*), HASH (#), SIDE1 (PF1), SIDE2 (PF2); `ui keys` lists them with their stock codes. A key is a short press,
NAME:long a long one (held 750 ms), NAME:down / NAME:up hold and let go. Never the PTTs: a remote key never transmits, but for the crossband repeater's # on its start page (XB-20.2).
`ui` prints what the screen shows (the screen, status bar, home cards or list rows, hints, toast). `status` prints
the compact status, one short line (`st scr=Home act=A keyed=- remote=0 mode=off A=RX/20 B=-/0 C=-/0 xb=0`): the
facts tests poll, cheap on Bluetooth where the whole readback takes seconds.

As a library (the test runner's):

    from ferrite_link import Radio
    with Radio("tcp:127.0.0.1:7300") as r:      # or "/dev/cu.usbserial-114220", or "ble:walkie-talkie" (pin="1234")
        r.keys("MENU 0 7")                      # or r.press("MENU"), r.press("HASH", long=True)
        r.knob(+2)
        ui = r.ui()                             # {"screen": {...}, "rows": [...], "sel": {...}, "hints": [...]}
        assert ui["screen"]["id"] == "Group" and ui["sel"]["name"] == "Theme"
"""
import argparse
import os
import socket
import struct
import sys
import time

PROTOCOL = 1  # decision 385: protocol 1 as 0.9 first releases it; until then no older one is kept
HELLO, INFO, AUTH, READ, WRITE, DELETE, BEGIN, END, SLOT, BAUD, REBOOT, KEY, CMD, MORE, UI = (
    0x01, 0x02, 0x04, 0x10, 0x11, 0x12, 0x13, 0x14, 0x16, 0x20, 0x21, 0x30, 0x31, 0x32, 0x33)
USED, READ_MANY, WRITE_MANY, LOG_READ = 0x15, 0x17, 0x18, 0x40  # LINK-39 - LINK-42
STREAM, STREAM_MORE = 0x19, 0x1A  # LINK-50
CAP_LZ, CAP_AUDIO, CAP_LINE_CMDS = 1, 2, 4  # LINK-35.2: HELLO's caps byte, after the build byte (decision 385)
Z = 0x7D  # LINK-51: a compressed frame (| 0x80 for an answer): the inner type, the frame count, the coded payload
LOG_SOURCES = ["APRS", "Pager", "SAME", "DSC", "AIS", "DTMF", "Selcall", "GPS"]  # LINK-42's source byte
SUBSCRIBE, EVENT, HEARD, MSGS, MSG_SEND, MSG_STOP = 0x50, 0x51, 0x52, 0x53, 0x54, 0x55  # LINK-43 - LINK-49
MSG_DELETE = 0x59  # LINK-52
AUDIO, AUDIO_DATA = 0x56, 0x57  # LINK-13.4: receive audio (the cable only)
AUDIO_RAW = 0x58  # LINK-13.9: the bench's raw stream
SETTINGS, CHANNEL, ZONE, PRESET, APRS = 1, 2, 3, 4, 5
CONTACT = 6  # APRS-42
POSITION = 7  # LINK-53: a channel's position by number, <ii lat, lon in 1e-7 degree; b"" clears it
DATA_READ, DATA_WRITE = 0x61, 0x62  # LINK-59, LINK-60 (decision 333): the SPI data range raw (writes: the cable only)
CLOCK = 0x63  # LINK-65 (decision 340): set or read the radio's clock
STATE, TUNE = 0x34, 0x35  # LINK-67, LINK-68 (decision 373): the radio state record, a slot tuned by command
BEACON = 0x36  # LINK-69: a beacon now, as the radio's own
PHONE_POS = 0x37  # LINK-72: the phone's position (GPS-3.1: used locally, never beaconed)
QUICK, WAYPOINT, DTMF = 8, 9, 10  # LINK-54 - LINK-56: flags + text; name(16) <ii lat, lon, source; len + symbols + name
WP_ADD = 0xFFFF  # LINK-55: WRITE_MANY's index for a new waypoint
PRESET_SKIPS = 11  # LINK-58: a range preset's skipped frequencies, <I each (10 Hz), 0-48; b"" clears
ONE_FRAME = (CHANNEL, ZONE, PRESET, CONTACT, POSITION, QUICK, WAYPOINT, DTMF, PRESET_SKIPS)  # READ_MANY/WRITE_MANY only; READ/WRITE are for SETTINGS and APRS (LINK-21)
# SUBSCRIBE's mask and flags, EVENT's kinds (Appendix G)
SUB_HEARD, SUB_MSGS, SUB_UI, SUB_LOG, RENEW = 1, 2, 4, 8, 1
EV_HEARD, EV_MSG, EV_MSG_STATE, EV_UI, EV_LOG = 1, 2, 3, 4, 5
WHATS = {"channel": CHANNEL, "zone": ZONE, "preset": PRESET}
# KEY's remote PTT (Appendix G): key 0x80, then the action (0 off, 1 on + slot, 2 keepalive); slot 0xFF: the active one
PTT, PTT_OFF, PTT_ON, PTT_ALIVE, ACTIVE = 0x80, 0, 1, 2, 0xFF
SLOTS = {"active": ACTIVE, "a": 0, "b": 1, "c": 2}
# KEY's keypad (LINK-19): stock's key code, then the action (0 up, 1 down, 2 short, 3 long); 0x81 the knob + i8 steps
KEY_UP, KEY_DOWN, KEY_SHORT, KEY_LONG, KNOB = 0, 1, 2, 3, 0x81
LONG_HOLD = 0.75  # the radio lets a remote long press go 750 ms after it went down
KEY_CODES = {"MENU": 6, "OK": 6, "BACK": 9, "EXIT": 9, "UP": 7, "DOWN": 8, "LEFT": 4, "RIGHT": 5, "SLOT": 3,
             "ABC": 3, "VM": 2, "V/M": 2, "0": 17, "1": 10, "2": 11, "3": 12, "4": 14, "5": 15, "6": 16, "7": 18,
             "8": 19, "9": 20, "STAR": 13, "*": 13, "HASH": 21, "#": 21, "SIDE1": 0, "PF1": 0, "SIDE2": 1, "PF2": 1}
ACTIONS = {"short": KEY_SHORT, "long": KEY_LONG, "down": KEY_DOWN, "up": KEY_UP}
STATUS = ["ok", "bad frame", "unknown type", "bad argument", "refused", "not authorised (send AUTH: --pin)",
          "pending (asking on the radio)", "busy", "not found", "invalid", "cancelled"]
MODES = ["FM", "NFM", "AM"]


def crc16(data):
    crc = 0xFFFF
    for b in data:
        crc ^= b << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


def cobs_encode(src):
    out, code_at, code = bytearray([0]), 0, 1
    for b in src:
        if b == 0:
            out[code_at] = code
            code_at, code = len(out), 1
            out.append(0)
        else:
            out.append(b)
            code += 1
            if code == 0xFF:
                out[code_at] = code
                code_at, code = len(out), 1
                out.append(0)
    out[code_at] = code
    return bytes(out)


def cobs_decode(src):
    out, i = bytearray(), 0
    while i < len(src):
        code = src[i]
        if code == 0 or i + code > len(src):
            raise ValueError("bad COBS")
        out += src[i + 1:i + code]
        i += code
        if code < 0xFF and i < len(src):
            out.append(0)
    return bytes(out)


BLE_DATA = "0000ffe1-0000-1000-8000-00805f9b34fb"  # FFE1: the BT513's transparent UART
BLE_FF31 = "0000ff31-0000-1000-8000-00805f9b34fb"  # FF31: the module's control; only the speed unlock (LINK-15.1)
BLE_CHUNK = 20  # the ATT payload of the default 23-byte MTU
BLE_PACE = 0.025  # seconds between chunks: 800 B/s, under the module's 9600-baud UART (960 B/s)


class BlePort:
    """The radio over Bluetooth: FFE1 writes (without response, 20 bytes, paced) and notifications, through bleak on
    its own thread and event loop so the callers stay synchronous. `target`: the advertised name or the address."""

    def __init__(self, target, timeout=15.0):
        import asyncio
        import threading
        self._asyncio = asyncio
        self._loop = asyncio.new_event_loop()
        self._thread = threading.Thread(target=self._loop.run_forever, daemon=True)
        self._thread.start()
        self._rx, self._lock, self._got = bytearray(), threading.Lock(), threading.Event()
        self.rx_total, self.rx_mark, self.last_rx, self.dropped_at = 0, 0, None, None
        self.slow = True  # paced at ~280 B/s by the radio (LINK-36): answers take longer than on the cable
        self.buf = bytearray()
        self.client = None
        self.client = self._run(self._connect(target, timeout), timeout + 15)

    def _run(self, coro, timeout=10.0):
        return self._asyncio.run_coroutine_threadsafe(coro, self._loop).result(timeout)

    async def _connect(self, target, timeout):
        from bleak import BleakClient, BleakScanner

        def match(dev, adv):
            names = {dev.name or "", getattr(adv, "local_name", None) or ""}
            return dev.address.lower() == target.lower() or target in names

        dev = await BleakScanner.find_device_by_filter(match, timeout=timeout)
        if dev is None:
            raise OSError(f"no Bluetooth device {target!r} found in {timeout:.0f} s (0 9 2 1 on? in range?)")
        client = BleakClient(dev, disconnected_callback=self._gone)
        await client.connect()
        await client.start_notify(BLE_DATA, self._notify)
        return client

    def _notify(self, _char, data):
        with self._lock:
            self._rx += data
            self.rx_total += len(data)
            self.last_rx = time.time()
        self._got.set()

    def _gone(self, _client):
        """The Bluetooth connection itself dropped (not just a slow answer): remembered for the time-out message."""
        self.dropped_at = time.time()
        print("ble: disconnected", file=sys.stderr)

    def diagnose(self, since):
        """Why an answer didn't come: the connection dropped, or it's up and the radio was silent or still sending."""
        if self.dropped_at:
            return f"Bluetooth disconnected {time.time() - self.dropped_at:.1f} s ago"
        if self.last_rx is None or self.last_rx < since:
            return "Bluetooth still connected; the radio sent nothing since the request"
        return (f"Bluetooth still connected; the radio was still sending ({self.rx_total - self.rx_mark} bytes since "
                f"the request, the last {time.time() - self.last_rx:.1f} s ago): slow, not dropped")

    def write(self, b):
        for i in range(0, len(b), BLE_CHUNK):
            self._run(self.client.write_gatt_char(BLE_DATA, bytes(b[i:i + BLE_CHUNK]), response=False))
            time.sleep(BLE_PACE)

    def read(self):
        self._got.wait(0.05)
        with self._lock:
            out = bytes(self._rx)
            self._rx.clear()
            self._got.clear()
        return out

    def baud(self, rate):
        pass  # the module's UART rate is set by unlock(), not on this side (LINK-15.1)

    def unlock(self):
        """LINK-15.1: the module's per-connection speed unlock, the only FF31 write ever made (approved Oct 14, 2026):
        `3F 3F 3F 3F`, selector `02` (115200), 15 random bytes, written with response (FF31 ignores a write without).
        The module answers `!!!!` and 16 bytes on FFE1; they are dropped so they don't run into the next frame.
        True if it answered."""
        import os
        self._run(self.client.write_gatt_char(BLE_FF31, bytes([0x3F] * 4 + [0x02]) + os.urandom(15), response=True))
        end, got = time.time() + 1.5, b""
        while time.time() < end and not (got.startswith(b"!!!!") and len(got) >= 20):
            got += self.read()
        self.buf.clear()
        return got.startswith(b"!!!!")

    def close(self):
        try:
            if self.client is not None:
                self._run(self.client.disconnect())
        finally:
            self._loop.call_soon_threadsafe(self._loop.stop)


def Port(spec):  # noqa: N802 (a class once; callers construct it)
    """The transport for PORT: ble:NAME_OR_ADDRESS, tcp:HOST:PORT, or a serial device."""
    if spec.startswith("ble:"):
        return BlePort(spec[4:])
    return StreamPort(spec)


class StreamPort:
    def __init__(self, spec):
        if spec.startswith("tcp:"):
            _, host, port = spec.split(":")
            self.sock = socket.create_connection((host, int(port)), timeout=3)
            self.ser = None
        else:
            import serial
            self.ser = serial.Serial(spec, 115200, timeout=0.05)
            self.sock = None
        self.buf = bytearray()

    def write(self, b):
        (self.sock.sendall if self.sock else self.ser.write)(b)

    def read(self):
        if self.sock:
            self.sock.settimeout(0.05)
            try:
                return self.sock.recv(4096)
            except socket.timeout:
                return b""
        # As soon as anything has come (pyserial's read(n) waits out the whole timeout for fewer than n bytes: every
        # short answer cost 50 ms), then whatever else is already there.
        got = self.ser.read(1)
        return got + self.ser.read(self.ser.in_waiting) if got else got

    def baud(self, rate):
        if self.ser:
            self.ser.baudrate = rate

    def close(self):
        (self.sock or self.ser).close()


class StreamGap(Exception):
    """LINK-50: a stream's frame didn't come (lost on the way, or none in time): `got` is what came before it."""

    def __init__(self, got):
        super().__init__(f"stream: a frame missing after {len(got)}")
        self.got = got


class Link:
    def __init__(self, port):
        self.p, self.seq = port, 0
        self.caps = 0  # LINK-35.2, from the HELLO answer (note_hello)
        self.streams = True
        self.window = 1  # LINK-41.1: WRITE_MANY frames in flight, 2 once HELLO says the radio takes them
        # LINK-51: compression, asked for in every HELLO (FERRITE_LZ=0: never), on when the answer grants it
        self.want_lz = os.environ.get("FERRITE_LZ", "1") != "0"
        self.lz = None  # (up encoder, down decoder, [up count, down count]) while on
        self.events = []  # EVENT frames (LINK-46) as they arrive: (event counter, payload)
        self.audio = []  # AUDIO_DATA payloads (LINK-13.4) as they arrive
        self.raw = []  # AUDIO_RAW payloads (LINK-13.9) as they arrive
        # Ctrl-C closes the terminal if it has the cable (TERM-2; nothing otherwise); its goodbye text is dropped.
        self.p.write(b"\x03")
        time.sleep(0.3)
        self.p.read()
        self.p.buf.clear()
        self.p.write(b"\x00")  # TERM-2: a 0x00 gives the cable to the link
        self.sent_at = time.time()

    # TERM-2: the radio keeps the port for the link while frames or syncs come within 10 s (link.rs ACTIVE_MS);
    # after a longer quiet, in auto, a frame's bytes go to the terminal until its closing 0x00 and the frame is
    # lost. A sync before a frame sent after this much quiet gives the port back first.
    QUIET_S = 5.0

    def send(self, ty, payload=b""):
        """One request, not waiting for its answer; its seq."""
        self.seq = (self.seq + 1) & 0xFF
        if self.lz and ty != HELLO:
            up, _, n = self.lz
            coded = up.encode(payload)  # the window takes it either way, as the radio's will
            if len(coded) + 2 < len(payload):
                ty, payload = Z, bytes([ty, n[0] & 0xFF]) + coded
            n[0] += 1
        body = bytes([ty, self.seq]) + payload
        sync = b"\x00" if time.time() - self.sent_at >= self.QUIET_S else b""
        self.p.write(sync + cobs_encode(body + struct.pack("<H", crc16(body))) + b"\x00")
        self.sent_at = time.time()
        return self.seq

    def answers(self):
        """The whole answers received so far: (type, seq, status, rest) each."""
        self.p.buf += self.p.read()
        out = []
        while b"\x00" in self.p.buf:
            raw, _, rest = bytes(self.p.buf).partition(b"\x00")
            self.p.buf = bytearray(rest)
            if not raw:
                continue
            try:
                f = cobs_decode(raw)
            except ValueError:
                continue  # terminal text, or a damaged frame
            if len(f) < 5 or crc16(f[:-2]) != struct.unpack("<H", f[-2:])[0]:
                continue
            if f[0] == AUDIO_DATA:  # LINK-13.4: pushed, plain, outside compression's window and count
                self.audio.append(bytes(f[2:-2]))
                continue
            if f[0] == AUDIO_RAW:  # LINK-13.9: likewise
                self.raw.append(bytes(f[2:-2]))
                continue
            if self.lz and f[0] != HELLO | 0x80:
                _, down, n = self.lz
                pl = bytes(f[2:-2])
                if f[0] & 0x7F == Z:
                    if len(pl) < 2:
                        continue  # too short to be a coded frame: damaged on the way
                    if pl[1] != n[1] & 0xFF:
                        n[1] += 1
                        continue  # a frame lost on the way: this one can't be read (a new HELLO starts over)
                    plain = down.decode(pl[2:])
                    n[1] += 1
                    f = bytes([pl[0], f[1]]) + plain + b"\0\0"
                else:
                    down.seen(pl)
                    n[1] += 1
            if f[0] == EVENT:  # pushed by the radio, not an answer (LINK-46): its type has no 0x80 bit
                self.events.append((f[1], bytes(f[2:-2])))
                continue
            out.append((f[0], f[1], f[2], f[3:-2]))
        return out

    def hello(self, name=b"ferrite_link.py"):
        """HELLO, asking for compression (LINK-51) unless FERRITE_LZ=0; the answer's caps noted."""
        return self.note_hello(self.ok(HELLO, bytes([PROTOCOL, 1 if self.want_lz else 0]) + bytes(7) + name))

    def note_hello(self, hello):
        """LINK-35.2: HELLO's answer (version text, build, caps, layouts, lists generation): what this session
        has (compression), from protocol 1's caps byte; everything else every radio has (decision 385)."""
        if hello[0] != PROTOCOL:
            raise ProtocolError(hello[0])
        rest = text(hello[3:])[1]
        caps = rest[1] if len(rest) > 1 else 0
        self.caps = caps
        self.streams = not os.environ.get("FERRITE_NO_STREAM")
        import linkz
        self.lz = (linkz.Lz(linkz.UP), linkz.Lz(linkz.DOWN), [0, 0]) if caps & CAP_LZ else None
        self.window = int(os.environ.get("FERRITE_WINDOW", "2"))
        return hello

    def stream(self, ty, payload=b"", timeout=None):
        """LINK-50: one STREAM of `ty`'s request: its answers' payloads (after the status) in order. Raises
        RadioError on a refusal, TimeoutError if a frame doesn't come."""
        if timeout is None:
            timeout = 6.0 if getattr(self.p, "slow", False) else 2.0
        seq = self.send(STREAM, bytes([ty]) + payload)
        self.seq = (self.seq + 255) & 0xFF  # the stream's frames take the seqs after it: the next request skips them
        out, end, want = [], time.time() + timeout, seq
        while time.time() < end:
            for t, s, status, rest in self.answers():
                if t not in (STREAM | 0x80, STREAM_MORE | 0x80):
                    continue
                if s != want:
                    self.resync()
                    raise StreamGap(out)  # a frame lost on the way: ask again from what came
                if status != 0:
                    raise RadioError(status, rest)
                out.append(bytes(rest))
                want = (want + 1) & 0xFF
                end = time.time() + timeout
                if t == STREAM | 0x80:
                    self.seq = (want + 1) & 0xFF
                    return out
        self.resync()
        raise StreamGap(out)

    def resync(self):
        """LINK-51: after a lost frame, a HELLO starts both ends' windows over (while compression is on)."""
        if self.lz is not None:
            time.sleep(0.3)
            self.answers()
            self.hello()

    def ask(self, ty, payload=b"", timeout=None, retry_pending=30):
        slow = getattr(self.p, "slow", False)
        if timeout is None:
            timeout = 6.0 if slow else 2.0  # Bluetooth: a 224-byte chunk is ~0.8 s idle, over 2 s on a busy radio
        resynced = False
        for _ in range(retry_pending + 2):
            t0 = time.time()
            if slow:
                self.p.rx_mark = self.p.rx_total
            seq = self.send(ty, payload)
            end = time.time() + timeout
            pending = False
            while time.time() < end and not pending:
                for t, s, status, rest in self.answers():
                    if t == ty | 0x80 and s == seq:
                        if status == 6:  # pending: the radio asks its owner; ask again
                            time.sleep(1)
                            pending = True
                            break
                        if ty == HELLO and status == 0:
                            self.note_hello(bytes(rest))  # LINK-51: whatever sent it, its answer says whether compression is on
                        return status, rest
            if not pending:
                # A frame sent just after the radio restarted (or after 10 s of quiet) can go to the terminal (TERM-2):
                # a read-only request with nothing back is sent once more after a 0x00 sync. Never a KEY, CMD, MORE
                # or write: repeating those would act twice or skip a chunk.
                if not resynced and self.lz is not None and ty != HELLO:
                    resynced = True  # LINK-51: a lost frame leaves the windows apart: start them over, ask again
                    self.resync()
                    continue
                if not resynced and ty in (HELLO, INFO, UI):
                    resynced = True
                    self.p.write(b"\x00")
                    self.sent_at = time.time()
                    continue
                why = f" ({self.p.diagnose(t0)})" if hasattr(self.p, "diagnose") else ""
                raise TimeoutError(f"no answer to 0x{ty:02X} in {timeout:.0f} s{why}")
        raise TimeoutError("the radio is still asking its owner")

    def ok(self, ty, payload=b""):
        s, rest = self.ask(ty, payload)
        if s != 0:
            why = rest.decode("ascii", "replace") if s == 9 else ""
            sys.exit(f"0x{ty:02X}: {STATUS[s] if s < len(STATUS) else s} {why}".strip())
        return rest

    def read(self, what, idx):
        """One record: a channel, zone or preset by READ_MANY of its index alone (LINK-21, LINK-40); the settings
        and APRS records by READ, in parts."""
        if what in ONE_FRAME:
            got = [r for i, r in self.read_many(what, idx, 1)[0] if i == idx]
            if not got:
                sys.exit(f"0x{READ_MANY:02X}: {STATUS[8]}")
            return got[0]
        rec = bytearray()
        while True:
            a = self.ok(READ, struct.pack("<BHHB", what, idx, len(rec), 200))
            total = struct.unpack("<H", a[:2])[0]
            rec += a[2:]
            if len(rec) >= total:
                return bytes(rec)

    def write(self, what, idx, rec, part=200):
        """One record: a channel, zone or preset by WRITE_MANY (LINK-41); the settings and APRS by WRITE, in parts."""
        if what in ONE_FRAME:
            self.write_many(what, [(idx, rec)])
            return
        for off in range(0, max(len(rec), 1), part):
            self.ok(WRITE, struct.pack("<BHHH", what, idx, off, len(rec)) + rec[off:off + part])

    def data_read(self, addr, n):
        """LINK-59: `n` bytes (1-232) of the SPI flash at `addr` (never calibration or the update area)."""
        return self.ok(DATA_READ, struct.pack("<BIB", 0, addr, n))

    def data_crc(self, addr, n):
        """LINK-59: the CRC-32 of `n` bytes at `addr`."""
        return struct.unpack("<I", self.ok(DATA_READ, struct.pack("<BII", 1, addr, n)))[0]

    def data_erase(self, addr):
        """LINK-60: the 4 KB sector at `addr` erased (the cable only)."""
        self.ok(DATA_WRITE, struct.pack("<BI", 0, addr))

    def data_program(self, addr, b):
        """LINK-60: 1-64 bytes programmed at `addr`, inside one 256-byte page, into erased bytes."""
        self.ok(DATA_WRITE, struct.pack("<BI", 1, addr) + bytes(b))

    def data_stamp(self, layout=3):
        """LINK-62: the layout header written last; the radio restarts on it."""
        self.ok(DATA_WRITE, struct.pack("<BH", 2, layout))

    def used(self, what):
        """LINK-39: the indexes of `what` that hold a record (protocol 3: in parts from a first index)."""
        out, first, n = [], 0, 1
        while first < n:
            a = self.ok(USED, struct.pack("<BH", what, first))
            n, at = struct.unpack("<HH", a[:4])
            bits = a[4:]
            k = min(n - at, len(bits) * 8)
            out += [at + i for i in range(k) if bits[i // 8] >> (i % 8) & 1]
            first = at + max(k, 1)
        return out

    def read_many(self, what, first=0, count=None):
        """LINK-40: every record held from `first`, `count` indexes on (default: to the end; channels are 0-3999):
        [(index, record)], and the requests it took."""
        if count is None:
            count = (4000 if what in (CHANNEL, POSITION) else 4096) - first
        if self.streams:
            out, end, asks = [], first + count, 0
            while first < end:
                asks += 1
                try:
                    frames, done = self.stream(READ_MANY, struct.pack("<BHH", what, first, min(end - first, 0xFFFF))), True
                except StreamGap as g:
                    frames, done = g.got, False
                    if asks > 8:
                        raise
                    time.sleep(0.3)  # what's still on its way is dropped with the gap
                    self.answers()
                for a in frames:
                    first = max(first, struct.unpack("<H", a[:2])[0])
                    r = a[2:]
                    while r:
                        i, n = struct.unpack("<HB", r[:3])
                        out.append((i, bytes(r[3:3 + n])))
                        r = r[3 + n:]
                if done:
                    break
            return out, asks
        out, asks, end = [], 0, first + count
        while first < end:
            a = self.ok(READ_MANY, struct.pack("<BHH", what, first, min(end - first, 0xFFFF)))
            asks += 1
            nxt, r = struct.unpack("<H", a[:2])[0], a[2:]
            while r:
                i, n = struct.unpack("<HB", r[:3])
                out.append((i, bytes(r[3:3 + n])))
                r = r[3 + n:]
            if nxt <= first:
                break  # the radio's last index (zones and presets end where they end)
            first = nxt
        return out, asks

    def write_many(self, what, recs):
        """LINK-41: (index, record) pairs, as many as fit in one frame each; raises on a refusal with how many went."""
        frames, frame = [], bytearray([what])
        for i, rec in recs:
            item = struct.pack("<HB", i, len(rec)) + rec
            if len(frame) + len(item) > 240:
                frames.append(bytes(frame))
                frame = bytearray([what])
            frame += item
        if len(frame) > 1:
            frames.append(bytes(frame))
        if self.window <= 1:
            sent = 0
            for fr in frames:
                s, rest = self.ask(WRITE_MANY, fr)
                if s != 0:
                    done = rest[0] if rest and s in (7, 9, 10) else 0
                    sys.exit(f"WRITE_MANY: {sent + done} written, then {why(s, rest[1:])}")
                sent += rest[0]
            return sent
        # LINK-41.1: `window` frames in flight; the radio takes the next from its UART's ring as it answers each, so
        # the store never waits for a round trip. In order: a refusal stops it (frames after it may have gone).
        timeout = 8.0 if getattr(self.p, "slow", False) else 4.0
        sent, flight, k, lost = 0, [], 0, 0
        while k < len(frames) or flight:
            while k < len(frames) and len(flight) < self.window:
                flight.append((self.send(WRITE_MANY, frames[k]), k))
                k += 1
            end = time.time() + timeout
            got = None
            while got is None and time.time() < end:
                for t, s, status, rest in self.answers():
                    if t == WRITE_MANY | 0x80 and s == flight[0][0]:
                        got = (status, rest)
            if got is None:
                # a frame or its answer lost on the way: start the windows over (LINK-51) and send again from the
                # first frame not answered (writing a record again changes nothing)
                lost += 1
                if os.environ.get("FERRITE_DEBUG"):
                    print(f"WRITE_MANY: no answer to frame {flight[0][1]} of {len(frames)}; again (lz counts {self.lz and self.lz[2]})", file=sys.stderr)
                if lost > 3:
                    raise TimeoutError(f"WRITE_MANY: no answer in {timeout:.0f} s after {sent} written")
                k, flight = flight[0][1], []
                self.resync()
                continue
            s, rest = got
            flight.pop(0)
            if s != 0:
                done = rest[0] if rest and s in (7, 9, 10) else 0
                sys.exit(f"WRITE_MANY: {sent + done} written, then {why(s, rest[1:])}")
            sent += rest[0]
        return sent

    def delete_many(self, what, idxs):
        """LINK-41.2: channels deleted several to a WRITE_MANY (records of length 0) when the radio says it can;
        else one DELETE each. How many went."""
        if what == CHANNEL:
            return self.write_many(CHANNEL, [(i, b"") for i in idxs])
        for i in idxs:
            self.ok(DELETE, struct.pack("<BH", what, i))
        return len(idxs)

    def log(self, most=0):
        """LINK-42: the event log newest first, `most` records (0: all): dicts."""
        out, before = [], 0xFFFFFFFF
        while True:
            a = self.ok(LOG_READ, struct.pack("<I", before))
            top, kept, n = struct.unpack("<IHB", a[:7])
            r = a[7:]
            for _ in range(n):
                e, r = parse_log(r)
                out.append(e)
                before = e["seq"]
                if most and len(out) >= most:
                    return out
            if n == 0:
                return out


def parse_log(r):
    """One event log record (LINK-42, and EVENT kind 5, LINK-57): a dict, and the bytes after it."""
    seq, ago, src, slot, flags, freq, ident, x, y = struct.unpack("<IIBBBIIHH", r[:23])
    r = r[23:]
    pos = None
    if flags & 2:
        lat, lon = struct.unpack("<ii", r[:8])
        pos, r = (lat / 1e7, lon / 1e7), r[8:]
    name, r = text(r)
    txt, r = text(r)
    return ({"seq": seq, "ago_s": ago, "src": LOG_SOURCES[src] if src < len(LOG_SOURCES) else src, "slot": slot,
             "unread": bool(flags & 1), "freq": freq, "id": ident, "x": x, "y": y, "pos": pos, "name": name,
             "text": txt}, r)


def why(status, rest):
    s = STATUS[status] if status < len(STATUS) else str(status)
    return f"{s}: {rest.decode('ascii', 'replace')}" if status == 9 else s


def remote_ptt(link, seconds, slot):
    """LINK-20: key, keepalives every 0.5 s, KEY off after `seconds` or Ctrl-C: always, in `finally`, refused or not."""
    ended = None
    on = None
    try:
        on = link.send(KEY, bytes([PTT, PTT_ON, SLOTS[slot]]))
        keyed = None
        end = time.time() + 0.6
        while time.time() < end and keyed is None:
            for t, s, status, rest in link.answers():
                if t == KEY | 0x80 and s == on:
                    keyed = status == 0
                    if keyed:
                        print(f"keyed: slot {'ABC'[rest[0]] if rest and rest[0] < 3 else '?'}")
                    else:
                        ended = f"refused: {why(status, rest)}"
                        print(ended)
        if keyed is None:
            print("keyed, as far as can be told: the cable holds its answers while transmitting (TX-43)")
        stop = time.time() + seconds
        while time.time() < stop and ended is None:
            link.send(KEY, bytes([PTT, PTT_ALIVE]))
            nxt = min(time.time() + 0.5, stop)
            while time.time() < nxt and ended is None:
                for t, s, status, rest in link.answers():
                    if t == KEY | 0x80 and status != 0:
                        ended = f"the radio unkeyed: {why(status, rest)}"
                        print(ended)
    except KeyboardInterrupt:
        print("Ctrl-C")
    finally:
        off = link.send(KEY, bytes([PTT, PTT_OFF]))
        # Answers held while keyed (TX-43) come out after the unkey; KEY off's own is the last.
        end = time.time() + 3
        done = False
        while time.time() < end and not done:
            for t, s, status, rest in link.answers():
                if t != KEY | 0x80:
                    continue
                if s == off:
                    done = True
                elif status != 0 and ended is None:
                    ended = f"the radio unkeyed: {why(status, rest)}"
                    print(ended)
        print("unkeyed: KEY off answered" if done else "KEY off sent; no answer within 3 s")
    return 1 if ended else 0


class ProtocolError(OSError):
    """The radio speaks another link protocol (decisions 385, 401: no older one is kept; from 0.9 each wire-format change raises the number)."""

    def __init__(self, got):
        self.got = got
        super().__init__(f"the radio speaks link protocol {got}, this tool {PROTOCOL}: update the radio or the tool")


class RadioError(Exception):
    """The radio refused a request: `status` (STATUS's index) and its reason."""

    def __init__(self, status, rest=b""):
        self.status = status
        super().__init__(why(status, rest))


def key_code(name):
    try:
        return KEY_CODES[name.upper()]
    except KeyError:
        raise ValueError(f"no key {name!r}: {', '.join(sorted(set(KEY_CODES)))}") from None


def parse_ui_line(line):
    """One `ui` line: (kind, [positional strings], {key: value}). Strings are in double quotes with \" and \\."""
    pos, kv, i, n = [], {}, 0, len(line)

    def quoted(i):
        out, i = [], i + 1
        while i < n and line[i] != '"':
            if line[i] == "\\" and i + 1 < n:
                i += 1
            out.append(line[i])
            i += 1
        return "".join(out), i + 1

    while i < n:
        if line[i] == " ":
            i += 1
        elif line[i] == '"':
            v, i = quoted(i)
            pos.append(v)
        else:
            j = i
            while j < n and line[j] not in ' ="':
                j += 1
            word = line[i:j]
            if j < n and line[j] == "=":
                if j + 1 < n and line[j + 1] == '"':
                    v, i = quoted(j + 1)
                else:
                    k = j + 1
                    while k < n and line[k] != " ":
                        k += 1
                    v, i = line[j + 1:k], k
                kv[word] = int(v) if v.lstrip("-").isdigit() else v
            else:
                pos.append(word)
                i = j
    return (pos[0] if pos else ""), pos[1:], kv


def parse_ui(text):
    """`ui`'s lines as a dict: screen, state, bar, toast/popup, qmenu, cards, rows (and sel, the selected row),
    help, hints and the screen's own lines (value, ask, entry, ...) under their first word; `lines` is the text."""
    ui = {"lines": text.splitlines(), "cards": [], "rows": [], "qmenu": [], "toast": None, "popup": None, "sel": None,
          "hints": [], "pvrows": []}
    for line in ui["lines"]:
        kind, pos, kv = parse_ui_line(line)
        if kind == "screen":
            ui["screen"] = {"id": pos[0] if pos else "", "name": pos[1] if len(pos) > 1 else "", **kv}
        elif kind == "card":
            ui["cards"].append({"slot": pos[0] if pos else "", **kv})
        elif kind == "row":
            num, rest = (int(pos[0]), pos[1:]) if pos and pos[0].isdigit() else (None, pos)
            r = {"num": num, "name": rest[0] if rest else "", "value": rest[1] if len(rest) > 1 else "",
                 "sel": "sel" in rest[2:], "dot": "dot" in rest[2:], "grey": "grey" in rest[2:]}
            r.update({k: v for k, v in kv.items() if k not in r})  # a screen's own fields (kind=, chips=)
            ui["rows"].append(r)
            if r["sel"]:
                ui["sel"] = r
        elif kind in ("toast", "popup"):
            ui[kind] = {"head": pos[0] if pos else "", "body": pos[1] if len(pos) > 1 else ""}
        elif kind == "qmenu":
            ui["qmenu"].append(pos)
        elif kind == "bcast":
            ui.setdefault("bcast", {})[pos[0] if pos else ""] = {"rds": pos[1] if len(pos) > 1 else "", **kv}
        elif kind == "pvrow":
            ui.setdefault("pvrows", []).append({"name": pos[0] if pos else "", "place": pos[1] if len(pos) > 1 else "",
                                                "freq": pos[2] if len(pos) > 2 else ""})
        elif kind == "hints":
            ui["hints"] = list(zip(pos[0::2], pos[1::2]))
        elif kind:
            ui[kind] = {"args": pos, **kv} if pos or not kv else kv
    return ui


class Radio:
    """The link as a test runner uses it (LINK-19): press keys, turn the knob, read the screen back.

    `port` is a serial device (the cable or a Bluetooth serial port) or tcp:HOST:PORT for the simulator."""

    def __init__(self, port, pin=None, name=b"ferrite_link.py"):
        self.port = Port(port)
        self.link = Link(self.port)
        hello = self.link.hello(name)
        self.hello = hello  # HELLO's answer: its build, capabilities and layouts (LINK-35.2, LINK-62)
        if not hello[2]:
            if not pin:
                raise RadioError(5)
            self._ok(AUTH, pin.encode("ascii"))
        self.ble_rate = ble_fast(self.link, name)
        if self.ble_rate and pin and not hello[2]:
            self._ok(AUTH, pin.encode("ascii"))  # the proving HELLO started the session over
        rest = text(hello[3:])[1]
        self.build, self.bench = build_name(rest), build_bench(rest)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def close(self):
        self.port.close()

    def _ok(self, ty, payload=b"", busy_s=3.0):
        """A request, again while the radio answers `busy` (a long press still held, the store writing)."""
        end = time.time() + busy_s
        while True:
            s, rest = self.link.ask(ty, payload)
            if s == 0:
                return rest
            if s != 7 or time.time() > end:
                raise RadioError(s, rest)
            time.sleep(0.05)

    def _key(self, name, action):
        self._ok(KEY, bytes([key_code(name), action]))

    def press(self, name, long=False):
        """A short press, or a long one (held 750 ms: the call returns once it has been let go)."""
        self._key(name, KEY_LONG if long else KEY_SHORT)
        if long:
            time.sleep(LONG_HOLD + 0.05)

    def down(self, name):
        self._key(name, KEY_DOWN)

    def up(self, name):
        self._key(name, KEY_UP)

    def keys(self, seq, long=False):
        """Several keys: "MENU 0 8 2 1 OK"; a word NAME:long, NAME:down or NAME:up says how (short by default)."""
        for word in seq.split():
            name, _, how = word.partition(":")
            how = how.lower() or ("long" if long else "short")
            if how not in ACTIONS:
                raise ValueError(f"{word}: :long, :down or :up")
            if how in ("short", "long"):
                self.press(name, long=how == "long")
            else:
                self._key(name, ACTIONS[how])

    def knob(self, steps):
        """Knob steps, + clockwise; more than 20 go in several requests."""
        while steps:
            n = max(-20, min(20, steps))
            self._ok(KEY, bytes([KNOB, n & 0xFF]))
            steps -= n

    def ui_text(self, what=0):
        """What the screen shows, as the radio's text (0 the state, 1 the key names)."""
        if self.link.streams:
            for _ in range(3):
                try:
                    return "".join(a[1:].decode("utf-8", "replace") for a in self.link.stream(UI, bytes([what])))  # LINK-50
                except StreamGap:
                    time.sleep(0.3)
                    self.link.answers()
        out, rest = [], self._ok(UI, bytes([what]))
        while True:
            out.append(rest[1:].decode("utf-8", "replace"))
            if rest[0] == 0:
                return "".join(out)
            rest = self._ok(MORE)

    def ui(self):
        return parse_ui(self.ui_text())

    def status_text(self):
        """The compact status line (LINK-19, UI what=2), without its newline."""
        return self.ui_text(2).strip()

    def status(self):
        """The compact status as a dict (parse_status): screen, active, keyed, remote, mode, slots, xb."""
        return parse_status(self.status_text())

    def cmd(self, line):
        """A terminal command line (LINK-34); its output."""
        out, rest = [], self._ok(CMD, line.encode("ascii"))
        while True:
            out.append(rest[1:].decode("utf-8", "replace"))
            if rest[0] == 0:
                return "".join(out)
            if len(rest) == 1:
                time.sleep(0.05)
            rest = self._ok(MORE)


def parse_status(line):
    """The compact status line (LINK-19, UI what=2) as a dict:
    {"screen": "Home", "active": "A", "keyed": None or "A"/"B"/"C"/"HF"/"SAT", "remote": bool, "mode": "off"|"xb"|...,
     "slots": {"A": {"state": "RX", "meter": 20}, ...}, "xb": None or {"state": N, "passes": N}}.
    A slot's state is its card's word as the full readback's `card ... state="..."` has it ("idle", "RX", "heard",
    "RX muted", "MON", "scanning", ...): the line's `-` is "idle" and `_` a space."""
    words = line.split()
    if not words or words[0] != "st":
        raise ValueError(f"not a status line: {line!r}")
    kv = dict(w.split("=", 1) for w in words[1:])
    slots = {}
    for s in "ABC":
        if s in kv:
            st, _, m = kv[s].rpartition("/")
            slots[s] = {"state": "idle" if st == "-" else st.replace("_", " "), "meter": int(m)}
    xb = kv.get("xb", "0").split(",")
    return {
        "screen": kv.get("scr"),
        "active": kv.get("act"),
        "keyed": None if kv.get("keyed", "-") == "-" else kv["keyed"],
        "remote": kv.get("remote") == "1",
        "mode": kv.get("mode"),
        "slots": slots,
        "xb": {"state": int(xb[1]), "passes": int(xb[2])} if xb[0] == "1" and len(xb) == 3 else None,
    }


def text(b):
    n = b[0]
    return b[1:1 + n].decode("ascii", "replace"), b[1 + n:]


def channel_fields(rec):
    num, zone, rx = struct.unpack("<HHI", rec[:8])  # protocol 3: the zone is 2 bytes (0xFFFF none; decision 333)
    name = rec[8:24].split(b"\x00")[0].decode("ascii", "replace")
    return num, zone, rx, name


SYMS = ["Car", "Home", "Digi", "WX", "HT", "iGate", "Truck", "Boat"]
MSG_STATES = ["unread", "read", "pending", "acked", "retrying", "failed"]


def parse_station(b):
    """LINK-44: one station record; (dict, the rest)."""
    flags = b[0]
    call, b = text(b[1:])
    sym, ago, times = struct.unpack("<BIH", b[:7])
    b = b[7:]
    st = {"call": call, "sym": SYMS[sym] if sym < len(SYMS) else sym, "ago_s": ago, "times": times}
    if flags & 1:
        lat, lon = struct.unpack("<ii", b[:8])
        st["pos"], b = (lat / 1e7, lon / 1e7), b[8:]
    if flags & 2:
        st["course"], b = struct.unpack("<HH", b[:4]), b[4:]
    if flags & 4:
        st["alt_m"], b = struct.unpack("<i", b[:4])[0], b[4:]
    st["via"], b = text(b)
    st["comment"], b = text(b)
    return st, b


def parse_message(b):
    """LINK-44: one message record; (dict, the rest)."""
    inbound, state, tries, ago = struct.unpack("<BBBI", b[:7])
    peer, b = text(b[7:])
    body, b = text(b)
    st = MSG_STATES[state] if state < len(MSG_STATES) else str(state)
    if state == 4:
        st += f" {tries}"
    return {"in": bool(inbound), "peer": peer, "text": body, "ago_s": ago, "state": st}, b


def aprs_cfg(rec):
    """LINK-43: the APRS record."""
    f5k, beacon, power, path, digi, after, ssid = struct.unpack("<HBBBBBB", rec[:8])
    return {"freq_mhz": f5k * 0.005 if f5k else "region's", "beacon_mode": beacon & 0x7F, "beacon_paused": bool(beacon & 0x80),
            "power": ["low", "mid", "high"][power], "path": path, "digipeater": bool(digi & 1), "digi_mycall": bool(digi & 2),
            "digi_wait_s": digi >> 4 & 7, "beacon_after_voice": bool(after), "ssid": ssid,
            "custom_path": rec[8:24].split(b"\x00")[0].decode("ascii", "replace"),
            "fx25_tx": (rec[24] if len(rec) > 24 else None)}  # APRS-45: check bytes, 0 = off


# Live View (LINK-47, LINK-48; ferrite_core::liveview): the text dictionary, byte k (1-31) for entry k - 1.
LV_DICT_VERSION = 1
LV_DICT = [b"row ", b'" "', b" sel", b" dot", b" grey", b"card ", b" active=", b' head="', b'" state="', b" freq=",
           b' name="', b'" mode="', b'" tone="', b'" shift="', b'" power="', b'" s="S', b'" meter=', b" rx=", b" scan=",
           b" kind=slot", b'hints "', b"screen ", b'" title="', b"bar lock=", b" watch=", b" inbox=", b" battery=",
           b" clock=", b"state dark=", b" keyed=", b'qmenu "']


def lv_decode(b):
    return b"".join(LV_DICT[c - 1] if 1 <= c <= 31 else bytes([c]) for c in b)


def lv_tokens(line):
    """A line's tokens: split at the spaces outside double quotes (backslash escapes inside them)."""
    out, start, quoted, esc = [], 0, False, False
    for i, c in enumerate(line):
        if esc:
            esc = False
        elif quoted and c == 0x5C:
            esc = True
        elif c == 0x22:
            quoted = not quoted
        elif c == 0x20 and not quoted:
            out.append(line[start:i])
            start = i + 1
    out.append(line[start:])
    return out


def lv_key(t):
    for i, c in enumerate(t):
        if c in (0x3D, 0x22):
            return t[:i + 1] if c == 0x3D and i > 0 else None
    return None


def fnv1a(h, b):
    for c in b:
        h = ((h ^ c) * 0x01000193) & 0xFFFFFFFF
    return h


class LiveView:
    """A Live View client (the reference for Console): apply() each UI event's payload after its kind byte; False
    means ask for the whole state again (SUBSCRIBE without RENEW)."""

    def __init__(self):
        self.lines, self.last = [], None

    def apply(self, ev):
        seq, flags, check, n = ev[0], ev[1], struct.unpack("<H", ev[2:4])[0], ev[4]
        if flags & 1:
            self.lines = []
        elif self.last is None or (self.last + 1) & 0xFF != seq:
            return False  # lost or late
        self.lines = (self.lines + [b""] * n)[:n]
        i, ops = 0, ev[5:]
        while i < len(ops):
            op, ln = ops[i] >> 6, ops[i] & 0x3F
            if op == 0:
                k = ops[i + 1]
                self.lines[ln] = lv_decode(ops[i + 2:i + 2 + k])
                i += 2 + k
                continue
            if op == 2:
                count, at = len(lv_tokens(self.lines[ln])), i + 1
            else:
                count, at = ops[i + 1], i + 2
            entries, i = ops[at], at + 1
            toks = (lv_tokens(self.lines[ln]) + [b""] * count)[:count]
            for _ in range(entries):
                idx, k = ops[i], ops[i + 1]
                v = lv_decode(ops[i + 2:i + 2 + k])
                i += 2 + k
                j = idx & 0x7F
                toks[j] = ((lv_key(toks[j]) or b"") + v) if idx & 0x80 else v
            self.lines[ln] = b" ".join(toks)
        self.last = seq
        if not flags & 2:
            h = 0x811C9DC5
            for line in self.lines:
                h = fnv1a(fnv1a(h, line), b"\n")
            if h & 0xFFFF != check:
                return False
        return True

    def text(self):
        return "".join(line.decode("utf-8", "replace") + "\n" for line in self.lines)


def subscribe(link, what, seconds, screen):
    """LINK-46: subscribe, then print each event for `seconds`, renewing every 5 s; the UI's bytes a second."""
    mask = sum({"heard": SUB_HEARD, "msgs": SUB_MSGS, "ui": SUB_UI, "log": SUB_LOG}[w] for w in what)
    r = link.ok(SUBSCRIBE, bytes([mask]))
    if mask & SUB_UI and r[1] != LV_DICT_VERSION:
        sys.exit(f"Live View dictionary {r[1]}: this tool knows {LV_DICT_VERSION}")
    lv, t0, renew, ui_bytes, ui_n = LiveView(), time.time(), time.time(), 0, 0
    try:
        while time.time() - t0 < seconds:
            link.answers()
            while link.events:
                _, p = link.events.pop(0)
                kind, body = p[0], p[1:]
                if kind == EV_HEARD:
                    st, _ = parse_station(body[1:])
                    print(f"heard {st['call']} ({st['sym']}, {st['times']}x, via {st['via'] or 'direct'}) {st.get('pos', 'no position')}; {body[0]} in the list")
                elif kind in (EV_MSG, EV_MSG_STATE):
                    m, _ = parse_message(body[2:])
                    what_ = "message" if kind == EV_MSG else "message state"
                    print(f"{what_}: {'from' if m['in'] else 'to'} {m['peer']}: {m['text']!r} [{m['state']}]")
                elif kind == EV_LOG:
                    e, _ = parse_log(body)
                    print(f"log #{e['seq']}: {e['src']} {e['name']} {e['text']!r}")
                elif kind == EV_UI:
                    ui_bytes, ui_n = ui_bytes + len(p) + 6, ui_n + 1
                    if not lv.apply(body):
                        print("ui: lost one: asking for the whole state")
                        link.ok(SUBSCRIBE, bytes([mask]))
                        continue
                    if screen and not body[1] & 2:
                        sys.stdout.write("\x1b[H\x1b[2J" + lv.text())
                    elif not screen:
                        print(f"ui #{body[0]}: {len(p) + 6} B{' (whole)' if body[1] & 1 else ''}{' ...' if body[1] & 2 else ''}")
            if time.time() - renew >= 5:
                link.ok(SUBSCRIBE, bytes([mask, 0, RENEW]))  # a keepalive: events stop after 10 s quiet
                renew = time.time()
            time.sleep(0.05)
    except KeyboardInterrupt:
        pass
    link.ok(SUBSCRIBE, bytes([0]))
    if mask & SUB_UI:
        dt = max(time.time() - t0, 0.001)
        print(f"ui: {ui_n} updates, {ui_bytes} B on the wire, {ui_bytes / dt:.1f} B/s")
    return 0


def ago(s):
    """An age from the radio: seconds, or AGO_UNKNOWN (u32::MAX) for a row kept from before the last restart."""
    return "before restart" if s == 0xFFFFFFFF else f"{s} s ago"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("port")
    ap.add_argument("cmd")
    ap.add_argument("args", nargs="*")
    ap.add_argument("--name")
    ap.add_argument("--rx", type=float, help="RX frequency in MHz")
    ap.add_argument("--seconds", type=float, default=5.0, help="key: how long to stay keyed")
    ap.add_argument("--slot", choices=sorted(SLOTS), default="active",
                    help="key: the active slot (PTT A), b (PTT B), or a/c only while active")
    ap.add_argument("--pin", help="the radio's PIN, for Bluetooth while a PIN lock is on (LINK-30)")
    ap.add_argument("--long", action="store_true", help="keys: every key a long press")
    ap.add_argument("--screen", action="store_true", help="subscribe ui: redraw the screen's text on each update")
    ap.add_argument("--raw", type=int, nargs="?", const=12000, metavar="RATE",
                    help="audio: the decoders' samples as 12-bit PCM at 12000 (default) or 16000 Hz (LINK-13.9, bench builds)")
    ap.add_argument("--poke", metavar="REG=HEX,...", help="audio --raw on a bench build: rewrite chip 1's registers every 100 ms in the same session (the radio's tune and audio routing rewrite them), e.g. 47=6942")
    a = ap.parse_args()
    port = Port(a.port)
    try:
        sys.exit(run(Link(port), a))
    finally:
        port.close()


# LINK-13.4 (decision 330): G.711 µ-law, as ferrite_core::rxaudio codes it
def ulaw_decode(codes):
    """One AUDIO_DATA block's codes (a byte a sample) to 16-bit samples."""
    out = []
    for u in codes:
        u = ~u & 0xFF
        t = (((u & 15) << 3) + 0x84) << ((u >> 4) & 7)
        out.append(0x84 - t if u & 0x80 else t - 0x84)
    return out


def audio(link, seconds, path):
    """LINK-13.4: listen to the radio's receive audio over the cable for SECONDS into a WAV (12 kHz, 16-bit mono):
    blocks while a squelch is open (silence for the gaps between, so the file keeps time), keep-alives otherwise."""
    import wave
    port = link.p
    fast = getattr(port, "ser", None) is not None  # the cable at 230400, as Console runs it: 115200 is too slow for µ-law
    if fast:
        link.ok(BAUD, struct.pack("<I", 230_400))
        time.sleep(0.05)
        port.baud(230_400)
        link.hello()  # within 1 s, or the radio goes back (LINK-11)
    rest = link.ok(AUDIO, b"\x01")
    rate, block = struct.unpack("<HH", rest[:4])
    t0 = time.time()
    samples, blocks, keep, lost, last_seq, nbytes, renew = [], 0, 0, 0, None, 0, t0
    last_at = t0
    try:
        while time.time() - t0 < seconds:
            link.answers()
            while link.audio:
                p = link.audio.pop(0)
                nbytes += len(p) + 6
                seq, slot, flags = struct.unpack("<HBB", p[:4])
                if last_seq is not None and (seq - last_seq) & 0xFFFF > 1:
                    lost += ((seq - last_seq) & 0xFFFF) - 1
                last_seq = seq
                now = time.time()
                if len(p) == 4:
                    keep += 1
                    gap = int((now - last_at) * rate)  # nothing heard: silence of the right length
                    samples.extend([0] * max(0, gap))
                else:
                    blocks += 1
                    samples.extend(ulaw_decode(p[4:]))
                last_at = now
            if time.time() - renew >= 3.0:
                link.send(AUDIO, b"\x01")  # keeps the session active (LINK-13.5: a quiet one stops listening)
                renew = time.time()
            time.sleep(0.005)
    finally:
        link.ok(AUDIO, b"\x00")
        if fast:
            link.ok(BAUD, struct.pack("<I", 115_200))
            time.sleep(0.05)
            port.baud(115_200)
            link.hello()
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(struct.pack(f"<{len(samples)}h", *samples))
    el = time.time() - t0
    print(f"{blocks} blocks ({block} samples each), {keep} keep-alives, {lost} lost; {nbytes} bytes in {el:.1f} s "
          f"({nbytes / el:.0f} B/s); {len(samples) / rate:.1f} s of audio to {path}")
    return 0


def raw_audio(link, seconds, path, rate, poke=None):
    """LINK-13.9 (decision 307), a bench build's only: the decoders' own samples, stepped from 24 kHz to RATE (12000,
    or 16000 for sweeps to 6 kHz) as 12-bit PCM, every block whatever the squelch, into a 16-bit WAV at RATE. The cable
    goes to 230400 for 12 kHz (19.1 KB/s), 460800 for 16 kHz (25.5 KB/s), and back to 115200 at the end. Reports
    frames lost on the cable (gaps in seq) and samples missing (gaps in the first sample's index: the radio's drops
    too; filled with silence, so the file keeps time), clipped blocks and the rate achieved."""
    import wave
    mode = {12000: 2, 16000: 3}.get(rate)
    if mode is None:
        sys.exit("--raw takes 12000 or 16000")
    baud = 230_400 if rate == 12000 else 460_800
    p = link.p
    fast = getattr(p, "ser", None) is not None  # the cable; the simulator's TCP has no rate
    if fast:
        link.ok(BAUD, struct.pack("<I", baud))
        time.sleep(0.05)
        p.baud(baud)
        link.hello()  # within 1 s, or the radio goes back (LINK-11)
    s, rest = link.ask(AUDIO, bytes([mode]))
    if s != 0:
        sys.exit(f"audio --raw: {why(s, rest)} (a bench build's only: tx-bench, bringup)")
    got_rate, block = struct.unpack("<HH", rest[:4])
    t0 = time.time()
    samples, blocks, lost_frames, missing, clipped, nbytes, last, renew = [], 0, 0, 0, 0, 0, None, t0
    t_first = None
    poked = 0.0
    try:
        while time.time() - t0 < seconds:
            link.answers()
            while link.raw:
                pl = link.raw.pop(0)
                nbytes += len(pl) + 7  # type, seq, CRC, COBS's byte and the delimiter
                seq, first, slot, flags = struct.unpack("<HIBB", pl[:8])
                if t_first is None:
                    t_first = time.time()
                if last is not None:
                    ls, lf, ln = last
                    lost_frames += (seq - ls - 1) & 0xFFFF
                    gap = (first - lf - ln) & 0xFFFFFFFF
                    if gap < 1 << 31:
                        missing += gap
                        samples.extend([0] * gap)
                body = pl[8:]
                for i in range(0, len(body) - 2, 3):
                    w = body[i] | body[i + 1] << 8 | body[i + 2] << 16
                    for v in (w & 0xFFF, w >> 12 & 0xFFF):
                        samples.append((v - 0x1000 if v & 0x800 else v) << 4)
                blocks += 1
                clipped += bool(flags & 0x80)
                last = (seq, first, (len(body) // 3) * 2)
            if time.time() - renew >= 3.0:
                link.send(AUDIO, bytes([mode]))  # keeps the session active (LINK-13.5)
                renew = time.time()
            if poke and time.time() - poked >= 0.1:
                for reg, val in poke:
                    link.ok(CMD, f"bk 1 {reg} {val}".encode())
                poked = time.time()
            time.sleep(0.002)
    finally:
        link.ok(AUDIO, b"\x00")
        if fast:
            link.ok(BAUD, struct.pack("<I", 115_200))
            time.sleep(0.05)
            p.baud(115_200)
            link.hello()
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(got_rate)
        w.writeframes(struct.pack(f"<{len(samples)}h", *samples))
    el = time.time() - (t_first or t0)
    print(f"raw {got_rate} Hz, 12-bit, at {baud if fast else 'tcp'}: {blocks} blocks ({block} samples each), "
          f"{lost_frames} frames lost on the cable, {missing} samples missing, {clipped} clipped blocks; "
          f"{len(samples) - missing} samples in {el:.1f} s ({(len(samples) - missing) / el:.0f} Hz), "
          f"{nbytes / el:.0f} B/s; {len(samples) / got_rate:.1f} s of audio to {path}")
    return 1 if lost_frames or missing else 0


BENCH_BIT = 0x80  # LINK-35: bit 7 of the build byte marks a bench build (decision 151: tx-dry is 0x82)


def build_name(rest):
    """LINK-35: the build byte after HELLO's version or INFO's callsign (absent before Oct 13, 2026): its flavour,
    bit 7 (a bench build) masked off; build_bench() says whether it is set. "tx+dry" (3): a full-power tx build with
    Dry TX on (TX-48), the PA drive held at 0 until it is turned off on the radio; build_dry() covers both dry kinds."""
    return {0: "receive", 1: "tx", 2: "tx-dry", 3: "tx+dry"}.get(rest[0] & 0x7F, f"unknown {rest[0]}") if rest else "not given"


def build_dry(rest):
    """LINK-35, TX-48: the PA drive is held at 0 now (a tx-dry build, or a tx build with Dry TX on)."""
    return build_name(rest) in ("tx-dry", "tx+dry")


def build_bench(rest):
    """LINK-35: a bench build (bit 7): the bench terminal commands (txtest, bk, mem, gpio, ...) are in it."""
    return bool(rest and rest[0] & BENCH_BIT)


UID_DIGITS = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"  # Crockford base32 (ferrite_core::uid)


def uid_text(b):
    """LINK-35.1, SELF-8: INFO's last 12 bytes (the unique ID as the MCU stores it) as About shows them: 20 base32
    digits in groups of five; None when absent (older firmware) or all zero (not read)."""
    if len(b) != 12 or not any(b):
        return None
    n = int.from_bytes(b, "big") << 4
    t = "".join(UID_DIGITS[(n >> (5 * (19 - i))) & 31] for i in range(20))
    return " ".join(t[i:i + 5] for i in range(0, 20, 5))


def ble_fast(link, name=b"ferrite_link.py"):
    """LINK-15.1 (decision 285): over Bluetooth, ask the radio for 115200, unlock the module, prove it with a HELLO.
    A radio from before answers `refused` and the link stays at 9600; if the unlock or the HELLO fails, the radio goes
    back to 9600 by itself (3 s without a HELLO, or a framing error: LINK-25.2) and the HELLO is tried again there.
    FERRITE_BLE_SLOW=1 skips it. Returns the rate in use."""
    p = link.p
    if not isinstance(p, BlePort) or os.environ.get("FERRITE_BLE_SLOW"):
        return None
    s, _ = link.ask(BAUD, struct.pack("<I", 115_200))
    if s != 0:
        return 9_600
    time.sleep(0.15)  # the answer has left; the radio switches once its ring is empty
    ok = p.unlock()
    for _ in range(3):
        try:
            link.hello(name)
            return 115_200 if ok else 9_600
        except (TimeoutError, RadioError):
            time.sleep(0.5)
    link.hello(name)
    return 9_600


def run(link, a):
    hello = link.hello()
    ble_fast(link)
    if not hello[2]:
        if not a.pin:
            sys.exit("the radio's PIN lock is on: give --pin (LINK-30)")
        link.ok(AUTH, a.pin.encode("ascii"))
    if a.cmd == "key":
        return remote_ptt(link, a.seconds, a.slot)
    if a.cmd in ("keys", "knob", "ui", "status"):
        r = Radio.__new__(Radio)  # this session, already past HELLO and AUTH
        r.port, r.link = link.p, link
        try:
            if a.cmd == "keys":
                r.keys(" ".join(a.args), long=a.long)
            elif a.cmd == "knob":
                r.knob(int(a.args[0]))
            elif a.cmd == "status":
                print(r.status_text())
            else:
                sys.stdout.write(r.ui_text(1 if a.args[:1] == ["keys"] else 0))
        except RadioError as e:
            print(f"{a.cmd}: {e}")
            return 1
        return 0
    if a.cmd == "term":
        bad = 0
        for line in a.args:
            status, rest = link.ask(CMD, line.encode("ascii"))
            while True:
                if status != 0:
                    print(f"{line}: {why(status, rest)}")
                    bad = 1
                    break
                sys.stdout.write(rest[1:].decode("utf-8", "replace"))
                if rest[0] == 0:
                    break
                if len(rest) == 1:
                    time.sleep(0.05)  # the radio is still working on it (a debug command)
                status, rest = link.ask(MORE)
        return bad
    if a.cmd == "slot":
        for n in [ "ABC".index(x.upper()) for x in a.args] or [0, 1, 2]:
            r = link.ok(SLOT, bytes([n]))
            s, act, ch, f, mode, pw, rxt, txt, off, fl = struct.unpack("<BBHIBBHHiB", r[:19])
            where = f"ch {ch + 1}" if ch != 0xFFFF else ("aprs" if fl & 2 else "vfo")
            print(f"{'ABC'[s]}{'*' if act else ' '} {where} {f / 100000:.5f} MHz {MODES[mode]} "
                  f"{['low', 'mid', 'high'][pw]} txtone 0x{txt:04X} rxtone 0x{rxt:04X} "
                  f"{'split tx ' + str(off / 100000) if fl & 1 else 'offset ' + str(off / 100000)}"
                  f"{' scanning' if fl & 4 else ''}")
        return 0
    if a.cmd == "hello":
        ver, rest = text(hello[3:])
        print(f"protocol {hello[0]}, max payload {hello[1]}, may write {bool(hello[2])}, firmware {ver}, "
              f"build {build_name(rest)}{' (bench)' if build_bench(rest) else ''}")
    elif a.cmd == "info":
        r = link.ok(INFO)
        ver, r = text(r)
        chans, zones, presets, slen, region = struct.unpack("<HHBHB", r[:8])
        call, rest = text(r[8:])
        uid = uid_text(rest[1:13])
        print(f"firmware {ver} ({build_name(rest)}{', bench' if build_bench(rest) else ''}); {chans} channels, {zones} zones, {presets} presets; "
              f"settings {slen} B; region {region}; callsign {call or '-'}; unique ID {uid or 'not given'}")
    elif a.cmd == "read-settings":
        rec = link.read(SETTINGS, 0)
        open(a.args[0], "wb").write(rec)
        print(f"{len(rec)} bytes to {a.args[0]} (PIN and tokens read as zeros)")
    elif a.cmd == "write-settings":
        rec = open(a.args[0], "rb").read()
        link.write(SETTINGS, 0, rec)
        print(f"{len(rec)} bytes written (region, Extend TX, PIN and cable port keep the radio's)")
    elif a.cmd == "read-channel":
        n = int(a.args[0]) - 1
        rec = link.read(CHANNEL, n)
        num, zone, rx, name = channel_fields(rec)
        print(f"channel {num + 1}: {name!r} {rx / 100000:.5f} MHz zone {zone if zone != 0xFFFF else 'none'}; "
              f"{len(rec)} bytes: {rec.hex()}")
    elif a.cmd == "write-channel":
        n = int(a.args[0]) - 1
        rec = bytearray(link.read(CHANNEL, n))
        if a.name is not None:
            rec[8:24] = a.name.encode("ascii")[:16].ljust(16, b"\x00")
        if a.rx is not None:
            rec[4:8] = struct.pack("<I", round(a.rx * 100000))
        link.ok(BEGIN, struct.pack("<H", 1))
        link.write(CHANNEL, n, bytes(rec))
        link.ok(END)
        print(f"channel {n + 1} written: {channel_fields(rec)[3]!r}")
    elif a.cmd == "delete-channel":
        n = int(a.args[0]) - 1
        link.ok(DELETE, struct.pack("<BH", CHANNEL, n))
        print(f"channel {n + 1} deleted")
    elif a.cmd == "used":
        what = WHATS[(a.args or ["channel"])[0]]
        got = link.used(what)
        base = 1 if what == CHANNEL else 0
        print(f"{len(got)} held: {' '.join(str(i + base) for i in got)}")
    elif a.cmd == "readmany":
        what = WHATS[(a.args or ["channel"])[0]]
        first = max(0, int(a.args[1]) - (1 if what == CHANNEL else 0)) if len(a.args) > 1 else 0
        count = int(a.args[2]) if len(a.args) > 2 else None
        recs, asks = link.read_many(what, first, count)
        for i, rec in recs:
            if what == CHANNEL:
                num, zone, rx, name = channel_fields(rec)
                print(f"channel {i + 1}: {name!r} {rx / 100000:.5f} MHz zone {zone if zone != 0xFFFF else 'none'}")
            else:
                print(f"{i}: {rec.hex()}")
        print(f"{len(recs)} records in {asks} requests")
    elif a.cmd == "log":
        for e in link.log(int(a.args[0]) if a.args else 0):
            where = f" {e['pos'][0]:.5f},{e['pos'][1]:.5f}" if e["pos"] else ""
            print(f"#{e['seq']} {ago(e['ago_s'])} {e['src']} {e['freq'] / 100000:.5f} MHz slot {'ABC'[e['slot']] if e['slot'] < 3 else e['slot']}"
                  f"{' unread' if e['unread'] else ''} id {e['id']}{where} {e['name']!r} {e['text']!r}")
    elif a.cmd == "baud":
        rate = int(a.args[0])
        link.ok(BAUD, struct.pack("<I", rate))
        time.sleep(0.05)
        link.p.baud(rate)
        link.hello()  # within 1 s, or the radio goes back
        print(f"now at {rate}")
    elif a.cmd == "aprs-cfg":
        rec = link.read(APRS, 0)
        for k, v in aprs_cfg(rec).items():
            print(f"{k}: {v}")
    elif a.cmd == "heard":
        i, n = 0, None
        while n is None or i < n:
            r = link.ok(HEARD, bytes([i]))
            n, rest = r[0], r[1:]
            if not rest:
                break
            while rest:
                st, rest = parse_station(rest)
                pos = "%.5f %.5f" % st["pos"] if "pos" in st else "no position"
                print(f"{st['call']:<10} {st['sym']:<5} {ago(st['ago_s']):>14}  x{st['times']:<3} {pos:<22} via {st['via'] or 'direct'}  {st['comment']}")
                i += 1
        print(f"{n} heard")
    elif a.cmd == "msgs":
        i, n = 0, None
        while n is None or i < n:
            r = link.ok(MSGS, struct.pack("<H", i))
            n, rest = struct.unpack("<H", r[:2])[0], r[2:]
            if not rest:
                break
            while rest:
                m, rest = parse_message(rest)
                print(f"{'from' if m['in'] else 'to':<4} {m['peer']:<9} {ago(m['ago_s']):>14}  [{m['state']}] {m['text']}")
                i += 1
        print(f"{n} messages")
    elif a.cmd == "msg-send":
        to, body = a.args[0].encode("ascii"), a.args[1].encode("ascii")
        r = link.ok(MSG_SEND, bytes([len(to)]) + to + bytes([len(body)]) + body)
        print("queued: " + ("APRS mode is on, it goes out now" if r[0] else "it waits for APRS mode (MENU 0 5 3)"))
    elif a.cmd == "msg-delete":
        # msg-delete in|out CALL TEXT: the newest message that way with that station and text (LINK-52)
        way, call, body = a.args[0], a.args[1].encode("ascii"), a.args[2].encode("ascii")
        link.ok(MSG_DELETE, bytes([way == "in"]) + bytes([len(call)]) + call + bytes([len(body)]) + body)
        print("deleted")
    elif a.cmd == "subscribe":
        what = (a.args[0] if a.args else "heard,msgs,ui").split(",")
        return subscribe(link, what, a.seconds if a.seconds != 5.0 else 3600, a.screen)
    elif a.cmd == "audio" and a.raw:
        return raw_audio(link, float(a.args[0]) if a.args else 10.0, a.args[1] if len(a.args) > 1 else "rx-raw.wav", a.raw, [x.split("=") for x in a.poke.split(",")] if a.poke else None)
    elif a.cmd == "audio":
        return audio(link, float(a.args[0]) if a.args else 10.0, a.args[1] if len(a.args) > 1 else "rx-audio.wav")
    elif a.cmd == "reboot":
        link.ok(REBOOT)  # LINK-24: answered, then the radio saves, goes quiet and resets into the bootloader
        print("restarting: the bootloader listens about 200 ms (run flash.py now, or it starts Ferrite again)")
    else:
        sys.exit(f"unknown command {a.cmd}; see --help")
    return 0


if __name__ == "__main__":
    main()
