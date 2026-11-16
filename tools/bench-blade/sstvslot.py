"""An SSTV picture slot read from the radio over the cable (DATA_READ, LINK-59; read only), saved as a PNG; with no
SLOT, every slot's header (SSTV-4.3).

  sstvslot.py PORT [SLOT OUT.png]"""
import os, struct, sys
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from ferrite_link import Link, Port  # noqa: E402

BASE, SLOT_LEN, PIXELS, W = 0x0C0000, 41 * 4096, 4096, 320
MODES = {8: "Robot 36", 44: "Martin M1", 60: "Scottie S1", 99: "PD90", 95: "PD120", 0: "-"}


def header(link, s):
    b = link.data_read(BASE + s * SLOT_LEN, 64)
    if b[:4] != b"SSTV":
        return None
    kind, vis, flags = b[5], b[6], b[7]
    lines, = struct.unpack("<H", b[10:12])
    utc, freq, = struct.unpack("<II", b[12:20])
    name = b[20:36].split(b"\0")[0].split(b"\xff")[0].decode(errors="replace")
    seq, = struct.unpack("<I", b[36:40])
    return dict(kind={1: "received", 2: "to send"}.get(kind, kind), mode=MODES.get(vis, vis), complete=not flags & 1,
                lines=None if lines == 0xFFFF else lines, freq=freq * 10 / 1e6 if freq else None, name=name, seq=seq, utc=utc)


def main():
    link = Link(Port(sys.argv[1]))
    link.hello(b"sstvslot.py")  # the cable to the link first (TERM-2), as ferrite_link.py does
    if len(sys.argv) < 3:
        for s in range(6):
            print(s, header(link, s))
        return
    s, out = int(sys.argv[2]), sys.argv[3]
    h = header(link, s)
    print(h)
    lines = (h or {}).get("lines") or 256
    raw = bytearray()
    a, end = BASE + s * SLOT_LEN + PIXELS, BASE + s * SLOT_LEN + PIXELS + lines * W * 2
    while a < end:
        n = min(232, end - a)
        raw += link.data_read(a, n)
        a += n
    from PIL import Image
    px = bytearray()
    for i in range(0, len(raw), 2):
        c = raw[i] | raw[i + 1] << 8
        r, g, b = c >> 11, c >> 5 & 63, c & 31
        px += bytes((r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2))
    Image.frombytes("RGB", (W, lines), bytes(px)).save(out)
    print(out, lines, "lines")


main()
