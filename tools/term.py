"""Drive the radio's cable terminal: each argument is a keystroke string (Python escapes: '\\r' Enter, '\\x1b' Esc).
Prints what came back, ANSI codes stripped. Send Esc first (the terminal may be on its banner); addresses take no 0x:
    term.py '\\x1b' ':' 'bk 1 37\\r' '\\x1b'
Port: FERRITE_PORT (a port or a radio's name), radios.toml's default, or the only cable. Needs pyserial (.venv)."""
import os, re, sys, time
import serial

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ferrite_link  # noqa: E402

try:
    s = serial.Serial(ferrite_link.resolve_port(None, cable_only=True), 115200, timeout=0.2)
except ferrite_link.PortError as e:
    sys.exit(str(e))


def rd(t=1.5):
    end, b = time.time() + t, b""
    while time.time() < end:
        b += s.read(4096)
    return b


for k in sys.argv[1:]:
    k = k.encode().decode("unicode_escape").encode()
    s.write(k)
    out = rd()
    txt = re.sub(rb"\x1b\[[0-9;?]*[A-Za-z]", b" ", out).decode("latin1")
    print(f"--- sent {k!r}: {len(out)} bytes")
    print("\n".join(l.rstrip() for l in re.split(r"[\r\n]+", txt) if l.strip())[:3000])
