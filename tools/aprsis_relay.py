#!/usr/bin/env python3
"""Real APRS traffic into a radio's receive path, on the bench: APRS-IS (read-only) to KISS on the cable.

    .venv/bin/python tools/aprsis_relay.py PORT --call VE7GPL [--filter r/49.25/-123.1/150] [--every 10] [--minutes 20]

It logs in read-only (passcode -1), turns each APRS-IS line (TNC2 text) back into an AX.25 UI frame (the original
source, destination and RF path; APRS-IS's own path additions, q-constructs and TCPIP, dropped) and writes it as a
KISS data frame to PORT, one every `--every` seconds at most (the newest waiting one; the rest are counted, not
queued). Lines that can't be AX.25 (a letter SSID from a D-STAR gateway or a hotspot, a callsign over 6 characters)
go out as third-party packets from --call, as an iGate sends them (--no-third-party skips them).

For the bench only: the sending radio must be on its tx-bench build with Dry TX on (0 8 8 5) and its APRS
frequency at 145.495 (0 5 4), its cable port on KISS (0 9 2 3), per the rules in CLAUDE.md. A frame from APRS-IS
on the air would be an iGate's job; with Dry TX at 145.495 nothing leaves the bench.
"""
import argparse
import os
import re
import socket
import sys
import time

import serial

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ferrite_link  # noqa: E402

FEND, FESC, TFEND, TFESC = 0xC0, 0xDB, 0xDC, 0xDD
CALL = re.compile(r"^([A-Z0-9]{1,6})(?:-(\d{1,2}))?(\*?)$")


def addr(call, last=False, repeated=False):
    """An AX.25 address field (7 bytes) for CALL[-SSID], or None if it can't be one."""
    m = CALL.match(call.upper())
    if not m or int(m.group(2) or 0) > 15:
        return None
    c, ssid = m.group(1).ljust(6), int(m.group(2) or 0)
    b = bytes(ord(ch) << 1 for ch in c)
    return b + bytes([0x60 | (0x80 if repeated else 0) | ssid << 1 | (1 if last else 0)])


def frame(line):
    """TNC2 `SRC>DEST,PATH:info` to an AX.25 UI frame (no FCS: KISS leaves it to the TNC), or None."""
    head, sep, info = line.partition(":")
    if not sep or not info or ">" not in head:
        return None
    src, _, rest = head.partition(">")
    parts = rest.split(",")
    dest, path = parts[0], []
    for p in parts[1:]:
        if p.startswith("q") or p.startswith("TCPIP") or p in ("TCPXX", "NOGATE", "RFONLY"):
            break  # APRS-IS's own additions: the RF path ends before them
        path.append(p)
    path = path[:8]
    calls = [dest, src] + path
    out = b""
    for i, c in enumerate(calls):
        a = addr(c.rstrip("*"), last=i == len(calls) - 1, repeated=c.endswith("*") and i >= 2)
        if a is None:
            return None
        out += a
    body = info.encode("latin-1", "replace")
    return out + b"\x03\xf0" + body if 0 < len(body) <= 256 else None


def third_party(line, call):
    """A line that can't be AX.25 (a letter SSID: D-STAR ports, hotspots) as an iGate sends it: a frame from `call`
    with the original header inside, `}SRC>DEST,TCPIP,CALL*:info` (APRS 1.0.1 chapter 17)."""
    head, sep, info = line.partition(":")
    if not sep or ">" not in head:
        return None
    src, _, rest = head.partition(">")
    dest = rest.split(",")[0]
    return frame(f"{call}>APZFRL:}}{src}>{dest},TCPIP,{call}*:{info}")


def kiss(f):
    esc = f.replace(bytes([FESC]), bytes([FESC, TFESC])).replace(bytes([FEND]), bytes([FESC, TFEND]))
    return bytes([FEND, 0x00]) + esc + bytes([FEND])


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("port")
    ap.add_argument("--call", required=True, help="callsign for the read-only APRS-IS login")
    ap.add_argument("--filter", default="r/49.25/-123.1/150", help="APRS-IS server filter (default 150 km around Vancouver)")
    ap.add_argument("--server", default="rotate.aprs2.net")
    ap.add_argument("--every", type=float, default=10.0, help="seconds between frames, at least")
    ap.add_argument("--minutes", type=float, default=20.0)
    ap.add_argument("--no-third-party", action="store_true", help="skip lines that can't be AX.25 instead of wrapping them")
    a = ap.parse_args()
    if a.every < 5:
        sys.exit("--every under 5 s: the radio's duty limit and the rules want gaps")
    try:
        ser = serial.Serial(ferrite_link.resolve_port(a.port, cable_only=True), 115200, timeout=0)
    except ferrite_link.PortError as e:
        sys.exit(str(e))
    s = socket.create_connection((a.server, 14580), timeout=30)
    s.sendall(f"user {a.call.upper()} pass -1 vers ferrite-relay 0.1 filter {a.filter}\r\n".encode())
    s.settimeout(1.0)
    buf, waiting, sent, skipped, dropped = b"", None, 0, 0, 0
    last, end = 0.0, time.time() + a.minutes * 60
    while time.time() < end:
        try:
            chunk = s.recv(4096)
            if not chunk:
                print("APRS-IS closed the connection")
                break
            buf += chunk
        except socket.timeout:
            pass
        while b"\n" in buf:
            raw, buf = buf.split(b"\n", 1)
            line = raw.decode("latin-1").strip()
            if not line or line.startswith("#"):
                if line.startswith("# logresp"):
                    print(line)
                continue
            f = frame(line)
            if f is None and not a.no_third_party:
                f = third_party(line, a.call.upper())
            if f is None:
                skipped += 1
                continue
            if waiting is not None:
                dropped += 1
            waiting = (line, f)
        if waiting and time.time() - last >= a.every:
            line, f = waiting
            ser.write(kiss(f))
            sent, last, waiting = sent + 1, time.time(), None
            print(f"{time.strftime('%H:%M:%S')} sent {sent}: {line[:110]}")
    print(f"done: {sent} sent, {dropped} not sent (rate), {skipped} not AX.25")


if __name__ == "__main__":
    main()
