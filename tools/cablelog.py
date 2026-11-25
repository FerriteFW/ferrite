"""Record the radio's cable log (Cable log on, 0 9 9 5) without sending a byte: any byte from the PC ends the log
until the radio restarts. Usage: cablelog.py SECONDS OUTFILE (appends). Port: FERRITE_PORT (a port or a radio's name), radios.toml's default, or the only cable."""
import os, sys, time
import serial

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ferrite_link  # noqa: E402

try:
    s = serial.Serial(ferrite_link.resolve_port(None, cable_only=True), 115200, timeout=0.2)
except ferrite_link.PortError as e:
    sys.exit(str(e))
end, out = time.time() + float(sys.argv[1]), open(sys.argv[2], "ab")
while time.time() < end:
    b = s.read(4096)
    if b:
        out.write(b)
        out.flush()
