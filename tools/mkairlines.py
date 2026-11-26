#!/usr/bin/env python3
"""The airline table for ACARS (ACARS-6): a CSV in, the SPI flash part out.

    python3 tools/mkairlines.py AIRLINES.csv OUT.bin

The CSV has a header line and the columns `iata,icao,name,country` (data.ferritefw.dev builds it; the country is
not stored yet). Console writes the output at 0x0C0000 (layout.rs AIRLINES, 32 KB). The format, little-endian,
as crates/ferrite-core/src/airlines.rs reads it:

    header (16): magic "FAIR" | version u16 (1) | count u16 | CRC-32 of the rows (zlib) u32 | 4 bytes 0
    row (29):    IATA [2] | ICAO [3] | name [24], ASCII, NUL-padded

Rows are sorted by IATA code; a code that comes twice keeps its first row (a warning names the others). Names are
cut to 24 bytes; accents are dropped (NFKD), any other non-ASCII character becomes '?'. The fixture
(tools/mkairlines-fixture.csv) and its output (tools/mkairlines-fixture.bin) are checked against the Rust packer by
airlines.rs's tests.
"""
import csv
import struct
import sys
import unicodedata
import zlib

MAGIC = b"FAIR"
VERSION = 1
HEADER = 16
ROW = 29
NAME = 24
REGION = 0x8000  # layout.rs AIRLINES
MAX_ROWS = (REGION - HEADER) // ROW


def ascii_of(s):
    s = unicodedata.normalize("NFKD", s.strip())
    return "".join(c for c in s if not unicodedata.combining(c)).encode("ascii", "replace")


def pack(rows):
    """[(iata, icao, name)] -> the table's bytes."""
    seen, out = {}, []
    for iata, icao, name in rows:
        code = ascii_of(iata).upper()
        if len(code) != 2:
            print(f"skipped: IATA code {iata!r} is not two characters", file=sys.stderr)
            continue
        if code in seen:
            print(f"skipped: {code.decode()} {name!r} (the code is {seen[code]!r}'s)", file=sys.stderr)
            continue
        seen[code] = name
        out.append(code + ascii_of(icao).upper()[:3].ljust(3, b"\0") + ascii_of(name)[:NAME].ljust(NAME, b"\0"))
    out.sort(key=lambda r: r[:2])
    if len(out) > MAX_ROWS:
        sys.exit(f"{len(out)} airlines: more than the {MAX_ROWS} the region holds")
    body = b"".join(out)
    return struct.pack("<4sHHI4x", MAGIC, VERSION, len(out), zlib.crc32(body)) + body


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    with open(sys.argv[1], newline="", encoding="utf-8") as f:
        rows = [(r["iata"], r["icao"], r["name"]) for r in csv.DictReader(f)]
    data = pack(rows)
    with open(sys.argv[2], "wb") as f:
        f.write(data)
    print(f"{sys.argv[2]}: {(len(data) - HEADER) // ROW} airlines, {len(data)} bytes")


if __name__ == "__main__":
    main()
